use nyatidraw_api::{LayerId, MaskCommand, MaskId, RasterEditTarget};
use nyatidraw_document::{LayerMask, LayerTree};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask};
use nyatidraw_tiles::TileSnapshot;

pub struct PreparedMaskEdit {
    pub tree: LayerTree,
    pub tiles: TileSnapshot,
    pub target: RasterEditTarget,
    pub tree_changed: bool,
    pub changed: bool,
}

/// # Errors
/// Rejects missing/protected owners and unsupported selection allocation before mutation.
pub fn prepare_mask_edit(
    command: MaskCommand,
    tree: &LayerTree,
    tiles: &TileSnapshot,
    selection: Option<&SelectionMask>,
    next_id: u128,
) -> Result<PreparedMaskEdit, String> {
    let owner = command.layer();
    let layer = tree.raster(owner).ok_or("마스크 소유 레이어가 없습니다.")?;
    let mut next_tree = tree.clone();
    let mut next_tiles = tiles.clone();
    let mut target = RasterEditTarget::Image(owner);
    if layer.locked
        && !matches!(
            command,
            MaskCommand::SelectImage(_) | MaskCommand::SelectMask(_)
        )
    {
        return Err("잠긴 레이어의 마스크를 변경할 수 없습니다.".into());
    }
    match command {
        MaskCommand::AddRevealAll(_) | MaskCommand::AddFromSelection(_) => {
            if layer.mask.is_some() {
                return Err("이미 마스크가 있습니다.".into());
            }
            let id = MaskId(next_id);
            if tree.mask_ids().contains(&id) {
                return Err("마스크 ID가 이미 사용 중입니다.".into());
            }
            let from_selection = matches!(command, MaskCommand::AddFromSelection(_));
            let mask = LayerMask {
                id,
                enabled: true,
                default_coverage: if from_selection { 0 } else { 255 },
            };
            if from_selection {
                next_tiles = nyatidraw_paint_cpu::mask_from_selection(
                    tiles,
                    id,
                    selection.ok_or("선택 영역이 없습니다.")?,
                    EditLimits::default(),
                )
                .map_err(|error| format!("마스크: {error:?}"))?;
            }
            next_tree
                .set_mask(owner, Some(mask))
                .map_err(|error| format!("마스크: {error:?}"))?;
            target = RasterEditTarget::Mask {
                layer: owner,
                mask: id,
            };
        }
        MaskCommand::SelectImage(_) => {}
        MaskCommand::SelectMask(_) => {
            target = RasterEditTarget::Mask {
                layer: owner,
                mask: layer.mask.ok_or("마스크가 없습니다.")?.id,
            };
        }
        MaskCommand::SetEnabled { enabled, .. } => {
            let mut mask = layer.mask.ok_or("마스크가 없습니다.")?;
            mask.enabled = enabled;
            next_tree
                .set_mask(owner, Some(mask))
                .map_err(|error| format!("마스크: {error:?}"))?;
            target = RasterEditTarget::Mask {
                layer: owner,
                mask: mask.id,
            };
        }
        MaskCommand::FillSelection { color, .. } => {
            let mask = layer.mask.ok_or("마스크가 없습니다.")?;
            next_tiles = fill_selection(tree, tiles, owner, mask, selection, color)?;
            target = RasterEditTarget::Mask {
                layer: owner,
                mask: mask.id,
            };
        }
        MaskCommand::Remove(_) => {
            let mask = layer.mask.ok_or("마스크가 없습니다.")?;
            next_tiles = tiles.without_masks([mask.id]);
            next_tree
                .set_mask(owner, None)
                .map_err(|error| format!("마스크: {error:?}"))?;
        }
    }
    let tree_changed = next_tree != *tree;
    let changed = tree_changed || next_tiles.root() != tiles.root();
    Ok(PreparedMaskEdit {
        tree: next_tree,
        tiles: next_tiles,
        target,
        tree_changed,
        changed,
    })
}

fn fill_selection(
    tree: &LayerTree,
    tiles: &TileSnapshot,
    owner: LayerId,
    mask: LayerMask,
    selection: Option<&SelectionMask>,
    color: [u8; 4],
) -> Result<TileSnapshot, String> {
    if !nyatidraw_document::composition_scope_nodes(
        tree.root(),
        nyatidraw_document::CompositionScope::Solo(nyatidraw_api::LayerTreeNodeId::Group(
            tree.root_id(),
        )),
    )
    .contains(&nyatidraw_api::LayerTreeNodeId::Raster(owner))
    {
        return Err(
            "칠할 레이어나 상위 그룹이 숨겨져 있습니다. 레이어 패널에서 표시를 켜주세요.".into(),
        );
    }
    nyatidraw_paint_cpu::fill_mask_selection(
        tiles,
        mask,
        selection.ok_or("선택 영역이 없습니다.")?,
        nyatidraw_paint_cpu::PremultipliedRgba8(color),
        EditLimits::default(),
    )
    .map_err(|error| format!("마스크 채우기: {error:?}"))
}

/// Routes explicit mask painting before the image mutation guard on either host.
#[must_use]
pub fn mask_edit_command(
    command: &nyatidraw_api::EditCommand,
    target: RasterEditTarget,
) -> Option<MaskCommand> {
    match (command, target) {
        (
            nyatidraw_api::EditCommand::FillSelection { color },
            RasterEditTarget::Mask { layer, .. },
        ) => Some(MaskCommand::FillSelection {
            layer,
            color: *color,
        }),
        _ => None,
    }
}

#[must_use]
pub fn valid_raster_target(
    tree: &LayerTree,
    active: LayerId,
    target: Option<RasterEditTarget>,
) -> RasterEditTarget {
    match target {
        Some(RasterEditTarget::Mask { layer, mask })
            if layer == active
                && tree
                    .raster(layer)
                    .and_then(|node| node.mask)
                    .is_some_and(|value| value.id == mask) =>
        {
            RasterEditTarget::Mask { layer, mask }
        }
        _ => RasterEditTarget::Image(active),
    }
}

/// Selection and sampling tools are shared; image mutation requires its explicit target.
#[must_use]
pub fn edit_allowed_on_mask(command: &nyatidraw_api::EditCommand) -> bool {
    use nyatidraw_api::EditCommand as C;
    matches!(
        command,
        C::PickColor { .. }
            | C::PickDisplayColor { .. }
            | C::ResizePage { .. }
            | C::SelectWand { .. }
            | C::CombineWand { .. }
            | C::SelectLasso { .. }
            | C::CombineLasso { .. }
            | C::SelectAll
            | C::InvertSelection
            | C::GrowSelection { .. }
            | C::ShrinkSelection { .. }
            | C::FeatherSelection { .. }
            | C::SelectLayerAlpha
            | C::ClearSelection
    )
}

use nyatidraw_api::{
    CanvasSpec, CommandRejectReason, ContentRootId, GroupId, LayerCommand, LayerId,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

pub struct PreparedLayerEdit {
    pub tree: LayerTree,
    pub active: LayerId,
    pub pixels: Option<LayerPixelChange>,
}

/// # Errors
/// Rejects invalid hierarchy edits before the host commits anything.
#[allow(clippy::too_many_lines)]
pub fn prepare_layer_edit(
    original: &LayerTree,
    preferred: LayerId,
    identifier: u128,
    command: LayerCommand,
) -> Result<PreparedLayerEdit, CommandRejectReason> {
    let mut tree = original.clone();
    let mut active = preferred;
    let next_id = identifier;
    let mut pixel_change = None;
    match command {
        LayerCommand::AddRaster | LayerCommand::AddGroup => {
            let is_group = matches!(command, LayerCommand::AddGroup);
            let (parent, index) = tree
                .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(active))
                .map_or(
                    (tree.root_id(), tree.root().children.len()),
                    |(parent, index)| (parent, index + 1),
                );
            let name = next_default_node_name(&tree, is_group);
            let node = if is_group {
                LayerTreeNode::Group(GroupNode {
                    clip_to_below: false,
                    blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                    id: GroupId(next_id),
                    name,
                    visible: true,
                    opacity_u16: u16::MAX,
                    children: Vec::new(),
                })
            } else {
                active = LayerId(next_id);
                LayerTreeNode::Raster(empty_raster(active, name))
            };
            let _ = next_id
                .checked_add(1)
                .ok_or(CommandRejectReason::RevisionExhausted)?;
            tree.insert(parent, index, node)
                .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
        }
        LayerCommand::DuplicateRaster(source) => {
            let _ = next_id
                .checked_add(1)
                .ok_or(CommandRejectReason::RevisionExhausted)?;
            active = LayerId(next_id);
            tree.duplicate_raster(source, active)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
            pixel_change = Some(LayerPixelChange::Duplicate {
                source,
                destination: active,
            });
        }
        LayerCommand::SetReference { layer, reference } => {
            tree.set_reference(layer, reference)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetLocked { layer, locked } => {
            tree.set_locked(layer, locked)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetAlphaLocked {
            layer,
            alpha_locked,
        } => {
            tree.set_alpha_locked(layer, alpha_locked)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetClipToBelow {
            node,
            clip_to_below,
        } => {
            tree.set_clip_to_below(node, clip_to_below)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetBlendMode { node, blend_mode } => {
            tree.set_blend_mode(node, blend_mode)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::Delete(node) => {
            tree.remove(node).map_err(|error| match error {
                nyatidraw_document::LayerTreeError::LockedNode(_) => {
                    CommandRejectReason::LayerLocked
                }
                _ => CommandRejectReason::UnknownLayer,
            })?;
            if raster_layer_ids(&tree).is_empty() {
                active = LayerId(next_id);
                let _ = next_id
                    .checked_add(1)
                    .ok_or(CommandRejectReason::RevisionExhausted)?;
                tree.insert(
                    tree.root_id(),
                    tree.root().children.len(),
                    LayerTreeNode::Raster(empty_raster(active, "레이어 1".into())),
                )
                .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
            }
            active = valid_active_layer(&tree, active);
        }
        LayerCommand::Rename { node, name } => {
            tree.rename(node, &name)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetVisibility { node, visible } => {
            tree.set_visibility(node, visible)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::SetOpacity { node, opacity_u16 } => {
            tree.set_opacity(node, opacity_u16)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
        }
        LayerCommand::Reorder {
            node,
            new_parent,
            index,
        } => {
            tree.reorder(node, new_parent, index)
                .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
        }
        LayerCommand::AddWhiteBackground => {
            let _ = next_id
                .checked_add(1)
                .ok_or(CommandRejectReason::RevisionExhausted)?;
            let layer = LayerId(next_id);
            tree.insert(
                tree.root_id(),
                0,
                LayerTreeNode::Raster(empty_raster(layer, "White".into())),
            )
            .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
            pixel_change = Some(LayerPixelChange::White(layer));
        }
        LayerCommand::SetActive(_) | LayerCommand::ToggleSolo(_) => {
            unreachable!("session commands handled separately")
        }
    }

    Ok(PreparedLayerEdit {
        tree,
        active,
        pixels: pixel_change,
    })
}

#[must_use]
pub fn empty_raster(id: LayerId, name: String) -> LayerNode {
    LayerNode {
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id,
        name,
        visible: true,
        locked: false,
        reference: false,
        opacity_u16: u16::MAX,
        content_root: ContentRootId(0),
    }
}

#[must_use]
pub fn raster_layer_ids(tree: &LayerTree) -> Vec<LayerId> {
    fn visit(group: &GroupNode, layers: &mut Vec<LayerId>) {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => layers.push(layer.id),
                LayerTreeNode::Group(group) => visit(group, layers),
            }
        }
    }
    let mut layers = Vec::new();
    visit(tree.root(), &mut layers);
    layers.reverse();
    layers
}

#[must_use]
pub fn valid_active_layer(tree: &LayerTree, preferred: LayerId) -> LayerId {
    if tree.raster(preferred).is_some() {
        preferred
    } else {
        raster_layer_ids(tree).first().copied().unwrap_or(preferred)
    }
}

/// # Errors
/// Fails when the document has exhausted its node identifiers.
pub fn next_layer_node_id(tree: &LayerTree) -> Result<u128, String> {
    fn visit(group: &GroupNode, maximum: &mut u128) {
        *maximum = (*maximum).max(group.id.0);
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => *maximum = (*maximum).max(layer.id.0),
                LayerTreeNode::Group(child_group) => visit(child_group, maximum),
            }
        }
    }

    let mut maximum = 0;
    visit(tree.root(), &mut maximum);
    maximum
        .checked_add(1)
        .ok_or_else(|| "layer identifier space is exhausted".to_owned())
}

fn next_default_node_name(tree: &LayerTree, group_name: bool) -> String {
    fn count(group: &GroupNode, groups: &mut usize, rasters: &mut usize) {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(_) => *rasters = rasters.saturating_add(1),
                LayerTreeNode::Group(child_group) => {
                    *groups = groups.saturating_add(1);
                    count(child_group, groups, rasters);
                }
            }
        }
    }

    let mut groups = 0;
    let mut rasters = 0;
    count(tree.root(), &mut groups, &mut rasters);
    if group_name {
        format!("Group {}", groups.saturating_add(1))
    } else {
        format!("Layer {}", rasters.saturating_add(1))
    }
}

/// # Panics
/// Panics if generated fixed-size tiles violate the canonical tile invariant.
#[must_use]
pub fn opaque_white_layer_tiles(canvas: CanvasSpec, layer: LayerId) -> TileSnapshot {
    let columns = canvas.width_px.div_ceil(TILE_EDGE);
    let rows = canvas.height_px.div_ceil(TILE_EDGE);
    TileSnapshot::from_tiles((0..rows).flat_map(|y| {
        (0..columns).map(move |x| {
            let mut pixels = vec![0; TILE_BYTE_LEN];
            let width = (canvas.width_px - x * TILE_EDGE).min(TILE_EDGE) as usize;
            let height = (canvas.height_px - y * TILE_EDGE).min(TILE_EDGE) as usize;
            for row in 0..height {
                let start = row * TILE_EDGE as usize * 4;
                pixels[start..start + width * 4].fill(u8::MAX);
            }
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: i32::try_from(x).expect("document tile column fits i32"),
                    y: i32::try_from(y).expect("document tile row fits i32"),
                },
                pixels,
            )
        })
    }))
    .expect("white raster tiles are canonical")
}

#[derive(Clone, Debug)]
pub enum LayerPixelChange {
    White(LayerId),
    Duplicate {
        source: LayerId,
        destination: LayerId,
    },
}

impl LayerPixelChange {
    /// # Errors
    /// Rejects occupied duplicate destinations or invalid tile replacements.
    pub fn apply(self, tiles: &TileSnapshot, canvas: CanvasSpec) -> Result<TileSnapshot, String> {
        match self {
            Self::White(layer) => tiles
                .with_replacements(
                    opaque_white_layer_tiles(canvas, layer)
                        .iter()
                        .map(|(key, tile)| (key, tile.pixels().to_vec())),
                )
                .map_err(|error| format!("background tiles: {error:?}")),
            Self::Duplicate {
                source,
                destination,
            } => {
                if source == destination || tiles.iter().any(|(key, _)| key.layer == destination) {
                    return Err("duplicate destination already contains artwork".into());
                }
                // Keep signed off-page coordinates and shared immutable pixel objects.
                TileSnapshot::from_objects(
                    tiles.iter().map(|(key, tile)| (key, tile.clone())).chain(
                        tiles
                            .iter()
                            .filter(|(key, _)| key.layer == source)
                            .map(|(key, tile)| {
                                (
                                    TileKey {
                                        layer: destination,
                                        ..key
                                    },
                                    tile.clone(),
                                )
                            }),
                    ),
                )
                .map_err(|error| format!("duplicate tiles: {error:?}"))
            }
        }
    }
}

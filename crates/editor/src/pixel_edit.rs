use nyatidraw_api::{CanvasSpec, EditCommand, EditSource, LayerId};
use nyatidraw_document::LayerTree;
use nyatidraw_paint_cpu::{
    EditLimits, PremultipliedRgba8, RasterFragment, SelectionMask, SelectionPaint, SelectionSource,
    WandRequest, lasso_selection_signed, paint_selection, transform_raster, translate_artwork,
    wand_selection,
};
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PixelEditError(pub String);

pub trait ArtworkClipboard {
    /// # Errors
    /// Returns clipboard publication errors without changing artwork.
    fn write(&mut self, fragment: &RasterFragment) -> Result<(), String>;
    /// # Errors
    /// Returns missing, invalid or unavailable clipboard data.
    fn read(&mut self) -> Result<RasterFragment, String>;
}

#[derive(Default)]
pub struct MemoryArtworkClipboard(pub Option<RasterFragment>);
impl ArtworkClipboard for MemoryArtworkClipboard {
    fn write(&mut self, fragment: &RasterFragment) -> Result<(), String> {
        self.0 = Some(fragment.clone());
        Ok(())
    }
    fn read(&mut self) -> Result<RasterFragment, String> {
        self.0
            .clone()
            .ok_or_else(|| "복사한 그림이 없습니다.".into())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerSource {
    Artwork(EditSource),
    Display(Option<nyatidraw_api::LayerTreeNodeId>),
}

/// # Errors
/// Rejects invalid sample targets or artwork composition.
pub fn sample_picker_pixel(
    tiles: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    point: [i32; 2],
    source: PickerSource,
) -> Result<[u8; 4], PixelEditError> {
    match source {
        PickerSource::Display(solo) => {
            nyatidraw_paint_cpu::sample_display_pixel(tiles, tree, point, solo)
        }
        PickerSource::Artwork(source) => {
            let source = match source {
                EditSource::ActiveLayer => SelectionSource::ActiveLayer,
                EditSource::ReferenceLayers => SelectionSource::ReferenceLayers,
                EditSource::AllVisible => SelectionSource::AllVisible,
            };
            nyatidraw_paint_cpu::sample_artwork_pixel(tiles, tree, target, point, source)
        }
    }
    .map_err(|error| PixelEditError(format!("{error:?}")))
}

#[must_use]
pub fn picker_color(pixel: [u8; 4]) -> Option<[u8; 4]> {
    nyatidraw_tiles::color::linear_premultiplied_to_srgb8(pixel).map(|mut color| {
        // Pick RGB only; brush opacity remains an independent control.
        color[3] = 255;
        color
    })
}

pub struct PixelEditResult {
    pub tree: Option<LayerTree>,
    pub active_layer: Option<LayerId>,
    pub sampled_color: Option<[u8; 4]>,
    pub tiles: Option<TileSnapshot>,
    pub canvas: Option<CanvasSpec>,
    pub selection: Option<Arc<SelectionMask>>,
}

/// Computes an edit without committing a project or mutating its input state.
/// # Errors
/// Rejects invalid targets, selection, clipboard access and bounded work.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
pub fn execute_pixel_edit(
    command: EditCommand,
    target: LayerId,
    canvas: CanvasSpec,
    tree: &LayerTree,
    current_selection: &Option<Arc<SelectionMask>>,
    tiles: &TileSnapshot,
    next_id: u128,
    clipboard: &mut dyn ArtworkClipboard,
    limits: EditLimits,
) -> Result<PixelEditResult, PixelEditError> {
    let mut candidate_selection = current_selection.clone();
    let selection = &mut candidate_selection;
    let reject = |error| PixelEditError(format!("{error:?}"));
    if matches!(
        command,
        EditCommand::ClearActiveLayer
            | EditCommand::DeleteSelectedPixels
            | EditCommand::CutSelection
            | EditCommand::Transform(_)
    ) && tree.raster(target).is_some_and(|layer| layer.alpha_locked)
    {
        return Err(PixelEditError(
            "투명도 잠금 중에는 지우기·잘라내기·변형을 할 수 없습니다. 잠금을 해제하세요.".into(),
        ));
    }
    if matches!(
        command,
        EditCommand::ClearActiveLayer
            | EditCommand::DeleteSelectedPixels
            | EditCommand::CutSelection
            | EditCommand::Transform(_)
            | EditCommand::FillSelection { .. }
            | EditCommand::FloodFill { .. }
            | EditCommand::FloodFillAdvanced { .. }
            | EditCommand::GradientSelection { .. }
    ) && tree.raster(target).is_none_or(|layer| layer.locked)
    {
        return Err(PixelEditError(
            "잠긴 레이어에는 그릴 수 없습니다. 잠금을 해제하세요.".into(),
        ));
    }
    let mut flood_mask = None;
    let mut sampled_color = None;
    let mut transformed = None;
    let mut changed_canvas = None;
    let mut changed_tree = None;
    let mut pasted_layer = None;
    let clear_after_transform = matches!(
        command,
        EditCommand::Transform(_)
            | EditCommand::ResizePage { .. }
            | EditCommand::CropPageToSelection
            | EditCommand::PasteSelection
    );
    let paint = match command {
        EditCommand::FreeTransform(_) => {
            return Err(PixelEditError(
                "FreeTransform requires its transaction worker".into(),
            ));
        }
        EditCommand::CopySelection | EditCommand::CutSelection => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| PixelEditError("먼저 복사할 영역을 선택하세요.".into()))?;
            let fragment = nyatidraw_paint_cpu::copy_selection(tiles, tree, target, mask, limits)
                .map_err(reject)?;
            if command == EditCommand::CutSelection {
                transformed = Some(
                    nyatidraw_paint_cpu::cut_selection(tiles, tree, target, mask, limits)
                        .map_err(reject)?,
                );
            }
            // Clipboard publication and all cut preflight must succeed before
            // the durable commit below can remove any original artwork.
            clipboard.write(&fragment).map_err(PixelEditError)?;
            None
        }
        EditCommand::PasteSelection => {
            let fragment = clipboard.read().map_err(PixelEditError)?;
            let id = crate::layer_edit::next_layer_node_id(tree)
                .map_err(PixelEditError)?
                .max(next_id);
            let layer = LayerId(id);
            let mut next_tree = tree.clone();
            let (parent, index) = tree
                .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(target))
                .ok_or_else(|| PixelEditError("붙여넣기 위치 레이어가 없습니다.".into()))?;
            next_tree
                .insert(
                    parent,
                    index + 1,
                    nyatidraw_document::LayerTreeNode::Raster(nyatidraw_document::LayerNode {
                        alpha_locked: false,
                        clip_to_below: false,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        id: layer,
                        name: "붙여넣기".into(),
                        visible: true,
                        locked: false,
                        reference: false,
                        opacity_u16: u16::MAX,
                        content_root: nyatidraw_api::ContentRootId(0),
                    }),
                )
                .map_err(|e| PixelEditError(format!("Paste layer: {e:?}")))?;
            transformed = Some(
                nyatidraw_paint_cpu::paste_fragment(tiles, &next_tree, layer, &fragment, limits)
                    .map_err(reject)?,
            );
            changed_tree = Some(next_tree);
            pasted_layer = Some(layer);
            None
        }
        EditCommand::PickColor { point, .. } | EditCommand::PickDisplayColor { point, .. } => {
            let source = match command {
                EditCommand::PickDisplayColor { solo, .. } => PickerSource::Display(solo),
                EditCommand::PickColor { source, .. } => PickerSource::Artwork(source),
                _ => unreachable!("picker arm"),
            };
            let pixel = sample_picker_pixel(tiles, tree, target, point, source)?;
            let color = picker_color(pixel).ok_or_else(|| {
                PixelEditError("투명한 픽셀입니다. 현재 색상을 유지합니다.".into())
            })?;
            sampled_color = Some(color);
            None
        }
        EditCommand::ClearActiveLayer => {
            transformed =
                Some(nyatidraw_paint_cpu::clear_raster(tiles, tree, target).map_err(reject)?);
            None
        }
        EditCommand::DeleteSelectedPixels => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| PixelEditError("선택 영역이 없습니다.".into()))?;
            transformed = Some(
                nyatidraw_paint_cpu::clear_selection(tiles, tree, target, mask, limits)
                    .map_err(reject)?,
            );
            None
        }
        EditCommand::SelectAll | EditCommand::InvertSelection => {
            let (origin, size) = selection_domain(canvas, tiles, target, selection.as_deref())?;
            let mask = match (command, selection.as_deref()) {
                (EditCommand::InvertSelection, Some(mask)) => {
                    nyatidraw_paint_cpu::invert_selection(mask, origin, size, limits)
                }
                _ => nyatidraw_paint_cpu::selection_all(origin, size, limits),
            }
            .map_err(reject)?;
            *selection = Some(Arc::new(mask));
            None
        }
        EditCommand::GrowSelection { radius } | EditCommand::ShrinkSelection { radius } => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| PixelEditError("먼저 영역을 선택하세요.".into()))?;
            let updated = if matches!(command, EditCommand::GrowSelection { .. }) {
                nyatidraw_paint_cpu::grow_selection(mask, radius, limits)
            } else {
                nyatidraw_paint_cpu::shrink_selection(mask, radius, limits)
            }
            .map_err(reject)?;
            *selection = Some(Arc::new(updated));
            None
        }
        EditCommand::SelectLayerAlpha => {
            let retained = selection.as_ref().map_or(0, |mask| {
                let [width, height] = mask.dimensions();
                u64::from(width) * u64::from(height)
            });
            let mut alpha_limits = limits;
            alpha_limits.max_workspace_bytes = alpha_limits
                .max_workspace_bytes
                .checked_sub(retained)
                .ok_or_else(|| PixelEditError("선택 작업 메모리 한도를 초과했습니다.".into()))?;
            let updated =
                nyatidraw_paint_cpu::select_layer_alpha(tiles, tree, target, alpha_limits)
                    .map_err(reject)?;
            *selection = Some(Arc::new(updated));
            None
        }
        EditCommand::ResizePage { size } => {
            let next = CanvasSpec {
                width_px: size[0],
                height_px: size[1],
                ..canvas
            };
            validate_page(next)?;
            if next != canvas {
                changed_canvas = Some(next);
                transformed = Some(nyatidraw_paint_cpu::SelectionPaintResult {
                    after: tiles.clone(),
                    changed_tiles: Vec::new(),
                });
            }
            None
        }
        EditCommand::CropPageToSelection => {
            let ([left, top], [width_px, height_px]) = selection
                .as_ref()
                .and_then(|mask| mask.bounds_signed())
                .ok_or_else(|| PixelEditError("선택 영역이 없습니다.".into()))?;
            let next = CanvasSpec {
                width_px,
                height_px,
                ..canvas
            };
            validate_page(next)?;
            let offset = [left, top].map(i32::checked_neg);
            let [Some(x), Some(y)] = offset else {
                return Err(PixelEditError("Crop coordinate overflow".into()));
            };
            transformed = Some(translate_artwork(tiles, [x, y], limits).map_err(reject)?);
            if next != canvas {
                changed_canvas = Some(next);
            }
            None
        }
        EditCommand::Transform(transform) => {
            transformed = Some(
                transform_raster(tiles, tree, target, selection.as_deref(), transform, limits)
                    .map_err(reject)?,
            );
            None
        }
        EditCommand::SelectWand {
            seed,
            tolerance,
            source,
        } => {
            let source = match source {
                EditSource::ActiveLayer => SelectionSource::ActiveLayer,
                EditSource::ReferenceLayers => SelectionSource::ReferenceLayers,
                EditSource::AllVisible => SelectionSource::AllVisible,
            };
            let mask = wand_selection(
                tiles,
                tree,
                WandRequest {
                    canvas,
                    active: target,
                    source,
                    seed,
                    tolerance,
                },
                limits,
            )
            .map_err(reject)?;
            *selection = Some(Arc::new(mask));
            None
        }
        EditCommand::SelectLasso { ref vertices }
        | EditCommand::CombineLasso { ref vertices, .. } => {
            let incoming = lasso_selection_signed(vertices, limits).map_err(reject)?;
            let mode = match command {
                EditCommand::CombineLasso { mode, .. } => mode,
                _ => nyatidraw_api::SelectionMode::Replace,
            };
            let mode = match mode {
                nyatidraw_api::SelectionMode::Replace => {
                    nyatidraw_paint_cpu::SelectionCombine::Replace
                }
                nyatidraw_api::SelectionMode::Add => nyatidraw_paint_cpu::SelectionCombine::Add,
                nyatidraw_api::SelectionMode::Subtract => {
                    nyatidraw_paint_cpu::SelectionCombine::Subtract
                }
            };
            let mask = if let Some(current) = selection.as_deref() {
                nyatidraw_paint_cpu::combine_selection(current, &incoming, mode, limits)
                    .map_err(reject)?
            } else if mode == nyatidraw_paint_cpu::SelectionCombine::Subtract {
                return Err(PixelEditError("뺄 선택 영역이 없습니다.".into()));
            } else {
                incoming
            };
            *selection = Some(Arc::new(mask));
            None
        }
        EditCommand::ClearSelection => {
            *selection = None;
            None
        }
        EditCommand::FillSelection { color } => {
            Some(SelectionPaint::Solid(PremultipliedRgba8(color)))
        }
        EditCommand::FloodFill {
            seed,
            tolerance,
            source,
            color,
        } => {
            if selection.is_some() {
                return Err(PixelEditError(
                    "Use FillSelection while a selection exists".into(),
                ));
            }
            let source = match source {
                EditSource::ActiveLayer => SelectionSource::ActiveLayer,
                EditSource::ReferenceLayers => SelectionSource::ReferenceLayers,
                EditSource::AllVisible => SelectionSource::AllVisible,
            };
            flood_mask = Some(Arc::new(
                wand_selection(
                    tiles,
                    tree,
                    WandRequest {
                        canvas,
                        active: target,
                        source,
                        seed,
                        tolerance,
                    },
                    limits,
                )
                .map_err(reject)?,
            ));
            Some(SelectionPaint::Solid(PremultipliedRgba8(color)))
        }
        EditCommand::FloodFillAdvanced {
            seed,
            tolerance,
            source,
            color,
            settings,
        } => {
            let source = match source {
                EditSource::ActiveLayer => SelectionSource::ActiveLayer,
                EditSource::ReferenceLayers => SelectionSource::ReferenceLayers,
                EditSource::AllVisible => SelectionSource::AllVisible,
            };
            transformed = Some(
                nyatidraw_paint_cpu::flood_fill(
                    tiles,
                    tree,
                    nyatidraw_paint_cpu::FloodFillRequest {
                        canvas,
                        active: target,
                        source,
                        seed,
                        tolerance,
                        settings,
                    },
                    selection.as_deref(),
                    PremultipliedRgba8(color),
                    limits,
                )
                .map_err(reject)?,
            );
            None
        }
        EditCommand::GradientSelection {
            start,
            end,
            start_color,
            end_color,
        } => Some(SelectionPaint::LinearGradient {
            start,
            end,
            start_color: PremultipliedRgba8(start_color),
            end_color: PremultipliedRgba8(end_color),
        }),
    };
    if let Some(paint) = paint {
        let mask = flood_mask
            .as_ref()
            .or(selection.as_ref())
            .ok_or_else(|| PixelEditError("NoSelection".into()))?;
        transformed =
            Some(paint_selection(tiles, tree, target, mask, paint, limits).map_err(reject)?);
    }

    let tiles = transformed
        .filter(|painted| {
            !painted.changed_tiles.is_empty() || changed_canvas.is_some() || changed_tree.is_some()
        })
        .map(|painted| painted.after);
    if clear_after_transform {
        *selection = None;
    }
    Ok(PixelEditResult {
        tree: changed_tree,
        active_layer: pasted_layer,
        sampled_color,
        tiles,
        canvas: changed_canvas,
        selection: candidate_selection,
    })
}

// All/invert have an explicit finite document domain, not the current viewport.
// Include active artwork's stored tile extents so off-page anatomy is reachable.
fn selection_domain(
    canvas: CanvasSpec,
    tiles: &TileSnapshot,
    target: LayerId,
    selection: Option<&SelectionMask>,
) -> Result<([i32; 2], [u32; 2]), PixelEditError> {
    let mut min = [0_i64; 2];
    let mut max = [i64::from(canvas.width_px), i64::from(canvas.height_px)];
    for (key, _) in tiles
        .iter()
        .filter(|(key, _)| key.layer == target && key.mip == 0)
    {
        let (x, y) = key.pixel_origin();
        for (axis, value) in [x, y].into_iter().enumerate() {
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value + i64::from(nyatidraw_tiles::TILE_EDGE));
        }
    }
    if let Some((origin, size)) = selection.and_then(SelectionMask::bounds_signed) {
        for axis in 0..2 {
            min[axis] = min[axis].min(i64::from(origin[axis]));
            max[axis] = max[axis].max(i64::from(origin[axis]) + i64::from(size[axis]));
        }
    }
    let invalid = || {
        PixelEditError(
            "전체선택 범위가 너무 큽니다. 필요한 부분을 사각형/올가미로 선택하세요.".into(),
        )
    };
    let origin = [
        i32::try_from(min[0]).map_err(|_| invalid())?,
        i32::try_from(min[1]).map_err(|_| invalid())?,
    ];
    let size = [
        u32::try_from(max[0] - min[0]).map_err(|_| invalid())?,
        u32::try_from(max[1] - min[1]).map_err(|_| invalid())?,
    ];
    Ok((origin, size))
}

fn validate_page(canvas: CanvasSpec) -> Result<(), PixelEditError> {
    if canvas.validate().is_err()
        || u64::from(canvas.width_px) * u64::from(canvas.height_px)
            > nyatidraw_tiles::MAX_FLATTENED_PIXELS
    {
        return Err(PixelEditError(
            "페이지 크기가 출력 한도를 넘거나 잘못되었습니다.".into(),
        ));
    }
    Ok(())
}

//! Writer-thread selection state and durable pixel edits. No GPU or UI handles.
use nyatidraw_api::{CanvasSpec, EditCommand, EditSource, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_paint_cpu::{
    EditLimits, PremultipliedRgba8, SelectionMask, SelectionPaint, SelectionSource, WandRequest,
    lasso_selection_signed, paint_selection, transform_raster, translate_artwork, wand_selection,
};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

pub(crate) enum EditFailure {
    Rejected(String),
    Fatal(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PickerSource {
    Artwork(EditSource),
    Display(Option<nyatidraw_api::LayerTreeNodeId>),
}

pub(crate) fn sample_picker_pixel(
    tiles: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    point: [i32; 2],
    source: PickerSource,
) -> Result<[u8; 4], EditFailure> {
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
    .map_err(|error| EditFailure::Rejected(format!("{error:?}")))
}

pub(crate) fn picker_color(pixel: [u8; 4]) -> Option<[u8; 4]> {
    nyatidraw_tiles::color::linear_premultiplied_to_srgb8(pixel).map(|mut color| {
        // Pick RGB only; brush opacity remains an independent control.
        color[3] = 255;
        color
    })
}

pub(crate) struct EditOutcome {
    pub(crate) tree: Option<LayerTree>,
    pub(crate) active_layer: Option<LayerId>,
    pub(crate) sampled_color: Option<[u8; 4]>,
    pub(crate) tiles: Option<TileSnapshot>,
    pub(crate) canvas: Option<CanvasSpec>,
    pub(crate) selection: Option<Arc<SelectionMask>>,
    pub(crate) history: nyatidraw_api::HistoryProjection,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) fn execute(
    command: EditCommand,
    target: LayerId,
    canvas: CanvasSpec,
    tree: &LayerTree,
    selection: &mut Option<Arc<SelectionMask>>,
    session: &mut HeadlessStrokeSession,
    next_id: &mut u128,
    db: &ProjectDb,
) -> Result<EditOutcome, EditFailure> {
    execute_with_clipboard(
        command,
        target,
        canvas,
        tree,
        selection,
        session,
        next_id,
        db,
        &mut crate::artwork_clipboard::SystemClipboard,
    )
}

// A worker consumes its command once; retain the ownership boundary even when
// a variant's payload can currently be read without moving it.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
pub(crate) fn execute_with_clipboard(
    command: EditCommand,
    target: LayerId,
    canvas: CanvasSpec,
    tree: &LayerTree,
    selection: &mut Option<Arc<SelectionMask>>,
    session: &mut HeadlessStrokeSession,
    next_id: &mut u128,
    db: &ProjectDb,
    clipboard: &mut dyn crate::artwork_clipboard::ArtworkClipboard,
) -> Result<EditOutcome, EditFailure> {
    let reject = |error| EditFailure::Rejected(format!("{error:?}"));
    let limits = EditLimits::default();
    if matches!(
        command,
        EditCommand::ClearActiveLayer
            | EditCommand::DeleteSelectedPixels
            | EditCommand::CutSelection
            | EditCommand::Transform(_)
    ) && tree.raster(target).is_some_and(|layer| layer.alpha_locked)
    {
        return Err(EditFailure::Rejected(
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
        return Err(EditFailure::Rejected(
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
            return Err(EditFailure::Rejected(
                "FreeTransform requires its transaction worker".into(),
            ));
        }
        EditCommand::CopySelection | EditCommand::CutSelection => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| EditFailure::Rejected("먼저 복사할 영역을 선택하세요.".into()))?;
            let fragment =
                nyatidraw_paint_cpu::copy_selection(session.tiles(), tree, target, mask, limits)
                    .map_err(reject)?;
            if command == EditCommand::CutSelection {
                transformed = Some(
                    nyatidraw_paint_cpu::cut_selection(session.tiles(), tree, target, mask, limits)
                        .map_err(reject)?,
                );
            }
            // Clipboard publication and all cut preflight must succeed before
            // the durable commit below can remove any original artwork.
            clipboard.write(&fragment).map_err(EditFailure::Rejected)?;
            None
        }
        EditCommand::PasteSelection => {
            let fragment = clipboard.read().map_err(EditFailure::Rejected)?;
            let id = crate::native_canvas::next_layer_node_id(tree)
                .map_err(EditFailure::Rejected)?
                .max(*next_id);
            let layer = LayerId(id);
            let mut next_tree = tree.clone();
            let (parent, index) = tree
                .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(target))
                .ok_or_else(|| EditFailure::Rejected("붙여넣기 위치 레이어가 없습니다.".into()))?;
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
                .map_err(|e| EditFailure::Rejected(format!("Paste layer: {e:?}")))?;
            transformed = Some(
                nyatidraw_paint_cpu::paste_fragment(
                    session.tiles(),
                    &next_tree,
                    layer,
                    &fragment,
                    limits,
                )
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
            let pixel = sample_picker_pixel(session.tiles(), tree, target, point, source)?;
            let color = picker_color(pixel).ok_or_else(|| {
                EditFailure::Rejected("투명한 픽셀입니다. 현재 색상을 유지합니다.".into())
            })?;
            sampled_color = Some(color);
            None
        }
        EditCommand::ClearActiveLayer => {
            transformed = Some(
                nyatidraw_paint_cpu::clear_raster(session.tiles(), tree, target).map_err(reject)?,
            );
            None
        }
        EditCommand::DeleteSelectedPixels => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| EditFailure::Rejected("선택 영역이 없습니다.".into()))?;
            transformed = Some(
                nyatidraw_paint_cpu::clear_selection(session.tiles(), tree, target, mask, limits)
                    .map_err(reject)?,
            );
            None
        }
        EditCommand::SelectAll | EditCommand::InvertSelection => {
            let (origin, size) =
                selection_domain(canvas, session.tiles(), target, selection.as_deref())?;
            let mask = if command == EditCommand::SelectAll || selection.is_none() {
                nyatidraw_paint_cpu::selection_all(origin, size, limits)
            } else {
                nyatidraw_paint_cpu::invert_selection(
                    selection.as_deref().expect("selection exists"),
                    origin,
                    size,
                    limits,
                )
            }
            .map_err(reject)?;
            *selection = Some(Arc::new(mask));
            None
        }
        EditCommand::GrowSelection { radius } | EditCommand::ShrinkSelection { radius } => {
            let mask = selection
                .as_deref()
                .ok_or_else(|| EditFailure::Rejected("먼저 영역을 선택하세요.".into()))?;
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
                .ok_or_else(|| {
                    EditFailure::Rejected("선택 작업 메모리 한도를 초과했습니다.".into())
                })?;
            let updated = nyatidraw_paint_cpu::select_layer_alpha(
                session.tiles(),
                tree,
                target,
                alpha_limits,
            )
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
                    after: session.tiles().clone(),
                    changed_tiles: Vec::new(),
                });
            }
            None
        }
        EditCommand::CropPageToSelection => {
            let ([left, top], [width_px, height_px]) = selection
                .as_ref()
                .and_then(|mask| mask.bounds_signed())
                .ok_or_else(|| EditFailure::Rejected("선택 영역이 없습니다.".into()))?;
            let next = CanvasSpec {
                width_px,
                height_px,
                ..canvas
            };
            validate_page(next)?;
            let offset = [left, top].map(i32::checked_neg);
            let [Some(x), Some(y)] = offset else {
                return Err(EditFailure::Rejected("Crop coordinate overflow".into()));
            };
            transformed = Some(translate_artwork(session.tiles(), [x, y], limits).map_err(reject)?);
            if next != canvas {
                changed_canvas = Some(next);
            }
            None
        }
        EditCommand::Transform(transform) => {
            transformed = Some(
                transform_raster(
                    session.tiles(),
                    tree,
                    target,
                    selection.as_deref(),
                    transform,
                    limits,
                )
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
                session.tiles(),
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
                return Err(EditFailure::Rejected("뺄 선택 영역이 없습니다.".into()));
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
                return Err(EditFailure::Rejected(
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
                    session.tiles(),
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
            pause_scratch_probe()?;
            let source = match source {
                EditSource::ActiveLayer => SelectionSource::ActiveLayer,
                EditSource::ReferenceLayers => SelectionSource::ReferenceLayers,
                EditSource::AllVisible => SelectionSource::AllVisible,
            };
            transformed = Some(
                nyatidraw_paint_cpu::flood_fill(
                    session.tiles(),
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
    let mut tiles = None;
    if let Some(paint) = paint {
        pause_scratch_probe()?;
        let mask = flood_mask
            .as_ref()
            .or(selection.as_ref())
            .ok_or_else(|| EditFailure::Rejected("NoSelection".into()))?;
        transformed = Some(
            paint_selection(session.tiles(), tree, target, mask, paint, limits).map_err(reject)?,
        );
    }
    if let Some(painted) = transformed
        && (!painted.changed_tiles.is_empty() || changed_canvas.is_some() || changed_tree.is_some())
    {
        let next = next_id
            .checked_add(1)
            .ok_or_else(|| EditFailure::Rejected("IdentifierExhausted".into()))?;
        let batch = session
            .prepare_structural_change(
                SnapshotId(*next_id),
                HistoryNodeId(*next_id),
                crate::native_canvas::system_timestamp_ns(),
                painted.after.clone(),
            )
            .map_err(|error| EditFailure::Fatal(format!("edit prepare: {error:?}")))?;
        if let Some(tree) = &changed_tree {
            db.commit_structural_with_layer_tree(&batch, tree)
        } else {
            changed_canvas.map_or_else(
                || db.commit_structural(&batch),
                |canvas| db.commit_structural_with_canvas(&batch, canvas),
            )
        }
        .map_err(|error| EditFailure::Fatal(format!("edit commit: {error}")))?;
        session
            .accept_structural_change(&batch)
            .map_err(|error| EditFailure::Fatal(format!("edit accept: {error:?}")))?;
        *next_id = next;
        tiles = Some(painted.after);
    }
    if clear_after_transform {
        *selection = None;
    }
    Ok(EditOutcome {
        tree: changed_tree,
        active_layer: pasted_layer,
        sampled_color,
        tiles,
        canvas: changed_canvas,
        selection: selection.clone(),
        history: session.history_projection(),
    })
}

// All/invert have an explicit finite document domain, not the current viewport.
// Include active artwork's stored tile extents so off-page anatomy is reachable.
fn selection_domain(
    canvas: CanvasSpec,
    tiles: &TileSnapshot,
    target: LayerId,
    selection: Option<&SelectionMask>,
) -> Result<([i32; 2], [u32; 2]), EditFailure> {
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
        EditFailure::Rejected(
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

fn validate_page(canvas: CanvasSpec) -> Result<(), EditFailure> {
    if canvas.validate().is_err()
        || u64::from(canvas.width_px) * u64::from(canvas.height_px)
            > nyatidraw_tiles::MAX_FLATTENED_PIXELS
    {
        return Err(EditFailure::Rejected(
            "페이지 크기가 출력 한도를 넘거나 잘못되었습니다.".into(),
        ));
    }
    Ok(())
}

// A scratch-only acceptance barrier proves Save/Close behavior while real work
// is pending, without putting a timing sleep on the native input/render thread.
pub(crate) fn probe_project() -> Option<std::path::PathBuf> {
    if !matches!(
        std::env::var("NAYATI_EDIT_PROBE").ok().as_deref(),
        Some("paused-fill" | "selected-brush" | "lasso-guide" | "close-native-fill")
    ) {
        return None;
    }
    let project = std::path::PathBuf::from(std::env::args_os().nth(1)?);
    if !project
        .parent()?
        .join(".nyatidraw-scratch-edit-probe")
        .is_file()
    {
        return None;
    }
    (project.file_name()?.to_str()? == "async-edit-scratch.ntdr").then_some(project)
}

fn pause_scratch_probe() -> Result<(), EditFailure> {
    if std::env::var("NAYATI_EDIT_PROBE").ok().as_deref() != Some("paused-fill") {
        return Ok(());
    }
    let Some(project) = probe_project() else {
        return Ok(());
    };
    let release = project.with_extension("edit-release");
    println!(
        "native-canvas event=edit-probe-paused release={}",
        release.display()
    );
    let started = std::time::Instant::now();
    while !release.is_file() {
        if started.elapsed() > std::time::Duration::from_mins(3) {
            return Err(EditFailure::Rejected("Scratch edit barrier timeout".into()));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// Only a marked scratch file can suspend the writer for UI responsiveness
/// acceptance. The native input and renderer threads never wait at this gate.
pub(crate) fn pause_artwork_probe(kind: &str) -> Result<(), String> {
    if std::env::var("NAYATI_ARTWORK_PAUSE").ok().as_deref() != Some(kind) {
        return Ok(());
    }
    let Some(project) = std::env::args_os().nth(1).map(std::path::PathBuf::from) else {
        return Ok(());
    };
    let expected_name = if kind == "page" {
        "page-scratch.ntdr"
    } else {
        "edit-source-scratch.ntdr"
    };
    if project.file_name().and_then(|name| name.to_str()) != Some(expected_name)
        || !project
            .parent()
            .is_some_and(|parent| parent.join(".nyatidraw-scratch-artwork-probe").is_file())
    {
        return Ok(());
    }
    let release = project.with_extension("artwork-release");
    println!(
        "native-canvas event=artwork-probe-paused kind={kind} release={}",
        release.display()
    );
    let started = std::time::Instant::now();
    while !release.is_file() {
        if started.elapsed() > std::time::Duration::from_mins(3) {
            return Err("Scratch artwork barrier timeout".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

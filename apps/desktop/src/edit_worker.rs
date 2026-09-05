//! Writer-thread selection state and durable pixel edits. No GPU or UI handles.
use nyatidraw_api::{CanvasSpec, EditCommand, EditSource, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_paint_cpu::{
    EditLimits, PremultipliedRgba8, SelectionMask, SelectionPaint, SelectionSource, WandRequest,
    lasso_selection, paint_selection, wand_selection,
};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

pub(crate) enum EditFailure {
    Rejected(String),
    Fatal(String),
}

pub(crate) struct EditOutcome {
    pub(crate) tiles: Option<TileSnapshot>,
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
    let reject = |error| EditFailure::Rejected(format!("{error:?}"));
    let limits = EditLimits::default();
    let mut flood_mask = None;
    let paint = match command {
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
        EditCommand::SelectLasso { vertices } => {
            let mask = lasso_selection(canvas, &vertices, limits).map_err(reject)?;
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
        let painted =
            paint_selection(session.tiles(), tree, target, mask, paint, limits).map_err(reject)?;
        if !painted.changed_tiles.is_empty() {
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
            db.commit_structural(&batch)
                .map_err(|error| EditFailure::Fatal(format!("edit commit: {error}")))?;
            session
                .accept_structural_change(&batch)
                .map_err(|error| EditFailure::Fatal(format!("edit accept: {error:?}")))?;
            *next_id = next;
            tiles = Some(painted.after);
        }
    }
    Ok(EditOutcome {
        tiles,
        selection: selection.clone(),
        history: session.history_projection(),
    })
}

// A scratch-only acceptance barrier proves Save/Close behavior while real work
// is pending, without putting a timing sleep on the native input/render thread.
pub(crate) fn probe_project() -> Option<std::path::PathBuf> {
    if !matches!(
        std::env::var("NAYATI_EDIT_PROBE").ok().as_deref(),
        Some("paused-fill" | "selected-brush")
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

//! Writer-thread selection state and durable pixel edits. No GPU or UI handles.
use nyatidraw_api::{CanvasSpec, EditCommand, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_paint_cpu::{EditLimits, SelectionMask};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

pub(crate) enum EditFailure {
    Rejected(String),
    Fatal(String),
}

pub(crate) use nyatidraw_editor::pixel_edit::PickerSource;
#[cfg(test)]
pub(crate) use nyatidraw_editor::pixel_edit::picker_color;

pub(crate) fn sample_picker_pixel(
    tiles: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    point: [i32; 2],
    source: PickerSource,
) -> Result<[u8; 4], EditFailure> {
    nyatidraw_editor::pixel_edit::sample_picker_pixel(tiles, tree, target, point, source)
        .map_err(|error| EditFailure::Rejected(error.0))
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
    if matches!(
        command,
        EditCommand::FloodFillAdvanced { .. }
            | EditCommand::FloodFill { .. }
            | EditCommand::FillSelection { .. }
            | EditCommand::GradientSelection { .. }
    ) {
        pause_scratch_probe()?;
    }
    let edited = nyatidraw_editor::pixel_edit::execute_pixel_edit(
        command,
        target,
        canvas,
        tree,
        selection,
        session.tiles(),
        *next_id,
        clipboard,
        EditLimits::default(),
    )
    .map_err(|error| EditFailure::Rejected(error.0))?;
    if let Some(tiles) = &edited.tiles {
        let next = next_id
            .checked_add(1)
            .ok_or_else(|| EditFailure::Rejected("IdentifierExhausted".into()))?;
        let batch = session
            .prepare_structural_change(
                SnapshotId(*next_id),
                HistoryNodeId(*next_id),
                crate::native_canvas::system_timestamp_ns(),
                tiles.clone(),
            )
            .map_err(|error| EditFailure::Fatal(format!("edit prepare: {error:?}")))?;
        if let Some(tree) = &edited.tree {
            db.commit_structural_with_layer_tree(&batch, tree)
        } else {
            edited.canvas.map_or_else(
                || db.commit_structural(&batch),
                |canvas| db.commit_structural_with_canvas(&batch, canvas),
            )
        }
        .map_err(|error| EditFailure::Fatal(format!("edit commit: {error}")))?;
        session
            .accept_structural_change(&batch)
            .map_err(|error| EditFailure::Fatal(format!("edit accept: {error:?}")))?;
        *next_id = next;
    }
    selection.clone_from(&edited.selection);
    Ok(EditOutcome {
        tree: edited.tree,
        active_layer: edited.active_layer,
        sampled_color: edited.sampled_color,
        tiles: edited.tiles,
        canvas: edited.canvas,
        selection: edited.selection,
        history: session.history_projection(),
    })
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

use crate::artwork_clipboard::{ArtworkClipboard, SystemClipboard};
use crate::edit_worker::EditFailure;
use nyatidraw_api::{
    CanvasSpec, HistoryNodeId, HistoryProjection, LayerId, SnapshotId, TransformCommand,
};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::{
    HeadlessStrokeSession,
    transform::{TransformError, TransformSession, TransformStore},
};
use nyatidraw_paint_cpu::SelectionMask;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TransformWorker(TransformSession);

pub(crate) struct TransformOutcome {
    pub(crate) tiles: TileSnapshot,
    pub(crate) tree: LayerTree,
    pub(crate) active_layer: LayerId,
    pub(crate) selection: Option<Arc<SelectionMask>>,
    pub(crate) projection: Option<nyatidraw_api::TransformProjection>,
    pub(crate) committed: bool,
    pub(crate) history: HistoryProjection,
}
struct DurableTransform<'a> {
    session: &'a mut HeadlessStrokeSession,
    db: &'a ProjectDb,
}
impl TransformStore for DurableTransform<'_> {
    fn current_snapshot(&self) -> SnapshotId {
        self.session.current_snapshot()
    }
    fn tiles(&self) -> &TileSnapshot {
        self.session.tiles()
    }
    fn commit(&mut self, id: u128, tiles: TileSnapshot, tree: &LayerTree) -> Result<(), String> {
        let batch = self
            .session
            .prepare_structural_change(
                SnapshotId(id),
                HistoryNodeId(id),
                crate::native_canvas::system_timestamp_ns(),
                tiles,
            )
            .map_err(|e| format!("transform prepare: {e:?}"))?;
        self.db
            .commit_structural_with_layer_tree(&batch, tree)
            .map_err(|e| format!("transform commit: {e}"))?;
        self.session
            .accept_structural_change(&batch)
            .map_err(|e| format!("transform accept: {e:?}"))
    }
}
impl TransformWorker {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute(
        &mut self,
        command: TransformCommand,
        target: LayerId,
        canvas: CanvasSpec,
        tree: &LayerTree,
        selection: &mut Option<Arc<SelectionMask>>,
        session: &mut HeadlessStrokeSession,
        next_id: &mut u128,
        db: &ProjectDb,
    ) -> Result<TransformOutcome, EditFailure> {
        self.execute_with_clipboard(
            command,
            target,
            canvas,
            tree,
            selection,
            session,
            next_id,
            db,
            &mut SystemClipboard,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_with_clipboard(
        &mut self,
        command: TransformCommand,
        target: LayerId,
        canvas: CanvasSpec,
        tree: &LayerTree,
        selection: &mut Option<Arc<SelectionMask>>,
        session: &mut HeadlessStrokeSession,
        next_id: &mut u128,
        db: &ProjectDb,
        clipboard: &mut dyn ArtworkClipboard,
    ) -> Result<TransformOutcome, EditFailure> {
        let result = self
            .0
            .execute(
                command,
                target,
                canvas,
                tree,
                selection,
                &mut DurableTransform { session, db },
                next_id,
                clipboard,
            )
            .map_err(|error| match error {
                TransformError::Rejected(message) => EditFailure::Rejected(message),
                TransformError::Fatal(message) => EditFailure::Fatal(message),
            })?;
        Ok(TransformOutcome {
            tiles: result.tiles,
            tree: result.tree,
            active_layer: result.active_layer,
            selection: result.selection,
            projection: result.projection,
            committed: result.committed,
            history: session.history_projection(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork_clipboard::MemoryClipboard;
    use nyatidraw_api::AffineTransform;
    use nyatidraw_paint_cpu::{EditLimits, selection_all};
    use nyatidraw_paint_cpu::{RasterFragment, flatten_layer_tree_rgba8};

    #[test]
    #[allow(clippy::too_many_lines)]
    fn provisional_paste_cancels_exactly_and_only_current_preview_commits() {
        // Product risk: provisional paste leaks into save/history, Cancel leaves
        // a layer, or a failed/stale preview commits unseen resampled artwork.
        const REOPEN: &str = "NYATIDRAW_TRANSFORM_WORKER_REOPEN";
        const EXPECTED_ROOT: &str = "NYATIDRAW_TRANSFORM_WORKER_EXPECTED_ROOT";
        if let Some(path) = std::env::var_os(REOPEN) {
            let db = ProjectDb::open(std::path::Path::new(&path)).unwrap();
            let reopened = db.load_reopened().unwrap().unwrap();
            let tree = db.load_layer_tree().unwrap().unwrap();
            assert_eq!(tree.root().children.len(), 2);
            assert_eq!(reopened.history().node_count(), 2);
            assert_eq!(
                format!("{:?}", reopened.current_tiles().root()),
                std::env::var(EXPECTED_ROOT).unwrap(),
                "fresh process must preserve the exact committed tile root",
            );
            let canvas = db.load_canvas_spec().unwrap();
            let page = flatten_layer_tree_rgba8(reopened.current_tiles(), &tree, canvas).unwrap();
            let saved_png = nyatidraw_png_io::decode_png(
                &std::path::Path::new(&path).with_extension("png"),
                LayerId(1),
            )
            .unwrap();
            let reopened_png = nyatidraw_png_io::decode_png_bytes(
                &nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
                LayerId(1),
                256,
            )
            .unwrap();
            assert_eq!(saved_png.canvas, canvas);
            assert_eq!(
                saved_png.tiles, reopened_png.tiles,
                "saved PNG pixels must match the fresh process artwork export"
            );
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-transform-worker-{}-{}.ntdr",
            std::process::id(),
            crate::native_canvas::system_timestamp_ns(),
        ));
        let db = ProjectDb::open(&path).unwrap();
        let tree = crate::native_canvas::default_layer_tree();
        let canvas = CanvasSpec {
            width_px: 16,
            height_px: 16,
            pixels_per_inch: 96,
        };
        db.persist_canvas_spec(canvas).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::default());
        let seed = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, session.tiles().clone())
            .unwrap();
        db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
        session.accept_structural_change(&seed).unwrap();
        let original = session.tiles().clone();
        let mut selection = Some(Arc::new(
            selection_all([-1, -1], [2, 2], EditLimits::default()).unwrap(),
        ));
        let before_selection = selection.clone();
        let mut worker = TransformWorker::default();
        let mut next = 10;
        let mut clipboard = MemoryClipboard::default();
        clipboard
            .write(&RasterFragment::new([-2, 2], [2, 2], [128, 0, 0, 128].repeat(4)).unwrap())
            .unwrap();
        let run = |worker: &mut TransformWorker,
                   command,
                   target,
                   selection: &mut _,
                   session: &mut _,
                   next: &mut _,
                   clipboard: &mut MemoryClipboard| {
            worker
                .execute_with_clipboard(
                    command, target, canvas, &tree, selection, session, next, &db, clipboard,
                )
                .unwrap_or_else(|e| match e {
                    EditFailure::Rejected(e) | EditFailure::Fatal(e) => panic!("{e}"),
                })
        };
        let preview = run(
            &mut worker,
            TransformCommand::Paste,
            LayerId(1),
            &mut selection,
            &mut session,
            &mut next,
            &mut clipboard,
        );
        let pasted = preview.active_layer;
        assert_eq!(preview.tree.root().children.len(), 2);
        assert!(!preview.tiles.is_empty());
        assert_eq!(session.tiles(), &original);
        assert_eq!(selection, before_selection);
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles(),
            &original
        );
        let canceled = run(
            &mut worker,
            TransformCommand::Cancel,
            pasted,
            &mut selection,
            &mut session,
            &mut next,
            &mut clipboard,
        );
        assert_eq!(canceled.tree, tree);
        assert_eq!(canceled.active_layer, LayerId(1));
        assert_eq!(canceled.tiles, original);
        assert_eq!(canceled.selection, before_selection);
        assert_eq!(next, 10);
        let first = run(
            &mut worker,
            TransformCommand::Paste,
            LayerId(1),
            &mut selection,
            &mut session,
            &mut next,
            &mut clipboard,
        );
        let generation = first.projection.as_ref().unwrap().generation;
        assert!(
            worker
                .execute_with_clipboard(
                    TransformCommand::Preview(AffineTransform {
                        scale_ppm: [0, 0],
                        ..Default::default()
                    }),
                    pasted,
                    canvas,
                    &tree,
                    &mut selection,
                    &mut session,
                    &mut next,
                    &db,
                    &mut clipboard,
                )
                .is_err()
        );
        assert!(
            worker
                .execute_with_clipboard(
                    TransformCommand::Commit { generation },
                    pasted,
                    canvas,
                    &tree,
                    &mut selection,
                    &mut session,
                    &mut next,
                    &db,
                    &mut clipboard,
                )
                .is_err()
        );
        assert_eq!(session.tiles(), &original);
        let latest = run(
            &mut worker,
            TransformCommand::Preview(AffineTransform {
                offset_milli: [5_000, 1_000],
                rotation_millidegrees: 27_000,
                ..Default::default()
            }),
            pasted,
            &mut selection,
            &mut session,
            &mut next,
            &mut clipboard,
        );
        let committed = run(
            &mut worker,
            TransformCommand::Commit {
                generation: latest.projection.as_ref().unwrap().generation,
            },
            pasted,
            &mut selection,
            &mut session,
            &mut next,
            &mut clipboard,
        );
        assert!(committed.committed);
        assert_eq!(session.tiles(), &latest.tiles);
        assert_eq!(selection, latest.selection);
        assert_eq!(session.history().node_count(), 2);
        let saved_head = session.current_snapshot();
        let saved_selection = selection.clone();
        let saved_next = next;
        // A second transform uses the retained selection and committed tree,
        // but even arbitrary preview/cancel must not create another history op.
        for command in [
            TransformCommand::Begin,
            TransformCommand::Preview(AffineTransform {
                offset_milli: [-7_000, 2_000],
                rotation_millidegrees: -19_000,
                ..Default::default()
            }),
            TransformCommand::Cancel,
        ] {
            let is_cancel = command == TransformCommand::Cancel;
            let outcome = worker
                .execute_with_clipboard(
                    command,
                    pasted,
                    canvas,
                    &committed.tree,
                    &mut selection,
                    &mut session,
                    &mut next,
                    &db,
                    &mut clipboard,
                )
                .unwrap_or_else(|e| match e {
                    EditFailure::Rejected(e) | EditFailure::Fatal(e) => panic!("{e}"),
                });
            assert!(!outcome.committed);
            assert_eq!(session.current_snapshot(), saved_head);
            assert_eq!(session.tiles(), &committed.tiles);
            assert_eq!(selection, saved_selection);
            assert_eq!(next, saved_next);
            if is_cancel {
                assert_eq!(outcome.tiles, committed.tiles);
                assert_eq!(outcome.tree, committed.tree);
                assert_eq!(outcome.active_layer, pasted);
                assert_eq!(outcome.selection, saved_selection);
                assert!(outcome.projection.is_none());
            }
        }
        let undo = session.prepare_undo_cursor().unwrap();
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, original);
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        assert_eq!(
            db.load_layer_tree().unwrap().unwrap(),
            tree,
            "one Undo must remove the provisional pasted layer and all previews"
        );
        let redo = session.prepare_redo_cursor().unwrap();
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, committed.tiles);
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
        assert_eq!(db.load_layer_tree().unwrap().unwrap(), committed.tree);
        assert_eq!(session.current_snapshot(), saved_head);
        let expected_root = format!("{:?}", session.tiles().root());
        let page = flatten_layer_tree_rgba8(session.tiles(), &committed.tree, canvas).unwrap();
        std::fs::write(
            path.with_extension("png"),
            nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
        )
        .unwrap();
        drop(db);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transform_worker::tests::provisional_paste_cancels_exactly_and_only_current_preview_commits", "--nocapture"])
            .env(REOPEN, &path)
            .env(EXPECTED_ROOT, expected_root)
            .status().unwrap();
        assert!(status.success());
        std::fs::remove_file(path.with_extension("png")).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

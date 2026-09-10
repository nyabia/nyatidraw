//! Disposable transform state owned by the project worker, never by the GPU.
use crate::artwork_clipboard::{ArtworkClipboard, SystemClipboard};
use crate::edit_worker::EditFailure;
use nyatidraw_api::{
    AffineTransform, CanvasSpec, ContentRootId, HistoryNodeId, HistoryProjection, LayerId,
    LayerTreeNodeId, SnapshotId, TransformCommand, TransformProjection,
};
use nyatidraw_document::{LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_paint_cpu::{AffineDraft, EditLimits, SelectionMask, paste_fragment, selection_all};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileSnapshot};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TransformWorker {
    draft: Option<Transaction>,
    generation: u64,
}

struct Transaction {
    snapshot: SnapshotId,
    original: TileSnapshot,
    original_tree: LayerTree,
    original_selection: Option<Arc<SelectionMask>>,
    original_layer: LayerId,
    canvas: CanvasSpec,
    source: TileSnapshot,
    tree: LayerTree,
    layer: LayerId,
    selection: Arc<SelectionMask>,
    affine: AffineDraft,
    projection: TransformProjection,
    displayed: TileSnapshot,
    displayed_selection: Arc<SelectionMask>,
    retained_bytes: u64,
}

pub(crate) struct TransformOutcome {
    pub(crate) tiles: TileSnapshot,
    pub(crate) tree: LayerTree,
    pub(crate) active_layer: LayerId,
    pub(crate) selection: Option<Arc<SelectionMask>>,
    pub(crate) projection: Option<TransformProjection>,
    pub(crate) committed: bool,
    pub(crate) history: HistoryProjection,
}

fn rejected(error: impl std::fmt::Debug) -> EditFailure {
    EditFailure::Rejected(format!("Transform: {error:?}"))
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

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::needless_pass_by_value
    )]
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
        match command {
            TransformCommand::Begin | TransformCommand::Paste => {
                if command == TransformCommand::Begin
                    && tree.raster(target).is_some_and(|layer| layer.alpha_locked)
                {
                    return Err(rejected(
                        "투명도 잠금 중에는 변형할 수 없습니다. 잠금을 해제하세요.",
                    ));
                }
                if self.draft.is_some() {
                    return Err(rejected("먼저 현재 변형을 확정하거나 취소하세요."));
                }
                let generation = self.next_generation()?;
                let mut source = session.tiles().clone();
                let mut source_tree = tree.clone();
                let mut layer = target;
                let mut retained_bytes = 0;
                let mask = if command == TransformCommand::Paste {
                    let fragment = clipboard.read().map_err(EditFailure::Rejected)?;
                    let id = crate::native_canvas::next_layer_node_id(tree)
                        .map_err(EditFailure::Rejected)?
                        .max(*next_id);
                    id.checked_add(1)
                        .ok_or_else(|| rejected("IdentifierExhausted"))?;
                    layer = LayerId(id);
                    let (parent, index) = tree
                        .parent_and_index(LayerTreeNodeId::Raster(target))
                        .ok_or_else(|| rejected("붙여넣기 위치 레이어가 없습니다."))?;
                    source_tree
                        .insert(
                            parent,
                            index + 1,
                            LayerTreeNode::Raster(LayerNode {
                                alpha_locked: false,
                                clip_to_below: false,
                                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                                id: layer,
                                name: "붙여넣기".into(),
                                visible: true,
                                locked: false,
                                reference: false,
                                opacity_u16: u16::MAX,
                                content_root: ContentRootId(0),
                            }),
                        )
                        .map_err(rejected)?;
                    let pasted = paste_fragment(
                        session.tiles(),
                        &source_tree,
                        layer,
                        &fragment,
                        EditLimits::default(),
                    )
                    .map_err(rejected)?;
                    retained_bytes =
                        (pasted.changed_tiles.len() as u64).saturating_mul(TILE_BYTE_LEN as u64);
                    source = pasted.after;
                    Arc::new(
                        selection_all(fragment.origin(), fragment.size(), EditLimits::default())
                            .map_err(rejected)?,
                    )
                } else {
                    selection
                        .clone()
                        .ok_or_else(|| rejected("먼저 변형할 영역을 선택하세요."))?
                };
                let (origin, size) = mask
                    .bounds_signed()
                    .ok_or_else(|| rejected("EmptySelection"))?;
                // The draft owns another binary source mask. Include retained
                // source masks/provisional paste payload in the preview budget.
                retained_bytes = retained_bytes.saturating_add(
                    u64::from(mask.dimensions()[0]) * u64::from(mask.dimensions()[1]) * 2,
                );
                let mut affine = AffineDraft::new(
                    session.current_snapshot(),
                    &source,
                    &source_tree,
                    layer,
                    &mask,
                )
                .map_err(rejected)?;
                let transform = AffineTransform::default();
                let result = affine
                    .preview(generation, &transform, preview_limits(retained_bytes)?)
                    .map_err(rejected)?;
                let projection = TransformProjection {
                    generation,
                    transform,
                    origin,
                    size,
                    corners_milli: corners_milli(result.corners),
                    can_commit: true,
                };
                let displayed = result.paint.after.clone();
                let displayed_selection = Arc::new(result.selection.clone());
                self.draft = Some(Transaction {
                    snapshot: session.current_snapshot(),
                    original: session.tiles().clone(),
                    original_tree: tree.clone(),
                    original_selection: selection.clone(),
                    original_layer: target,
                    canvas,
                    source,
                    tree: source_tree,
                    layer,
                    selection: mask,
                    affine,
                    projection,
                    displayed,
                    displayed_selection,
                    retained_bytes,
                });
            }
            TransformCommand::Preview(transform) => {
                let generation = self.next_generation()?;
                let tx = self.draft.as_mut().ok_or_else(|| rejected("NoTransform"))?;
                tx.projection.can_commit = false;
                tx.guard(target, canvas, tree, selection, session)?;
                let [width, height] = tx.displayed_selection.dimensions();
                let retained = tx
                    .retained_bytes
                    .saturating_add(u64::from(width) * u64::from(height));
                let result = tx
                    .affine
                    .preview(generation, &transform, preview_limits(retained)?)
                    .map_err(rejected)?;
                tx.projection.generation = generation;
                tx.projection.transform = transform;
                tx.projection.corners_milli = corners_milli(result.corners);
                tx.projection.can_commit = true;
                tx.displayed = result.paint.after.clone();
                tx.displayed_selection = Arc::new(result.selection.clone());
            }
            TransformCommand::Commit { generation } => {
                let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
                if !tx.projection.can_commit {
                    return Err(rejected("미리보기를 다시 갱신한 뒤 확정하세요."));
                }
                tx.guard(target, canvas, tree, selection, session)?;
                let result = tx
                    .affine
                    .prepare_commit(
                        generation,
                        tx.snapshot,
                        &tx.source,
                        &tx.tree,
                        tx.layer,
                        &tx.selection,
                    )
                    .map_err(rejected)?;
                let after = result.paint.after.clone();
                let retained_selection = Arc::new(result.selection.clone());
                let changed = after.root() != tx.original.root() || tx.tree != tx.original_tree;
                if changed {
                    // Reserve both provisional layer and operation identifiers
                    // only at successful commit; Cancel consumes no document id.
                    let id = (*next_id).max(tx.layer.0.saturating_add(1));
                    let next = id
                        .checked_add(1)
                        .ok_or_else(|| rejected("IdentifierExhausted"))?;
                    let batch = session
                        .prepare_structural_change(
                            SnapshotId(id),
                            HistoryNodeId(id),
                            crate::native_canvas::system_timestamp_ns(),
                            after.clone(),
                        )
                        .map_err(|e| EditFailure::Fatal(format!("transform prepare: {e:?}")))?;
                    db.commit_structural_with_layer_tree(&batch, &tx.tree)
                        .map_err(|e| EditFailure::Fatal(format!("transform commit: {e}")))?;
                    session
                        .accept_structural_change(&batch)
                        .map_err(|e| EditFailure::Fatal(format!("transform accept: {e:?}")))?;
                    *next_id = next;
                }
                *selection = Some(retained_selection);
                let outcome = TransformOutcome {
                    tiles: after,
                    tree: tx.tree.clone(),
                    active_layer: tx.layer,
                    selection: selection.clone(),
                    projection: None,
                    committed: changed,
                    history: session.history_projection(),
                };
                self.draft = None;
                return Ok(outcome);
            }
            TransformCommand::Cancel => {
                let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
                // A stale source may not be visually rolled back over newer
                // artwork. Normal actor routing prevents edits during drafts.
                tx.guard(target, canvas, tree, selection, session)?;
                let outcome = TransformOutcome {
                    tiles: tx.original.clone(),
                    tree: tx.original_tree.clone(),
                    active_layer: tx.original_layer,
                    selection: tx.original_selection.clone(),
                    projection: None,
                    committed: false,
                    history: session.history_projection(),
                };
                self.draft = None;
                return Ok(outcome);
            }
        }
        let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
        Ok(TransformOutcome {
            tiles: tx.displayed.clone(),
            tree: tx.tree.clone(),
            active_layer: tx.layer,
            selection: Some(tx.displayed_selection.clone()),
            projection: Some(tx.projection.clone()),
            committed: false,
            history: session.history_projection(),
        })
    }

    fn next_generation(&mut self) -> Result<u64, EditFailure> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| rejected("GenerationExhausted"))?;
        Ok(self.generation)
    }
}

impl Transaction {
    #[allow(clippy::ref_option)] // Equality against the pinned optional Arc is the source guard.
    fn guard(
        &self,
        target: LayerId,
        canvas: CanvasSpec,
        tree: &LayerTree,
        selection: &Option<Arc<SelectionMask>>,
        session: &HeadlessStrokeSession,
    ) -> Result<(), EditFailure> {
        if target != self.layer
            || canvas != self.canvas
            || tree != &self.original_tree
            || selection != &self.original_selection
            || session.current_snapshot() != self.snapshot
            || session.tiles().root() != self.original.root()
        {
            return Err(rejected("SourceChanged"));
        }
        Ok(())
    }
}

fn preview_limits(retained_bytes: u64) -> Result<EditLimits, EditFailure> {
    let mut limits = EditLimits::default();
    limits.max_workspace_bytes = limits
        .max_workspace_bytes
        .checked_sub(retained_bytes)
        .ok_or_else(|| rejected("LimitExceeded"))?;
    Ok(limits)
}

#[allow(clippy::cast_possible_truncation)]
fn corners_milli(corners: [[f64; 2]; 4]) -> [[i64; 2]; 4] {
    // Affine preflight has already bounded finite geometry to signed pixels.
    corners.map(|point| point.map(|value| (value * 1_000.0).round() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork_clipboard::MemoryClipboard;
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

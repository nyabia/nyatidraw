#![forbid(unsafe_code)]

use std::collections::BTreeMap;

mod projection;

pub use projection::{ProjectionError, ProjectionState};

use nyatidraw_api::{
    HISTORY_PROJECTION_MAX_ENTRIES, HistoryBranchProjection, HistoryEntryProjection, HistoryNodeId,
    HistoryOperationLabel, HistoryProjection, LayerId, SnapshotId,
};
use nyatidraw_brush::{BrushSnapshot, RecordedStroke};
use nyatidraw_history::{History, HistoryError, HistoryNode, OperationRecord};
use nyatidraw_input::StylusSample;
use nyatidraw_project::{
    CommitBatchError, ProjectCommitBatch, ProjectHistoryCursor, ProjectStructuralBatch,
    ReopenedProject,
};
use nyatidraw_stroke::{
    MaterializationError, MaterializationStrategy, StrokeColor, StrokeCommit, StrokeCommitError,
    materialize_with_strategy,
};
use nyatidraw_tiles::TileSnapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleState {
    Starting,
    Running,
    Quiescing,
    FinishingActiveStroke,
    MaterializingLatest,
    CommittingLatest,
    EnsuringExports,
    ClosingRepository,
    Exited,
}

impl LifecycleState {
    #[must_use]
    pub const fn accepts_new_strokes(self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HeadlessStrokeError {
    Seal(StrokeCommitError),
    Materialize(MaterializationError),
    Batch(CommitBatchError),
    History(HistoryError),
    SnapshotChanged,
    ContentRootChanged,
    CursorUnavailable,
    CursorChanged,
    CursorTilesChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryCursorOperation {
    Undo,
    RedoTo(HistoryNodeId),
}

/// A cursor move that has been validated in memory but not yet made durable.
///
/// Persist `target` through the repository before accepting it in the session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedHistoryCursorMove {
    operation: HistoryCursorOperation,
    source: ProjectHistoryCursor,
    target: ProjectHistoryCursor,
}

impl PreparedHistoryCursorMove {
    #[must_use]
    pub const fn target(&self) -> ProjectHistoryCursor {
        self.target
    }
}

/// Storage-neutral editor seam for preparing and accepting a closed stroke.
///
/// Preparation is deterministic and does not mutate editor state. A caller can
/// durably store the returned batch and only then call `accept_committed`, so a
/// storage failure cannot silently advance the live content root.
#[derive(Clone, Debug)]
pub struct HeadlessStrokeSession {
    current_snapshot: SnapshotId,
    tiles: TileSnapshot,
    history: History,
    current_cursor: ProjectHistoryCursor,
    cursors: BTreeMap<Option<HistoryNodeId>, ProjectHistoryCursor>,
}

impl HeadlessStrokeSession {
    #[must_use]
    pub fn new(current_snapshot: SnapshotId, tiles: TileSnapshot) -> Self {
        let current_cursor = ProjectHistoryCursor {
            snapshot_id: current_snapshot,
            history_head: None,
            root: tiles.root(),
        };
        Self {
            current_snapshot,
            history: History::new(tiles.root().id),
            tiles,
            current_cursor,
            cursors: BTreeMap::from([(None, current_cursor)]),
        }
    }

    /// Resumes from a storage-neutral, fully reconstructed durable state.
    ///
    /// The reopened history is retained instead of creating a new linear
    /// cursor, so a later append preserves existing redo branches.
    #[must_use]
    pub fn from_reopened(reopened: ReopenedProject) -> Self {
        let reopened = reopened.into_parts();
        Self {
            current_snapshot: reopened.current_snapshot,
            tiles: reopened.current_tiles,
            history: reopened.history,
            current_cursor: reopened.current_cursor,
            cursors: reopened.cursors,
        }
    }

    /// Seals and CPU-materializes a closed round-brush sample stream.
    ///
    /// # Errors
    ///
    /// Returns an error when sealing, replay, or cross-object invariants fail.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_round_stroke(
        &self,
        next_snapshot: SnapshotId,
        history_node: HistoryNodeId,
        timestamp_ns: u64,
        layer: LayerId,
        brush: BrushSnapshot,
        recorded: RecordedStroke,
        color: StrokeColor,
        samples: Vec<StylusSample>,
    ) -> Result<ProjectCommitBatch, HeadlessStrokeError> {
        let stroke = StrokeCommit::seal(
            self.current_snapshot,
            layer,
            &self.tiles,
            brush,
            recorded,
            color,
            samples,
        )
        .map_err(HeadlessStrokeError::Seal)?;
        let materialized =
            materialize_with_strategy(MaterializationStrategy::CpuReplay, &stroke, &self.tiles)
                .map_err(HeadlessStrokeError::Materialize)?;
        let node = HistoryNode {
            id: history_node,
            parent: self.history.head(),
            timestamp_ns,
            operation: OperationRecord::Stroke {
                commit_id: stroke.id,
                recorded: stroke.recorded.clone(),
            },
            before_root: stroke.before_root.id,
            after_root: materialized.after.root().id,
        };
        ProjectCommitBatch::new(
            next_snapshot,
            self.tiles.clone(),
            stroke,
            materialized,
            node,
        )
        .map_err(HeadlessStrokeError::Batch)
    }

    /// Advances editor state only after the caller has durably committed the
    /// exact prepared batch.
    ///
    /// # Errors
    ///
    /// Returns an error if editor state changed after preparation or history
    /// can no longer accept the node.
    pub fn accept_committed(
        &mut self,
        batch: &ProjectCommitBatch,
    ) -> Result<(), HeadlessStrokeError> {
        self.accept_materialized(batch)
    }

    /// Prepares an exact content-root transition for a non-stroke artwork
    /// operation. The caller must store it durably before accepting it.
    ///
    /// # Errors
    ///
    /// Returns an error if the supplied after-root cannot form a valid
    /// structural history transition from the current session state.
    pub fn prepare_structural_change(
        &self,
        next_snapshot: SnapshotId,
        history_node: HistoryNodeId,
        timestamp_ns: u64,
        after: TileSnapshot,
    ) -> Result<ProjectStructuralBatch, HeadlessStrokeError> {
        let node = HistoryNode {
            id: history_node,
            parent: self.history.head(),
            timestamp_ns,
            operation: OperationRecord::StructuralChange,
            before_root: self.tiles.root().id,
            after_root: after.root().id,
        };
        ProjectStructuralBatch::new(next_snapshot, self.tiles.clone(), after, node)
            .map_err(HeadlessStrokeError::Batch)
    }

    /// Advances the session after the exact structural batch is durable.
    ///
    /// # Errors
    ///
    /// Returns an error when the session changed after preparation or history
    /// cannot accept the supplied structural node.
    pub fn accept_structural_change(
        &mut self,
        batch: &ProjectStructuralBatch,
    ) -> Result<(), HeadlessStrokeError> {
        if batch.history_node.parent != self.history.head()
            || batch.before.root() != self.tiles.root()
            || batch.snapshot_id.0 <= self.current_snapshot.0
        {
            return Err(HeadlessStrokeError::SnapshotChanged);
        }
        self.history
            .append(batch.history_node.clone())
            .map_err(HeadlessStrokeError::History)?;
        self.current_snapshot = batch.snapshot_id;
        self.tiles = batch.after.clone();
        let cursor = ProjectHistoryCursor {
            snapshot_id: batch.snapshot_id,
            history_head: Some(batch.history_node.id),
            root: self.tiles.root(),
        };
        self.current_cursor = cursor;
        self.cursors.insert(cursor.history_head, cursor);
        Ok(())
    }

    /// Advances the in-memory materialization root without claiming durable
    /// storage. This is the explicit seam for a live preview worker while the
    /// project writer remains disconnected.
    ///
    /// # Errors
    ///
    /// Returns an error if editor state changed after preparation or history
    /// can no longer accept the node.
    pub fn accept_ephemeral_materialization(
        &mut self,
        batch: &ProjectCommitBatch,
    ) -> Result<(), HeadlessStrokeError> {
        self.accept_materialized(batch)
    }

    fn accept_materialized(
        &mut self,
        batch: &ProjectCommitBatch,
    ) -> Result<(), HeadlessStrokeError> {
        if batch.stroke.parent_snapshot != self.current_snapshot {
            return Err(HeadlessStrokeError::SnapshotChanged);
        }
        if batch.before.root() != self.tiles.root() {
            return Err(HeadlessStrokeError::ContentRootChanged);
        }
        self.history
            .append(batch.history_node.clone())
            .map_err(HeadlessStrokeError::History)?;
        self.current_snapshot = batch.snapshot_id;
        self.tiles = batch.materialized.after.clone();
        let cursor = ProjectHistoryCursor {
            snapshot_id: batch.snapshot_id,
            history_head: Some(batch.history_node.id),
            root: self.tiles.root(),
        };
        self.current_cursor = cursor;
        self.cursors.insert(cursor.history_head, cursor);
        Ok(())
    }

    /// Prepares a non-destructive undo cursor move.
    ///
    /// The returned target must be durably persisted before calling
    /// [`Self::accept_history_cursor_move`] with its loaded immutable tiles.
    ///
    /// # Errors
    ///
    /// Returns the normal history error at the initial root, or
    /// `CursorUnavailable` when the target has no persisted snapshot.
    pub fn prepare_undo_cursor(&self) -> Result<PreparedHistoryCursorMove, HeadlessStrokeError> {
        let mut history = self.history.clone();
        history.undo().map_err(HeadlessStrokeError::History)?;
        let head = history.head();
        let target = self
            .cursors
            .get(&head)
            .copied()
            .ok_or(HeadlessStrokeError::CursorUnavailable)?;
        Ok(PreparedHistoryCursorMove {
            operation: HistoryCursorOperation::Undo,
            source: self.current_cursor,
            target,
        })
    }

    /// Prepares an explicit, non-destructive redo branch selection.
    ///
    /// # Errors
    ///
    /// Returns a history error for an invalid branch selection or
    /// `CursorUnavailable` when the selected branch has no persisted snapshot.
    pub fn prepare_redo_to_cursor(
        &self,
        candidate: HistoryNodeId,
    ) -> Result<PreparedHistoryCursorMove, HeadlessStrokeError> {
        let mut history = self.history.clone();
        history
            .redo_to(candidate)
            .map_err(HeadlessStrokeError::History)?;
        let target = self
            .cursors
            .get(&Some(candidate))
            .copied()
            .ok_or(HeadlessStrokeError::CursorUnavailable)?;
        Ok(PreparedHistoryCursorMove {
            operation: HistoryCursorOperation::RedoTo(candidate),
            source: self.current_cursor,
            target,
        })
    }

    /// Prepares redo when the current cursor has exactly one child branch.
    ///
    /// # Errors
    ///
    /// Returns the normal nothing-to-redo or ambiguous-redo history error, or
    /// `CursorUnavailable` when the selected child has no persisted cursor.
    pub fn prepare_redo_cursor(&self) -> Result<PreparedHistoryCursorMove, HeadlessStrokeError> {
        let mut history = self.history.clone();
        history.redo().map_err(HeadlessStrokeError::History)?;
        let candidate = history
            .head()
            .ok_or(HeadlessStrokeError::CursorUnavailable)?;
        let target = self
            .cursors
            .get(&Some(candidate))
            .copied()
            .ok_or(HeadlessStrokeError::CursorUnavailable)?;
        Ok(PreparedHistoryCursorMove {
            operation: HistoryCursorOperation::RedoTo(candidate),
            source: self.current_cursor,
            target,
        })
    }

    /// Accepts a cursor move only after its repository transaction succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if the session changed after preparation, or if the
    /// supplied tiles do not exactly match the persisted target root.
    pub fn accept_history_cursor_move(
        &mut self,
        prepared: PreparedHistoryCursorMove,
        tiles: TileSnapshot,
    ) -> Result<(), HeadlessStrokeError> {
        if prepared.source != self.current_cursor {
            return Err(HeadlessStrokeError::CursorChanged);
        }
        if tiles.root() != prepared.target.root {
            return Err(HeadlessStrokeError::CursorTilesChanged);
        }
        let root = match prepared.operation {
            HistoryCursorOperation::Undo => self.history.undo(),
            HistoryCursorOperation::RedoTo(candidate) => self.history.redo_to(candidate),
        }
        .map_err(HeadlessStrokeError::History)?;
        if root != prepared.target.root.id || self.history.head() != prepared.target.history_head {
            return Err(HeadlessStrokeError::CursorChanged);
        }
        self.current_snapshot = prepared.target.snapshot_id;
        self.current_cursor = prepared.target;
        self.tiles = tiles;
        Ok(())
    }

    #[must_use]
    pub const fn current_snapshot(&self) -> SnapshotId {
        self.current_snapshot
    }

    #[must_use]
    pub const fn history(&self) -> &History {
        &self.history
    }

    /// Produces a bounded current-branch ancestry for editor chrome directly
    /// from the already-open session. This does not enumerate storage and
    /// exposes only semantic labels and direct redo IDs, never artwork roots.
    #[must_use]
    pub fn history_projection(&self) -> HistoryProjection {
        self.history_projection_after(None)
    }

    /// Projects one bounded page of redo candidates at the current cursor.
    #[must_use]
    pub fn history_projection_after(&self, after: Option<HistoryNodeId>) -> HistoryProjection {
        let mut entries = Vec::with_capacity(HISTORY_PROJECTION_MAX_ENTRIES);
        let mut cursor = self.history.head();
        while let Some(id) = cursor {
            if entries.len() == HISTORY_PROJECTION_MAX_ENTRIES {
                break;
            }
            let Some(node) = self.history.node(id) else {
                // `History` validates its cursor internally. Keeping this
                // fallback honest avoids fabricating rows if a future caller
                // changes that contract.
                break;
            };
            entries.push(HistoryEntryProjection {
                operation: match node.operation {
                    OperationRecord::Stroke { .. } => HistoryOperationLabel::Stroke,
                    OperationRecord::StructuralChange => HistoryOperationLabel::Structural,
                },
            });
            cursor = node.parent;
        }

        let has_older_entries = cursor.is_some() || entries.len() == HISTORY_PROJECTION_MAX_ENTRIES;
        if !has_older_entries {
            entries.push(HistoryEntryProjection {
                operation: HistoryOperationLabel::Initial,
            });
        }
        let mut candidates = self.history.redo_candidates_after(after);
        let redo_branches = candidates
            .by_ref()
            .take(HISTORY_PROJECTION_MAX_ENTRIES)
            .filter_map(|id| self.history.node(id))
            .map(|node| HistoryBranchProjection {
                node: node.id,
                operation: match node.operation {
                    OperationRecord::Stroke { .. } => HistoryOperationLabel::Stroke,
                    OperationRecord::StructuralChange => HistoryOperationLabel::Structural,
                },
                timestamp_ns: node.timestamp_ns,
            })
            .collect();
        HistoryProjection {
            entries,
            current: 0,
            has_older_entries,
            redo_branches,
            redo_page_after: after,
            has_more_redo_branches: candidates.next().is_some(),
        }
    }

    #[must_use]
    pub const fn tiles(&self) -> &TileSnapshot {
        &self.tiles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    #[test]
    fn first_edit_can_undo_to_initial_artwork_without_restarting() {
        // Product risk: a fresh editor session must retain its initial cursor,
        // or the first imported/background artwork cannot be undone until reopen.
        let initial = TileSnapshot::empty();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), initial.clone());
        let painted = TileSnapshot::from_tiles([(
            TileKey {
                layer: LayerId(1),
                mip: 0,
                x: 0,
                y: 0,
            },
            vec![255; TILE_BYTE_LEN],
        )])
        .expect("valid artwork tile");
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, painted.clone())
            .expect("first edit");
        session
            .accept_structural_change(&batch)
            .expect("accept saved edit");
        let undo = session
            .prepare_undo_cursor()
            .expect("initial cursor retained");
        assert_eq!(undo.target().history_head, None);
        assert_eq!(undo.target().root, initial.root());
        session
            .accept_history_cursor_move(undo, initial)
            .expect("accept saved undo");
        let redo = session
            .prepare_redo_to_cursor(HistoryNodeId(1))
            .expect("first edit branch retained");
        assert_eq!(redo.target().root, painted.root());
    }
}

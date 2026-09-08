//! Transactional 128-operation retention and indexed orphan reclamation.
use super::{
    CURRENT_HISTORY_CURSOR, Envelope, HISTORY, History, INITIAL_HISTORY_CURSOR,
    MAX_REOPEN_HISTORY_NODES, ProjectDb, ProjectHistoryCursor, ProjectOpenError, ReadableTable,
    RecordKind, SNAPSHOTS, STATE, STROKES, canvas_history, decode_envelope, decode_history_cursor,
    decode_history_node, decode_history_node_id, decode_initial_history_cursor,
    decode_project_head, decode_snapshot_id, encode_history_node, encode_initial_history_cursor,
    layer_history,
};
use redb::ReadableTableMetadata;
use std::collections::{BTreeMap, BTreeSet};

impl ProjectDb {
    #[allow(clippy::too_many_lines)]
    pub(super) fn retain_history(
        &self,
        tx: &redb::WriteTransaction,
    ) -> Result<(), ProjectOpenError> {
        let mut table = tx.open_table(HISTORY).map_err(|e| self.io(e))?;
        if table.len().map_err(|e| self.io(e))? <= nyatidraw_history::HISTORY_LIMIT as u64 {
            return Ok(());
        }
        let mut nodes = Vec::new();
        for entry in table.iter().map_err(|e| self.io(e))? {
            let (key, value) = entry.map_err(|e| self.io(e))?;
            if nodes.len() == MAX_REOPEN_HISTORY_NODES {
                return Err(self.corrupt("retention history exceeds migration limit"));
            }
            let envelope = decode_envelope(RecordKind::HistoryNode, value.value())
                .map_err(|e| self.corrupt(e))?;
            let node = decode_history_node(&envelope.payload).map_err(|e| self.corrupt(e))?;
            if decode_history_node_id(key.value()).map_err(|e| self.corrupt(e))? != node.id {
                return Err(self.corrupt("retention history key mismatch"));
            }
            nodes.push(node);
        }
        let mut state = tx.open_table(STATE).map_err(|e| self.io(e))?;
        let current = state
            .get(CURRENT_HISTORY_CURSOR)
            .map_err(|e| self.io(e))?
            .map(|value| value.value().to_vec());
        let initial = state
            .get(INITIAL_HISTORY_CURSOR)
            .map_err(|e| self.io(e))?
            .ok_or_else(|| self.corrupt("retention initial cursor missing"))?
            .value()
            .to_vec();
        let initial = decode_envelope(RecordKind::InitialHistoryCursor, &initial)
            .map_err(|e| self.corrupt(e))?;
        let mut initial =
            decode_initial_history_cursor(&initial.payload).map_err(|e| self.corrupt(e))?;
        let current = match current {
            Some(bytes) => {
                let envelope = decode_envelope(RecordKind::HistoryCursor, &bytes)
                    .map_err(|e| self.corrupt(e))?;
                decode_history_cursor(&envelope.payload).map_err(|e| self.corrupt(e))?
            }
            None => initial,
        };
        let mut history = History::from_persisted(initial.root.id, current.history_head, nodes)
            .map_err(|e| self.corrupt(format!("retention graph: {e:?}")))?;
        if history.current_root() != current.root.id {
            return Err(self.corrupt("retention current root mismatch"));
        }
        let baseline = history.enforce_limit();
        let keep: BTreeSet<_> = history.nodes().map(|node| node.id).collect();
        let mut snapshots = tx.open_table(SNAPSHOTS).map_err(|e| self.io(e))?;
        let mut heads = BTreeMap::new();
        for entry in snapshots.iter().map_err(|e| self.io(e))? {
            let (key, value) = entry.map_err(|e| self.io(e))?;
            let envelope = decode_envelope(RecordKind::SnapshotHead, value.value())
                .map_err(|e| self.corrupt(e))?;
            let head = decode_project_head(&envelope.payload).map_err(|e| self.corrupt(e))?;
            if decode_snapshot_id(key.value()).map_err(|e| self.corrupt(e))? != head.snapshot_id {
                return Err(self.corrupt("retention snapshot key mismatch"));
            }
            heads.insert(head.history_head, head);
        }
        if let Some(id) = baseline {
            let head = heads
                .get(&id)
                .ok_or_else(|| self.corrupt("retention baseline missing"))?;
            initial = ProjectHistoryCursor {
                snapshot_id: head.snapshot_id,
                history_head: None,
                root: head.after_root,
            };
            if initial.root.id != history.initial_root() {
                return Err(self.corrupt("retention baseline root mismatch"));
            }
            let bytes = Envelope::new(
                RecordKind::InitialHistoryCursor,
                encode_initial_history_cursor(initial.snapshot_id, initial.root),
            )
            .encode();
            state
                .insert(INITIAL_HISTORY_CURSOR, bytes.as_slice())
                .map_err(|e| self.io(e))?;
        }
        drop(state);
        // Rewrite retained parent links and remove obsolete branches atomically
        // with the new artwork. Never leave a dangling parent at the cutoff.
        table
            .retain(|key, _| decode_history_node_id(key).is_ok_and(|id| keep.contains(&id)))
            .map_err(|e| self.io(e))?;
        for node in history.nodes() {
            let bytes = Envelope::new(RecordKind::HistoryNode, encode_history_node(node)).encode();
            table
                .insert(node.id.0.to_le_bytes().as_slice(), bytes.as_slice())
                .map_err(|e| self.io(e))?;
        }
        let kept_snapshots: BTreeSet<_> = heads
            .iter()
            .filter(|(id, _)| keep.contains(id))
            .map(|(_, head)| head.snapshot_id)
            .collect();
        let kept_strokes: BTreeSet<_> = heads
            .iter()
            .filter(|(id, _)| keep.contains(id))
            .filter_map(|(_, head)| head.stroke_commit.map(|id| (id.0).0))
            .collect();
        snapshots
            .retain(|key, _| decode_snapshot_id(key).is_ok_and(|id| kept_snapshots.contains(&id)))
            .map_err(|e| self.io(e))?;
        drop(snapshots);
        tx.open_table(STROKES)
            .map_err(|e| self.io(e))?
            .retain(|key, _| kept_strokes.iter().any(|id| id.as_slice() == key))
            .map_err(|e| self.io(e))?;
        for definition in [
            layer_history::SNAPSHOT_LAYERS,
            canvas_history::SNAPSHOT_CANVAS,
        ] {
            // Opening a missing table would falsely enable metadata history.
            if tx.list_tables().map_err(|e| self.io(e))?.any(|handle| {
                use redb::TableHandle;
                handle.name() == definition.name()
            }) {
                tx.open_table(definition)
                    .map_err(|e| self.io(e))?
                    .retain(|key, _| {
                        decode_snapshot_id(key).is_ok_and(|id| {
                            id == initial.snapshot_id || kept_snapshots.contains(&id)
                        })
                    })
                    .map_err(|e| self.io(e))?;
            }
        }
        let mut live = BTreeSet::from([initial.root.hash, current.root.hash]);
        let mut candidates = BTreeSet::new();
        for (id, head) in &heads {
            let set = if keep.contains(id) {
                &mut live
            } else {
                &mut candidates
            };
            set.extend([head.before_root.hash, head.after_root.hash]);
        }
        self.reclaim_roots(tx, &candidates, &live)?;
        Ok(())
    }
}

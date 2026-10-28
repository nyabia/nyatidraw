use std::collections::BTreeSet;

use nyatidraw_document::{AnimationTimeline, LayerTree};
use nyatidraw_project::{
    ANIMATION_SCHEMA_FLAG, MAX_ANIMATION_RECORD_BYTES, decode_animation_timeline,
    encode_animation_timeline,
};

use super::{
    Envelope, META, ProjectDb, ProjectHistoryCursor, ProjectOpenError, ReadableTable, RecordKind,
    SNAPSHOTS, STATE, SnapshotId, TableDefinition, decode_envelope, decode_snapshot_id,
};

pub(super) const SNAPSHOT_ANIMATION: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("snapshot_animation");
pub(super) const CURRENT_ANIMATION: &str = "current_animation_timeline";

#[derive(Clone, Copy)]
pub(super) enum TimelineChange<'a> {
    Inherit,
    Replace(Option<&'a AnimationTimeline>),
}

#[cfg(test)]
mod tests;

impl ProjectDb {
    fn animation_enabled(&self) -> Result<bool, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let metadata = transaction
            .open_table(META)
            .map_err(|error| self.corrupt(error))?;
        Ok(metadata
            .get("schema_version")
            .map_err(|error| self.corrupt(error))?
            .is_some_and(|value| value.value() & ANIMATION_SCHEMA_FLAG != 0))
    }

    fn current_animation_record(&self) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        let enabled = self.animation_enabled()?;
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let value = match transaction.open_table(STATE) {
            Ok(state) => state
                .get(CURRENT_ANIMATION)
                .map_err(|error| self.corrupt(error))?
                .map(|value| self.copy_animation_record(value.value()))
                .transpose()?,
            Err(redb::TableError::TableDoesNotExist(_)) => None,
            Err(error) => return Err(self.corrupt(error)),
        };
        if enabled != value.is_some() {
            return Err(self.corrupt("animation marker/current metadata mismatch"));
        }
        Ok(value)
    }

    fn copy_animation_record(&self, bytes: &[u8]) -> Result<Vec<u8>, ProjectOpenError> {
        if bytes.len() > MAX_ANIMATION_RECORD_BYTES {
            return Err(self.corrupt("animation metadata exceeds byte limit"));
        }
        decode_envelope(RecordKind::AnimationTimeline, bytes)
            .map_err(|error| self.corrupt(error))?;
        Ok(bytes.to_vec())
    }

    fn decode_animation_record(
        &self,
        bytes: &[u8],
        tree: Option<&LayerTree>,
    ) -> Result<Option<AnimationTimeline>, ProjectOpenError> {
        if bytes.len() > MAX_ANIMATION_RECORD_BYTES {
            return Err(self.corrupt("animation metadata exceeds byte limit"));
        }
        let envelope = decode_envelope(RecordKind::AnimationTimeline, bytes)
            .map_err(|error| self.corrupt(error))?;
        if envelope.payload == [1, 0, 0] {
            return Ok(None);
        }
        let tree = tree.ok_or_else(|| self.corrupt("animated snapshot has no layer tree"))?;
        decode_animation_timeline(&envelope.payload, tree).map_err(|error| self.corrupt(error))
    }

    /// Loads the current timeline; absence denotes static artwork.
    /// # Errors
    /// Rejects missing, unsupported, corrupt or dangling animation metadata.
    pub fn load_animation_timeline(&self) -> Result<Option<AnimationTimeline>, ProjectOpenError> {
        let Some(bytes) = self.current_animation_record()? else {
            return Ok(None);
        };
        let tree = self.load_layer_tree()?;
        self.decode_animation_record(&bytes, tree.as_ref())
    }

    /// Loads animation metadata belonging to an exact persisted history cursor.
    /// # Errors
    /// Rejects forged cursors, missing records and invalid cel references.
    pub fn load_cursor_animation_timeline(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<Option<AnimationTimeline>, ProjectOpenError> {
        match cursor.history_head {
            Some(_) => self.validate_cursor_snapshot(cursor)?,
            None => self.validate_initial_cursor(cursor)?,
        }
        let Some(bytes) = self.cursor_animation_record(cursor)? else {
            return Ok(None);
        };
        let tree = self.load_cursor_layer_tree(cursor)?;
        self.decode_animation_record(&bytes, tree.as_ref())
    }

    pub(super) fn cursor_animation_record(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        if !self.animation_enabled()? {
            return Ok(None);
        }
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let table = transaction
            .open_table(SNAPSHOT_ANIMATION)
            .map_err(|error| self.corrupt(error))?;
        let value = table
            .get(cursor.snapshot_id.0.to_le_bytes().as_slice())
            .map_err(|error| self.corrupt(error))?
            .ok_or_else(|| self.corrupt("snapshot animation missing"))?;
        self.copy_animation_record(value.value()).map(Some)
    }

    pub(super) fn capture_snapshot_animation(
        &self,
        transaction: &redb::WriteTransaction,
        snapshot: SnapshotId,
        initial_snapshot: SnapshotId,
        tree: Option<&LayerTree>,
        replacement: TimelineChange<'_>,
    ) -> Result<(), ProjectOpenError> {
        let current = self.current_animation_record()?;
        if current.is_none() && !matches!(replacement, TimelineChange::Replace(Some(_))) {
            return Ok(());
        }
        let current_tree = if tree.is_none() {
            self.load_layer_tree()?
        } else {
            None
        };
        let tree = tree.or(current_tree.as_ref());
        let next = match replacement {
            TimelineChange::Replace(timeline) => Envelope::new(
                RecordKind::AnimationTimeline,
                encode_animation_timeline(timeline).map_err(|error| self.io(error))?,
            )
            .encode(),
            TimelineChange::Inherit => current
                .clone()
                .ok_or_else(|| self.corrupt("animation inheritance missing"))?,
        };
        self.decode_animation_record(&next, tree)?;
        let mut table = transaction
            .open_table(SNAPSHOT_ANIMATION)
            .map_err(|error| self.io(error))?;
        if current.is_none() {
            let mut ids = BTreeSet::new();
            let snapshots = transaction
                .open_table(SNAPSHOTS)
                .map_err(|error| self.io(error))?;
            for entry in snapshots.iter().map_err(|error| self.io(error))? {
                let (key, _) = entry.map_err(|error| self.io(error))?;
                ids.insert(decode_snapshot_id(key.value()).map_err(|error| self.corrupt(error))?);
                if ids.len() > super::MAX_REOPEN_HISTORY_NODES {
                    return Err(self.corrupt("too many animation baseline snapshots"));
                }
            }
            ids.insert(
                self.load_initial_cursor()?
                    .map_or(initial_snapshot, |cursor| cursor.snapshot_id),
            );
            let baseline = Envelope::new(
                RecordKind::AnimationTimeline,
                encode_animation_timeline(None).map_err(|error| self.io(error))?,
            )
            .encode();
            for id in ids {
                table
                    .insert(id.0.to_le_bytes().as_slice(), baseline.as_slice())
                    .map_err(|error| self.io(error))?;
            }
        }
        if table
            .get(snapshot.0.to_le_bytes().as_slice())
            .map_err(|error| self.io(error))?
            .is_some()
        {
            return Err(self.corrupt("immutable animation snapshot already exists"));
        }
        table
            .insert(snapshot.0.to_le_bytes().as_slice(), next.as_slice())
            .map_err(|error| self.io(error))?;
        drop(table);
        let mut state = transaction
            .open_table(STATE)
            .map_err(|error| self.io(error))?;
        state
            .insert(CURRENT_ANIMATION, next.as_slice())
            .map_err(|error| self.io(error))?;
        drop(state);
        let mut metadata = transaction
            .open_table(META)
            .map_err(|error| self.io(error))?;
        let marker = metadata
            .get("schema_version")
            .map_err(|error| self.io(error))?
            .ok_or_else(|| self.corrupt("schema marker missing"))?
            .value();
        metadata
            .insert("schema_version", marker | ANIMATION_SCHEMA_FLAG)
            .map_err(|error| self.io(error))?;
        Ok(())
    }

    pub(super) fn validate_animation_history(&self) -> Result<(), ProjectOpenError> {
        let enabled = self.animation_enabled()?;
        let current = self.current_animation_record()?;
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let table = match transaction.open_table(SNAPSHOT_ANIMATION) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) if !enabled => return Ok(()),
            Err(error) => return Err(self.corrupt(error)),
        };
        if !enabled {
            return Err(self.corrupt("animation history lacks required capability"));
        }
        let parts = self
            .load_reopened()?
            .ok_or_else(|| self.corrupt("animation history has no cursor"))?
            .into_parts();
        let initial = self
            .load_initial_cursor()?
            .ok_or_else(|| self.corrupt("animation history has no baseline"))?;
        let mut cursors = parts.cursors;
        cursors.insert(None, initial);
        let expected: BTreeSet<_> = cursors.values().map(|cursor| cursor.snapshot_id).collect();
        let mut actual = BTreeSet::new();
        for entry in table.iter().map_err(|error| self.corrupt(error))? {
            let (key, value) = entry.map_err(|error| self.corrupt(error))?;
            actual.insert(decode_snapshot_id(key.value()).map_err(|error| self.corrupt(error))?);
            self.copy_animation_record(value.value())?;
            if actual.len() > super::MAX_REOPEN_HISTORY_NODES + 1 {
                return Err(self.corrupt("too many animation snapshots"));
            }
        }
        if actual != expected {
            return Err(self.corrupt("animation snapshot coverage mismatch"));
        }
        for cursor in cursors.values() {
            self.load_cursor_animation_timeline(*cursor)?;
        }
        if current != self.cursor_animation_record(parts.current_cursor)? {
            return Err(self.corrupt("current animation differs from durable cursor"));
        }
        Ok(())
    }
}

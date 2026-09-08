//! Immutable layer metadata shares the pixel/history transaction boundary.
use super::{
    CURRENT_LAYER_TREE, Envelope, LAYER_HISTORY_SCHEMA_VERSION, LayerTree,
    MAX_LAYER_TREE_RECORD_BYTES, MAX_REOPEN_HISTORY_NODES, META, ProjectDb, ProjectHistoryCursor,
    ProjectOpenError, ReadableTable, RecordKind, SNAPSHOTS, STATE, SnapshotId, TableDefinition,
    decode_envelope, decode_layer_tree, encode_layer_tree,
};
use nyatidraw_project::{
    CANVAS_HISTORY_SCHEMA_VERSION, COMPRESSED_TILE_SCHEMA_FLAG, SELECTION_STROKE_SCHEMA_VERSION,
};
use std::collections::BTreeSet;

pub(super) const SNAPSHOT_LAYERS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("snapshot_layers");
const LEGACY_LAYERS: &str = "legacy_layer_history_baseline";
const LEGACY_REFERENCE: &[u8] = b"legacy";

impl ProjectDb {
    pub(super) fn layer_history_enabled(&self) -> Result<bool, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let metadata = transaction
            .open_table(META)
            .map_err(|error| self.io(error))?;
        Ok(metadata
            .get("schema_version")
            .map_err(|error| self.io(error))?
            .is_some_and(|value| {
                matches!(
                    value.value() & !COMPRESSED_TILE_SCHEMA_FLAG,
                    LAYER_HISTORY_SCHEMA_VERSION
                        | SELECTION_STROKE_SCHEMA_VERSION
                        | CANVAS_HISTORY_SCHEMA_VERSION
                )
            }))
    }

    /// Loads the hierarchy belonging to a validated history cursor. Legacy
    /// projects have only a current tree until the first metadata commit.
    ///
    /// # Errors
    /// Rejects missing/corrupt metadata or a cursor that is not stored exactly.
    pub fn load_cursor_layer_tree(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<Option<LayerTree>, ProjectOpenError> {
        match cursor.history_head {
            Some(_) => self.validate_cursor_snapshot(cursor)?,
            None => self.validate_initial_cursor(cursor)?,
        }
        match self.cursor_layer_record(cursor)? {
            Some(bytes) => self.decode_optional_layers(&bytes),
            None => self.load_layer_tree(),
        }
    }

    pub(super) fn cursor_layer_record(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        if !self.layer_history_enabled()? {
            return Ok(None);
        }
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let layers = transaction
            .open_table(SNAPSHOT_LAYERS)
            .map_err(|error| self.corrupt(error))?;
        let value = layers
            .get(cursor.snapshot_id.0.to_le_bytes().as_slice())
            .map_err(|error| self.corrupt(error))?
            .ok_or_else(|| self.corrupt("snapshot layer metadata missing"))?;
        let bytes = if value.value() == LEGACY_REFERENCE {
            let state = transaction
                .open_table(STATE)
                .map_err(|error| self.corrupt(error))?;
            self.read_legacy_layers(&state)?
        } else {
            self.copy_layer_record(value.value())?
        };
        self.decode_optional_layers(&bytes)?;
        Ok(Some(bytes))
    }

    // Called before any snapshot/current-state writes. Upgrade, immutable
    // record and current hierarchy either all commit or all roll back.
    #[allow(clippy::too_many_lines)]
    pub(super) fn capture_snapshot_layers(
        &self,
        transaction: &redb::WriteTransaction,
        snapshot: SnapshotId,
        initial_snapshot: SnapshotId,
        tree: Option<&LayerTree>,
        force: bool,
    ) -> Result<(), ProjectOpenError> {
        let enabled = {
            let metadata = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            metadata
                .get("schema_version")
                .map_err(|error| self.io(error))?
                .is_some_and(|value| {
                    matches!(
                        value.value() & !COMPRESSED_TILE_SCHEMA_FLAG,
                        LAYER_HISTORY_SCHEMA_VERSION
                            | SELECTION_STROKE_SCHEMA_VERSION
                            | CANVAS_HISTORY_SCHEMA_VERSION
                    )
                })
        };
        if !enabled && tree.is_none() && !force {
            return Ok(());
        }
        let current = {
            let state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            let value = state
                .get(CURRENT_LAYER_TREE)
                .map_err(|error| self.io(error))?;
            value.map_or_else(
                || Ok(Vec::new()),
                |value| self.copy_layer_record(value.value()),
            )?
        };
        self.decode_optional_layers(&current)?;
        let next = match tree {
            Some(tree) => Envelope::new(
                RecordKind::LayerTree,
                encode_layer_tree(tree).map_err(|error| self.io(error))?,
            )
            .encode(),
            None => current.clone(),
        };
        self.decode_optional_layers(&next)?;
        if !enabled {
            let mut ids = BTreeSet::new();
            {
                let snapshots = transaction
                    .open_table(SNAPSHOTS)
                    .map_err(|error| self.io(error))?;
                for entry in snapshots.iter().map_err(|error| self.io(error))? {
                    let (key, _) = entry.map_err(|error| self.io(error))?;
                    ids.insert(snapshot_key(key.value()).map_err(|error| self.corrupt(error))?);
                    if ids.len() > MAX_REOPEN_HISTORY_NODES {
                        return Err(self.corrupt("too many legacy snapshots"));
                    }
                }
            }
            let initial = self
                .load_initial_cursor()?
                .map_or(initial_snapshot, |cursor| cursor.snapshot_id);
            ids.insert(initial.0);
            {
                let mut layers = transaction
                    .open_table(SNAPSHOT_LAYERS)
                    .map_err(|error| self.io(error))?;
                for id in ids {
                    layers
                        .insert(id.to_le_bytes().as_slice(), LEGACY_REFERENCE)
                        .map_err(|error| self.io(error))?;
                }
            }
            {
                let mut state = transaction
                    .open_table(STATE)
                    .map_err(|error| self.io(error))?;
                state
                    .insert(LEGACY_LAYERS, current.as_slice())
                    .map_err(|error| self.io(error))?;
            }
            let mut metadata = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            metadata
                .insert("schema_version", LAYER_HISTORY_SCHEMA_VERSION)
                .map_err(|error| self.io(error))?;
        }
        {
            let mut layers = transaction
                .open_table(SNAPSHOT_LAYERS)
                .map_err(|error| self.io(error))?;
            if layers
                .get(snapshot.0.to_le_bytes().as_slice())
                .map_err(|error| self.io(error))?
                .is_some()
            {
                return Err(self.corrupt("immutable snapshot layer record already exists"));
            }
            layers
                .insert(snapshot.0.to_le_bytes().as_slice(), next.as_slice())
                .map_err(|error| self.io(error))?;
        }
        if tree.is_some() {
            let mut state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            state
                .insert(CURRENT_LAYER_TREE, next.as_slice())
                .map_err(|error| self.io(error))?;
        }
        Ok(())
    }

    pub(super) fn validate_layer_history(&self) -> Result<(), ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let layers = match transaction.open_table(SNAPSHOT_LAYERS) {
            Ok(layers) => layers,
            Err(redb::TableError::TableDoesNotExist(_)) if !self.layer_history_enabled()? => {
                return Ok(());
            }
            Err(error) => return Err(self.corrupt(error)),
        };
        if !self.layer_history_enabled()? {
            return Err(self.corrupt("layer history exists under a legacy schema marker"));
        }
        let state = transaction
            .open_table(STATE)
            .map_err(|error| self.corrupt(error))?;
        let legacy = self.read_legacy_layers(&state)?;
        self.decode_optional_layers(&legacy)?;
        let initial = self
            .load_initial_cursor()?
            .ok_or_else(|| self.corrupt("layer history has no initial cursor"))?;
        let snapshots = transaction
            .open_table(SNAPSHOTS)
            .map_err(|error| self.corrupt(error))?;
        let mut expected = BTreeSet::from([initial.snapshot_id.0]);
        for entry in snapshots.iter().map_err(|error| self.corrupt(error))? {
            let (key, _) = entry.map_err(|error| self.corrupt(error))?;
            expected.insert(snapshot_key(key.value()).map_err(|error| self.corrupt(error))?);
            if expected.len() > MAX_REOPEN_HISTORY_NODES + 1 {
                return Err(self.corrupt("too many layer snapshots"));
            }
        }
        for entry in layers.iter().map_err(|error| self.corrupt(error))? {
            let (key, value) = entry.map_err(|error| self.corrupt(error))?;
            let id = snapshot_key(key.value()).map_err(|error| self.corrupt(error))?;
            if !expected.remove(&id) {
                return Err(self.corrupt("orphan snapshot layer metadata"));
            }
            if value.value() != LEGACY_REFERENCE {
                self.decode_optional_layers(value.value())?;
            }
        }
        if !expected.is_empty() {
            return Err(self.corrupt("snapshot layer metadata missing"));
        }
        let cursor = self.load_stored_cursor()?.unwrap_or(initial);
        let cursor_record = self
            .cursor_layer_record(cursor)?
            .ok_or_else(|| self.corrupt("current layer history missing"))?;
        let current = state
            .get(CURRENT_LAYER_TREE)
            .map_err(|error| self.corrupt(error))?;
        if current.as_ref().map_or(&[][..], redb::AccessGuard::value) != cursor_record {
            return Err(self.corrupt("current hierarchy differs from its immutable snapshot"));
        }
        Ok(())
    }

    fn read_legacy_layers(
        &self,
        state: &impl ReadableTable<&'static str, &'static [u8]>,
    ) -> Result<Vec<u8>, ProjectOpenError> {
        let value = state
            .get(LEGACY_LAYERS)
            .map_err(|error| self.corrupt(error))?
            .ok_or_else(|| self.corrupt("legacy layer baseline missing"))?;
        self.copy_layer_record(value.value())
    }

    fn copy_layer_record(&self, bytes: &[u8]) -> Result<Vec<u8>, ProjectOpenError> {
        if bytes.len() > MAX_LAYER_TREE_RECORD_BYTES {
            return Err(self.corrupt("layer history exceeds record byte limit"));
        }
        Ok(bytes.to_vec())
    }

    fn decode_optional_layers(&self, bytes: &[u8]) -> Result<Option<LayerTree>, ProjectOpenError> {
        if bytes.is_empty() {
            return Ok(None);
        }
        if bytes.len() > MAX_LAYER_TREE_RECORD_BYTES {
            return Err(self.corrupt("layer history exceeds record byte limit"));
        }
        let envelope =
            decode_envelope(RecordKind::LayerTree, bytes).map_err(|error| self.corrupt(error))?;
        decode_layer_tree(&envelope.payload)
            .map(Some)
            .map_err(|error| self.corrupt(error))
    }
}

fn snapshot_key(bytes: &[u8]) -> Result<u128, &'static str> {
    Ok(u128::from_le_bytes(
        bytes.try_into().map_err(|_| "invalid layer snapshot key")?,
    ))
}

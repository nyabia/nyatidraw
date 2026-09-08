//! Immutable output-page metadata. Upgrade, page and pixels share one commit.
use super::{
    CANVAS_HISTORY_SCHEMA_VERSION, CanvasSpec, Envelope, MAX_REOPEN_HISTORY_NODES, META, ProjectDb,
    ProjectHistoryCursor, ProjectOpenError, ReadableTable, RecordKind, SNAPSHOTS, SnapshotId,
    TableDefinition, decode_envelope, write_canvas_metadata,
};
use nyatidraw_project::COMPRESSED_TILE_SCHEMA_FLAG;
use std::collections::BTreeSet;

pub(super) const SNAPSHOT_CANVAS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("snapshot_canvas");
const RECORD_BYTES: usize = 52 + 12;

impl ProjectDb {
    pub(super) fn canvas_history_enabled(&self) -> Result<bool, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let metadata = transaction
            .open_table(META)
            .map_err(|error| self.io(error))?;
        Ok(metadata
            .get("schema_version")
            .map_err(|error| self.io(error))?
            .is_some_and(|value| {
                value.value() & !COMPRESSED_TILE_SCHEMA_FLAG == CANVAS_HISTORY_SCHEMA_VERSION
            }))
    }

    /// Loads the page belonging to a validated cursor. Pre-upgrade snapshots
    /// use the last known legacy page dimensions, frozen at first page change.
    ///
    /// # Errors
    /// Rejects forged cursors and missing, invalid or corrupted page records.
    pub fn load_cursor_canvas_spec(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<CanvasSpec, ProjectOpenError> {
        match cursor.history_head {
            Some(_) => self.validate_cursor_snapshot(cursor)?,
            None => self.validate_initial_cursor(cursor)?,
        }
        self.cursor_canvas_record(cursor)?
            .map_or_else(|| self.load_canvas_spec(), Ok)
    }

    pub(super) fn cursor_canvas_record(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<Option<CanvasSpec>, ProjectOpenError> {
        if !self.canvas_history_enabled()? {
            return Ok(None);
        }
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let pages = transaction
            .open_table(SNAPSHOT_CANVAS)
            .map_err(|error| self.corrupt(error))?;
        let value = pages
            .get(cursor.snapshot_id.0.to_le_bytes().as_slice())
            .map_err(|error| self.corrupt(error))?
            .ok_or_else(|| self.corrupt("snapshot page metadata missing"))?;
        self.decode_canvas_record(value.value()).map(Some)
    }

    pub(super) fn capture_snapshot_canvas(
        &self,
        transaction: &redb::WriteTransaction,
        snapshot: SnapshotId,
        initial_snapshot: SnapshotId,
        canvas: Option<CanvasSpec>,
    ) -> Result<(), ProjectOpenError> {
        let enabled = {
            let metadata = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            metadata
                .get("schema_version")
                .map_err(|error| self.io(error))?
                .is_some_and(|value| {
                    value.value() & !COMPRESSED_TILE_SCHEMA_FLAG == CANVAS_HISTORY_SCHEMA_VERSION
                })
        };
        if !enabled && canvas.is_none() {
            return Ok(());
        }
        let current = self.load_canvas_spec()?;
        let next = canvas
            .unwrap_or(current)
            .validate()
            .map_err(|error| self.io(format!("invalid canvas: {error:?}")))?;
        if !enabled {
            // Legacy history did not retain per-snapshot dimensions. Freeze
            // its current value for every old branch, including initial Undo.
            let mut ids = BTreeSet::new();
            {
                let snapshots = transaction
                    .open_table(SNAPSHOTS)
                    .map_err(|error| self.io(error))?;
                for entry in snapshots.iter().map_err(|error| self.io(error))? {
                    let (key, _) = entry.map_err(|error| self.io(error))?;
                    ids.insert(snapshot_key(key.value()).map_err(|error| self.corrupt(error))?);
                    if ids.len() > MAX_REOPEN_HISTORY_NODES {
                        return Err(self.corrupt("too many legacy page snapshots"));
                    }
                }
            }
            ids.insert(
                self.load_initial_cursor()?
                    .map_or(initial_snapshot, |cursor| cursor.snapshot_id)
                    .0,
            );
            let baseline = encode_canvas(current);
            let mut pages = transaction
                .open_table(SNAPSHOT_CANVAS)
                .map_err(|error| self.io(error))?;
            for id in ids {
                pages
                    .insert(id.to_le_bytes().as_slice(), baseline.as_slice())
                    .map_err(|error| self.io(error))?;
            }
        }
        let bytes = encode_canvas(next);
        {
            let mut pages = transaction
                .open_table(SNAPSHOT_CANVAS)
                .map_err(|error| self.io(error))?;
            if pages
                .get(snapshot.0.to_le_bytes().as_slice())
                .map_err(|error| self.io(error))?
                .is_some()
            {
                return Err(self.corrupt("immutable snapshot page record already exists"));
            }
            pages
                .insert(snapshot.0.to_le_bytes().as_slice(), bytes.as_slice())
                .map_err(|error| self.io(error))?;
        }
        let mut metadata = transaction
            .open_table(META)
            .map_err(|error| self.io(error))?;
        write_canvas_metadata(&mut metadata, next).map_err(|error| self.io(error))?;
        metadata
            .insert("schema_version", CANVAS_HISTORY_SCHEMA_VERSION)
            .map_err(|error| self.io(error))?;
        Ok(())
    }

    pub(super) fn validate_canvas_history(&self) -> Result<(), ProjectOpenError> {
        let enabled = self.canvas_history_enabled()?;
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let pages = match transaction.open_table(SNAPSHOT_CANVAS) {
            Ok(pages) => pages,
            Err(redb::TableError::TableDoesNotExist(_)) if !enabled => return Ok(()),
            Err(error) => return Err(self.corrupt(error)),
        };
        if !enabled {
            return Err(self.corrupt("page history exists under a legacy schema marker"));
        }
        let initial = self
            .load_initial_cursor()?
            .ok_or_else(|| self.corrupt("page history has no initial cursor"))?;
        let mut expected = BTreeSet::from([initial.snapshot_id.0]);
        let snapshots = transaction
            .open_table(SNAPSHOTS)
            .map_err(|error| self.corrupt(error))?;
        for entry in snapshots.iter().map_err(|error| self.corrupt(error))? {
            let (key, _) = entry.map_err(|error| self.corrupt(error))?;
            expected.insert(snapshot_key(key.value()).map_err(|error| self.corrupt(error))?);
            if expected.len() > MAX_REOPEN_HISTORY_NODES + 1 {
                return Err(self.corrupt("too many page snapshots"));
            }
        }
        for entry in pages.iter().map_err(|error| self.corrupt(error))? {
            let (key, value) = entry.map_err(|error| self.corrupt(error))?;
            if !expected.remove(&snapshot_key(key.value()).map_err(|error| self.corrupt(error))?) {
                return Err(self.corrupt("orphan snapshot page metadata"));
            }
            self.decode_canvas_record(value.value())?;
        }
        if !expected.is_empty() {
            return Err(self.corrupt("snapshot page metadata missing"));
        }
        let cursor = self.load_stored_cursor()?.unwrap_or(initial);
        if self.cursor_canvas_record(cursor)? != Some(self.load_canvas_spec()?) {
            return Err(self.corrupt("current page differs from its immutable snapshot"));
        }
        Ok(())
    }

    fn decode_canvas_record(&self, bytes: &[u8]) -> Result<CanvasSpec, ProjectOpenError> {
        if bytes.len() != RECORD_BYTES {
            return Err(self.corrupt("invalid page record length"));
        }
        let envelope =
            decode_envelope(RecordKind::CanvasSpec, bytes).map_err(|error| self.corrupt(error))?;
        if envelope.payload.len() != 12 {
            return Err(self.corrupt("invalid page payload length"));
        }
        let mut fields = envelope
            .payload
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four-byte field")));
        CanvasSpec {
            width_px: fields.next().expect("width"),
            height_px: fields.next().expect("height"),
            pixels_per_inch: fields.next().expect("resolution"),
        }
        .validate()
        .map_err(|error| self.corrupt(format!("invalid snapshot page: {error:?}")))
    }
}

fn encode_canvas(canvas: CanvasSpec) -> Vec<u8> {
    let payload = [canvas.width_px, canvas.height_px, canvas.pixels_per_inch]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    Envelope::new(RecordKind::CanvasSpec, payload).encode()
}

fn snapshot_key(bytes: &[u8]) -> Result<u128, &'static str> {
    Ok(u128::from_le_bytes(
        bytes.try_into().map_err(|_| "invalid page snapshot key")?,
    ))
}

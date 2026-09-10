#![forbid(unsafe_code)]

//! Durable redb storage for immutable stroke materialization records.

use std::path::{Path, PathBuf};

mod canvas_history;
mod history_retention;
mod layer_history;
#[cfg(feature = "legacy-migration")]
mod migration;
mod object_retention;
#[cfg(feature = "legacy-migration")]
pub use migration::migrate_legacy_copy;

#[cfg(feature = "diagnostic")]
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    thread,
    time::Duration,
};

use nyatidraw_api::{CanvasSpec, ContentRootId, HistoryNodeId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_history::{History, HistoryNode};
use nyatidraw_project::{
    ALPHA_LOCK_STROKE_SCHEMA_FLAG, CANVAS_HISTORY_SCHEMA_VERSION, COMPRESSED_TILE_SCHEMA_FLAG,
    CONFIGURABLE_BRUSH_SCHEMA_FLAG, Envelope, LAYER_COMPOSITING_SCHEMA_FLAG,
    LAYER_HISTORY_SCHEMA_VERSION, MAX_LAYER_TREE_RECORD_BYTES, OpenMode, ProjectCommitBatch,
    ProjectHistoryCursor, ProjectOpenError, ProjectRepository, ProjectStructuralBatch, RecordKind,
    ReopenedProject, RootManifest, SCHEMA_CAPABILITY_FLAGS, SCHEMA_VERSION,
    SELECTION_STROKE_SCHEMA_VERSION, SIGNED_SELECTION_SCHEMA_FLAG, decode_history_cursor,
    decode_history_node, decode_initial_history_cursor, decode_layer_tree, decode_project_head,
    decode_root_manifest, decode_stroke_commit, encode_history_cursor, encode_history_node,
    encode_initial_history_cursor, encode_layer_tree, encode_project_head, encode_root_manifest,
    encode_stroke_commit, open_mode,
};
use nyatidraw_stroke::{MaterializationStrategy, materialize_with_strategy};
use nyatidraw_tiles::{ContentRoot, ObjectHash, TileObject, TileSnapshot};
use redb::{Database, DatabaseError, Durability, ReadableDatabase, ReadableTable, TableDefinition};

const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("state");
const OBJECTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("objects");
const ROOTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("roots");
const STROKES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("strokes");
const HISTORY: TableDefinition<&[u8], &[u8]> = TableDefinition::new("history");
const SNAPSHOTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("snapshots");
const CURRENT_SNAPSHOT: &str = "current_snapshot";
const CURRENT_HISTORY_CURSOR: &str = "current_history_cursor";
const INITIAL_HISTORY_CURSOR: &str = "initial_history_cursor";
const CURRENT_LAYER_TREE: &str = "current_layer_tree";
const CANVAS_METADATA_VERSION: u64 = 1;
const CANVAS_METADATA_VERSION_KEY: &str = "canvas_metadata_version";
const CANVAS_WIDTH_KEY: &str = "canvas_width_px";
const CANVAS_HEIGHT_KEY: &str = "canvas_height_px";
const CANVAS_PPI_KEY: &str = "canvas_pixels_per_inch";
/// Reopen must enumerate persisted nodes to rebuild redo branches. This cap
/// bounds malformed or hostile projects before allocating an arbitrary graph.
const MAX_REOPEN_HISTORY_NODES: usize = 100_000;

/// A diagnostic-only process-kill synchronization boundary.
///
/// This type and [`ProjectDb::commit_pausing_for_diagnostics`] only exist with
/// the `diagnostic` feature. They are for scratch-project recovery probes, not
/// a production transaction control surface.
#[cfg(feature = "diagnostic")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCommitBoundary {
    /// The populated write transaction exists but `redb::WriteTransaction::commit`
    /// has not been called.
    BeforeCommit,
    /// `redb::WriteTransaction::commit` has returned successfully with
    /// `Durability::Immediate`, but control has not returned to the caller.
    AfterDurableCommit,
}

#[cfg(feature = "diagnostic")]
#[derive(Clone, Debug)]
pub struct DiagnosticCommitPause {
    pub boundary: DiagnosticCommitBoundary,
    /// A fresh scratch path. The child creates and syncs this file only after
    /// reaching `boundary`, then waits for the probe parent to terminate it.
    pub stage_path: PathBuf,
}

#[derive(Clone, Debug)]
enum CommitControl {
    Normal,
    #[cfg(test)]
    Failure(CommitFailurePoint),
    #[cfg(feature = "diagnostic")]
    Pause(DiagnosticCommitPause),
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitFailurePoint {
    /// Drops the populated redb write transaction before `commit`.
    BeforeCommitAbort,
    /// Returns an error after redb reports the immediate commit complete.
    AfterCommitReturn,
}

#[derive(Debug)]
pub struct ProjectDb {
    db: ProjectDatabase,
    path: PathBuf,
}

// A read-only preflight prevents redb's writable-open/drop allocator bookkeeping
// from modifying an otherwise clean but application-invalid project.
enum ProjectDatabase {
    Writable(Database),
    Validation(redb::ReadOnlyDatabase),
}

impl std::fmt::Debug for ProjectDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProjectDatabase")
    }
}

impl ProjectDatabase {
    fn begin_read(&self) -> Result<redb::ReadTransaction, redb::TransactionError> {
        match self {
            Self::Writable(db) => db.begin_read(),
            Self::Validation(db) => db.begin_read(),
        }
    }

    fn begin_write(&self) -> Result<redb::WriteTransaction, redb::TransactionError> {
        match self {
            Self::Writable(db) => db.begin_write(),
            Self::Validation(_) => Err(redb::StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "validation is read-only",
            ))
            .into()),
        }
    }
}

impl ProjectDb {
    /// Opens a project, initializing only a missing or exactly empty file.
    /// During alpha, legacy containers are automatically converted through a
    /// validated sibling copy and the old file is retained as a `.bak`.
    ///
    /// A marked existing project is fully decoded through its current head
    /// before this returns. A corrupt record therefore never becomes a valid
    /// empty project. Clean application-invalid files are checked read-only;
    /// unclean database recovery may need redb's allocator repair writes.
    ///
    /// # Errors
    ///
    /// Returns `Locked` when another writer already owns the project file,
    /// `InvalidNonEmpty` for an unmarked non-empty file, `Corrupt` for a
    /// marked project whose committed graph fails validation, and `Io` for
    /// storage failures.
    pub fn open(path: &Path) -> Result<Self, ProjectOpenError> {
        Self::open_inner(path, true)
    }

    fn open_inner(path: &Path, allow_retention: bool) -> Result<Self, ProjectOpenError> {
        let mode = open_mode(path)?;
        if mode == OpenMode::OpenExisting {
            match redb::ReadOnlyDatabase::open(path) {
                Ok(db) => {
                    let check = Self {
                        db: ProjectDatabase::Validation(db),
                        path: path.to_owned(),
                    };
                    check.validate_existing()?;
                    #[cfg(feature = "legacy-migration")]
                    if allow_retention && migration::needs_repack(&check)? {
                        drop(check);
                        migration::upgrade_alpha_in_place(path)?;
                        return Self::open_inner(path, allow_retention);
                    }
                }
                #[cfg(feature = "legacy-migration")]
                Err(DatabaseError::UpgradeRequired(_)) => {
                    migration::upgrade_alpha_in_place(path)?;
                    return Self::open_inner(path, allow_retention);
                }
                // A crash may require redb to repair its allocator. Its normal
                // writer path below retains the existing recovery semantics.
                Err(DatabaseError::RepairAborted) => {}
                Err(DatabaseError::DatabaseAlreadyOpen) => {
                    return Err(ProjectOpenError::Locked {
                        path: path.to_owned(),
                    });
                }
                #[cfg(not(feature = "legacy-migration"))]
                Err(DatabaseError::UpgradeRequired(_)) => {
                    return Err(ProjectOpenError::LegacyContainer {
                        path: path.to_owned(),
                    });
                }
                Err(_) => {
                    return Err(ProjectOpenError::InvalidNonEmpty {
                        path: path.to_owned(),
                    });
                }
            }
        }
        let db = match mode {
            OpenMode::Initialize => Database::create(path).map_err(|error| match error {
                DatabaseError::DatabaseAlreadyOpen => ProjectOpenError::Locked {
                    path: path.to_owned(),
                },
                error => ProjectOpenError::Io {
                    path: path.to_owned(),
                    message: error.to_string(),
                },
            })?,
            OpenMode::OpenExisting => Database::open(path).map_err(|error| match error {
                DatabaseError::UpgradeRequired(_) => ProjectOpenError::LegacyContainer {
                    path: path.to_owned(),
                },
                DatabaseError::DatabaseAlreadyOpen => ProjectOpenError::Locked {
                    path: path.to_owned(),
                },
                _ => ProjectOpenError::InvalidNonEmpty {
                    path: path.to_owned(),
                },
            })?,
        };
        let mut this = Self {
            db: ProjectDatabase::Writable(db),
            path: path.to_owned(),
        };
        if mode == OpenMode::Initialize {
            this.write_marker()?;
            // Only a newly initialized, tiny metadata-only DB is compacted.
            // redb can otherwise retain a ~1MiB first-transaction allocation.
            // Never run this on ordinary Save/Open/Close of existing artwork.
            if let ProjectDatabase::Writable(db) = &mut this.db {
                db.compact().map_err(|error| ProjectOpenError::Io {
                    path: path.to_owned(),
                    message: error.to_string(),
                })?;
            }
        } else {
            this.validate_existing()?;
            // The read-only preflight can defer to writable crash recovery.
            // Once repaired and validated, apply the same one-time alpha repack.
            #[cfg(feature = "legacy-migration")]
            if allow_retention && migration::needs_repack(&this)? {
                drop(this);
                migration::upgrade_alpha_in_place(path)?;
                return Self::open_inner(path, allow_retention);
            }
            // Validate old artwork/metadata before a destructive retention
            // migration. Invalid projects must remain untouched.
            if allow_retention
                && this.load_persisted_history_nodes()?.len() > nyatidraw_history::HISTORY_LIMIT
            {
                let mut transaction = this.db.begin_write().map_err(|error| this.io(error))?;
                transaction
                    .set_durability(Durability::Immediate)
                    .map_err(|error| this.io(error))?;
                this.retain_history(&transaction)?;
                transaction.commit().map_err(|error| this.io(error))?;
            }
        }
        Ok(this)
    }

    fn validate_existing(&self) -> Result<(), ProjectOpenError> {
        if !self.has_valid_marker()? {
            return Err(ProjectOpenError::InvalidNonEmpty {
                path: self.path.clone(),
            });
        }
        self.load_canvas_spec()?;
        self.load_layer_tree()?;
        self.load_reopened()?;
        self.validate_layer_history()?;
        self.validate_canvas_history()?;
        Ok(())
    }

    /// Atomically stores one closed stroke, both immutable tile roots, and its
    /// new snapshot head with immediate redb durability.
    ///
    /// # Errors
    ///
    /// Returns `Io` when the write transaction cannot be completed.
    pub fn commit(&self, batch: &ProjectCommitBatch) -> Result<(), ProjectOpenError> {
        self.commit_inner(batch, &CommitControl::Normal)
    }

    /// Stores an exact non-stroke content transition and its history node.
    ///
    /// # Errors
    ///
    /// Returns an error without advancing the durable cursor when the batch
    /// does not continue the current root or redb cannot commit immediately.
    pub fn commit_structural(
        &self,
        batch: &ProjectStructuralBatch,
    ) -> Result<(), ProjectOpenError> {
        self.commit_structural_inner(batch, None, None, &CommitControl::Normal)
    }

    /// Commits artwork, layer hierarchy, history and current cursor atomically.
    /// Legacy history inherits one frozen compatibility tree on first use.
    ///
    /// # Errors
    /// Rejects invalid lineage/metadata or storage failure without publishing
    /// a partial tree, pixel root or history cursor.
    pub fn commit_structural_with_layer_tree(
        &self,
        batch: &ProjectStructuralBatch,
        tree: &LayerTree,
    ) -> Result<(), ProjectOpenError> {
        self.commit_structural_inner(batch, Some(tree), None, &CommitControl::Normal)
    }

    /// Commits page dimensions, artwork and history in one immediate transaction.
    /// Legacy snapshots inherit their last known page size on the first change.
    ///
    /// # Errors
    /// Rejects invalid dimensions/lineage or failed durability without a partial
    /// page change. Old writers reject the upgraded schema marker.
    pub fn commit_structural_with_canvas(
        &self,
        batch: &ProjectStructuralBatch,
        canvas: CanvasSpec,
    ) -> Result<(), ProjectOpenError> {
        canvas
            .validate()
            .map_err(|error| self.io(format!("invalid canvas: {error:?}")))?;
        self.commit_structural_inner(batch, None, Some(canvas), &CommitControl::Normal)
    }

    #[allow(clippy::too_many_lines)]
    fn commit_structural_inner(
        &self,
        batch: &ProjectStructuralBatch,
        tree: Option<&LayerTree>,
        canvas: Option<CanvasSpec>,
        control: &CommitControl,
    ) -> Result<(), ProjectOpenError> {
        self.validate_structural_lineage(batch)?;
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        self.capture_snapshot_layers(
            &transaction,
            batch.snapshot_id,
            SnapshotId(0),
            tree,
            canvas.is_some(),
        )?;
        self.capture_snapshot_canvas(&transaction, batch.snapshot_id, SnapshotId(0), canvas)?;
        {
            let mut objects = transaction
                .open_table(OBJECTS)
                .map_err(|error| self.io(error))?;
            store_tile_objects(&mut objects, &batch.before).map_err(|error| self.io(error))?;
            store_tile_objects(&mut objects, &batch.after).map_err(|error| self.io(error))?;
        }
        self.store_roots(&transaction, &batch.before, &batch.after)?;
        {
            let mut history = transaction
                .open_table(HISTORY)
                .map_err(|error| self.io(error))?;
            let bytes = Envelope::new(
                RecordKind::HistoryNode,
                encode_history_node(&batch.history_node),
            )
            .encode();
            history
                .insert(
                    batch.history_node.id.0.to_le_bytes().as_slice(),
                    bytes.as_slice(),
                )
                .map_err(|error| self.io(error))?;
        }
        // A structural first commit has no stroke record, but reopen walks the
        // same schema as stroke-backed heads before inspecting that optional
        // reference. Publish the empty table as part of the same transaction.
        drop(
            transaction
                .open_table(STROKES)
                .map_err(|error| self.io(error))?,
        );
        {
            let mut snapshots = transaction
                .open_table(SNAPSHOTS)
                .map_err(|error| self.io(error))?;
            let bytes =
                Envelope::new(RecordKind::SnapshotHead, encode_project_head(batch.head())).encode();
            snapshots
                .insert(
                    batch.snapshot_id.0.to_le_bytes().as_slice(),
                    bytes.as_slice(),
                )
                .map_err(|error| self.io(error))?;
        }
        {
            let mut state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            state
                .insert(
                    CURRENT_SNAPSHOT,
                    batch.snapshot_id.0.to_le_bytes().as_slice(),
                )
                .map_err(|error| self.io(error))?;
            let cursor = Envelope::new(
                RecordKind::HistoryCursor,
                encode_history_cursor(batch.snapshot_id, batch.history_node.id, batch.after.root()),
            )
            .encode();
            state
                .insert(CURRENT_HISTORY_CURSOR, cursor.as_slice())
                .map_err(|error| self.io(error))?;
            if batch.history_node.parent.is_none()
                && state
                    .get(INITIAL_HISTORY_CURSOR)
                    .map_err(|error| self.io(error))?
                    .is_none()
            {
                let initial = Envelope::new(
                    RecordKind::InitialHistoryCursor,
                    encode_initial_history_cursor(SnapshotId(0), batch.before.root()),
                )
                .encode();
                state
                    .insert(INITIAL_HISTORY_CURSOR, initial.as_slice())
                    .map_err(|error| self.io(error))?;
            }
        }
        self.finish_commit(transaction, control)
    }

    /// Commits a scratch fixture but pauses at one transaction boundary for an
    /// external process-kill probe.
    ///
    /// The method is compiled only with the `diagnostic` feature. It creates
    /// and `sync_all`s `pause.stage_path` once the named boundary is reached,
    /// then waits indefinitely; the parent probe must terminate the exact
    /// child PID. Do not use it with user artwork.
    ///
    /// # Errors
    ///
    /// Returns the same storage errors as [`Self::commit`], or an I/O error if
    /// the owned diagnostic stage file cannot be created and synced.
    #[cfg(feature = "diagnostic")]
    pub fn commit_pausing_for_diagnostics(
        &self,
        batch: &ProjectCommitBatch,
        pause: DiagnosticCommitPause,
    ) -> Result<(), ProjectOpenError> {
        self.commit_inner(batch, &CommitControl::Pause(pause))
    }

    /// Pauses a scratch-only atomic metadata/pixel transaction at a crash boundary.
    ///
    /// # Errors
    /// Returns the same validation/storage errors as the normal structural commit.
    #[cfg(feature = "diagnostic")]
    pub fn commit_layers_pausing_for_diagnostics(
        &self,
        batch: &ProjectStructuralBatch,
        tree: &LayerTree,
        pause: DiagnosticCommitPause,
    ) -> Result<(), ProjectOpenError> {
        self.commit_structural_inner(batch, Some(tree), None, &CommitControl::Pause(pause))
    }

    #[allow(clippy::too_many_lines)]
    fn commit_inner(
        &self,
        batch: &ProjectCommitBatch,
        control: &CommitControl,
    ) -> Result<(), ProjectOpenError> {
        self.validate_commit_lineage(batch)?;
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        self.capture_snapshot_layers(
            &transaction,
            batch.snapshot_id,
            batch.stroke.parent_snapshot,
            None,
            batch.stroke.selection().is_some(),
        )?;
        self.capture_snapshot_canvas(
            &transaction,
            batch.snapshot_id,
            batch.stroke.parent_snapshot,
            None,
        )?;
        if batch.stroke.selection().is_some()
            || batch.stroke.brush.preset.engine_version >= 2
            || batch.stroke.alpha_locked()
        {
            let mut metadata = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            let current = metadata
                .get("schema_version")
                .map_err(|error| self.io(error))?
                .map_or(SCHEMA_VERSION, |value| value.value());
            metadata
                .insert(
                    "schema_version",
                    (current & !SCHEMA_CAPABILITY_FLAGS).max(
                        if batch.stroke.selection().is_some() {
                            SELECTION_STROKE_SCHEMA_VERSION
                        } else {
                            SCHEMA_VERSION
                        },
                    ) | (current & SCHEMA_CAPABILITY_FLAGS)
                        | if batch.stroke.brush.preset.engine_version >= 2 {
                            CONFIGURABLE_BRUSH_SCHEMA_FLAG
                        } else {
                            0
                        }
                        | if batch
                            .stroke
                            .selection()
                            .is_some_and(|selection| selection.origin() != [0, 0])
                        {
                            SIGNED_SELECTION_SCHEMA_FLAG
                        } else {
                            0
                        }
                        | if batch.stroke.alpha_locked() {
                            ALPHA_LOCK_STROKE_SCHEMA_FLAG
                        } else {
                            0
                        },
                )
                .map_err(|error| self.io(error))?;
        }

        {
            let mut objects = transaction
                .open_table(OBJECTS)
                .map_err(|error| self.io(error))?;
            store_tile_objects(&mut objects, &batch.before).map_err(|error| self.io(error))?;
            store_tile_objects(&mut objects, &batch.materialized.after)
                .map_err(|error| self.io(error))?;
        }
        self.store_roots(&transaction, &batch.before, &batch.materialized.after)?;
        {
            let mut strokes = transaction
                .open_table(STROKES)
                .map_err(|error| self.io(error))?;
            let envelope = Envelope::new(
                RecordKind::StrokeCommit,
                encode_stroke_commit(&batch.stroke),
            )
            .encode();
            strokes
                .insert((batch.stroke.id.0).0.as_slice(), envelope.as_slice())
                .map_err(|error| self.io(error))?;
        }
        {
            let mut history = transaction
                .open_table(HISTORY)
                .map_err(|error| self.io(error))?;
            let envelope = Envelope::new(
                RecordKind::HistoryNode,
                encode_history_node(&batch.history_node),
            )
            .encode();
            history
                .insert(
                    batch.history_node.id.0.to_le_bytes().as_slice(),
                    envelope.as_slice(),
                )
                .map_err(|error| self.io(error))?;
        }
        {
            let mut snapshots = transaction
                .open_table(SNAPSHOTS)
                .map_err(|error| self.io(error))?;
            let envelope =
                Envelope::new(RecordKind::SnapshotHead, encode_project_head(batch.head())).encode();
            snapshots
                .insert(
                    batch.snapshot_id.0.to_le_bytes().as_slice(),
                    envelope.as_slice(),
                )
                .map_err(|error| self.io(error))?;
        }
        {
            let mut state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            state
                .insert(
                    CURRENT_SNAPSHOT,
                    batch.snapshot_id.0.to_le_bytes().as_slice(),
                )
                .map_err(|error| self.io(error))?;
            let cursor = ProjectHistoryCursor {
                snapshot_id: batch.snapshot_id,
                history_head: Some(batch.history_node.id),
                root: batch.materialized.after.root(),
            };
            let cursor = Envelope::new(
                RecordKind::HistoryCursor,
                encode_history_cursor(cursor.snapshot_id, batch.history_node.id, cursor.root),
            )
            .encode();
            state
                .insert(CURRENT_HISTORY_CURSOR, cursor.as_slice())
                .map_err(|error| self.io(error))?;
            if batch.history_node.parent.is_none()
                && state
                    .get(INITIAL_HISTORY_CURSOR)
                    .map_err(|error| self.io(error))?
                    .is_none()
            {
                let initial = Envelope::new(
                    RecordKind::InitialHistoryCursor,
                    encode_initial_history_cursor(
                        batch.stroke.parent_snapshot,
                        batch.before.root(),
                    ),
                )
                .encode();
                state
                    .insert(INITIAL_HISTORY_CURSOR, initial.as_slice())
                    .map_err(|error| self.io(error))?;
            }
        }

        self.finish_commit(transaction, control)
    }

    fn finish_commit(
        &self,
        transaction: redb::WriteTransaction,
        control: &CommitControl,
    ) -> Result<(), ProjectOpenError> {
        self.retain_history(&transaction)?;
        {
            let mut metadata = transaction.open_table(META).map_err(|e| self.io(e))?;
            let version = metadata
                .get("schema_version")
                .map_err(|e| self.io(e))?
                .ok_or_else(|| self.corrupt("schema marker missing"))?
                .value();
            metadata
                .insert("schema_version", version | COMPRESSED_TILE_SCHEMA_FLAG)
                .map_err(|e| self.io(e))?;
        }
        match control {
            #[cfg(test)]
            CommitControl::Failure(CommitFailurePoint::BeforeCommitAbort) => {
                drop(transaction);
                return Err(self.io("injected interruption before redb transaction commit"));
            }
            #[cfg(feature = "diagnostic")]
            CommitControl::Pause(pause)
                if pause.boundary == DiagnosticCommitBoundary::BeforeCommit =>
            {
                pause_for_diagnostic_kill(&pause.stage_path, pause.boundary)
                    .map_err(|error| self.io(error))?;
            }
            _ => {}
        }

        transaction.commit().map_err(|error| self.io(error))?;

        match control {
            #[cfg(test)]
            CommitControl::Failure(CommitFailurePoint::AfterCommitReturn) => {
                return Err(self.io("injected interruption after redb transaction commit"));
            }
            #[cfg(feature = "diagnostic")]
            CommitControl::Pause(pause)
                if pause.boundary == DiagnosticCommitBoundary::AfterDurableCommit =>
            {
                pause_for_diagnostic_kill(&pause.stage_path, pause.boundary)
                    .map_err(|error| self.io(error))?;
            }
            _ => {}
        }

        Ok(())
    }

    fn validate_commit_lineage(&self, batch: &ProjectCommitBatch) -> Result<(), ProjectOpenError> {
        let current = match self.load_stored_cursor()? {
            Some(cursor) => {
                self.validate_cursor_snapshot(cursor)?;
                cursor
            }
            None => match self.load_current()? {
                Some(batch) => ProjectHistoryCursor {
                    snapshot_id: batch.snapshot_id,
                    history_head: Some(batch.history_node.id),
                    root: batch.materialized.after.root(),
                },
                None => {
                    if let Some(cursor) = self.load_initial_cursor()? {
                        cursor
                    } else {
                        if self.has_persisted_history_nodes()? {
                            return Err(self.corrupt("history exists without a current cursor"));
                        }
                        if batch.history_node.parent.is_none() {
                            return Ok(());
                        }
                        return Err(self
                            .corrupt("commit lineage mismatch: empty project requires root node"));
                    }
                }
            },
        };
        if batch.history_node.parent != current.history_head
            || batch.stroke.parent_snapshot != current.snapshot_id
            || batch.before.root() != current.root
        {
            return Err(
                self.corrupt("commit lineage mismatch: parent snapshot/root/history cursor")
            );
        }
        Ok(())
    }

    fn validate_structural_lineage(
        &self,
        batch: &ProjectStructuralBatch,
    ) -> Result<(), ProjectOpenError> {
        let current = match self.load_stored_cursor()? {
            Some(cursor) => {
                self.validate_cursor_snapshot(cursor)?;
                cursor
            }
            None => match self.load_current()? {
                Some(batch) => ProjectHistoryCursor {
                    snapshot_id: batch.snapshot_id,
                    history_head: Some(batch.history_node.id),
                    root: batch.materialized.after.root(),
                },
                None => {
                    if let Some(cursor) = self.load_initial_cursor()? {
                        cursor
                    } else {
                        if self.has_persisted_history_nodes()? {
                            return Err(self.corrupt("history exists without a current cursor"));
                        }
                        if batch.history_node.parent.is_none() {
                            return Ok(());
                        }
                        return Err(self
                            .corrupt("commit lineage mismatch: empty project requires root node"));
                    }
                }
            },
        };
        if batch.history_node.parent != current.history_head
            || batch.snapshot_id.0 <= current.snapshot_id.0
            || batch.before.root() != current.root
        {
            return Err(self.corrupt("structural commit lineage mismatch"));
        }
        Ok(())
    }

    #[cfg(test)]
    fn commit_with_failure(
        &self,
        batch: &ProjectCommitBatch,
        failure_point: CommitFailurePoint,
    ) -> Result<(), ProjectOpenError> {
        self.commit_inner(batch, &CommitControl::Failure(failure_point))
    }

    /// Loads and replays the current durable stroke batch.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` when any envelope, hash, root, stroke replay, or
    /// cross-record invariant fails.
    #[allow(clippy::too_many_lines)] // Keep capability, replay, and root validation in one read transaction.
    pub fn load_current(&self) -> Result<Option<ProjectCommitBatch>, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match transaction.open_table(STATE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(snapshot_bytes) = state
            .get(CURRENT_SNAPSHOT)
            .map_err(|error| self.io(error))?
        else {
            return Ok(None);
        };
        let snapshot_id =
            decode_snapshot_id(snapshot_bytes.value()).map_err(|message| self.corrupt(message))?;
        drop(snapshot_bytes);
        drop(state);

        let snapshots = transaction
            .open_table(SNAPSHOTS)
            .map_err(|error| self.corrupt(error))?;
        let head_bytes = get_required(
            &snapshots,
            snapshot_id.0.to_le_bytes().as_slice(),
            "snapshot head missing",
        )
        .map_err(|message| self.corrupt(message))?;
        let head_envelope = decode_envelope(RecordKind::SnapshotHead, &head_bytes)
            .map_err(|message| self.corrupt(format!("snapshot head corrupt: {message}")))?;
        let head = decode_project_head(&head_envelope.payload)
            .map_err(|error| self.corrupt(format!("snapshot head corrupt: {error}")))?;
        if head.snapshot_id != snapshot_id {
            return Err(self.corrupt("current snapshot key/head mismatch"));
        }
        drop(snapshots);

        let roots = transaction
            .open_table(ROOTS)
            .map_err(|error| self.corrupt(error))?;
        let objects = transaction
            .open_table(OBJECTS)
            .map_err(|error| self.corrupt(error))?;
        let before = load_root(&roots, &objects, head.before_root)
            .map_err(|message| self.corrupt(format!("before root corrupt: {message}")))?;
        let after = load_root(&roots, &objects, head.after_root)
            .map_err(|message| self.corrupt(format!("after root corrupt: {message}")))?;
        drop(objects);
        drop(roots);

        let strokes = transaction
            .open_table(STROKES)
            .map_err(|error| self.corrupt(error))?;
        let Some(stroke_commit) = head.stroke_commit else {
            return Ok(None);
        };
        let stroke_bytes = get_required(
            &strokes,
            (stroke_commit.0).0.as_slice(),
            "stroke commit missing",
        )
        .map_err(|message| self.corrupt(message))?;
        let stroke_envelope = decode_envelope(RecordKind::StrokeCommit, &stroke_bytes)
            .map_err(|message| self.corrupt(format!("stroke commit corrupt: {message}")))?;
        let stroke = decode_stroke_commit(&stroke_envelope.payload, &before)
            .map_err(|error| self.corrupt(error))?;
        if stroke.alpha_locked()
            || stroke
                .selection()
                .is_some_and(|selection| selection.origin() != [0, 0])
        {
            let metadata = transaction
                .open_table(META)
                .map_err(|error| self.corrupt(error))?;
            let schema = metadata
                .get("schema_version")
                .map_err(|error| self.corrupt(error))?
                .map_or(0, |value| value.value());
            if schema & SIGNED_SELECTION_SCHEMA_FLAG == 0
                && stroke
                    .selection()
                    .is_some_and(|selection| selection.origin() != [0, 0])
            {
                return Err(
                    self.corrupt("signed stroke selection lacks required reader capability")
                );
            }
            if stroke.alpha_locked() && schema & ALPHA_LOCK_STROKE_SCHEMA_FLAG == 0 {
                return Err(self.corrupt("alpha-locked stroke lacks required reader capability"));
            }
        }
        drop(strokes);

        let history = transaction
            .open_table(HISTORY)
            .map_err(|error| self.corrupt(error))?;
        let history_bytes = get_required(
            &history,
            head.history_head.0.to_le_bytes().as_slice(),
            "history node missing",
        )
        .map_err(|message| self.corrupt(message))?;
        let history_envelope = decode_envelope(RecordKind::HistoryNode, &history_bytes)
            .map_err(|message| self.corrupt(format!("history node corrupt: {message}")))?;
        let history_node =
            decode_history_node(&history_envelope.payload).map_err(|error| self.corrupt(error))?;

        let materialized =
            materialize_with_strategy(MaterializationStrategy::CpuReplay, &stroke, &before)
                .map_err(|error| self.corrupt(format!("stroke replay failed: {error:?}")))?;
        if materialized.after != after {
            return Err(self.corrupt("stored after tiles differ from CPU replay"));
        }
        let batch =
            ProjectCommitBatch::new(snapshot_id, before, stroke, materialized, history_node)
                .map_err(|error| self.corrupt(format!("commit graph mismatch: {error:?}")))?;
        if batch.head() != head {
            return Err(self.corrupt("reconstructed project head mismatch"));
        }
        Ok(Some(batch))
    }

    /// Reconstructs the whole persisted history DAG at the durable cursor.
    ///
    /// This intentionally keeps [`Self::load_current`] available for callers
    /// that only need the current closed-stroke batch. Reopen consumers should
    /// use this method so branch metadata is not discarded between processes.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` for an invalid history envelope/key/link/cursor or
    /// when the bounded history enumeration limit is exceeded.
    pub fn load_reopened(&self) -> Result<Option<ReopenedProject>, ProjectOpenError> {
        let stored_cursor = self.load_stored_cursor()?;
        let initial_cursor = self.load_initial_cursor()?;
        let current = self.load_current()?;
        let (current, current_snapshot, current_tiles, current_cursor) =
            match (stored_cursor, current) {
                (Some(cursor), Some(batch)) => {
                    if cursor.snapshot_id != batch.snapshot_id
                        || cursor.history_head != Some(batch.history_node.id)
                        || cursor.root != batch.materialized.after.root()
                    {
                        return Err(self.corrupt("current cursor/state snapshot mismatch"));
                    }
                    (
                        Some(batch.clone()),
                        batch.snapshot_id,
                        batch.materialized.after,
                        cursor,
                    )
                }
                (Some(cursor), None) => {
                    let tiles = self.load_cursor_tiles(cursor)?;
                    (None, cursor.snapshot_id, tiles, cursor)
                }
                (None, Some(batch)) => {
                    let cursor = ProjectHistoryCursor {
                        snapshot_id: batch.snapshot_id,
                        history_head: Some(batch.history_node.id),
                        root: batch.materialized.after.root(),
                    };
                    (
                        Some(batch.clone()),
                        batch.snapshot_id,
                        batch.materialized.after,
                        cursor,
                    )
                }
                (None, None) => {
                    let Some(cursor) = initial_cursor else {
                        if self.has_persisted_history_nodes()? {
                            return Err(self.corrupt("history exists without a current cursor"));
                        }
                        return Ok(None);
                    };
                    let tiles = self.load_cursor_tiles(cursor)?;
                    (None, cursor.snapshot_id, tiles, cursor)
                }
            };
        let nodes = self.load_persisted_history_nodes()?;
        if nodes.is_empty() {
            if current.is_some() || current_cursor.history_head.is_some() {
                return Err(self.corrupt("current state requires a persisted history node"));
            }
            let mut cursors = std::collections::BTreeMap::new();
            cursors.insert(None, current_cursor);
            return ReopenedProject::new(
                None,
                current_snapshot,
                current_tiles,
                current_cursor,
                History::new(current_cursor.root.id),
                cursors,
            )
            .map(Some)
            .map_err(|error| self.corrupt(format!("reopened initial state corrupt: {error:?}")));
        }
        let initial_root = initial_root_for(&nodes)
            .ok_or_else(|| self.corrupt("history graph has no initial node"))?;
        let history = History::from_persisted(initial_root, current_cursor.history_head, nodes)
            .map_err(|error| self.corrupt(format!("history graph corrupt: {error:?}")))?;
        let mut cursors = self.load_persisted_cursors(&history)?;
        if let Some(initial) = initial_cursor {
            if initial.history_head.is_some() || initial.root.id != history.initial_root() {
                return Err(self.corrupt("initial cursor/history root mismatch"));
            }
            cursors.insert(None, initial);
        }
        ReopenedProject::new(
            current,
            current_snapshot,
            current_tiles,
            current_cursor,
            history,
            cursors,
        )
        .map(Some)
        .map_err(|error| self.corrupt(format!("reopened state corrupt: {error:?}")))
    }

    /// Makes an existing snapshot/head pair durable without writing artwork.
    ///
    /// # Errors
    ///
    /// Returns corruption when the target does not exactly match its stored
    /// snapshot head, or I/O when the immediate state transaction fails.
    pub fn persist_history_cursor(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<(), ProjectOpenError> {
        match cursor.history_head {
            Some(_) => self.validate_cursor_snapshot(cursor)?,
            None => self.validate_initial_cursor(cursor)?,
        }
        let layers = self.cursor_layer_record(cursor)?;
        let canvas = self.cursor_canvas_record(cursor)?;
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        if let Some(canvas) = canvas {
            let mut metadata = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            write_canvas_metadata(&mut metadata, canvas).map_err(|error| self.io(error))?;
        }
        {
            let mut state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            if let Some(layers) = layers {
                if layers.is_empty() {
                    state
                        .remove(CURRENT_LAYER_TREE)
                        .map_err(|error| self.io(error))?;
                } else {
                    state
                        .insert(CURRENT_LAYER_TREE, layers.as_slice())
                        .map_err(|error| self.io(error))?;
                }
            }
            if let Some(history_head) = cursor.history_head {
                state
                    .insert(
                        CURRENT_SNAPSHOT,
                        cursor.snapshot_id.0.to_le_bytes().as_slice(),
                    )
                    .map_err(|error| self.io(error))?;
                let bytes = Envelope::new(
                    RecordKind::HistoryCursor,
                    encode_history_cursor(cursor.snapshot_id, history_head, cursor.root),
                )
                .encode();
                state
                    .insert(CURRENT_HISTORY_CURSOR, bytes.as_slice())
                    .map_err(|error| self.io(error))?;
            } else {
                state
                    .remove(CURRENT_SNAPSHOT)
                    .map_err(|error| self.io(error))?;
                state
                    .remove(CURRENT_HISTORY_CURSOR)
                    .map_err(|error| self.io(error))?;
            }
        }
        transaction.commit().map_err(|error| self.io(error))
    }

    /// Loads one cursor's immutable root after validating its snapshot head.
    ///
    /// # Errors
    ///
    /// Returns corruption when the cursor is not an exact snapshot head or
    /// when its canonical root cannot be reconstructed.
    pub fn load_cursor_tiles(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<TileSnapshot, ProjectOpenError> {
        match cursor.history_head {
            Some(_) => self.validate_cursor_snapshot(cursor)?,
            None => self.validate_initial_cursor(cursor)?,
        }
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let roots = transaction
            .open_table(ROOTS)
            .map_err(|error| self.corrupt(error))?;
        let objects = transaction
            .open_table(OBJECTS)
            .map_err(|error| self.corrupt(error))?;
        load_root(&roots, &objects, cursor.root)
            .map_err(|message| self.corrupt(format!("cursor root corrupt: {message}")))
    }

    /// Loads the finite export/display extent. Projects created before canvas
    /// metadata existed retain the current 1024x768 compatibility default and
    /// are not modified while opening.
    ///
    /// # Errors
    ///
    /// Returns corruption for a present but incomplete, unsupported, or
    /// invalid canvas metadata record.
    pub fn load_canvas_spec(&self) -> Result<CanvasSpec, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let table = transaction
            .open_table(META)
            .map_err(|error| self.corrupt(error))?;
        let version = metadata_value(&table, CANVAS_METADATA_VERSION_KEY)
            .map_err(|error| self.corrupt(error))?;
        let Some(version) = version else {
            if self.canvas_history_enabled()? {
                return Err(self.corrupt("versioned page history requires current canvas metadata"));
            }
            return Ok(CanvasSpec::DEFAULT);
        };
        if version != CANVAS_METADATA_VERSION {
            return Err(self.corrupt(format!("unsupported canvas metadata version {version}")));
        }
        let width = required_metadata_value(&table, CANVAS_WIDTH_KEY)
            .map_err(|error| self.corrupt(error))?;
        let height = required_metadata_value(&table, CANVAS_HEIGHT_KEY)
            .map_err(|error| self.corrupt(error))?;
        let pixels_per_inch =
            required_metadata_value(&table, CANVAS_PPI_KEY).map_err(|error| self.corrupt(error))?;
        let canvas = CanvasSpec {
            width_px: u32::try_from(width).map_err(|_| self.corrupt("canvas width exceeds u32"))?,
            height_px: u32::try_from(height)
                .map_err(|_| self.corrupt("canvas height exceeds u32"))?,
            pixels_per_inch: u32::try_from(pixels_per_inch)
                .map_err(|_| self.corrupt("canvas resolution exceeds u32"))?,
        };
        canvas
            .validate()
            .map_err(|error| self.corrupt(format!("invalid canvas metadata: {error:?}")))
    }

    /// Atomically publishes a validated finite canvas specification.
    ///
    /// # Errors
    ///
    /// Returns an error when the specification is invalid or immediate
    /// metadata durability cannot be established.
    pub fn persist_canvas_spec(&self, canvas: CanvasSpec) -> Result<(), ProjectOpenError> {
        if self.canvas_history_enabled()? {
            return Err(self.io("page history requires an atomic structural commit"));
        }
        let canvas = canvas
            .validate()
            .map_err(|error| self.io(format!("invalid canvas spec: {error:?}")))?;
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        {
            let mut table = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            write_canvas_metadata(&mut table, canvas).map_err(|error| self.io(error))?;
        }
        transaction.commit().map_err(|error| self.io(error))
    }

    /// Atomically publishes one complete validated layer hierarchy with
    /// immediate durability. A failed encoding or transaction leaves the
    /// previously published hierarchy current.
    ///
    /// # Errors
    ///
    /// Returns an encoding or storage error without changing the prior state.
    pub fn persist_layer_tree(&self, tree: &LayerTree) -> Result<(), ProjectOpenError> {
        if self.layer_history_enabled()? {
            return Err(self.io("layer history requires an atomic structural commit"));
        }
        let payload = encode_layer_tree(tree)
            .map_err(|error| self.io(format!("layer tree cannot be encoded: {error}")))?;
        let bytes = Envelope::new(RecordKind::LayerTree, payload).encode();
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        self.mark_layer_compositing(&transaction)?;
        {
            let mut state = transaction
                .open_table(STATE)
                .map_err(|error| self.io(error))?;
            state
                .insert(CURRENT_LAYER_TREE, bytes.as_slice())
                .map_err(|error| self.io(error))?;
        }
        transaction.commit().map_err(|error| self.io(error))
    }

    /// Loads and fully validates the current durable layer hierarchy.
    ///
    /// # Errors
    ///
    /// Returns scoped corruption for a malformed envelope or hierarchy and
    /// never rewrites the stored record.
    pub fn load_layer_tree(&self) -> Result<Option<LayerTree>, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match transaction.open_table(STATE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(bytes) = state
            .get(CURRENT_LAYER_TREE)
            .map_err(|error| self.io(error))?
        else {
            return Ok(None);
        };
        if bytes.value().len() > MAX_LAYER_TREE_RECORD_BYTES {
            return Err(self.corrupt("layer tree corrupt: record exceeds byte limit"));
        }
        let envelope = decode_envelope(RecordKind::LayerTree, bytes.value())
            .map_err(|message| self.corrupt(format!("layer tree corrupt: {message}")))?;
        self.validate_layer_payload_capability(&envelope.payload)?;
        decode_layer_tree(&envelope.payload)
            .map(Some)
            .map_err(|error| self.corrupt(format!("layer tree corrupt: {error}")))
    }

    fn validate_cursor_snapshot(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<(), ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let snapshots = transaction
            .open_table(SNAPSHOTS)
            .map_err(|error| self.corrupt(error))?;
        let bytes = get_required(
            &snapshots,
            cursor.snapshot_id.0.to_le_bytes().as_slice(),
            "cursor snapshot head missing",
        )
        .map_err(|message| self.corrupt(message))?;
        let envelope = decode_envelope(RecordKind::SnapshotHead, &bytes)
            .map_err(|message| self.corrupt(format!("cursor snapshot head corrupt: {message}")))?;
        let head = decode_project_head(&envelope.payload)
            .map_err(|error| self.corrupt(format!("cursor snapshot head corrupt: {error}")))?;
        if head.snapshot_id != cursor.snapshot_id
            || cursor.history_head != Some(head.history_head)
            || head.after_root != cursor.root
        {
            return Err(self.corrupt("cursor/snapshot head mismatch"));
        }
        Ok(())
    }

    fn validate_initial_cursor(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<(), ProjectOpenError> {
        if cursor.history_head.is_some() || self.load_initial_cursor()? != Some(cursor) {
            return Err(self.corrupt("initial cursor/state mismatch"));
        }
        Ok(())
    }

    fn load_stored_cursor(&self) -> Result<Option<ProjectHistoryCursor>, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match transaction.open_table(STATE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(bytes) = state
            .get(CURRENT_HISTORY_CURSOR)
            .map_err(|error| self.io(error))?
        else {
            return Ok(None);
        };
        let envelope = decode_envelope(RecordKind::HistoryCursor, bytes.value())
            .map_err(|message| self.corrupt(format!("current cursor corrupt: {message}")))?;
        decode_history_cursor(&envelope.payload)
            .map(Some)
            .map_err(|error| self.corrupt(format!("current cursor corrupt: {error}")))
    }

    fn load_initial_cursor(&self) -> Result<Option<ProjectHistoryCursor>, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match transaction.open_table(STATE) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(bytes) = state
            .get(INITIAL_HISTORY_CURSOR)
            .map_err(|error| self.io(error))?
        else {
            return Ok(None);
        };
        let envelope = decode_envelope(RecordKind::InitialHistoryCursor, bytes.value())
            .map_err(|message| self.corrupt(format!("initial cursor corrupt: {message}")))?;
        decode_initial_history_cursor(&envelope.payload)
            .map(Some)
            .map_err(|error| self.corrupt(format!("initial cursor corrupt: {error}")))
    }

    fn load_persisted_history_nodes(&self) -> Result<Vec<HistoryNode>, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let history = match transaction.open_table(HISTORY) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
            Err(error) => return Err(self.corrupt(error)),
        };
        let mut nodes = Vec::new();
        for entry in history.iter().map_err(|error| self.corrupt(error))? {
            if nodes.len() == MAX_REOPEN_HISTORY_NODES {
                return Err(self.corrupt(format!(
                    "history node count exceeds reopen limit {MAX_REOPEN_HISTORY_NODES}"
                )));
            }
            let (key, value) = entry.map_err(|error| self.corrupt(error))?;
            let key_id = decode_history_node_id(key.value())
                .map_err(|message| self.corrupt(format!("history node key corrupt: {message}")))?;
            let envelope = decode_envelope(RecordKind::HistoryNode, value.value())
                .map_err(|message| self.corrupt(format!("history node corrupt: {message}")))?;
            let node = decode_history_node(&envelope.payload)
                .map_err(|error| self.corrupt(format!("history node corrupt: {error}")))?;
            if node.id != key_id {
                return Err(self.corrupt("history node key/content mismatch"));
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    fn has_persisted_history_nodes(&self) -> Result<bool, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let history = match transaction.open_table(HISTORY) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(self.corrupt(error)),
        };
        history
            .iter()
            .map_err(|error| self.corrupt(error))?
            .next()
            .transpose()
            .map(|entry| entry.is_some())
            .map_err(|error| self.corrupt(error))
    }

    fn load_persisted_cursors(
        &self,
        history: &History,
    ) -> Result<
        std::collections::BTreeMap<Option<HistoryNodeId>, ProjectHistoryCursor>,
        ProjectOpenError,
    > {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let snapshots = transaction
            .open_table(SNAPSHOTS)
            .map_err(|error| self.corrupt(error))?;
        let mut cursors = std::collections::BTreeMap::new();
        for (count, entry) in snapshots
            .iter()
            .map_err(|error| self.corrupt(error))?
            .enumerate()
        {
            if count == MAX_REOPEN_HISTORY_NODES {
                return Err(self.corrupt(format!(
                    "snapshot count exceeds reopen limit {MAX_REOPEN_HISTORY_NODES}"
                )));
            }
            let (key, value) = entry.map_err(|error| self.corrupt(error))?;
            let snapshot_id = decode_snapshot_id(key.value())
                .map_err(|message| self.corrupt(format!("snapshot key corrupt: {message}")))?;
            let envelope = decode_envelope(RecordKind::SnapshotHead, value.value())
                .map_err(|message| self.corrupt(format!("snapshot head corrupt: {message}")))?;
            let head = decode_project_head(&envelope.payload)
                .map_err(|error| self.corrupt(format!("snapshot head corrupt: {error}")))?;
            if head.snapshot_id != snapshot_id {
                return Err(self.corrupt("snapshot key/head mismatch"));
            }
            let Some(node) = history.node(head.history_head) else {
                continue;
            };
            if node.before_root != head.before_root.id || node.after_root != head.after_root.id {
                return Err(self.corrupt("snapshot/history root transition mismatch"));
            }
            let cursor = ProjectHistoryCursor {
                snapshot_id,
                history_head: Some(head.history_head),
                root: head.after_root,
            };
            if cursors.insert(Some(head.history_head), cursor).is_some() {
                return Err(self.corrupt("multiple snapshots for one history node"));
            }
        }
        if cursors.len() != history.node_count() {
            return Err(self.corrupt("history node snapshot missing"));
        }
        Ok(cursors)
    }

    fn write_marker(&self) -> Result<(), ProjectOpenError> {
        let mut transaction = self.db.begin_write().map_err(|error| self.io(error))?;
        transaction
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        {
            let mut table = transaction
                .open_table(META)
                .map_err(|error| self.io(error))?;
            table
                .insert(
                    "schema_version",
                    SCHEMA_VERSION | COMPRESSED_TILE_SCHEMA_FLAG,
                )
                .map_err(|error| self.io(error))?;
            table
                .insert(object_retention::REFERENCE_VERSION_KEY, 1)
                .map_err(|e| self.io(e))?;
            table
                .insert("tile_storage_revision", 1)
                .map_err(|e| self.io(e))?;
            write_canvas_metadata(&mut table, CanvasSpec::DEFAULT)
                .map_err(|error| self.io(error))?;
        }
        transaction
            .open_table(object_retention::TILE_ROOT_REFS)
            .map_err(|e| self.io(e))?;
        transaction.commit().map_err(|error| self.io(error))
    }

    fn has_valid_marker(&self) -> Result<bool, ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let table = match transaction.open_table(META) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(self.io(error)),
        };
        Ok(table
            .get("schema_version")
            .map_err(|error| self.io(error))?
            .is_some_and(|value| {
                matches!(
                    value.value() & !SCHEMA_CAPABILITY_FLAGS,
                    SCHEMA_VERSION
                        | LAYER_HISTORY_SCHEMA_VERSION
                        | SELECTION_STROKE_SCHEMA_VERSION
                        | CANVAS_HISTORY_SCHEMA_VERSION
                )
            }))
    }

    #[allow(clippy::needless_pass_by_value)]
    fn io(&self, error: impl ToString) -> ProjectOpenError {
        ProjectOpenError::Io {
            path: self.path.clone(),
            message: error.to_string(),
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    fn corrupt(&self, error: impl ToString) -> ProjectOpenError {
        ProjectOpenError::Corrupt {
            path: self.path.clone(),
            message: error.to_string(),
        }
    }
}

#[cfg(feature = "diagnostic")]
fn pause_for_diagnostic_kill(
    stage_path: &Path,
    boundary: DiagnosticCommitBoundary,
) -> Result<(), std::io::Error> {
    let stage_tmp = stage_path.with_extension(format!("stage-tmp-{}", std::process::id()));
    let mut stage = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage_tmp)?;
    let boundary = match boundary {
        DiagnosticCommitBoundary::BeforeCommit => "before-commit",
        DiagnosticCommitBoundary::AfterDurableCommit => "after-durable-commit",
    };
    writeln!(stage, "pid={} boundary={boundary}", std::process::id())?;
    stage.sync_all()?;
    drop(stage);
    fs::rename(stage_tmp, stage_path)?;
    loop {
        thread::sleep(Duration::from_millis(10));
    }
}

impl ProjectRepository for ProjectDb {
    fn commit(&self, batch: &ProjectCommitBatch) -> Result<(), ProjectOpenError> {
        Self::commit(self, batch)
    }

    fn load_current(&self) -> Result<Option<ProjectCommitBatch>, ProjectOpenError> {
        Self::load_current(self)
    }

    fn load_reopened(&self) -> Result<Option<ReopenedProject>, ProjectOpenError> {
        Self::load_reopened(self)
    }

    fn persist_history_cursor(&self, cursor: ProjectHistoryCursor) -> Result<(), ProjectOpenError> {
        Self::persist_history_cursor(self, cursor)
    }

    fn load_cursor_tiles(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<TileSnapshot, ProjectOpenError> {
        Self::load_cursor_tiles(self, cursor)
    }

    fn load_canvas_spec(&self) -> Result<CanvasSpec, ProjectOpenError> {
        Self::load_canvas_spec(self)
    }

    fn persist_canvas_spec(&self, canvas: CanvasSpec) -> Result<(), ProjectOpenError> {
        Self::persist_canvas_spec(self, canvas)
    }

    fn persist_layer_tree(&self, tree: &LayerTree) -> Result<(), ProjectOpenError> {
        Self::persist_layer_tree(self, tree)
    }

    fn load_layer_tree(&self) -> Result<Option<LayerTree>, ProjectOpenError> {
        Self::load_layer_tree(self)
    }
}

fn initial_root_for(nodes: &[HistoryNode]) -> Option<ContentRootId> {
    nodes
        .iter()
        .find(|node| node.parent.is_none())
        .map(|node| node.before_root)
}

fn store_tile_objects(
    table: &mut redb::Table<'_, &[u8], &[u8]>,
    snapshot: &TileSnapshot,
) -> Result<(), redb::StorageError> {
    for (_, tile) in snapshot.iter() {
        if let Some(existing) = table.get(tile.hash().0.as_slice())?
            && let Ok(envelope) = Envelope::decode(existing.value(), RecordKind::Tile)
            && envelope.payload == tile.pixels()
        {
            continue;
        }
        let envelope = Envelope::new(RecordKind::Tile, tile.pixels().to_vec()).encode_for_storage();
        insert_changed_envelope(table, tile.hash().0.as_slice(), &envelope)?;
    }
    Ok(())
}

fn store_root(
    table: &mut redb::Table<'_, &[u8], &[u8]>,
    snapshot: &TileSnapshot,
) -> Result<(), redb::StorageError> {
    let envelope = Envelope::new(RecordKind::ContentRoot, encode_root_manifest(snapshot)).encode();
    insert_changed_envelope(table, snapshot.root().hash.0.as_slice(), &envelope)?;
    Ok(())
}

fn insert_changed_envelope(
    table: &mut redb::Table<'_, &[u8], &[u8]>,
    key: &[u8],
    envelope: &[u8],
) -> Result<(), redb::StorageError> {
    // Immutable hashes often recur in both roots and in subsequent commits.
    // Compare the full record, not just key presence: a damaged existing value
    // must still be replaced by the canonical record, as before this shortcut.
    let unchanged = table
        .get(key)?
        .is_some_and(|value| value.value() == envelope);
    if !unchanged {
        table.insert(key, envelope)?;
    }
    Ok(())
}

fn load_root(
    roots: &redb::ReadOnlyTable<&[u8], &[u8]>,
    objects: &redb::ReadOnlyTable<&[u8], &[u8]>,
    expected: ContentRoot,
) -> Result<TileSnapshot, String> {
    let manifest_bytes = get_required(roots, expected.hash.0.as_slice(), "root manifest missing")?;
    let envelope = decode_envelope(RecordKind::ContentRoot, &manifest_bytes)
        .map_err(|message| format!("root manifest corrupt: {message}"))?;
    let manifest = decode_root_manifest(&envelope.payload)
        .map_err(|error| format!("root manifest corrupt: {error}"))?;
    if manifest.root != expected {
        return Err("root manifest key/content mismatch".into());
    }
    snapshot_from_manifest(objects, manifest)
}

fn snapshot_from_manifest(
    objects: &redb::ReadOnlyTable<&[u8], &[u8]>,
    manifest: RootManifest,
) -> Result<TileSnapshot, String> {
    let mut tiles = Vec::with_capacity(manifest.tiles.len());
    // A root can reference one content-addressed object at many signed keys.
    // Validate each unique record once in this read transaction and share its
    // immutable pixels. This cache never survives a load or masks later disk
    // corruption by borrowing objects from an earlier snapshot.
    let mut validated = std::collections::BTreeMap::<ObjectHash, TileObject>::new();
    for (key, expected_hash) in manifest.tiles {
        let object = match validated.entry(expected_hash) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let bytes =
                    get_required(objects, expected_hash.0.as_slice(), "tile object missing")?;
                let envelope = decode_envelope(RecordKind::Tile, &bytes)
                    .map_err(|message| format!("tile object corrupt: {message}"))?;
                let object = TileObject::new(envelope.payload)
                    .map_err(|error| format!("tile object corrupt: {error:?}"))?;
                if object.hash() != expected_hash {
                    return Err("tile object key/content mismatch".into());
                }
                entry.insert(object).clone()
            }
        };
        tiles.push((key, object));
    }
    let snapshot = TileSnapshot::from_objects(tiles).map_err(|error| format!("{error:?}"))?;
    if snapshot.root() != manifest.root {
        return Err("reconstructed tile root mismatch".into());
    }
    Ok(snapshot)
}

fn get_required(
    table: &redb::ReadOnlyTable<&[u8], &[u8]>,
    key: &[u8],
    missing: &'static str,
) -> Result<Vec<u8>, String> {
    table
        .get(key)
        .map_err(|error| error.to_string())?
        .map(|guard| guard.value().to_vec())
        .ok_or_else(|| missing.into())
}

fn metadata_value(
    table: &redb::ReadOnlyTable<&str, u64>,
    key: &str,
) -> Result<Option<u64>, String> {
    table
        .get(key)
        .map(|value| value.map(|value| value.value()))
        .map_err(|error| error.to_string())
}

fn required_metadata_value(
    table: &redb::ReadOnlyTable<&str, u64>,
    key: &str,
) -> Result<u64, String> {
    metadata_value(table, key)?.ok_or_else(|| format!("canvas metadata missing {key}"))
}

fn write_canvas_metadata(
    table: &mut redb::Table<'_, &str, u64>,
    canvas: CanvasSpec,
) -> Result<(), String> {
    table
        .insert(CANVAS_METADATA_VERSION_KEY, CANVAS_METADATA_VERSION)
        .map_err(|error| error.to_string())?;
    table
        .insert(CANVAS_WIDTH_KEY, u64::from(canvas.width_px))
        .map_err(|error| error.to_string())?;
    table
        .insert(CANVAS_HEIGHT_KEY, u64::from(canvas.height_px))
        .map_err(|error| error.to_string())?;
    table
        .insert(CANVAS_PPI_KEY, u64::from(canvas.pixels_per_inch))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn decode_envelope(expected: RecordKind, bytes: &[u8]) -> Result<Envelope, String> {
    Envelope::decode(bytes, expected).map_err(|error| error.to_string())
}

fn decode_snapshot_id(bytes: &[u8]) -> Result<SnapshotId, &'static str> {
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| "current snapshot id has invalid length")?;
    Ok(SnapshotId(u128::from_le_bytes(bytes)))
}

fn decode_history_node_id(bytes: &[u8]) -> Result<HistoryNodeId, &'static str> {
    let bytes: [u8; 16] = bytes
        .try_into()
        .map_err(|_| "history node key has invalid length")?;
    Ok(HistoryNodeId(u128::from_le_bytes(bytes)))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nyatidraw_api::{ContentRootId, GroupId, HistoryNodeId, LayerId, LayerTreeNodeId};
    use nyatidraw_brush::{
        BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, ROUND_BRUSH_ENGINE_VERSION,
        RoundBrushEvaluator, begin_round_stroke,
    };
    use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
    use nyatidraw_editor::HeadlessStrokeSession;
    use nyatidraw_history::OperationRecord;
    use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
    use nyatidraw_stroke::StrokeColor;
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    use super::*;

    fn temp(name: &str) -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "nyatidraw-{name}-{}-{sequence}.redb",
            std::process::id()
        ))
    }

    fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 1_000,
            device_id: 9,
            phase,
            position_document: Point { x, y },
            pressure: 0.75,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 4,
        }
    }

    fn prepared_batch_from(
        parent_snapshot: SnapshotId,
        before: TileSnapshot,
        snapshot_id: SnapshotId,
        history_id: HistoryNodeId,
        seed: u64,
        sequence: u64,
    ) -> ProjectCommitBatch {
        let preset = BrushPreset {
            id: BrushPresetId(17),
            schema_version: 1,
            engine_version: 1,
            size_px: 18.0,
            opacity: 0.9,
            flow: 0.6,
            spacing_ratio: 0.2,
            size_pressure: true,
            opacity_pressure: true,
            size_min_ratio: 0.0,
            opacity_min_ratio: 0.0,
            hardness: 1.0,
        };
        let samples = vec![
            sample(sequence, PointerPhase::Begin, -8.0, 5.0),
            sample(sequence + 1, PointerPhase::Move, 32.0, 18.0),
            sample(sequence + 2, PointerPhase::End, 150.0, 31.0),
        ];
        let mut evaluator = RoundBrushEvaluator::new(seed);
        let mut dabs = Vec::new();
        let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
        evaluator.push(&mut token, &samples[1..], &mut dabs);
        let recorded = evaluator.end(token, &mut dabs);
        HeadlessStrokeSession::new(parent_snapshot, before)
            .prepare_round_stroke(
                snapshot_id,
                history_id,
                12_000,
                LayerId(4),
                BrushSnapshot { preset },
                recorded,
                StrokeColor::new([18, 9, 3, 24]).expect("premultiplied fixture"),
                samples,
            )
            .expect("prepare fixture")
    }

    fn prepared_batch() -> ProjectCommitBatch {
        prepared_batch_from(
            SnapshotId(1),
            TileSnapshot::empty(),
            SnapshotId(2),
            HistoryNodeId(3),
            42,
            10,
        )
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn alpha_locked_stroke_wire_reopen_and_gate_preserve_recoloring() {
        // Product risk: older writers drop alpha-lock replay, an extension
        // changes old hashes, or reopen silently changes soft-edge alpha.
        use nyatidraw_stroke::{CpuReplayMaterializer, StrokeMaterializer, StrokeSelection};
        const REOPEN: &str = "NYATIDRAW_ALPHA_LOCK_REOPEN";
        let before = TileSnapshot::from_tiles([-1, 0, 1].map(|x| {
            let pixels = (0..TILE_BYTE_LEN / 4)
                .flat_map(|index| {
                    let alpha = if index % 5 == 0 { 0 } else { 128 };
                    [0, alpha / 2, 0, alpha]
                })
                .collect::<Vec<_>>();
            (
                TileKey {
                    layer: LayerId(4),
                    mip: 0,
                    x,
                    y: 0,
                },
                pixels,
            )
        }))
        .unwrap();
        let template = prepared_batch_from(
            SnapshotId(1),
            before,
            SnapshotId(2),
            HistoryNodeId(3),
            42,
            10,
        );
        let bare = encode_stroke_commit(&template.stroke);
        assert!(!template.stroke.alpha_locked());
        assert_eq!(
            decode_stroke_commit(&bare, &template.before).unwrap(),
            template.stroke
        );
        let mut durable = None;
        for origin in [None, Some([0, 0]), Some([-16, -4])] {
            let selection = origin.map(|origin| {
                std::sync::Arc::new(
                    StrokeSelection::from_packed_bits_at(
                        origin,
                        [176, 64],
                        &vec![255; 176 * 64 / 8],
                    )
                    .unwrap(),
                )
            });
            let session = HeadlessStrokeSession::new(
                template.stroke.parent_snapshot,
                template.before.clone(),
            );
            let prepare = |alpha_locked| {
                session
                    .prepare_round_stroke_with_options(
                        template.snapshot_id,
                        template.history_node.id,
                        template.history_node.timestamp_ns,
                        template.stroke.layer,
                        template.stroke.brush,
                        template.stroke.recorded.clone(),
                        template.stroke.color,
                        template.stroke.samples().to_vec(),
                        selection.clone(),
                        alpha_locked,
                    )
                    .unwrap()
            };
            let unlocked = prepare(false);
            let legacy = session
                .prepare_round_stroke_with_selection(
                    template.snapshot_id,
                    template.history_node.id,
                    template.history_node.timestamp_ns,
                    template.stroke.layer,
                    template.stroke.brush,
                    template.stroke.recorded.clone(),
                    template.stroke.color,
                    template.stroke.samples().to_vec(),
                    selection.clone(),
                )
                .unwrap();
            assert_eq!(
                encode_stroke_commit(&unlocked.stroke),
                encode_stroke_commit(&legacy.stroke),
                "unlocked wire must remain byte-identical to legacy sealing"
            );
            assert_eq!(unlocked.stroke.id, legacy.stroke.id);
            let batch = prepare(true);
            let wire = encode_stroke_commit(&batch.stroke);
            assert_eq!(
                &wire[32..bare.len()],
                &bare[32..],
                "only semantic identity and optional suffix may change"
            );
            assert_eq!(&wire[bare.len()..bare.len() + 8], b"NYALP001");
            assert_eq!(
                decode_stroke_commit(&wire, &template.before).unwrap(),
                batch.stroke
            );
            for mode in 0..4 {
                let mut bad = wire.clone();
                match mode {
                    0 => {
                        bad.drain(bare.len()..bare.len() + 8);
                    }
                    1 => {
                        bad.splice(bare.len()..bare.len(), *b"NYALP001");
                    }
                    2 => {
                        bad[bare.len() + 4] ^= 1;
                    }
                    _ => {
                        bad.pop();
                    }
                }
                assert!(
                    decode_stroke_commit(&bad, &template.before).is_err(),
                    "noncanonical alpha extension must fail closed"
                );
            }
            for (key, original) in template.before.iter() {
                let after = batch.materialized.after.get(key).unwrap();
                assert!(
                    original
                        .pixels()
                        .chunks_exact(4)
                        .zip(after.pixels().chunks_exact(4))
                        .all(|(a, b)| a[3] == b[3]),
                    "durable recoloring must retain every alpha byte"
                );
            }
            assert!(!batch.materialized.changed_tiles.is_empty());
            durable = Some(batch);
        }
        let batch = durable.unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let db = ProjectDb::open(&PathBuf::from(path)).unwrap();
            let loaded = db.load_current().unwrap().unwrap();
            assert_eq!(loaded.stroke, batch.stroke);
            assert_eq!(loaded.materialized.after, batch.materialized.after);
            assert_eq!(
                CpuReplayMaterializer
                    .materialize(&loaded.stroke, &loaded.before)
                    .unwrap()
                    .after,
                batch.materialized.after
            );
            return;
        }
        let path = temp("alpha-locked-stroke");
        let db = ProjectDb::open(&path).unwrap();
        assert!(
            db.commit_with_failure(&batch, CommitFailurePoint::BeforeCommitAbort)
                .is_err()
        );
        assert!(db.load_current().unwrap().is_none());
        let marker = |db: &ProjectDb| {
            db.db
                .begin_read()
                .unwrap()
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value()
        };
        assert_eq!(
            marker(&db) & ALPHA_LOCK_STROKE_SCHEMA_FLAG,
            0,
            "failed commit cannot upgrade reader capability"
        );
        db.commit(&batch).unwrap();
        assert_ne!(marker(&db) & ALPHA_LOCK_STROKE_SCHEMA_FLAG, 0);
        let old_flags = SCHEMA_CAPABILITY_FLAGS & !ALPHA_LOCK_STROKE_SCHEMA_FLAG;
        assert!(
            !(1..=4).contains(&(marker(&db) & !old_flags)),
            "older reader must reject before modifying artwork"
        );
        drop(db);
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::alpha_locked_stroke_wire_reopen_and_gate_preserve_recoloring"
                ])
                .env(REOPEN, &path)
                .status()
                .unwrap()
                .success()
        );
        let db = ProjectDb::open(&path).unwrap();
        let session = HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        let structural = session
            .prepare_structural_change(
                SnapshotId(3),
                HistoryNodeId(4),
                13_000,
                session.tiles().clone(),
            )
            .unwrap();
        db.commit_structural_with_layer_tree(&structural, &layer_tree_fixture())
            .unwrap();
        let after_layer = marker(&db);
        assert_ne!(
            after_layer & ALPHA_LOCK_STROKE_SCHEMA_FLAG,
            0,
            "metadata upgrade cannot drop capability for an older stroke"
        );
        db.persist_history_cursor(ProjectHistoryCursor {
            snapshot_id: batch.snapshot_id,
            history_head: Some(batch.history_node.id),
            root: batch.materialized.after.root(),
        })
        .unwrap();
        let tx = db.db.begin_write().unwrap();
        tx.open_table(META)
            .unwrap()
            .insert(
                "schema_version",
                after_layer & !ALPHA_LOCK_STROKE_SCHEMA_FLAG,
            )
            .unwrap();
        tx.commit().unwrap();
        assert!(
            db.load_current().is_err(),
            "missing semantic-stroke reader gate is corruption"
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn configurable_brush_survives_reopen_and_metadata_upgrades_without_losing_reader_gate() {
        // Risk: old writers must not discard configurable brush meaning, even
        // after a layer/page upgrade or Undo preserves the v2 redo branch.
        let legacy = prepared_batch();
        let preset = BrushPreset {
            schema_version: 2,
            engine_version: ROUND_BRUSH_ENGINE_VERSION,
            opacity_pressure: false,
            size_min_ratio: 0.2,
            opacity_min_ratio: 0.1,
            hardness: 0.3,
            ..legacy.stroke.brush.preset
        };
        let samples = legacy.stroke.samples().to_vec();
        let mut evaluator = RoundBrushEvaluator::new(42);
        let mut dabs = Vec::new();
        let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
        evaluator.push(&mut token, &samples[1..], &mut dabs);
        let recorded = evaluator.end(token, &mut dabs);
        let batch = HeadlessStrokeSession::new(SnapshotId(1), TileSnapshot::empty())
            .prepare_round_stroke(
                SnapshotId(2),
                HistoryNodeId(3),
                12_000,
                LayerId(4),
                BrushSnapshot { preset },
                recorded,
                legacy.stroke.color,
                samples,
            )
            .unwrap();
        let path = temp("configurable-brush");
        let database = ProjectDb::open(&path).unwrap();
        database.commit(&batch).unwrap();
        drop(database);
        let database = ProjectDb::open(&path).unwrap();
        assert_eq!(
            database
                .load_current()
                .unwrap()
                .unwrap()
                .stroke
                .brush
                .preset,
            preset
        );
        assert_eq!(
            database.load_reopened().unwrap().unwrap().current_tiles(),
            &batch.materialized.after
        );
        for id in 3..=4 {
            let session =
                HeadlessStrokeSession::from_reopened(database.load_reopened().unwrap().unwrap());
            let change = session
                .prepare_structural_change(
                    SnapshotId(id),
                    HistoryNodeId(id + 1),
                    13_000,
                    session.tiles().clone(),
                )
                .unwrap();
            if id == 3 {
                database
                    .commit_structural_with_layer_tree(&change, &layer_tree_fixture())
                    .unwrap();
            } else {
                database
                    .commit_structural_with_canvas(&change, CanvasSpec::DEFAULT)
                    .unwrap();
            }
        }
        assert!(database.layer_history_enabled().unwrap());
        assert!(database.canvas_history_enabled().unwrap());
        let session =
            HeadlessStrokeSession::from_reopened(database.load_reopened().unwrap().unwrap());
        database
            .persist_history_cursor(session.prepare_undo_cursor().unwrap().target())
            .unwrap();
        let tx = database.db.begin_read().unwrap();
        let marker = tx
            .open_table(META)
            .unwrap()
            .get("schema_version")
            .unwrap()
            .unwrap()
            .value();
        assert_eq!(
            marker,
            CANVAS_HISTORY_SCHEMA_VERSION
                | COMPRESSED_TILE_SCHEMA_FLAG
                | CONFIGURABLE_BRUSH_SCHEMA_FLAG
                | LAYER_COMPOSITING_SCHEMA_FLAG
        );
        // The original reader masked compression only; this must not resemble
        // one of its known levels 1..=4.
        assert!(!(1..=4).contains(&(marker & !COMPRESSED_TILE_SCHEMA_FLAG)));
        drop(tx);
        drop(database);
        let reopened = ProjectDb::open(&path).unwrap();
        assert_eq!(
            reopened.load_reopened().unwrap().unwrap().current_tiles(),
            &batch.materialized.after
        );
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One end-to-end artwork/backup/schema invariant.
    fn legacy_raw_artwork_reopens_exactly_and_new_writes_gate_old_readers() {
        // Product risk: automatic alpha recompression must preserve artwork and
        // its original backup; old writers must not read the compressed schema.
        let path = temp("raw-compatibility");
        let first = prepared_batch();
        let db = ProjectDb::open(&path).unwrap();
        db.commit(&first).unwrap();
        let tx = db.db.begin_write().unwrap();
        {
            let mut table = tx.open_table(OBJECTS).unwrap();
            let records: Vec<_> = table
                .iter()
                .unwrap()
                .map(|entry| {
                    let (key, value) = entry.unwrap();
                    (
                        key.value().to_vec(),
                        Envelope::decode(value.value(), RecordKind::Tile)
                            .unwrap()
                            .encode(),
                    )
                })
                .collect();
            for (key, bytes) in records {
                table.insert(key.as_slice(), bytes.as_slice()).unwrap();
            }
        }
        {
            let mut meta = tx.open_table(META).unwrap();
            meta.insert("schema_version", SCHEMA_VERSION).unwrap();
            meta.remove(object_retention::REFERENCE_VERSION_KEY)
                .unwrap();
            meta.remove("tile_storage_revision").unwrap();
        }
        tx.delete_table(object_retention::TILE_ROOT_REFS).unwrap();
        tx.commit().unwrap();
        drop(db);
        let raw_records = |path: &Path| {
            let db = redb::ReadOnlyDatabase::open(path).unwrap();
            let tx = db.begin_read().unwrap();
            let objects = tx
                .open_table(OBJECTS)
                .unwrap()
                .iter()
                .unwrap()
                .map(|entry| {
                    let (key, value) = entry.unwrap();
                    (key.value().to_vec(), value.value().to_vec())
                })
                .collect::<Vec<_>>();
            let marker = tx
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value();
            (objects, marker)
        };
        let original = raw_records(&path);
        #[cfg(feature = "legacy-migration")]
        let original_file = std::fs::read(&path).unwrap();
        let db = ProjectDb::open(&path).unwrap();
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles(),
            &first.materialized.after
        );
        drop(db);
        #[cfg(not(feature = "legacy-migration"))]
        assert_eq!(raw_records(&path), original);
        #[cfg(feature = "legacy-migration")]
        {
            let rebuilt = raw_records(&path);
            assert_eq!(rebuilt.1, original.1 | COMPRESSED_TILE_SCHEMA_FLAG);
            assert_eq!(rebuilt.0.len(), original.0.len());
            let mut before_len = 0;
            let mut after_len = 0;
            for ((old_key, old), (new_key, new)) in original.0.iter().zip(&rebuilt.0) {
                assert_eq!(old_key, new_key);
                assert_eq!(
                    Envelope::decode(old, RecordKind::Tile).unwrap(),
                    Envelope::decode(new, RecordKind::Tile).unwrap()
                );
                before_len += old.len();
                after_len += new.len();
            }
            assert!(after_len < before_len);
            migration::tests::remove_verified_backup(&path, &original_file);
        }
        let db = ProjectDb::open(&path).unwrap();
        let mut second = prepared_batch_from(
            first.snapshot_id,
            first.materialized.after.clone(),
            SnapshotId(8),
            HistoryNodeId(9),
            99,
            20,
        );
        second.history_node.parent = Some(first.history_node.id);
        db.commit(&second).unwrap();
        let read = db.db.begin_read().unwrap();
        let marker = read
            .open_table(META)
            .unwrap()
            .get("schema_version")
            .unwrap()
            .unwrap()
            .value();
        assert_eq!(marker, SCHEMA_VERSION | COMPRESSED_TILE_SCHEMA_FLAG);
        assert!(!matches!(marker, 1..=4));
        drop(read);
        drop(db);
        assert_eq!(
            ProjectDb::open(&path)
                .unwrap()
                .load_reopened()
                .unwrap()
                .unwrap()
                .current_tiles(),
            &second.materialized.after
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn retention_preserves_artwork_metadata_and_128_undo_steps_across_restart() {
        // Product risk: evicting history must not drop current/off-page pixels,
        // split page/layer state, or publish a partially pruned transaction.
        fn tiles(index: u8) -> TileSnapshot {
            TileSnapshot::from_tiles([
                (
                    TileKey::from_pixel(LayerId(20), 128, -140, 22),
                    vec![index; TILE_BYTE_LEN],
                ),
                (
                    TileKey::from_pixel(LayerId(20), 128, 0, 0),
                    vec![255; TILE_BYTE_LEN],
                ),
                (
                    TileKey::from_pixel(LayerId(20), 128, 128, 0),
                    vec![255; TILE_BYTE_LEN],
                ),
            ])
            .unwrap()
        }
        fn tree(index: u8) -> LayerTree {
            let mut tree = layer_tree_fixture();
            tree.rename(
                LayerTreeNodeId::Raster(LayerId(20)),
                &format!("Layer {index}"),
            )
            .unwrap();
            tree
        }
        fn page(index: u8) -> CanvasSpec {
            CanvasSpec {
                width_px: 1000 + u32::from(index),
                height_px: 800,
                pixels_per_inch: 72,
            }
        }
        if let Some(path) = std::env::var_os("NYATIDRAW_RETENTION_REOPEN") {
            let database = ProjectDb::open(Path::new(&path)).unwrap();
            let reopened = database.load_reopened().unwrap().unwrap();
            assert_eq!(reopened.history().node_count(), 128);
            assert_eq!(reopened.current_tiles(), &tiles(132));
            {
                use redb::ReadableTableMetadata;
                let tx = database.db.begin_read().unwrap();
                assert_eq!(tx.open_table(ROOTS).unwrap().len().unwrap(), 129);
                // One shared object must survive reclamation of old roots,
                // even though it appears at two coordinates per root.
                assert_eq!(tx.open_table(OBJECTS).unwrap().len().unwrap(), 130);
            }
            let mut session = HeadlessStrokeSession::from_reopened(reopened);
            for index in (4..132).rev() {
                let movement = session.prepare_undo_cursor().unwrap();
                let cursor = movement.target();
                database.persist_history_cursor(cursor).unwrap();
                let restored = database.load_cursor_tiles(cursor).unwrap();
                assert_eq!(restored, tiles(index));
                assert_eq!(
                    database.load_cursor_layer_tree(cursor).unwrap(),
                    Some(tree(index))
                );
                assert_eq!(
                    database.load_cursor_canvas_spec(cursor).unwrap(),
                    page(index)
                );
                session
                    .accept_history_cursor_move(movement, restored)
                    .unwrap();
            }
            assert!(session.prepare_undo_cursor().is_err());
            for _ in 0..128 {
                let movement = session.prepare_redo_cursor().unwrap();
                database.persist_history_cursor(movement.target()).unwrap();
                let restored = database.load_cursor_tiles(movement.target()).unwrap();
                session
                    .accept_history_cursor_move(movement, restored)
                    .unwrap();
            }
            assert_eq!(session.tiles(), &tiles(132));
            return;
        }
        let path = temp("retention-128");
        let database = ProjectDb::open(&path).unwrap();
        database.persist_layer_tree(&tree(0)).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        for index in 1..=132_u8 {
            let batch = session
                .prepare_structural_change(
                    SnapshotId(u128::from(index)),
                    HistoryNodeId(u128::from(index)),
                    u64::from(index),
                    tiles(index),
                )
                .unwrap();
            if index == 129 {
                assert!(
                    database
                        .commit_structural_inner(
                            &batch,
                            Some(&tree(index)),
                            Some(page(index)),
                            &CommitControl::Failure(CommitFailurePoint::BeforeCommitAbort)
                        )
                        .is_err()
                );
                let before = database.load_reopened().unwrap().unwrap();
                assert_eq!(before.history().node_count(), 128);
                assert_eq!(before.current_tiles(), &tiles(128));
                assert_eq!(
                    before.history().initial_root(),
                    TileSnapshot::empty().root().id
                );
                database.validate_layer_history().unwrap();
                database.validate_canvas_history().unwrap();
            }
            database
                .commit_structural_inner(
                    &batch,
                    Some(&tree(index)),
                    Some(page(index)),
                    &CommitControl::Normal,
                )
                .unwrap();
            session.accept_structural_change(&batch).unwrap();
            assert!(session.history().node_count() <= 128);
        }
        drop(session);
        drop(database);
        #[cfg(feature = "legacy-migration")]
        let legacy_original = {
            // The existing full-history child acceptance must also pass through
            // the real alpha v2 -> v3 auto-conversion, not only fresh redb 4 files.
            let legacy = path.with_extension("v2.ntdr");
            migration::tests::write_v2_copy(&path, &legacy);
            std::fs::remove_file(&path).unwrap();
            std::fs::rename(&legacy, &path).unwrap();
            std::fs::read(&path).unwrap()
        };
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::retention_preserves_artwork_metadata_and_128_undo_steps_across_restart",
                "--nocapture",
            ])
            .env("NYATIDRAW_RETENTION_REOPEN", &path)
            .status()
            .unwrap();
        assert!(status.success(), "fresh process validates all 128 steps");
        #[cfg(feature = "legacy-migration")]
        migration::tests::remove_verified_backup(&path, &legacy_original);
        let database = ProjectDb::open(&path).unwrap();
        let mut session =
            HeadlessStrokeSession::from_reopened(database.load_reopened().unwrap().unwrap());
        for _ in 0..3 {
            let movement = session.prepare_undo_cursor().unwrap();
            database.persist_history_cursor(movement.target()).unwrap();
            let restored = database.load_cursor_tiles(movement.target()).unwrap();
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
        }
        let prepared = prepared_batch_from(
            session.current_snapshot(),
            session.tiles().clone(),
            SnapshotId(140),
            HistoryNodeId(140),
            902,
            700,
        );
        let batch = ProjectCommitBatch::new(
            prepared.snapshot_id,
            prepared.before,
            prepared.stroke,
            prepared.materialized,
            HistoryNode {
                parent: session.history().head(),
                ..prepared.history_node
            },
        )
        .unwrap();
        database.commit(&batch).unwrap();
        session.accept_committed(&batch).unwrap();
        let expected = session.tiles().clone();
        let expected_count = session.history().node_count();
        drop(database);
        let database = ProjectDb::open(&path).unwrap();
        let reopened = database.load_reopened().unwrap().unwrap();
        assert_eq!(reopened.current_tiles(), &expected);
        assert_eq!(reopened.history().node_count(), expected_count);
        assert_eq!(database.load_layer_tree().unwrap(), Some(tree(129)));
        assert_eq!(database.load_canvas_spec().unwrap(), page(129));
        drop(database);
        std::fs::remove_file(path).unwrap();
    }

    fn branched_batch(
        parent: &ProjectCommitBatch,
        snapshot_id: SnapshotId,
        history_id: HistoryNodeId,
        seed: u64,
        sequence: u64,
    ) -> ProjectCommitBatch {
        let prepared = prepared_batch_from(
            parent.snapshot_id,
            parent.materialized.after.clone(),
            snapshot_id,
            history_id,
            seed,
            sequence,
        );
        let history_node = HistoryNode {
            parent: Some(parent.history_node.id),
            ..prepared.history_node.clone()
        };
        ProjectCommitBatch::new(
            prepared.snapshot_id,
            prepared.before.clone(),
            prepared.stroke.clone(),
            prepared.materialized.clone(),
            history_node,
        )
        .expect("branch fixture remains a valid materialized transition")
    }

    #[test]
    fn repeated_object_writes_repair_damaged_records_and_preserve_reopened_history() {
        // Product risk: skipping an existing hash must not retain damaged
        // artwork/root bytes or lose a durable history transition.
        for kind in [RecordKind::Tile, RecordKind::ContentRoot] {
            let path = temp("immutable-write-repair");
            let first = prepared_batch();
            let second = branched_batch(&first, SnapshotId(3), HistoryNodeId(4), 84, 20);
            let database = ProjectDb::open(&path).unwrap();
            database.commit(&first).unwrap();
            let transaction = database.db.begin_write().unwrap();
            {
                let (definition, key) = if kind == RecordKind::Tile {
                    (
                        OBJECTS,
                        first.materialized.after.iter().next().unwrap().1.hash(),
                    )
                } else {
                    (ROOTS, first.materialized.after.root().hash)
                };
                transaction
                    .open_table(definition)
                    .unwrap()
                    .insert(key.0.as_slice(), b"damaged-record".as_slice())
                    .unwrap();
            }
            transaction.commit().unwrap();
            // The caller still owns canonical immutable before pixels. As in
            // the original writer, persisting them repairs the altered value.
            database.commit(&second).unwrap();
            drop(database);
            let database = ProjectDb::open(&path).unwrap();
            let reopened = database.load_reopened().unwrap().unwrap();
            assert_eq!(reopened.history().node_count(), 2);
            assert_eq!(
                database.load_current().unwrap().unwrap().materialized.after,
                second.materialized.after
            );
            drop(database);
            println!(
                "immutable-write-scratch kind={kind:?} bytes={}",
                std::fs::metadata(&path).unwrap().len()
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn shared_tile_reads_preserve_artwork_and_recheck_corruption_on_each_load() {
        // Product risk: per-root deduplication must neither merge signed/layer
        // keys nor hide a corrupted shared object behind a previous valid load.
        for fault in ["missing", "checksum", "wrong-content", "wrong-length"] {
            let path = temp(fault);
            let database = ProjectDb::open(&path).unwrap();
            let pixels = vec![32; TILE_BYTE_LEN];
            let tiles = TileSnapshot::from_tiles([
                (
                    TileKey::from_pixel(LayerId(1), 128, -129, -1),
                    pixels.clone(),
                ),
                (TileKey::from_pixel(LayerId(2), 128, 128, 0), pixels),
            ])
            .unwrap();
            let batch = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty())
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, tiles.clone())
                .unwrap();
            database.commit_structural(&batch).unwrap();
            let reopened = database.load_reopened().unwrap().unwrap();
            assert_eq!(reopened.current_tiles(), &tiles);
            let manifest = decode_root_manifest(&encode_root_manifest(&tiles)).unwrap();
            let key = tiles.iter().next().unwrap().1.hash();
            let transaction = database.db.begin_write().unwrap();
            {
                let mut objects = transaction.open_table(OBJECTS).unwrap();
                if fault == "missing" {
                    objects.remove(key.0.as_slice()).unwrap();
                } else {
                    let bytes = match fault {
                        "checksum" => vec![0],
                        "wrong-content" => {
                            Envelope::new(RecordKind::Tile, vec![33; TILE_BYTE_LEN]).encode()
                        }
                        _ => Envelope::new(RecordKind::Tile, vec![32; 4]).encode(),
                    };
                    objects.insert(key.0.as_slice(), bytes.as_slice()).unwrap();
                }
            }
            transaction.commit().unwrap();
            let transaction = database.db.begin_read().unwrap();
            let objects = transaction.open_table(OBJECTS).unwrap();
            assert!(
                snapshot_from_manifest(&objects, manifest).is_err(),
                "{fault}"
            );
            drop(objects);
            drop(transaction);
            drop(database);
            let before_reopen = std::fs::read(&path).unwrap();
            assert!(
                matches!(
                    ProjectDb::open(&path),
                    Err(ProjectOpenError::Corrupt { .. })
                ),
                "{fault}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before_reopen);
            std::fs::remove_file(path).unwrap();
        }
    }

    fn layer_tree_fixture() -> LayerTree {
        let raster = |id, name: &str| {
            LayerTreeNode::Raster(LayerNode {
                alpha_locked: false,
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: LayerId(id),
                name: name.into(),
                visible: true,
                locked: false,
                reference: false,
                opacity_u16: u16::MAX,
                content_root: ContentRootId(id + 100),
            })
        };
        LayerTree::new(GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![
                raster(10, "Background"),
                LayerTreeNode::Group(GroupNode {
                    clip_to_below: false,
                    blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                    id: GroupId(2),
                    name: "Ink".into(),
                    visible: true,
                    opacity_u16: u16::MAX,
                    children: vec![raster(20, "Ink 1"), raster(30, "Ink 2")],
                }),
            ],
        })
        .expect("fixture layer tree")
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn immutable_layer_history_upgrades_atomically_and_restores_branch_metadata() {
        // Product risk: Undo must restore metadata and pixels together, and a
        // failed first metadata commit must not half-upgrade a legacy project.
        let path = temp("layer-history-upgrade");
        let database = ProjectDb::open(&path).expect("scratch project");
        let baseline = layer_tree_fixture();
        database
            .persist_layer_tree(&baseline)
            .expect("legacy hierarchy");
        let first = prepared_batch();
        database.commit(&first).expect("legacy stroke");
        let reopened = database
            .load_reopened()
            .expect("load legacy")
            .expect("legacy history");
        let before = reopened.clone().into_parts().current_cursor;
        let session = HeadlessStrokeSession::from_reopened(reopened);
        let batch = session
            .prepare_structural_change(SnapshotId(3), HistoryNodeId(4), 50, session.tiles().clone())
            .expect("metadata-only history node");
        let mut changed = baseline.clone();
        changed
            .set_reference(LayerId(20), true)
            .expect("reference source");
        changed
            .rename(LayerTreeNodeId::Raster(LayerId(20)), "Renamed")
            .expect("rename");
        changed
            .set_opacity(LayerTreeNodeId::Group(GroupId(2)), 20_000)
            .expect("opacity");
        assert!(
            database
                .commit_structural_inner(
                    &batch,
                    Some(&changed),
                    None,
                    &CommitControl::Failure(CommitFailurePoint::BeforeCommitAbort)
                )
                .is_err()
        );
        assert!(!database.layer_history_enabled().expect("version unchanged"));
        assert_eq!(
            database.load_layer_tree().expect("old tree"),
            Some(baseline.clone())
        );
        assert_eq!(
            database
                .load_reopened()
                .expect("old history")
                .expect("old head")
                .into_parts()
                .current_cursor,
            before
        );
        database
            .commit_structural_with_layer_tree(&batch, &changed)
            .expect("atomic upgrade");
        assert!(database.layer_history_enabled().expect("v2 marker"));
        assert!(
            database.persist_layer_tree(&baseline).is_err(),
            "out-of-band metadata must not bypass history"
        );
        let after = database
            .load_reopened()
            .expect("new history")
            .expect("new head")
            .into_parts()
            .current_cursor;
        // A normal stroke after the upgrade must inherit this snapshot tree.
        let prepared = prepared_batch_from(
            after.snapshot_id,
            batch.after.clone(),
            SnapshotId(4),
            HistoryNodeId(5),
            84,
            20,
        );
        let stroke = ProjectCommitBatch::new(
            prepared.snapshot_id,
            prepared.before.clone(),
            prepared.stroke.clone(),
            prepared.materialized.clone(),
            HistoryNode {
                parent: after.history_head,
                ..prepared.history_node.clone()
            },
        )
        .expect("stroke after metadata");
        database
            .commit(&stroke)
            .expect("inherit current metadata in stroke transaction");
        let stroke_cursor = database
            .load_reopened()
            .expect("stroke reopen")
            .expect("stroke head")
            .into_parts()
            .current_cursor;
        drop(database);
        for (cursor, tree) in [
            (before, &baseline),
            (after, &changed),
            (stroke_cursor, &changed),
            (before, &baseline),
        ] {
            let database = ProjectDb::open(&path).expect("validated reopen");
            assert_eq!(
                database
                    .load_cursor_layer_tree(cursor)
                    .expect("target tree")
                    .as_ref(),
                Some(tree)
            );
            database
                .persist_history_cursor(cursor)
                .expect("atomic cursor and tree move");
            drop(database);
            let reopened = ProjectDb::open(&path).expect("reopen selected branch");
            assert_eq!(
                reopened.load_layer_tree().expect("restored tree").as_ref(),
                Some(tree)
            );
            assert_eq!(
                reopened
                    .load_reopened()
                    .expect("restored history")
                    .expect("head")
                    .history()
                    .node_count(),
                3
            );
        }
        std::fs::remove_file(path).expect("remove scratch project");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn signed_selection_reopen_preserves_offpage_pixels_and_reader_gate() {
        // Risk: an origin rewrite can paint a different part of the artwork;
        // later metadata upgrades must not let old writers strip that origin.
        use nyatidraw_stroke::{CpuReplayMaterializer, StrokeMaterializer, StrokeSelection};
        let template = prepared_batch();
        let dimensions = [16, 16];
        let packed = vec![255; 32];
        let prepare = |origin| {
            HeadlessStrokeSession::new(template.stroke.parent_snapshot, template.before.clone())
                .prepare_round_stroke_with_selection(
                    template.snapshot_id,
                    template.history_node.id,
                    template.history_node.timestamp_ns,
                    template.stroke.layer,
                    template.stroke.brush,
                    template.stroke.recorded.clone(),
                    template.stroke.color,
                    template.stroke.samples().to_vec(),
                    Some(std::sync::Arc::new(
                        StrokeSelection::from_packed_bits_at(origin, dimensions, &packed).unwrap(),
                    )),
                )
                .unwrap()
        };
        let legacy = prepare([0, 0]);
        let batch = prepare([-16, -4]);
        let bare = encode_stroke_commit(&template.stroke);
        let legacy_wire = encode_stroke_commit(&legacy.stroke);
        let mut expected_legacy = bare.clone();
        expected_legacy[..32].copy_from_slice(&(legacy.stroke.id.0).0);
        expected_legacy.extend_from_slice(b"NYSEL001");
        expected_legacy.extend_from_slice(&16_u32.to_le_bytes());
        expected_legacy.extend_from_slice(&16_u32.to_le_bytes());
        expected_legacy.extend_from_slice(&packed);
        assert_eq!(
            legacy_wire, expected_legacy,
            "origin zero retains exact v1 layout"
        );
        assert_ne!(legacy.stroke.id, batch.stroke.id);
        assert_eq!(
            (legacy.stroke.id.0).0,
            [
                205, 97, 5, 39, 97, 29, 183, 168, 5, 193, 42, 145, 132, 113, 118, 120, 111, 228,
                83, 244, 241, 50, 238, 204, 231, 127, 204, 57, 90, 13, 222, 104
            ],
            "legacy selected hash fixture"
        );
        let wire = encode_stroke_commit(&batch.stroke);
        assert_eq!(&wire[bare.len()..bare.len() + 8], b"NYSEL002");
        assert_eq!(
            decode_stroke_commit(&wire, &template.before).unwrap(),
            batch.stroke
        );
        for mutation in 0..3 {
            let mut bad = wire.clone();
            match mutation {
                0 => bad[bare.len() + 8] ^= 1, // Valid mask translated without resealing.
                1 => bad[bare.len() + 8..bare.len() + 16].fill(0), // Noncanonical signed zero.
                _ => {
                    bad.pop();
                } // Truncated mask.
            }
            assert!(decode_stroke_commit(&bad, &template.before).is_err());
        }
        assert!(!batch.materialized.changed_tiles.is_empty());
        for (key, tile) in batch.materialized.after.iter() {
            let (ox, oy) = key.pixel_origin();
            for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
                let x = ox + i64::try_from(index % 128).unwrap();
                let y = oy + i64::try_from(index / 128).unwrap();
                if !(-16..0).contains(&x) || !(-4..12).contains(&y) {
                    assert_eq!(
                        pixel, [0; 4],
                        "selected replay must not leak outside signed mask"
                    );
                }
            }
        }
        if let Some(path) = std::env::var_os("NYATIDRAW_SIGNED_SELECTION_REOPEN") {
            let db = ProjectDb::open(&PathBuf::from(path)).unwrap();
            let loaded = db.load_current().unwrap().unwrap();
            assert_eq!(loaded.stroke, batch.stroke);
            assert_eq!(loaded.materialized.after, batch.materialized.after);
            assert_eq!(
                CpuReplayMaterializer
                    .materialize(&loaded.stroke, &loaded.before)
                    .unwrap()
                    .after,
                batch.materialized.after
            );
            return;
        }
        let path = temp("signed-selection");
        let db = ProjectDb::open(&path).unwrap();
        assert!(
            db.commit_with_failure(&batch, CommitFailurePoint::BeforeCommitAbort)
                .is_err()
        );
        assert!(db.load_current().unwrap().is_none());
        db.commit(&batch).unwrap();
        drop(db);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::signed_selection_reopen_preserves_offpage_pixels_and_reader_gate",
            ])
            .env("NYATIDRAW_SIGNED_SELECTION_REOPEN", &path)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "fresh process must recover signed pixels and semantic replay"
        );
        let db = ProjectDb::open(&path).unwrap();
        for id in 3..=4 {
            let session =
                HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
            let change = session
                .prepare_structural_change(
                    SnapshotId(id),
                    HistoryNodeId(id + 1),
                    13_000,
                    session.tiles().clone(),
                )
                .unwrap();
            if id == 3 {
                db.commit_structural_with_layer_tree(&change, &layer_tree_fixture())
                    .unwrap();
            } else {
                db.commit_structural_with_canvas(&change, CanvasSpec::DEFAULT)
                    .unwrap();
            }
        }
        let tx = db.db.begin_read().unwrap();
        let marker = tx
            .open_table(META)
            .unwrap()
            .get("schema_version")
            .unwrap()
            .unwrap()
            .value();
        assert_eq!(
            marker,
            CANVAS_HISTORY_SCHEMA_VERSION
                | COMPRESSED_TILE_SCHEMA_FLAG
                | SIGNED_SELECTION_SCHEMA_FLAG
                | LAYER_COMPOSITING_SCHEMA_FLAG
        );
        assert!(
            !(1..=4).contains(
                &(marker & !(COMPRESSED_TILE_SCHEMA_FLAG | CONFIGURABLE_BRUSH_SCHEMA_FLAG))
            ),
            "previous reader must reject signed redo branches"
        );
        drop(tx);
        let session = HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        // Restore the stroke cursor before forging its marker; no file rewrite
        // can silently accept the signed record as a legacy selection.
        let target = ProjectHistoryCursor {
            snapshot_id: batch.snapshot_id,
            history_head: Some(batch.history_node.id),
            root: batch.materialized.after.root(),
        };
        drop(session);
        db.persist_history_cursor(target).unwrap();
        let tx = db.db.begin_write().unwrap();
        {
            tx.open_table(META)
                .unwrap()
                .insert("schema_version", marker & !SIGNED_SELECTION_SCHEMA_FLAG)
                .unwrap();
        }
        tx.commit().unwrap();
        assert!(
            db.load_current().is_err(),
            "missing signed reader gate is corruption"
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn selected_stroke_recovery_preserves_coverage_identity_and_atomic_format_upgrade() {
        use nyatidraw_stroke::{StrokeCommit, StrokeSelection};
        let path = temp("selected-stroke-upgrade");
        let template = prepared_batch();
        let old_wire = encode_stroke_commit(&template.stroke);
        assert_eq!(
            decode_stroke_commit(&old_wire, &template.before).unwrap(),
            template.stroke,
            "legacy unselected records retain their exact identity"
        );
        let mut packed = vec![255; (129_usize * 41).div_ceil(8)];
        *packed.last_mut().unwrap() = 1;
        let selection =
            std::sync::Arc::new(StrokeSelection::from_packed_bits(129, 41, &packed).unwrap());
        let stroke: &StrokeCommit = &template.stroke;
        let batch = HeadlessStrokeSession::new(stroke.parent_snapshot, template.before.clone())
            .prepare_round_stroke_with_selection(
                template.snapshot_id,
                template.history_node.id,
                template.history_node.timestamp_ns,
                stroke.layer,
                stroke.brush,
                stroke.recorded.clone(),
                stroke.color,
                stroke.samples().to_vec(),
                Some(selection),
            )
            .unwrap();
        let encoded = encode_stroke_commit(&batch.stroke);
        assert_eq!(
            decode_stroke_commit(&encoded, &template.before).unwrap(),
            batch.stroke
        );
        for mutation in 0..5 {
            let mut corrupt = encoded.clone();
            match mutation {
                0 => {
                    corrupt[old_wire.len()] ^= 1;
                } // Unknown extension.
                1 => {
                    *corrupt.last_mut().unwrap() |= 128;
                } // Noncanonical tail.
                2 => {
                    corrupt[old_wire.len() + 8..old_wire.len() + 12]
                        .copy_from_slice(&u32::MAX.to_le_bytes());
                }
                3 => {
                    corrupt.pop();
                } // Truncated coverage.
                _ => {
                    corrupt[old_wire.len() + 16] ^= 1;
                } // Valid mask, wrong sealed ID.
            }
            assert!(
                decode_stroke_commit(&corrupt, &template.before).is_err(),
                "mutation {mutation}"
            );
        }
        let database = ProjectDb::open(&path).unwrap();
        assert!(
            database
                .commit_with_failure(&batch, CommitFailurePoint::BeforeCommitAbort)
                .is_err()
        );
        assert!(
            !database.layer_history_enabled().unwrap(),
            "abort must not half-upgrade the format"
        );
        assert!(database.load_reopened().unwrap().is_none());
        database.commit(&batch).unwrap();
        drop(database);
        let database = ProjectDb::open(&path).unwrap();
        assert!(database.layer_history_enabled().unwrap());
        assert_eq!(
            database.load_current().unwrap().unwrap().stroke,
            batch.stroke
        );
        let reopened = database.load_reopened().unwrap().unwrap();
        assert_eq!(reopened.current_tiles(), &batch.materialized.after);
        let session = HeadlessStrokeSession::from_reopened(reopened);
        database
            .persist_history_cursor(session.prepare_undo_cursor().unwrap().target())
            .unwrap();
        let transaction = database.db.begin_read().unwrap();
        assert_eq!(
            transaction
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value(),
            SELECTION_STROKE_SCHEMA_VERSION | COMPRESSED_TILE_SCHEMA_FLAG,
            "Undo must not downgrade a project with selected redo branches"
        );
        drop(transaction);
        drop(database);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn broken_layer_history_records_reject_reopen_without_overwriting_artwork() {
        // Product risk: missing/historically corrupt metadata or a split
        // current-tree/cursor state must never reopen as apparently valid art.
        for fault in ["missing", "historical-checksum", "current-mismatch"] {
            let path = temp(fault);
            let database = ProjectDb::open(&path).expect("scratch project");
            let baseline = layer_tree_fixture();
            database.persist_layer_tree(&baseline).expect("legacy tree");
            let first = prepared_batch();
            database.commit(&first).expect("legacy artwork");
            let session = HeadlessStrokeSession::from_reopened(
                database.load_reopened().expect("load").expect("head"),
            );
            let batch = session
                .prepare_structural_change(
                    SnapshotId(3),
                    HistoryNodeId(4),
                    50,
                    session.tiles().clone(),
                )
                .expect("metadata history");
            let mut changed = baseline.clone();
            changed
                .rename(LayerTreeNodeId::Raster(LayerId(20)), "Changed")
                .expect("rename");
            database
                .commit_structural_with_layer_tree(&batch, &changed)
                .expect("upgrade");
            let transaction = database
                .db
                .begin_write()
                .expect("inject scratch corruption");
            if fault == "current-mismatch" {
                let bytes = Envelope::new(
                    RecordKind::LayerTree,
                    encode_layer_tree(&baseline).expect("encode"),
                )
                .encode();
                transaction
                    .open_table(STATE)
                    .expect("state")
                    .insert(CURRENT_LAYER_TREE, bytes.as_slice())
                    .expect("split metadata");
            } else {
                let mut table = transaction
                    .open_table(super::layer_history::SNAPSHOT_LAYERS)
                    .expect("layers");
                if fault == "missing" {
                    table
                        .remove(3_u128.to_le_bytes().as_slice())
                        .expect("missing snapshot metadata");
                } else {
                    let mut bytes = Envelope::new(
                        RecordKind::LayerTree,
                        encode_layer_tree(&baseline).expect("encode"),
                    )
                    .encode();
                    *bytes.last_mut().expect("payload") ^= 1;
                    table
                        .insert(2_u128.to_le_bytes().as_slice(), bytes.as_slice())
                        .expect("corrupt noncurrent branch");
                }
            }
            transaction.commit().expect("publish corruption");
            drop(database);
            let bytes = std::fs::read(&path).expect("before open");
            assert!(
                matches!(
                    ProjectDb::open(&path),
                    Err(ProjectOpenError::Corrupt { .. })
                ),
                "{fault}"
            );
            assert_eq!(
                std::fs::read(&path).expect("after rejection"),
                bytes,
                "{fault}: preserve invalid original"
            );
            std::fs::remove_file(path).expect("remove scratch project");
        }
    }

    #[test]
    fn layer_metadata_mutations_survive_close_and_reopen_exactly() {
        // Product risk: visibility, opacity, or compositing order appearing
        // saved but reverting after restart changes the artwork presentation.
        let path = temp("layer-tree-reopen");
        let _ = std::fs::remove_file(&path);
        let root = GroupId(1);
        let ink = GroupId(2);
        let mut expected = layer_tree_fixture();
        expected
            .set_visibility(LayerTreeNodeId::Group(ink), false)
            .expect("hide ink group");
        expected
            .set_opacity(LayerTreeNodeId::Raster(LayerId(30)), 31_337)
            .expect("change raster opacity");
        expected
            .reorder(LayerTreeNodeId::Raster(LayerId(20)), root, 1)
            .expect("move raster into root ordering");

        let database = ProjectDb::open(&path).expect("initialize scratch project");
        database
            .persist_layer_tree(&expected)
            .expect("durably publish layer tree");
        drop(database);

        let database = ProjectDb::open(&path).expect("reopen layer project");
        assert_eq!(
            database
                .load_layer_tree()
                .expect("decode durable layer tree"),
            Some(expected)
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn page_history_upgrade_and_cursor_moves_never_split_dimensions_from_artwork() {
        let path = temp("page-history-atomic");
        let database = ProjectDb::open(&path).unwrap();
        let baseline = CanvasSpec {
            width_px: 257,
            height_px: 65,
            pixels_per_inch: 96,
        };
        let resized = CanvasSpec {
            width_px: 129,
            height_px: 41,
            pixels_per_inch: 144,
        };
        let branched = CanvasSpec {
            width_px: 65,
            height_px: 129,
            pixels_per_inch: 300,
        };
        database.persist_canvas_spec(baseline).unwrap();
        let template = prepared_batch();
        database.commit(&template).unwrap();
        let old = database.load_reopened().unwrap().unwrap();
        let old_cursor = old.clone().into_parts().current_cursor;
        let initial = database.load_initial_cursor().unwrap().unwrap();
        let mut session = HeadlessStrokeSession::from_reopened(old);
        let page = session
            .prepare_structural_change(SnapshotId(3), HistoryNodeId(4), 50, session.tiles().clone())
            .unwrap();
        assert!(
            database
                .commit_structural_inner(
                    &page,
                    None,
                    Some(resized),
                    &CommitControl::Failure(CommitFailurePoint::BeforeCommitAbort)
                )
                .is_err()
        );
        assert!(!database.canvas_history_enabled().unwrap());
        assert!(!database.layer_history_enabled().unwrap());
        assert_eq!(database.load_canvas_spec().unwrap(), baseline);
        assert_eq!(
            database
                .load_reopened()
                .unwrap()
                .unwrap()
                .into_parts()
                .current_cursor,
            old_cursor
        );
        // Ambiguous acknowledgement must still leave a complete durable commit.
        assert!(
            database
                .commit_structural_inner(
                    &page,
                    None,
                    Some(resized),
                    &CommitControl::Failure(CommitFailurePoint::AfterCommitReturn)
                )
                .is_err()
        );
        database.validate_canvas_history().unwrap();
        session.accept_structural_change(&page).unwrap();
        let page_cursor = database
            .load_reopened()
            .unwrap()
            .unwrap()
            .into_parts()
            .current_cursor;
        assert!(
            database.persist_canvas_spec(baseline).is_err(),
            "no out-of-band page writes after upgrade"
        );
        let mut packed = vec![255; (129_usize * 41).div_ceil(8)];
        *packed.last_mut().unwrap() = 1;
        let mask = std::sync::Arc::new(
            nyatidraw_stroke::StrokeSelection::from_packed_bits(129, 41, &packed).unwrap(),
        );
        let stroke = &template.stroke;
        let selected = session
            .prepare_round_stroke_with_selection(
                SnapshotId(4),
                HistoryNodeId(5),
                60,
                stroke.layer,
                stroke.brush,
                stroke.recorded.clone(),
                stroke.color,
                stroke.samples().to_vec(),
                Some(mask),
            )
            .unwrap();
        database.commit(&selected).unwrap();
        assert!(
            database.canvas_history_enabled().unwrap(),
            "selected strokes cannot downgrade v4 to v3"
        );
        let selected_cursor = database
            .load_reopened()
            .unwrap()
            .unwrap()
            .into_parts()
            .current_cursor;
        database.persist_history_cursor(old_cursor).unwrap();
        let branch_session =
            HeadlessStrokeSession::from_reopened(database.load_reopened().unwrap().unwrap());
        let branch = branch_session
            .prepare_structural_change(
                SnapshotId(5),
                HistoryNodeId(6),
                70,
                branch_session.tiles().clone(),
            )
            .unwrap();
        database
            .commit_structural_with_canvas(&branch, branched)
            .unwrap();
        let branch_cursor = database
            .load_reopened()
            .unwrap()
            .unwrap()
            .into_parts()
            .current_cursor;
        drop(database);
        for (cursor, canvas, tiles) in [
            (old_cursor, baseline, &template.materialized.after),
            (page_cursor, resized, &page.after),
            (selected_cursor, resized, &selected.materialized.after),
            (branch_cursor, branched, &branch.after),
            (initial, baseline, &template.before),
        ] {
            let database = ProjectDb::open(&path).unwrap();
            assert_eq!(database.load_cursor_canvas_spec(cursor).unwrap(), canvas);
            database.persist_history_cursor(cursor).unwrap();
            drop(database);
            let reopened = ProjectDb::open(&path).unwrap();
            assert_eq!(reopened.load_canvas_spec().unwrap(), canvas);
            let artwork = reopened.load_reopened().unwrap().unwrap();
            assert_eq!(artwork.current_tiles(), tiles);
            assert_eq!(artwork.history().node_count(), 4);
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn corrupt_page_history_is_rejected_without_rewriting_the_original_project() {
        for fault in [
            "missing",
            "checksum",
            "orphan",
            "zero",
            "current",
            "downgrade",
            "unmarked-current",
        ] {
            let path = temp(&format!("page-history-{fault}"));
            let database = ProjectDb::open(&path).unwrap();
            let template = prepared_batch();
            database.commit(&template).unwrap();
            let session =
                HeadlessStrokeSession::from_reopened(database.load_reopened().unwrap().unwrap());
            let batch = session
                .prepare_structural_change(
                    SnapshotId(3),
                    HistoryNodeId(4),
                    50,
                    session.tiles().clone(),
                )
                .unwrap();
            database
                .commit_structural_with_canvas(&batch, CanvasSpec::DEFAULT)
                .unwrap();
            database.validate_canvas_history().unwrap();
            let transaction = database.db.begin_write().unwrap();
            if fault == "unmarked-current" {
                transaction
                    .open_table(META)
                    .unwrap()
                    .remove(CANVAS_METADATA_VERSION_KEY)
                    .unwrap();
            } else if fault == "current" || fault == "downgrade" {
                let mut metadata = transaction.open_table(META).unwrap();
                let (key, value) = if fault == "current" {
                    (CANVAS_WIDTH_KEY, 99)
                } else {
                    ("schema_version", SELECTION_STROKE_SCHEMA_VERSION)
                };
                metadata.insert(key, value).unwrap();
            } else {
                let mut pages = transaction
                    .open_table(super::canvas_history::SNAPSHOT_CANVAS)
                    .unwrap();
                let key = 2_u128.to_le_bytes(); // A non-current snapshot must also be validated.
                let mut bytes = pages.get(key.as_slice()).unwrap().unwrap().value().to_vec();
                match fault {
                    "missing" => {
                        pages.remove(key.as_slice()).unwrap();
                    }
                    "checksum" => {
                        *bytes.last_mut().unwrap() ^= 1;
                        pages.insert(key.as_slice(), bytes.as_slice()).unwrap();
                    }
                    "orphan" => {
                        pages
                            .insert(99_u128.to_le_bytes().as_slice(), bytes.as_slice())
                            .unwrap();
                    }
                    "zero" => {
                        let invalid = Envelope::new(RecordKind::CanvasSpec, vec![0; 12]).encode();
                        pages.insert(key.as_slice(), invalid.as_slice()).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            transaction.commit().unwrap();
            drop(database);
            let original = std::fs::read(&path).unwrap();
            assert!(
                matches!(
                    ProjectDb::open(&path),
                    Err(ProjectOpenError::Corrupt { .. })
                ),
                "{fault}"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                original,
                "{fault}: invalid file preservation"
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn canvas_metadata_reopens_and_legacy_projects_default_without_rewrite() {
        // Product risk: export must retain an explicitly chosen finite extent,
        // while older artwork must not be rewritten merely to obtain its
        // compatibility canvas size.
        let path = temp("canvas-metadata-migration");
        let _ = std::fs::remove_file(&path);
        let expected = CanvasSpec {
            width_px: 1_920,
            height_px: 1_080,
            pixels_per_inch: 144,
        };
        let database = ProjectDb::open(&path).expect("initialize canvas project");
        database
            .persist_canvas_spec(expected)
            .expect("persist explicit canvas");
        drop(database);
        assert_eq!(
            ProjectDb::open(&path)
                .expect("reopen explicit canvas")
                .load_canvas_spec()
                .expect("load explicit canvas"),
            expected
        );

        let database = Database::open(&path).expect("open legacy migration fixture");
        let mut transaction = database
            .begin_write()
            .expect("begin legacy migration mutation");
        transaction.set_durability(Durability::Immediate).unwrap();
        {
            let mut metadata = transaction.open_table(META).expect("open metadata");
            for key in [
                CANVAS_METADATA_VERSION_KEY,
                CANVAS_WIDTH_KEY,
                CANVAS_HEIGHT_KEY,
                CANVAS_PPI_KEY,
            ] {
                metadata.remove(key).expect("remove new canvas metadata");
            }
        }
        transaction
            .commit()
            .expect("commit legacy migration fixture");
        drop(database);

        let metadata = |path: &Path| {
            let db = redb::ReadOnlyDatabase::open(path).unwrap();
            let tx = db.begin_read().unwrap();
            tx.open_table(META)
                .unwrap()
                .iter()
                .unwrap()
                .map(|entry| {
                    let (key, value) = entry.unwrap();
                    (key.value().to_owned(), value.value())
                })
                .collect::<Vec<_>>()
        };
        let before = metadata(&path);
        let database = ProjectDb::open(&path).expect("open legacy project");
        assert_eq!(
            database.load_canvas_spec().expect("load legacy default"),
            CanvasSpec::DEFAULT
        );
        drop(database);
        assert_eq!(metadata(&path), before);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn white_background_structural_commit_preserves_existing_ink_after_reopen() {
        // Product risk: adding a background after drawing must not replace
        // existing ink or sever the durable undo lineage at restart.
        let path = temp("white-background-after-ink");
        let _ = std::fs::remove_file(&path);
        let ink = prepared_batch();
        let white_key = TileKey {
            layer: LayerId(2),
            mip: 0,
            x: 0,
            y: 0,
        };
        let after = ink
            .materialized
            .after
            .with_replacements([(white_key, vec![u8::MAX; TILE_BYTE_LEN])])
            .expect("append exact opaque background tile");
        let white = ProjectStructuralBatch::new(
            SnapshotId(3),
            ink.materialized.after.clone(),
            after.clone(),
            HistoryNode {
                id: HistoryNodeId(4),
                parent: Some(ink.history_node.id),
                timestamp_ns: 13_000,
                operation: OperationRecord::StructuralChange,
                before_root: ink.materialized.after.root().id,
                after_root: after.root().id,
            },
        )
        .expect("structural background batch joins ink root");

        let database = ProjectDb::open(&path).expect("initialize scratch project");
        database.commit(&ink).expect("durably commit ink");
        database
            .commit_structural(&white)
            .expect("durably commit white background after ink");
        drop(database);

        let database = ProjectDb::open(&path).expect("reopen combined artwork");
        let reopened = database
            .load_reopened()
            .expect("decode combined artwork")
            .expect("combined durable state exists");
        assert_eq!(reopened.current_snapshot(), SnapshotId(3));
        assert_eq!(reopened.current_tiles(), &after);
        assert_eq!(
            reopened
                .current_tiles()
                .get(white_key)
                .expect("white background tile retained")
                .pixels(),
            vec![u8::MAX; TILE_BYTE_LEN]
        );
        assert_eq!(reopened.history().node_count(), 2);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn corrupt_layer_tree_records_are_rejected_without_rewriting_the_project() {
        // Product risk: opening malformed structural metadata must neither
        // accept a changed composition nor repair user artwork in place.
        let valid_payload = encode_layer_tree(&layer_tree_fixture()).expect("encode fixture");
        for (name, record) in [
            ("checksum", {
                let mut bytes =
                    Envelope::new(RecordKind::LayerTree, valid_payload.clone()).encode();
                *bytes.last_mut().expect("non-empty envelope") ^= 0x80;
                bytes
            }),
            ("unknown-node-tag", {
                let mut payload = valid_payload.clone();
                payload[2] = 0xff;
                Envelope::new(RecordKind::LayerTree, payload).encode()
            }),
            ("trailing-payload", {
                let mut payload = valid_payload.clone();
                payload.push(0xa5);
                Envelope::new(RecordKind::LayerTree, payload).encode()
            }),
        ] {
            let path = temp(name);
            let _ = std::fs::remove_file(&path);
            let database = ProjectDb::open(&path).expect("initialize layer corruption fixture");
            database
                .persist_layer_tree(&layer_tree_fixture())
                .expect("publish valid layer tree first");
            drop(database);

            let database = Database::open(&path).expect("open raw corruption fixture");
            let mut transaction = database.begin_write().expect("begin raw mutation");
            transaction.set_durability(Durability::Immediate).unwrap();
            {
                let mut state = transaction.open_table(STATE).expect("state table");
                state
                    .insert(CURRENT_LAYER_TREE, record.as_slice())
                    .expect("replace layer record");
            }
            transaction.commit().expect("commit corrupt fixture");
            drop(database);

            let before = std::fs::read(&path).expect("fingerprint bytes before rejected open");
            assert!(matches!(
                ProjectDb::open(&path),
                Err(ProjectOpenError::Corrupt { message, .. })
                    if message.contains("layer tree corrupt")
            ));
            let after = std::fs::read(&path).expect("fingerprint bytes after rejected open");
            assert_eq!(after, before, "{name}: rejected open rewrote the project");
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn composition_metadata_reader_gate_is_atomic_and_survives_fresh_reopen() {
        // Product risk: an older writer must not discard shading semantics,
        // and failed metadata publication must not advance either head or gate.
        let changed_tree = || {
            let mut tree = layer_tree_fixture();
            tree.set_alpha_locked(LayerId(20), true).unwrap();
            tree.set_clip_to_below(LayerTreeNodeId::Raster(LayerId(20)), true)
                .unwrap();
            tree.set_blend_mode(
                LayerTreeNodeId::Raster(LayerId(20)),
                nyatidraw_api::LayerBlendMode::Multiply,
            )
            .unwrap();
            tree.set_clip_to_below(LayerTreeNodeId::Group(GroupId(2)), true)
                .unwrap();
            tree.set_blend_mode(
                LayerTreeNodeId::Group(GroupId(2)),
                nyatidraw_api::LayerBlendMode::Multiply,
            )
            .unwrap();
            tree
        };
        if let Some(path) = std::env::var_os("NYATIDRAW_COMPOSITION_METADATA_REOPEN") {
            let db = ProjectDb::open(Path::new(&path)).unwrap();
            assert_eq!(db.load_layer_tree().unwrap(), Some(changed_tree()));
            let reopened = db.load_reopened().unwrap().unwrap();
            assert_eq!(reopened.current_snapshot(), SnapshotId(3));
            assert_eq!(
                reopened.current_tiles(),
                &prepared_batch().materialized.after
            );
            return;
        }
        let path = temp("composition-metadata");
        let db = ProjectDb::open(&path).unwrap();
        let first = prepared_batch();
        db.commit(&first).unwrap();
        let session = HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        let batch = session
            .prepare_structural_change(SnapshotId(3), HistoryNodeId(4), 50, session.tiles().clone())
            .unwrap();
        let tree = changed_tree();
        let marker = |db: &ProjectDb| {
            let transaction = db.db.begin_read().unwrap();
            transaction
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value()
        };
        let before_marker = marker(&db);
        assert_eq!(before_marker & LAYER_COMPOSITING_SCHEMA_FLAG, 0);
        assert!(
            db.commit_structural_inner(
                &batch,
                Some(&tree),
                None,
                &CommitControl::Failure(CommitFailurePoint::BeforeCommitAbort)
            )
            .is_err()
        );
        assert_eq!(marker(&db), before_marker);
        assert!(db.load_layer_tree().unwrap().is_none());
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_snapshot(),
            first.snapshot_id
        );
        db.commit_structural_with_layer_tree(&batch, &tree).unwrap();
        let upgraded = marker(&db);
        assert_ne!(upgraded & LAYER_COMPOSITING_SCHEMA_FLAG, 0);
        let old_known = COMPRESSED_TILE_SCHEMA_FLAG
            | CONFIGURABLE_BRUSH_SCHEMA_FLAG
            | SIGNED_SELECTION_SCHEMA_FLAG;
        assert!(
            !(1..=4).contains(&(upgraded & !old_known)),
            "older reader rejects before editing"
        );
        drop(db);
        assert!(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::composition_metadata_reader_gate_is_atomic_and_survives_fresh_reopen"
                ])
                .env("NYATIDRAW_COMPOSITION_METADATA_REOPEN", &path)
                .status()
                .unwrap()
                .success()
        );
        let db = ProjectDb::open(&path).unwrap();
        let session = HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        let undo = session.prepare_undo_cursor().unwrap();
        db.persist_history_cursor(undo.target()).unwrap();
        assert!(db.load_layer_tree().unwrap().is_none());
        assert_eq!(
            marker(&db),
            upgraded,
            "Undo cannot downgrade retained redo records"
        );
        // Forge a missing flag while current state is legacy. Historical v3
        // metadata must still reject read-only, not only the current tree.
        let transaction = db.db.begin_write().unwrap();
        transaction
            .open_table(META)
            .unwrap()
            .insert("schema_version", upgraded & !LAYER_COMPOSITING_SCHEMA_FLAG)
            .unwrap();
        transaction.commit().unwrap();
        drop(db);
        let original = std::fs::read(&path).unwrap();
        assert!(matches!(
            ProjectDb::open(&path),
            Err(ProjectOpenError::Corrupt { .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cursor_only_undo_and_redo_survive_reopen_without_losing_the_branch() {
        // Product risk: a cursor-only undo/redo must survive a process restart
        // without rewriting artwork or deleting the redo child.
        let path = temp("cursor-reopen");
        let _ = std::fs::remove_file(&path);
        let first = prepared_batch();
        let branch_a = branched_batch(&first, SnapshotId(3), HistoryNodeId(4), 84, 20);
        let branch_b = branched_batch(&branch_a, SnapshotId(4), HistoryNodeId(5), 126, 30);
        let mut before_close = History::new(first.before.root().id);
        before_close
            .append(first.history_node.clone())
            .expect("accept shared parent");
        before_close
            .append(branch_a.history_node.clone())
            .expect("accept branch A");
        before_close
            .append(branch_b.history_node.clone())
            .expect("accept branch B");
        assert_eq!(
            before_close.undo(),
            Ok(branch_a.materialized.after.root().id)
        );
        assert_eq!(
            before_close.current_root(),
            branch_a.materialized.after.root().id
        );

        let database = ProjectDb::open(&path).expect("initialize scratch project");
        database.commit(&first).expect("save shared parent");
        database.commit(&branch_a).expect("save branch A");
        database.commit(&branch_b).expect("save branch B");
        let reopened = database
            .load_reopened()
            .expect("reconstruct complete durable history")
            .expect("durable cursor exists");
        let mut session = HeadlessStrokeSession::from_reopened(reopened);
        let undo = session.prepare_undo_cursor().expect("prepare durable undo");
        let undo_tiles = database
            .load_cursor_tiles(undo.target())
            .expect("hydrate branch A tiles");
        database
            .persist_history_cursor(undo.target())
            .expect("durably publish branch A cursor");
        session
            .accept_history_cursor_move(undo, undo_tiles)
            .expect("accept durable undo");
        drop(session);
        drop(database);

        let reopened = ProjectDb::open(&path)
            .expect("reopen after durable undo")
            .load_reopened()
            .expect("reconstruct durable undo history")
            .expect("durable cursor exists");
        assert_eq!(reopened.current_snapshot(), branch_a.snapshot_id);
        assert_eq!(
            reopened.current_tiles().root(),
            branch_a.materialized.after.root(),
            "reopened cursor retains durable undo root A"
        );
        assert_eq!(reopened.history().node_count(), 3);
        assert_eq!(
            reopened.history().redo_candidates(),
            vec![branch_b.history_node.id]
        );
        let database = ProjectDb::open(&path).expect("open for durable redo");
        let mut session = HeadlessStrokeSession::from_reopened(reopened);
        let redo = session
            .prepare_redo_to_cursor(branch_b.history_node.id)
            .expect("prepare explicit durable redo");
        let redo_tiles = database
            .load_cursor_tiles(redo.target())
            .expect("hydrate branch B tiles");
        database
            .persist_history_cursor(redo.target())
            .expect("durably publish branch B cursor");
        session
            .accept_history_cursor_move(redo, redo_tiles)
            .expect("accept durable redo");
        drop(session);
        drop(database);

        let reopened = ProjectDb::open(&path)
            .expect("reopen after durable redo")
            .load_reopened()
            .expect("reconstruct durable redo history")
            .expect("durable cursor exists");
        assert_eq!(reopened.current_snapshot(), branch_b.snapshot_id);
        assert_eq!(
            reopened.current_tiles().root(),
            branch_b.materialized.after.root()
        );
        let database = ProjectDb::open(&path).expect("open for durable undo to initial");
        let mut session = HeadlessStrokeSession::from_reopened(reopened);
        for expected_root in [
            branch_a.materialized.after.root(),
            first.materialized.after.root(),
            first.before.root(),
        ] {
            let undo = session.prepare_undo_cursor().expect("prepare durable undo");
            let tiles = database
                .load_cursor_tiles(undo.target())
                .expect("hydrate undo target tiles");
            database
                .persist_history_cursor(undo.target())
                .expect("durably publish undo target");
            session
                .accept_history_cursor_move(undo, tiles)
                .expect("accept durable undo target");
            assert_eq!(session.tiles().root(), expected_root);
        }
        drop(session);
        drop(database);

        let reopened = ProjectDb::open(&path)
            .expect("reopen at durable initial root")
            .load_reopened()
            .expect("reconstruct initial cursor")
            .expect("initial cursor exists");
        assert!(reopened.current().is_none());
        assert_eq!(reopened.current_snapshot(), first.stroke.parent_snapshot);
        assert_eq!(reopened.current_tiles().root(), first.before.root());
        assert_eq!(reopened.history().head(), None);
        assert_eq!(
            reopened.history().redo_candidates(),
            vec![first.history_node.id]
        );

        let database = ProjectDb::open(&path).expect("open for initial redo");
        let mut session = HeadlessStrokeSession::from_reopened(reopened);
        let redo = session
            .prepare_redo_to_cursor(first.history_node.id)
            .expect("prepare first-stroke redo");
        let tiles = database
            .load_cursor_tiles(redo.target())
            .expect("hydrate first-stroke tiles");
        database
            .persist_history_cursor(redo.target())
            .expect("durably publish first-stroke redo");
        session
            .accept_history_cursor_move(redo, tiles)
            .expect("accept first-stroke redo");
        drop(session);
        drop(database);

        let reopened = ProjectDb::open(&path)
            .expect("reopen after initial redo")
            .load_reopened()
            .expect("reconstruct first-stroke cursor")
            .expect("first-stroke cursor exists");
        assert_eq!(reopened.current_snapshot(), first.snapshot_id);
        assert_eq!(
            reopened.current_tiles().root(),
            first.materialized.after.root()
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn recovery_matrix_preserves_durable_heads_and_scopes_corruption() {
        let path = temp("commit-recovery");
        let _ = std::fs::remove_file(&path);
        let first = prepared_batch();
        let second = branched_batch(&first, SnapshotId(3), HistoryNodeId(4), 84, 20);
        let invalid_second_root = prepared_batch_from(
            first.snapshot_id,
            first.materialized.after.clone(),
            SnapshotId(5),
            HistoryNodeId(6),
            126,
            30,
        );
        let database = ProjectDb::open(&path).expect("initialize scratch project");
        database.commit(&first).expect("first durable commit");
        assert!(matches!(
            database.commit(&invalid_second_root),
            Err(ProjectOpenError::Corrupt { message, .. }) if message.contains("commit lineage mismatch")
        ));
        assert!(
            database
                .commit_with_failure(&second, CommitFailurePoint::BeforeCommitAbort)
                .is_err()
        );
        drop(database);
        assert_eq!(
            ProjectDb::open(&path)
                .expect("reopen after abort")
                .load_current()
                .expect("load aborted head")
                .expect("first head exists")
                .snapshot_id,
            first.snapshot_id
        );

        let database = ProjectDb::open(&path).expect("reopen before post-commit interruption");
        assert!(
            database
                .commit_with_failure(&second, CommitFailurePoint::AfterCommitReturn)
                .is_err()
        );
        drop(database);
        assert_eq!(
            ProjectDb::open(&path)
                .expect("reopen after committed interruption")
                .load_current()
                .expect("load committed head")
                .expect("second head exists")
                .snapshot_id,
            second.snapshot_id
        );
        let _ = std::fs::remove_file(path);

        for (name, expected_scope, mutation) in corruption_matrix() {
            let path = temp(name);
            let _ = std::fs::remove_file(&path);
            let batch = prepared_batch();
            let database = ProjectDb::open(&path).expect("initialize corruption scratch project");
            database.commit(&batch).expect("durable corruption fixture");
            drop(database);
            mutate_record(&path, &batch, mutation);

            match ProjectDb::open(&path).expect_err("corrupt project is rejected") {
                ProjectOpenError::Corrupt { message, .. } => {
                    assert!(message.contains(expected_scope), "{name}: {message}");
                }
                other => panic!("{name}: expected scoped corruption, got {other:?}"),
            }
            let _ = std::fs::remove_file(path);
        }
    }

    #[derive(Clone, Copy)]
    enum Corruption {
        RemoveHead,
        CorruptHead,
        RemoveRoot,
        CorruptRoot,
        RemoveTile,
        CorruptTile,
        RemoveStroke,
        CorruptStroke,
        RemoveHistory,
        CorruptHistory,
    }

    fn corruption_matrix() -> [(&'static str, &'static str, Corruption); 10] {
        [
            (
                "head-missing",
                "snapshot head missing",
                Corruption::RemoveHead,
            ),
            (
                "head-corrupt",
                "snapshot head corrupt",
                Corruption::CorruptHead,
            ),
            (
                "root-missing",
                "root manifest missing",
                Corruption::RemoveRoot,
            ),
            (
                "root-corrupt",
                "root manifest corrupt",
                Corruption::CorruptRoot,
            ),
            (
                "tile-missing",
                "tile object missing",
                Corruption::RemoveTile,
            ),
            (
                "tile-corrupt",
                "tile object corrupt",
                Corruption::CorruptTile,
            ),
            (
                "stroke-missing",
                "stroke commit missing",
                Corruption::RemoveStroke,
            ),
            (
                "stroke-corrupt",
                "stroke commit corrupt",
                Corruption::CorruptStroke,
            ),
            (
                "history-missing",
                "history node missing",
                Corruption::RemoveHistory,
            ),
            (
                "history-corrupt",
                "history node corrupt",
                Corruption::CorruptHistory,
            ),
        ]
    }

    fn mutate_record(path: &Path, batch: &ProjectCommitBatch, mutation: Corruption) {
        let database = Database::open(path).expect("open redb corruption fixture");
        let mut transaction = database.begin_write().expect("write corruption fixture");
        transaction.set_durability(Durability::Immediate).unwrap();
        let corrupt = [0_u8];
        match mutation {
            Corruption::RemoveHead => mutate_snapshot(&mut transaction, batch, None),
            Corruption::CorruptHead => mutate_snapshot(&mut transaction, batch, Some(&corrupt)),
            Corruption::RemoveRoot => mutate_root(&mut transaction, batch, None),
            Corruption::CorruptRoot => mutate_root(&mut transaction, batch, Some(&corrupt)),
            Corruption::RemoveTile => mutate_tile(&mut transaction, batch, None),
            Corruption::CorruptTile => mutate_tile(&mut transaction, batch, Some(&corrupt)),
            Corruption::RemoveStroke => mutate_stroke(&mut transaction, batch, None),
            Corruption::CorruptStroke => mutate_stroke(&mut transaction, batch, Some(&corrupt)),
            Corruption::RemoveHistory => mutate_history(&mut transaction, batch, None),
            Corruption::CorruptHistory => mutate_history(&mut transaction, batch, Some(&corrupt)),
        }
        transaction.commit().expect("commit corruption fixture");
    }

    fn mutate_snapshot(
        transaction: &mut redb::WriteTransaction,
        batch: &ProjectCommitBatch,
        replacement: Option<&[u8]>,
    ) {
        let mut table = transaction.open_table(SNAPSHOTS).expect("snapshot table");
        let key = batch.snapshot_id.0.to_le_bytes();
        match replacement {
            Some(value) => table.insert(key.as_slice(), value),
            None => table.remove(key.as_slice()),
        }
        .expect("mutate snapshot");
    }

    fn mutate_root(
        transaction: &mut redb::WriteTransaction,
        batch: &ProjectCommitBatch,
        replacement: Option<&[u8]>,
    ) {
        let mut table = transaction.open_table(ROOTS).expect("root table");
        let key = batch.materialized.after.root().hash;
        match replacement {
            Some(value) => table.insert(key.0.as_slice(), value),
            None => table.remove(key.0.as_slice()),
        }
        .expect("mutate root");
    }

    fn mutate_tile(
        transaction: &mut redb::WriteTransaction,
        batch: &ProjectCommitBatch,
        replacement: Option<&[u8]>,
    ) {
        let mut table = transaction.open_table(OBJECTS).expect("object table");
        let key = first_tile_hash(batch);
        match replacement {
            Some(value) => table.insert(key.0.as_slice(), value),
            None => table.remove(key.0.as_slice()),
        }
        .expect("mutate tile");
    }

    fn mutate_stroke(
        transaction: &mut redb::WriteTransaction,
        batch: &ProjectCommitBatch,
        replacement: Option<&[u8]>,
    ) {
        let mut table = transaction.open_table(STROKES).expect("stroke table");
        let key = batch.stroke.id.0;
        match replacement {
            Some(value) => table.insert(key.0.as_slice(), value),
            None => table.remove(key.0.as_slice()),
        }
        .expect("mutate stroke");
    }

    fn mutate_history(
        transaction: &mut redb::WriteTransaction,
        batch: &ProjectCommitBatch,
        replacement: Option<&[u8]>,
    ) {
        let mut table = transaction.open_table(HISTORY).expect("history table");
        let key = batch.history_node.id.0.to_le_bytes();
        match replacement {
            Some(value) => table.insert(key.as_slice(), value),
            None => table.remove(key.as_slice()),
        }
        .expect("mutate history");
    }

    fn first_tile_hash(batch: &ProjectCommitBatch) -> ObjectHash {
        batch
            .materialized
            .after
            .iter()
            .next()
            .expect("fixture materializes a tile")
            .1
            .hash()
    }

    #[test]
    fn invalid_nonempty_file_is_never_overwritten() {
        let path = temp("invalid");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"invalid").expect("write invalid fixture");
        assert!(matches!(
            ProjectDb::open(&path),
            Err(ProjectOpenError::InvalidNonEmpty { .. })
        ));
        assert_eq!(std::fs::read(&path).expect("preserved file"), b"invalid");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn concurrent_open_reports_locked_and_reopens_after_owner_drops() {
        // Product risk: a second activation must not be misreported as an
        // invalid project, and the valid project must remain reopenable.
        let path = temp("locked");
        let _ = std::fs::remove_file(&path);

        let owner = ProjectDb::open(&path).expect("initialize scratch project");
        assert!(matches!(
            ProjectDb::open(&path),
            Err(ProjectOpenError::Locked { path: ref locked_path })
                if locked_path == &path
        ));

        drop(owner);
        let reopened = ProjectDb::open(&path).expect("reopen after lock owner drops");
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }
}

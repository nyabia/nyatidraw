//! Project file policy, independent of any storage backend.
mod wire;

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use nyatidraw_api::{CanvasSpec, HistoryNodeId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_history::{History, HistoryError};
use nyatidraw_tiles::{ContentRoot, TileSnapshot};

pub use wire::*;

/// Backend-neutral durable project boundary.
///
/// A writer publishes one immutable closed-stroke batch only after its backend
/// durability boundary succeeds. Readers reconstruct the current durable head
/// and report corruption without modifying the project file.
pub trait ProjectRepository {
    /// Stores one closed-stroke batch as the next durable head.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the backend cannot establish durability.
    fn commit(&self, batch: &ProjectCommitBatch) -> Result<(), ProjectOpenError>;

    /// Reconstructs the current durable head, if the project has one.
    ///
    /// # Errors
    ///
    /// Returns corruption scoped to the failed persisted record without
    /// initializing or overwriting the project file.
    fn load_current(&self) -> Result<Option<ProjectCommitBatch>, ProjectOpenError>;

    /// Reconstructs the current durable batch and its complete history DAG.
    ///
    /// Implementations must bound record enumeration and reject corrupt graph
    /// links rather than silently dropping redo branches.
    ///
    /// # Errors
    ///
    /// Returns an I/O or corruption error without modifying the project.
    fn load_reopened(&self) -> Result<Option<ReopenedProject>, ProjectOpenError>;

    /// Atomically makes an already-persisted history cursor current.
    ///
    /// This transaction changes no artwork objects or history nodes.
    ///
    /// # Errors
    ///
    /// Returns an error when the target cursor is not a valid persisted
    /// snapshot, or when immediate durability cannot be established.
    fn persist_history_cursor(&self, cursor: ProjectHistoryCursor) -> Result<(), ProjectOpenError>;

    /// Loads immutable tiles identified by an already-validated cursor.
    ///
    /// # Errors
    ///
    /// Returns corruption or I/O errors without changing the project.
    fn load_cursor_tiles(
        &self,
        cursor: ProjectHistoryCursor,
    ) -> Result<TileSnapshot, ProjectOpenError>;

    /// Loads the finite canvas authoritative for project export and display.
    ///
    /// Backends may map projects created before canvas metadata existed to the
    /// documented compatibility default, but must not rewrite the project just
    /// to perform that migration.
    ///
    /// # Errors
    ///
    /// Returns storage or corruption errors without changing the project.
    fn load_canvas_spec(&self) -> Result<CanvasSpec, ProjectOpenError>;

    /// Publishes one validated finite canvas specification independently of
    /// immutable artwork/history records.
    ///
    /// # Errors
    ///
    /// Returns an error when the specification is invalid or its metadata
    /// cannot be durably published.
    fn persist_canvas_spec(&self, canvas: CanvasSpec) -> Result<(), ProjectOpenError>;

    /// Atomically publishes the current validated raster/group hierarchy.
    ///
    /// Layer metadata is a separate durable record from pixel/history data so
    /// a failed structural save cannot invalidate the last artwork head.
    ///
    /// # Errors
    ///
    /// Returns an encoding or storage error without replacing the prior tree.
    fn persist_layer_tree(&self, tree: &LayerTree) -> Result<(), ProjectOpenError>;

    /// Loads the current durable layer hierarchy, if one has been published.
    ///
    /// # Errors
    ///
    /// Returns corruption without modifying or reinitializing the project.
    fn load_layer_tree(&self) -> Result<Option<LayerTree>, ProjectOpenError>;
}

/// A durable history position, separate from a content-changing commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectHistoryCursor {
    pub snapshot_id: SnapshotId,
    pub history_head: Option<HistoryNodeId>,
    pub root: ContentRoot,
}

/// Storage-neutral state needed to resume a headless editor after reopening.
///
/// `current` supplies the materialized current tiles while `history` retains
/// every validated branch, including redo candidates not on the current path.
#[derive(Clone, Debug)]
pub struct ReopenedProject {
    current: Option<ProjectCommitBatch>,
    current_snapshot: SnapshotId,
    current_tiles: TileSnapshot,
    history: History,
    current_cursor: ProjectHistoryCursor,
    cursors: BTreeMap<Option<HistoryNodeId>, ProjectHistoryCursor>,
}

/// Owned storage-neutral fields transferred into an editor session.
#[derive(Clone, Debug)]
pub struct ReopenedProjectParts {
    pub current: Option<ProjectCommitBatch>,
    pub current_snapshot: SnapshotId,
    pub current_tiles: TileSnapshot,
    pub history: History,
    pub current_cursor: ProjectHistoryCursor,
    pub cursors: BTreeMap<Option<HistoryNodeId>, ProjectHistoryCursor>,
}

impl ReopenedProject {
    /// Joins a validated current batch to a reconstructed history cursor.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable cursor or immutable root does not
    /// agree with the current materialized batch.
    pub fn new(
        current: Option<ProjectCommitBatch>,
        current_snapshot: SnapshotId,
        current_tiles: TileSnapshot,
        current_cursor: ProjectHistoryCursor,
        history: History,
        cursors: BTreeMap<Option<HistoryNodeId>, ProjectHistoryCursor>,
    ) -> Result<Self, HistoryError> {
        if history.head() != current_cursor.history_head {
            return Err(HistoryError::CorruptCursor);
        }
        if history.current_root() != current_tiles.root().id
            || current_cursor.root != current_tiles.root()
        {
            return Err(HistoryError::CursorRootMismatch);
        }
        if current_cursor.snapshot_id != current_snapshot {
            return Err(HistoryError::CursorRootMismatch);
        }
        if let Some(batch) = &current
            && (current_cursor.history_head != Some(batch.history_node.id)
                || current_snapshot != batch.snapshot_id
                || current_tiles != batch.materialized.after)
        {
            return Err(HistoryError::CursorRootMismatch);
        }
        if cursors.get(&current_cursor.history_head) != Some(&current_cursor) {
            return Err(HistoryError::CursorRootMismatch);
        }
        Ok(Self {
            current,
            current_snapshot,
            current_tiles,
            history,
            current_cursor,
            cursors,
        })
    }

    #[must_use]
    pub const fn current(&self) -> Option<&ProjectCommitBatch> {
        self.current.as_ref()
    }

    #[must_use]
    pub const fn current_snapshot(&self) -> SnapshotId {
        self.current_snapshot
    }

    #[must_use]
    pub const fn history(&self) -> &History {
        &self.history
    }

    #[must_use]
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }

    #[must_use]
    pub fn current_tiles(&self) -> &TileSnapshot {
        &self.current_tiles
    }

    #[must_use]
    pub fn cursor_for_head(&self, head: Option<HistoryNodeId>) -> Option<ProjectHistoryCursor> {
        self.cursors.get(&head).copied()
    }

    #[must_use]
    pub fn into_parts(self) -> ReopenedProjectParts {
        ReopenedProjectParts {
            current: self.current,
            current_snapshot: self.current_snapshot,
            current_tiles: self.current_tiles,
            history: self.history,
            current_cursor: self.current_cursor,
            cursors: self.cursors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenMode {
    Initialize,
    OpenExisting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectOpenError {
    Io { path: PathBuf, message: String },
    Locked { path: PathBuf },
    InvalidNonEmpty { path: PathBuf },
    Corrupt { path: PathBuf, message: String },
}

impl fmt::Display for ProjectOpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "I/O error for {}: {message}", path.display()),
            Self::Locked { path } => write!(f, "project is locked: {}", path.display()),
            Self::InvalidNonEmpty { path } => {
                write!(f, "non-empty invalid project preserved: {}", path.display())
            }
            Self::Corrupt { path, message } => {
                write!(f, "corrupt project at {}: {message}", path.display())
            }
        }
    }
}
impl std::error::Error for ProjectOpenError {}

/// Missing and exactly empty files are eligible for backend initialization.
/// Existing non-empty files must be validated by the backend; invalid ones are never replaced.
///
/// # Errors
///
/// Returns [`ProjectOpenError::Io`] when metadata cannot be read or the path
/// is not a regular file.
pub fn open_mode(path: &Path) -> Result<OpenMode, ProjectOpenError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() == 0 => Ok(OpenMode::Initialize),
        Ok(meta) if meta.is_file() => Ok(OpenMode::OpenExisting),
        Ok(_) => Err(ProjectOpenError::Io {
            path: path.to_owned(),
            message: "path is not a regular file".into(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OpenMode::Initialize),
        Err(e) => Err(ProjectOpenError::Io {
            path: path.to_owned(),
            message: e.to_string(),
        }),
    }
}

pub const SCHEMA_VERSION: u64 = 1;
/// Adds immutable per-snapshot layer metadata. Writers using the original
/// schema must reject this marker instead of silently discarding that history.
pub const LAYER_HISTORY_SCHEMA_VERSION: u64 = 2;

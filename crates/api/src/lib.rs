#![forbid(unsafe_code)]

mod dock;
mod protocol;

use std::collections::BTreeSet;

pub use dock::{
    DockAxis, DockLayoutError, DockLayoutRecovery, DockLayoutRecoveryStatus, DockMutationError,
    DockNode, DockPosition, DockTree, PanelKind,
};
pub use protocol::{
    CommandEnvelope, CommandRejectReason, DockCommand, DrawingTool, EditorCommand, EditorEvent,
    EventEnvelope, HISTORY_PROJECTION_MAX_ENTRIES, HistoryCommand, HistoryEntryProjection,
    HistoryOperationLabel, HistoryProjection, LayerCommand, LayerProjection, LayerProjectionKind,
    ProjectCommand, ToolCommand, UiProjection, ViewportCommand, ViewportProjection,
    WorkspaceProjection,
};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(pub u128);
    };
}

id_type!(DocumentId);
id_type!(LayerId);
id_type!(GroupId);
id_type!(ContentRootId);
id_type!(SnapshotId);
id_type!(HistoryNodeId);
id_type!(CommandId);

/// Finite document extent in top-left-origin document pixels.
///
/// Sparse tile storage may contain signed coordinates while the document canvas
/// remains the half-open rectangle `[0, width_px) x [0, height_px)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanvasSpec {
    pub width_px: u32,
    pub height_px: u32,
    pub pixels_per_inch: u32,
}

impl CanvasSpec {
    /// Migration-safe extent used by the current Windows canvas.
    pub const DEFAULT: Self = Self {
        width_px: 1_024,
        height_px: 768,
        pixels_per_inch: 96,
    };

    /// Validates that the canvas has non-zero dimensions and resolution.
    ///
    /// # Errors
    ///
    /// Returns [`CanvasSpecError::EmptyCanvas`] for a zero dimension and
    /// [`CanvasSpecError::ZeroResolution`] for zero pixels per inch.
    pub fn validate(self) -> Result<Self, CanvasSpecError> {
        if self.width_px == 0 || self.height_px == 0 {
            return Err(CanvasSpecError::EmptyCanvas);
        }
        if self.pixels_per_inch == 0 {
            return Err(CanvasSpecError::ZeroResolution);
        }
        Ok(self)
    }
}

impl Default for CanvasSpec {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanvasSpecError {
    EmptyCanvas,
    ZeroResolution,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Revision(pub u64);

impl Revision {
    /// Returns the next UI-visible revision, or `None` after `u64::MAX`.
    #[must_use]
    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LayerTreeNodeId {
    Raster(LayerId),
    Group(GroupId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileCoordinate {
    pub mip: u8,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompositeTileKey {
    pub group: GroupId,
    pub tile: TileCoordinate,
}

/// Cache entries invalidated by one semantic document change.
///
/// Pixel edits normally add exact group/tile keys. Structural edits add group
/// IDs to `all_tiles`, leaving enumeration of cached coordinates to the cache
/// owner rather than turning a local edit into a document-sized scan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompositeInvalidation {
    exact_tiles: BTreeSet<CompositeTileKey>,
    all_tiles: BTreeSet<GroupId>,
}

impl CompositeInvalidation {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            exact_tiles: BTreeSet::new(),
            all_tiles: BTreeSet::new(),
        }
    }

    pub fn invalidate_tile(&mut self, group: GroupId, tile: TileCoordinate) {
        self.exact_tiles.insert(CompositeTileKey { group, tile });
    }

    pub fn invalidate_all_tiles(&mut self, group: GroupId) {
        self.all_tiles.insert(group);
    }

    pub fn extend(&mut self, other: Self) {
        self.exact_tiles.extend(other.exact_tiles);
        self.all_tiles.extend(other.all_tiles);
    }

    pub fn exact_tiles(&self) -> impl Iterator<Item = CompositeTileKey> + '_ {
        self.exact_tiles.iter().copied()
    }

    pub fn all_tiles(&self) -> impl Iterator<Item = GroupId> + '_ {
        self.all_tiles.iter().copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.exact_tiles.is_empty() && self.all_tiles.is_empty()
    }
}

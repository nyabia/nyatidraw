use crate::{
    CommandId, DockLayoutError, DockPosition, DockTree, GroupId, HistoryNodeId, LayerId,
    LayerTreeNodeId, PanelKind, Revision,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandEnvelope {
    pub id: CommandId,
    /// Projection revision on which the user action was based.
    pub based_on: Revision,
    pub command: EditorCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorCommand {
    History(HistoryCommand),
    Edit(EditCommand),
    Project(ProjectCommand),
    Tool(ToolCommand),
    Layer(LayerCommand),
    Dock(DockCommand),
    Viewport(ViewportCommand),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditSource {
    ActiveLayer,
    ReferenceLayers,
    AllVisible,
}

/// Completed document-space gestures; raw pointer samples never enter this lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditCommand {
    SelectWand {
        seed: [i32; 2],
        tolerance: u8,
        source: EditSource,
    },
    /// Admission rejects payloads outside 3..=4096 vertices.
    SelectLasso {
        vertices: Vec<[i32; 2]>,
    },
    ClearSelection,
    /// Colors are premultiplied linear RGBA8, matching durable tile pixels.
    FillSelection {
        color: [u8; 4],
    },
    FloodFill {
        seed: [i32; 2],
        tolerance: u8,
        source: EditSource,
        color: [u8; 4],
    },
    GradientSelection {
        start: [i32; 2],
        end: [i32; 2],
        start_color: [u8; 4],
        end_color: [u8; 4],
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditProjection {
    pub busy: bool,
    pub has_selection: bool,
    pub selected_pixels: u64,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrawingTool {
    Move,
    Pencil,
    Pen,
    Brush,
    Eraser,
    Wand,
    Lasso,
    Fill,
    Gradient,
}

impl DrawingTool {
    #[must_use]
    pub const fn is_edit(self) -> bool {
        matches!(self, Self::Wand | Self::Lasso | Self::Fill | Self::Gradient)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditSettings {
    pub source: EditSource,
    pub tolerance: u8,
}

impl Default for EditSettings {
    fn default() -> Self {
        Self {
            source: EditSource::ActiveLayer,
            tolerance: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCommand {
    /// Cancels only an unfinished native edit gesture, leaving artwork intact.
    CancelGesture,
    Select(DrawingTool),
    CycleBrushFamily,
    SetSizeTenths(u16),
    SetOpacityU16(u16),
    SetColor([u8; 4]),
    SetEditSource(EditSource),
    SetEditTolerance(u8),
}

/// Discrete viewport operations keep the cross-thread protocol deterministic
/// and `Eq` while the renderer remains the authority for the resulting affine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportCommand {
    FitDocument,
    ActualPixels,
    PanBy {
        logical_x: i32,
        logical_y: i32,
    },
    /// Centers one finite-page coordinate while preserving zoom and rotation.
    /// Coordinates are ten-thousandths of the output page extent.
    CenterPageAt {
        page_x_10k: i32,
        page_y_10k: i32,
    },
    /// Number of 1.25x zoom increments; negative values zoom out.
    ZoomSteps(i8),
    /// Pointer-centered zoom in logical child-window coordinates.
    ZoomAt {
        steps: i8,
        logical_x: i32,
        logical_y: i32,
    },
    /// Number of 45 degree clockwise increments; negative values rotate left.
    RotateQuarterSteps(i8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryCommand {
    Undo,
    Redo,
    RedoTo(HistoryNodeId),
    /// Pages direct redo children without changing the durable cursor.
    ShowRedoBranches {
        after: Option<HistoryNodeId>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectCommand {
    Save,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerCommand {
    SetActive(LayerId),
    SetReference {
        layer: LayerId,
        reference: bool,
    },
    /// Inserts one empty raster immediately above the active raster.
    AddRaster,
    /// Inserts one empty group immediately above the active raster.
    AddGroup,
    Delete(LayerTreeNodeId),
    Rename {
        node: LayerTreeNodeId,
        name: String,
    },
    /// Session-only isolation. This never changes durable visibility/export.
    ToggleSolo(LayerTreeNodeId),
    /// Materializes the reserved transparent background raster as opaque white.
    /// This is an artwork operation, not a viewport appearance preference.
    AddWhiteBackground,
    SetVisibility {
        node: LayerTreeNodeId,
        visible: bool,
    },
    SetOpacity {
        node: LayerTreeNodeId,
        opacity_u16: u16,
    },
    Reorder {
        node: LayerTreeNodeId,
        new_parent: GroupId,
        index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DockCommand {
    Replace(DockTree),
    ActivatePanel(PanelKind),
    MovePanel {
        panel: PanelKind,
        target: PanelKind,
        position: DockPosition,
    },
    MoveToolbarToTop {
        panel: PanelKind,
        before: Option<PanelKind>,
    },
    ResetToSafeDefault,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventEnvelope {
    pub sequence: u64,
    pub event: EditorEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorEvent {
    CommandAccepted {
        command: CommandId,
        revision: Revision,
    },
    ProjectionChanged {
        revision: Revision,
    },
    CommandRejected {
        command: CommandId,
        reason: CommandRejectReason,
    },
    LayoutRecovered {
        revision: Revision,
        reason: DockLayoutError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandRejectReason {
    StaleProjection {
        based_on: Revision,
        current: Revision,
    },
    UnknownLayer,
    InvalidLayerMove,
    InvalidLayout,
    InvalidViewport,
    CommandQueueBusy,
    WorkspaceFailed,
    UnsupportedCommand,
    RevisionExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceProjection {
    Empty,
    Loading,
    Ready,
    Error { summary: String },
    Recovery { summary: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerProjectionKind {
    Raster,
    Group,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerProjection {
    pub id: LayerTreeNodeId,
    pub parent: GroupId,
    /// Bottom-to-top sibling index before a proposed move.
    pub index: usize,
    pub depth: u16,
    pub kind: LayerProjectionKind,
    pub name: String,
    pub visible: bool,
    pub reference: bool,
    pub opacity_u16: u16,
}

/// Maximum number of branch-ancestry rows sent to the editor chrome.
///
/// The writer already owns the reconstructed in-memory history session, so a
/// projection walks at most this many parent links and never asks the UI to
/// enumerate project storage.
pub const HISTORY_PROJECTION_MAX_ENTRIES: usize = 64;

/// The semantic operation represented by a history row.
///
/// Labels carry no artwork or content roots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryOperationLabel {
    Initial,
    Stroke,
    Structural,
}

/// One bounded, non-interactive row in the history-panel projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryEntryProjection {
    pub operation: HistoryOperationLabel,
}

/// A direct child of the current cursor, eligible for explicit redo.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryBranchProjection {
    pub node: HistoryNodeId,
    pub operation: HistoryOperationLabel,
    pub timestamp_ns: u64,
}

/// Current-cursor ancestry supplied by the project writer.
///
/// Entries are newest first, including the initial state when the bounded
/// window reaches it. `current` always identifies the current cursor row;
/// `has_older_entries` means the retained ancestry continues before the last
/// projected operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryProjection {
    pub entries: Vec<HistoryEntryProjection>,
    pub current: usize,
    pub has_older_entries: bool,
    pub redo_branches: Vec<HistoryBranchProjection>,
    pub redo_page_after: Option<HistoryNodeId>,
    pub has_more_redo_branches: bool,
}

impl HistoryProjection {
    #[must_use]
    pub fn initial() -> Self {
        Self {
            entries: vec![HistoryEntryProjection {
                operation: HistoryOperationLabel::Initial,
            }],
            current: 0,
            has_older_entries: false,
            redo_branches: Vec::new(),
            redo_page_after: None,
            has_more_redo_branches: false,
        }
    }
}

/// Small immutable state published to the UI at semantic revision boundaries.
///
/// Pixel data, GPU handles, and native/raw input samples are intentionally not
/// representable in this protocol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiProjection {
    pub revision: Revision,
    pub document_title: String,
    pub dirty: bool,
    pub workspace: WorkspaceProjection,
    pub active_layer: Option<LayerId>,
    pub solo_node: Option<LayerTreeNodeId>,
    pub layers: Vec<LayerProjection>,
    pub history: HistoryProjection,
    pub edit: EditProjection,
    pub dock: DockTree,
    pub viewport: ViewportProjection,
    pub drawing_tool: DrawingTool,
    pub brush_size_tenths: u16,
    pub brush_opacity_u16: u16,
    pub brush_color: [u8; 4],
    pub edit_settings: EditSettings,
}

/// Fixed-point mirror of the renderer-owned viewport for UI display and
/// command feedback. It never participates in raw pointer coordinate mapping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportProjection {
    pub pan_x_milli: i64,
    pub pan_y_milli: i64,
    pub zoom_ppm: u32,
    pub rotation_millidegrees: i32,
}

impl UiProjection {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            revision: Revision::default(),
            document_title: String::new(),
            dirty: false,
            workspace: WorkspaceProjection::Empty,
            active_layer: None,
            solo_node: None,
            layers: Vec::new(),
            history: HistoryProjection::initial(),
            edit: EditProjection::default(),
            dock: DockTree::safe_default(),
            viewport: ViewportProjection {
                zoom_ppm: 1_000_000,
                ..ViewportProjection::default()
            },
            drawing_tool: DrawingTool::Brush,
            brush_size_tenths: 280,
            brush_opacity_u16: 60_292,
            brush_color: [26, 199, 232, 255],
            edit_settings: EditSettings::default(),
        }
    }

    /// Advances the semantic UI revision without modifying any input queue.
    ///
    /// # Errors
    ///
    /// Returns `RevisionExhausted` instead of wrapping the observable version.
    pub fn advance_revision(&mut self) -> Result<Revision, CommandRejectReason> {
        self.revision = self
            .revision
            .checked_next()
            .ok_or(CommandRejectReason::RevisionExhausted)?;
        Ok(self.revision)
    }
}

impl Default for UiProjection {
    fn default() -> Self {
        Self::empty()
    }
}

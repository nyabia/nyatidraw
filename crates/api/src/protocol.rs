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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionMode {
    #[default]
    Replace,
    Add,
    Subtract,
}

/// Completed document-space gestures; raw pointer samples never enter this lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditCommand {
    CopySelection,
    CutSelection,
    /// Paste as a new raster above the active one, preserving source position.
    PasteSelection,
    /// Read one signed document pixel; never changes artwork or history.
    PickColor {
        point: [i32; 2],
        source: EditSource,
    },
    /// Temporary canvas picker: session Solo affects the sampled artwork,
    /// while checkerboard, page border and editor overlays never do.
    PickDisplayColor {
        point: [i32; 2],
        solo: Option<crate::LayerTreeNodeId>,
        input_sequence: (u64, u64),
    },
    /// Clears every pixel of the active raster, including off-page artwork.
    /// Keeps the layer and its properties; independent of selection.
    ClearActiveLayer,
    /// Change the output extent, preserving every signed artwork pixel.
    ResizePage {
        size: [u32; 2],
    },
    /// Rebase all artwork to the selection bounds' top-left and use its extent
    /// as the new output page. Pixels outside that rectangle are retained.
    CropPageToSelection,
    /// Cut the selection (or whole active raster when absent), then transform
    /// and composite it back. Successful completion clears the selection.
    Transform(RasterTransform),
    /// Provisional free transform; only explicit Commit creates history.
    FreeTransform(TransformCommand),
    SelectWand {
        seed: [i32; 2],
        tolerance: u8,
        source: EditSource,
    },
    /// Admission rejects payloads outside 3..=4096 vertices.
    SelectLasso {
        vertices: Vec<[i32; 2]>,
    },
    CombineLasso {
        vertices: Vec<[i32; 2]>,
        mode: SelectionMode,
    },
    /// Finite page plus active artwork and current selection bounds.
    SelectAll,
    InvertSelection,
    DeleteSelectedPixels,
    GrowSelection {
        radius: u8,
    },
    ShrinkSelection {
        radius: u8,
    },
    SelectLayerAlpha,
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
    /// Connected fill with boundary controls, clipped by any current selection.
    FloodFillAdvanced {
        seed: [i32; 2],
        tolerance: u8,
        source: EditSource,
        color: [u8; 4],
        settings: FillSettings,
    },
    GradientSelection {
        start: [i32; 2],
        end: [i32; 2],
        start_color: [u8; 4],
        end_color: [u8; 4],
    },
}

/// Integer pixel-art transform: source flips, clockwise quarter-turns, nearest
/// pixel-center resize, then translation from the source bounds' top-left.
/// No page clipping is applied to the resulting signed artwork.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterTransform {
    pub offset: [i32; 2],
    pub quarter_turns: u8,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Final dimensions after rotation. None preserves the rotated dimensions.
    pub size: Option<[u32; 2]>,
}

/// Document-space free transform about the selected bounds' center.
/// Flips precede scale and clockwise rotation, followed by translation.
/// Integer UI units keep command equality exact; sampling uses premultiplied
/// linear pixels. This is artwork editing, not a viewport transformation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AffineTransform {
    pub offset_milli: [i64; 2],
    pub scale_ppm: [u32; 2],
    pub rotation_millidegrees: i32,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for AffineTransform {
    fn default() -> Self {
        Self {
            offset_milli: [0, 0],
            scale_ppm: [1_000_000, 1_000_000],
            rotation_millidegrees: 0,
            flip_x: false,
            flip_y: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransformCommand {
    Begin,
    Paste,
    Preview(AffineTransform),
    Commit { generation: u64 },
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransformProjection {
    pub generation: u64,
    pub transform: AffineTransform,
    pub origin: [i32; 2],
    pub size: [u32; 2],
    pub corners_milli: [[i64; 2]; 4],
    pub can_commit: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditProjection {
    pub busy: bool,
    /// Writer-owned cancellation availability, including a transform starting
    /// or previewing asynchronously. Committing artwork is not cancellable.
    pub can_cancel: bool,
    pub has_selection: bool,
    pub selected_pixels: u64,
    pub error: Option<String>,
    pub transform: Option<TransformProjection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrawingTool {
    /// Viewport navigation only; never moves artwork.
    Move,
    MoveSelection,
    Pencil,
    Pen,
    Brush,
    Eraser,
    Wand,
    Lasso,
    RectangleSelection,
    Eyedropper,
    Fill,
    Gradient,
}

impl DrawingTool {
    #[must_use]
    pub const fn is_edit(self) -> bool {
        matches!(
            self,
            Self::Wand
                | Self::Lasso
                | Self::RectangleSelection
                | Self::MoveSelection
                | Self::Eyedropper
                | Self::Fill
                | Self::Gradient
        )
    }
}

/// Built-in dry-pencil templates. Session selection is separate from the
/// immutable engine-versioned preset captured by each artwork stroke.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PencilTemplate {
    #[default]
    Mechanical2H,
    Graphite2B,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FillSettings {
    pub gap_close_px: u8,
    pub expand_px: u8,
    pub antialias: bool,
}

impl Default for FillSettings {
    fn default() -> Self {
        Self {
            gap_close_px: 0,
            expand_px: 1,
            antialias: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditSettings {
    pub source: EditSource,
    pub tolerance: u8,
    pub selection_mode: SelectionMode,
    pub fill: FillSettings,
}

impl Default for EditSettings {
    fn default() -> Self {
        Self {
            source: EditSource::ActiveLayer,
            tolerance: 0,
            selection_mode: SelectionMode::Replace,
            fill: FillSettings::default(),
        }
    }
}

/// Fixed-point, session-only controls for the current brush template.
/// Ratios use the full `u16` range; no raw input or renderer state crosses UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrushSettings {
    pub size_pressure: bool,
    pub opacity_pressure: bool,
    pub size_minimum_u16: u16,
    pub opacity_minimum_u16: u16,
    pub hardness_u16: u16,
    /// Session input smoothing strength. Zero is exact bypass.
    pub smoothing: u8,
}

impl BrushSettings {
    #[must_use]
    pub const fn for_tool(tool: DrawingTool) -> Self {
        if matches!(tool, DrawingTool::Pencil) {
            return Self::for_pencil_template(PencilTemplate::Mechanical2H);
        }
        Self {
            size_pressure: true,
            opacity_pressure: false,
            size_minimum_u16: 0,
            opacity_minimum_u16: 0,
            hardness_u16: match tool {
                DrawingTool::Brush => 0,
                _ => u16::MAX,
            },
            smoothing: 0,
        }
    }

    #[must_use]
    pub const fn for_pencil_template(template: PencilTemplate) -> Self {
        Self {
            size_pressure: true,
            opacity_pressure: true,
            size_minimum_u16: match template {
                PencilTemplate::Mechanical2H => 53_739,
                PencilTemplate::Graphite2B => 22_937,
            },
            opacity_minimum_u16: 3_277,
            hardness_u16: u16::MAX,
            smoothing: 0,
        }
    }
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self::for_tool(DrawingTool::Brush)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCommand {
    /// Cancels a transform draft (including its pending start), otherwise an
    /// active native gesture, otherwise clears the completed selection. Each
    /// command consumes one stage and never deletes selected artwork.
    CancelGesture,
    Select(DrawingTool),
    SelectPencilTemplate(PencilTemplate),
    CycleBrushFamily,
    CycleSelectionFamily,
    CycleFillFamily,
    SetSizeTenths(u16),
    /// Applies relative steps to the writer-owned size, avoiding stale UI values.
    AdjustSizeSteps {
        tool: DrawingTool,
        steps: i8,
    },
    SetOpacityU16(u16),
    SetSizePressure(bool),
    SetOpacityPressure(bool),
    SetSizeMinimumU16(u16),
    SetOpacityMinimumU16(u16),
    SetHardnessU16(u16),
    SetSmoothing(u8),
    SetColor([u8; 4]),
    SwapColors,
    SetEditSource(EditSource),
    SetEditTolerance(u8),
    SetFillSettings(FillSettings),
    SetFillGapClose(u8),
    SetFillExpansion(u8),
    SetFillAntialias(bool),
    SetSelectionMode(SelectionMode),
}

/// Discrete viewport operations keep the cross-thread protocol deterministic
/// and `Eq` while the renderer remains the authority for the resulting affine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportCommand {
    /// Fit the page, resetting rotation and view mirroring.
    FitDocument,
    /// One document pixel per physical pixel; reset rotation and view mirroring.
    ActualPixels,
    PanBy {
        logical_x: i32,
        logical_y: i32,
    },
    /// Centers one finite-page coordinate while preserving zoom and orientation.
    /// Coordinates are ten-thousandths of the output page extent.
    CenterPageAt {
        page_x_10k: i32,
        page_y_10k: i32,
    },
    /// Viewport-center-anchored 1.25x zoom increments; negative values zoom out.
    ZoomSteps(i8),
    /// Pointer-centered zoom in logical child-window coordinates.
    ZoomAt {
        steps: i8,
        logical_x: i32,
        logical_y: i32,
    },
    /// Center-anchored 45 degree clockwise increments; negative values rotate left.
    RotateQuarterSteps(i8),
    /// Reflect document X in the view around the current viewport center.
    ToggleMirrorHorizontal,
    /// Reset only rotation, preserving the center document anchor, zoom and mirror.
    ResetRotation,
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

/// Raster/group composition over a premultiplied linear-light backdrop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayerBlendMode {
    #[default]
    Normal,
    Multiply,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerCommand {
    SetAlphaLocked {
        layer: LayerId,
        alpha_locked: bool,
    },
    SetClipToBelow {
        node: LayerTreeNodeId,
        clip_to_below: bool,
    },
    SetBlendMode {
        node: LayerTreeNodeId,
        blend_mode: LayerBlendMode,
    },
    SetActive(LayerId),
    SetReference {
        layer: LayerId,
        reference: bool,
    },
    /// Inserts one empty raster immediately above the active raster.
    AddRaster,
    DuplicateRaster(LayerId),
    SetLocked {
        layer: LayerId,
        locked: bool,
    },
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
    LayerLocked,
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
// Orthogonal user layer toggles, not states of one state machine.
#[allow(clippy::struct_excessive_bools)]
pub struct LayerProjection {
    pub alpha_locked: bool,
    pub clip_to_below: bool,
    pub blend_mode: LayerBlendMode,
    pub id: LayerTreeNodeId,
    pub parent: GroupId,
    /// Bottom-to-top sibling index before a proposed move.
    pub index: usize,
    pub depth: u16,
    pub kind: LayerProjectionKind,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub reference: bool,
    pub opacity_u16: u16,
}

/// Maximum number of branch-ancestry rows sent to the editor chrome.
///
/// The writer already owns the reconstructed in-memory history session, so a
/// projection walks at most this many parent links and never asks the UI to
/// enumerate project storage.
pub const HISTORY_PROJECTION_MAX_ENTRIES: usize = 128;

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
    pub canvas: crate::CanvasSpec,
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
    pub pencil_template: PencilTemplate,
    pub brush_size_tenths: u16,
    /// Sizes used at accepted drawing Begin, newest first, at most four unique
    /// tenths of a pixel. Session-only; selection and Undo/Redo do not update it.
    pub recent_brush_sizes: Vec<u16>,
    pub brush_opacity_u16: u16,
    pub brush_settings: BrushSettings,
    /// Straight sRGB8 UI color; alpha is linear. Convert before artwork commands.
    pub brush_color: [u8; 4],
    pub background_color: [u8; 4],
    /// Accepted painting Begin colors, newest first, at most ten unique sRGB8
    /// colors. Selection, eyedropper, eraser and Undo/Redo do not update this.
    pub recent_colors: Vec<[u8; 4]>,
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
    pub mirrored_horizontal: bool,
}

impl UiProjection {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            revision: Revision::default(),
            canvas: crate::CanvasSpec::DEFAULT,
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
            drawing_tool: DrawingTool::Pencil,
            pencil_template: PencilTemplate::Mechanical2H,
            brush_size_tenths: 50,
            recent_brush_sizes: Vec::new(),
            brush_opacity_u16: u16::MAX,
            brush_settings: BrushSettings::for_tool(DrawingTool::Pencil),
            brush_color: [0, 0, 0, 255],
            background_color: [255; 4],
            recent_colors: Vec::new(),
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

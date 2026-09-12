use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

#[cfg(test)]
#[path = "signed_selection_acceptance.rs"]
mod signed_selection_acceptance;

#[cfg(test)]
#[path = "affine_acceptance.rs"]
mod affine_acceptance;

#[cfg(test)]
#[path = "flat_fill_acceptance.rs"]
mod flat_fill_acceptance;

#[cfg(test)]
#[path = "alpha_lock_acceptance.rs"]
mod alpha_lock_acceptance;
#[cfg(test)]
#[path = "shading_gpu_acceptance.rs"]
mod shading_gpu_acceptance;

#[cfg(test)]
#[path = "flat_fill_gpu_acceptance.rs"]
mod flat_fill_gpu_acceptance;

#[cfg(test)]
#[path = "transform_gpu_acceptance.rs"]
mod transform_gpu_acceptance;

#[path = "transform_runtime.rs"]
mod transform_runtime;

#[path = "picker_preview.rs"]
mod picker_preview;
pub(crate) use picker_preview::PickerSnapshot;

use nyatidraw_api::{
    BrushSettings, CanvasSpec, CommandEnvelope, CommandRejectReason, ContentRootId, DockCommand,
    DockTree, DrawingTool, EditCommand, EditorCommand, EditorEvent, GroupId, HistoryNodeId,
    HistoryProjection, LayerCommand, LayerId, ProjectCommand, Revision, SnapshotId, ToolCommand,
    ViewportCommand, ViewportProjection,
};
use nyatidraw_brush::{
    BrushDab, BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot,
    ROUND_BRUSH_ENGINE_VERSION, ROUND_BRUSH_PRESET_SCHEMA_VERSION, RecordedStroke,
    RoundBrushEvaluator, RoundBrushStroke, begin_round_stroke,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::{HeadlessStrokeSession, ProjectionError, ProjectionState};
use nyatidraw_input::{
    PenButtons, Point, PointerPhase, StrokeSmoother, StylusSample, ViewportTransform,
};
use nyatidraw_paint_cpu::{
    flatten_layer_tree_rgba8, render_page_preview_rgba8, render_raster_page_preview_rgba8,
};
use nyatidraw_paint_gpu::{
    CompositeRenderError, GpuCompositeScene, GpuRoundDabPainter, LayerUploadError,
    LiveStrokeDisposition, LiveStrokeToken, SignedDirtyRect,
};
use nyatidraw_project::{ProjectOpenError, ReopenedProject};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_stroke::{MAX_SAMPLES_PER_STROKE, StrokeColor};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

use crate::{
    edit_worker::{EditFailure, EditOutcome},
    elapsed_since_launch,
    live_ink::{AdmittedSample, CloseStatus, ExportStatus, InputDiscontinuity, LiveInkBridge},
    performance::{HistoryTiming, HistoryWorkerStep, HistoryWorkerTiming, Span, Stage},
    preview::{
        LAYER_THUMBNAIL_MAX_HEIGHT, LAYER_THUMBNAIL_MAX_WIDTH, MAX_LAYER_THUMBNAILS,
        NAVIGATOR_MAX_HEIGHT, NAVIGATOR_MAX_WIDTH, NavigatorViewport, layer_thumbnail_frame,
        navigator_frame,
    },
};

const LIVE_BRUSH: BrushPreset = BrushPreset {
    id: BrushPresetId(1),
    schema_version: ROUND_BRUSH_PRESET_SCHEMA_VERSION,
    engine_version: ROUND_BRUSH_ENGINE_VERSION,
    size_px: 5.0,
    opacity: 1.0,
    flow: 0.38,
    spacing_ratio: 0.12,
    size_pressure: true,
    opacity_pressure: false,
    size_min_ratio: 0.0,
    opacity_min_ratio: 0.0,
    hardness: 0.0,
};
#[cfg(test)]
const LIVE_BRUSH_COLOR: StrokeColor = StrokeColor([26, 199, 232, 255]);
const LIVE_LAYER: LayerId = LayerId(1);
const BACKGROUND_LAYER: LayerId = LayerId(2);
const ROOT_GROUP: GroupId = GroupId(100);
static TRANSPARENT_HISTORY_TILE: [u8; TILE_BYTE_LEN] = [0; TILE_BYTE_LEN];
const INK_GROUP: GroupId = GroupId(10);
const MATERIALIZATION_QUEUE_CAPACITY: usize = 4;
// A completion carries the exact CPU tile payload needed to retire the GPU
// preview. Keep this bounded to the same admission budget as writer requests.
const CLOSED_STROKE_COMPLETION_QUEUE_CAPACITY: usize = MATERIALIZATION_QUEUE_CAPACITY;
const PROJECT_PATH_ENV: &str = "NAYATI_PROJECT_PATH";
const DURABILITY_PROBE_ENV: &str = "NAYATI_DESKTOP_DURABILITY_PROBE";
const CLOSE_RAW_QUEUE_PROBE_ENV: &str = "NAYATI_CLOSE_RAW_QUEUE_PROBE";
const ACTIVE_CLOSE_PROBE_ENV: &str = "NAYATI_ACTIVE_CLOSE_PROBE";
const SAVE_PROBE_ENV: &str = "NAYATI_SAVE_PROBE";
const PROTOCOL_PROBE_ENV: &str = "NAYATI_PROTOCOL_PROBE";
const HISTORY_PROBE_ENV: &str = "NAYATI_HISTORY_PROBE";
const INPUT_SAFETY_PROBE_ENV: &str = "NAYATI_INPUT_SAFETY_PROBE";
static ACTIVE_PROBE_USED: AtomicBool = AtomicBool::new(false);
static SAVE_PROBE_USED: AtomicBool = AtomicBool::new(false);

fn history_probe_command() -> Option<EditorCommand> {
    use nyatidraw_api::HistoryCommand;
    let value = std::env::var(HISTORY_PROBE_ENV).ok()?;
    let layer = match value.as_str() {
        "reference-r:1" => Some(LayerCommand::SetReference {
            layer: LayerId(1),
            reference: true,
        }),
        "rename-r:1" => Some(LayerCommand::Rename {
            node: nyatidraw_api::LayerTreeNodeId::Raster(LayerId(1)),
            name: "Renamed layer".into(),
        }),
        "opacity-g:10" => Some(LayerCommand::SetOpacity {
            node: nyatidraw_api::LayerTreeNodeId::Group(GroupId(10)),
            opacity_u16: 32_768,
        }),
        "reorder-r:1" => Some(LayerCommand::Reorder {
            node: nyatidraw_api::LayerTreeNodeId::Raster(LayerId(1)),
            new_parent: ROOT_GROUP,
            index: 0,
        }),
        _ => value
            .strip_prefix("delete-r:")
            .and_then(|id| id.parse().ok())
            .map(|id| LayerCommand::Delete(nyatidraw_api::LayerTreeNodeId::Raster(LayerId(id))))
            .or_else(|| {
                value
                    .strip_prefix("delete-g:")
                    .and_then(|id| id.parse().ok())
                    .map(|id| {
                        LayerCommand::Delete(nyatidraw_api::LayerTreeNodeId::Group(GroupId(id)))
                    })
            }),
    };
    if let Some(layer) = layer {
        return Some(EditorCommand::Layer(layer));
    }
    match value.as_str() {
        "undo" => Some(HistoryCommand::Undo),
        "redo" => Some(HistoryCommand::Redo),
        "page" => Some(HistoryCommand::ShowRedoBranches { after: None }),
        _ => value
            .strip_prefix("redo:")
            .and_then(|id| id.parse().ok())
            .map(|id| HistoryCommand::RedoTo(HistoryNodeId(id)))
            .or_else(|| {
                value
                    .strip_prefix("page:")
                    .and_then(|id| id.parse().ok())
                    .map(|id| HistoryCommand::ShowRedoBranches {
                        after: Some(HistoryNodeId(id)),
                    })
            }),
    }
    .map(EditorCommand::History)
}

pub(crate) fn preset_for_ui_preview(projection: &nyatidraw_api::UiProjection) -> BrushPreset {
    DrawingConfig::from_projection(projection).preset()
}

#[derive(Clone, Copy, Debug)]
struct DrawingConfig {
    edit_settings: nyatidraw_api::EditSettings,
    tool: DrawingTool,
    pencil_template: nyatidraw_api::PencilTemplate,
    size_tenths: u16,
    opacity_u16: u16,
    brush_settings: BrushSettings,
    remembered: [RememberedBrush; 5],
    last_painting_slot: usize,
    color: [u8; 4],
    background_color: [u8; 4],
}

#[derive(Clone, Copy, Debug)]
struct RememberedBrush {
    size_tenths: u16,
    opacity_u16: u16,
    settings: BrushSettings,
}

impl RememberedBrush {
    const fn for_tool(tool: DrawingTool) -> Self {
        Self {
            size_tenths: 50,
            opacity_u16: u16::MAX,
            settings: BrushSettings::for_tool(tool),
        }
    }
}

impl DrawingConfig {
    fn from_projection(projection: &nyatidraw_api::UiProjection) -> Self {
        let mut drawing = Self {
            edit_settings: projection.edit_settings,
            tool: projection.drawing_tool,
            pencil_template: projection.pencil_template,
            size_tenths: projection.brush_size_tenths,
            opacity_u16: projection.brush_opacity_u16,
            brush_settings: projection.brush_settings,
            remembered: [
                RememberedBrush::for_tool(DrawingTool::Pencil),
                RememberedBrush::for_tool(DrawingTool::Pen),
                RememberedBrush::for_tool(DrawingTool::Brush),
                RememberedBrush::for_tool(DrawingTool::Eraser),
                RememberedBrush {
                    settings: BrushSettings::for_pencil_template(
                        nyatidraw_api::PencilTemplate::Graphite2B,
                    ),
                    ..RememberedBrush::for_tool(DrawingTool::Pencil)
                },
            ],
            last_painting_slot: Self::painting_slot(projection.drawing_tool).unwrap_or(2),
            color: projection.brush_color,
            background_color: projection.background_color,
        };
        if projection.drawing_tool == DrawingTool::Pencil
            && projection.pencil_template == nyatidraw_api::PencilTemplate::Graphite2B
        {
            drawing.last_painting_slot = 4;
        }
        drawing.remember_current();
        drawing
    }

    const fn painting_slot(tool: DrawingTool) -> Option<usize> {
        match tool {
            DrawingTool::Pencil => Some(0),
            DrawingTool::Pen => Some(1),
            DrawingTool::Brush => Some(2),
            DrawingTool::Eraser => Some(3),
            _ => None,
        }
    }

    fn remember_current(&mut self) {
        self.remembered[self.last_painting_slot] = RememberedBrush {
            size_tenths: self.size_tenths,
            opacity_u16: self.opacity_u16,
            settings: self.brush_settings,
        };
    }

    fn select_tool(&mut self, tool: DrawingTool) {
        self.remember_current();
        self.tool = tool;
        if let Some(mut slot) = Self::painting_slot(tool) {
            if tool == DrawingTool::Pencil
                && self.pencil_template == nyatidraw_api::PencilTemplate::Graphite2B
            {
                slot = 4;
            }
            let brush = self.remembered[slot];
            self.last_painting_slot = slot;
            self.size_tenths = brush.size_tenths;
            self.opacity_u16 = brush.opacity_u16;
            self.brush_settings = brush.settings;
        }
    }

    fn preset(self) -> BrushPreset {
        if self.tool == DrawingTool::Pencil {
            let kind = match self.pencil_template {
                nyatidraw_api::PencilTemplate::Mechanical2H => {
                    nyatidraw_brush::PencilKind::Mechanical2H
                }
                nyatidraw_api::PencilTemplate::Graphite2B => {
                    nyatidraw_brush::PencilKind::Graphite2B
                }
            };
            return BrushPreset {
                size_px: f32::from(self.size_tenths) / 10.0,
                opacity: f32::from(self.opacity_u16) / f32::from(u16::MAX),
                size_pressure: self.brush_settings.size_pressure,
                opacity_pressure: self.brush_settings.opacity_pressure,
                size_min_ratio: f32::from(self.brush_settings.size_minimum_u16)
                    / f32::from(u16::MAX),
                opacity_min_ratio: f32::from(self.brush_settings.opacity_minimum_u16)
                    / f32::from(u16::MAX),
                ..nyatidraw_brush::pencil_preset(kind)
            };
        }
        let (id, flow, spacing_ratio) = match self.tool {
            DrawingTool::Pencil => (BrushPresetId(2), 0.82, 0.08),
            DrawingTool::Pen => (BrushPresetId(3), 1.0, 0.08),
            DrawingTool::Eraser => (BrushPresetId(4), 1.0, 0.08),
            _ => (BrushPresetId(1), 0.38, 0.12),
        };
        BrushPreset {
            id,
            size_px: f32::from(self.size_tenths) / 10.0,
            opacity: f32::from(self.opacity_u16) / f32::from(u16::MAX),
            flow,
            spacing_ratio,
            size_pressure: self.brush_settings.size_pressure,
            opacity_pressure: self.brush_settings.opacity_pressure,
            size_min_ratio: f32::from(self.brush_settings.size_minimum_u16) / f32::from(u16::MAX),
            opacity_min_ratio: f32::from(self.brush_settings.opacity_minimum_u16)
                / f32::from(u16::MAX),
            hardness: f32::from(self.brush_settings.hardness_u16) / f32::from(u16::MAX),
            ..LIVE_BRUSH
        }
    }

    /// A flipped pen uses the remembered eraser, without changing the UI tool
    /// or overwriting the selected brush's settings for the next painted stroke.
    fn preset_for_stroke(mut self, hardware_eraser: bool) -> BrushPreset {
        if hardware_eraser && self.tool != DrawingTool::Eraser {
            self.select_tool(DrawingTool::Eraser);
        }
        self.preset()
    }

    fn smoothing_for_stroke(mut self, hardware_eraser: bool) -> u8 {
        if hardware_eraser && self.tool != DrawingTool::Eraser {
            self.select_tool(DrawingTool::Eraser);
        }
        self.brush_settings.smoothing
    }

    fn stroke_color(self) -> StrokeColor {
        StrokeColor(nyatidraw_tiles::color::srgb8_to_linear_premultiplied(
            self.color,
        ))
    }

    fn gpu_color(self) -> [f32; 4] {
        let color = self.stroke_color().0;
        let alpha = f32::from(color[3]);
        if color[3] == 0 {
            return [0.0; 4];
        }
        // Match the canonical CPU color, including premultiplication rounding.
        [
            f32::from(color[0]) / alpha,
            f32::from(color[1]) / alpha,
            f32::from(color[2]) / alpha,
            alpha / 255.0,
        ]
    }
}

/// The storage identity is selected exactly once for each canvas runtime.
/// In particular, an untitled document must not choose a fresh store whenever
/// its presentation surface suspends and resumes.
#[derive(Clone, Debug)]
pub(crate) enum ProjectLocation {
    Explicit {
        path: PathBuf,
        bootstrap_png: Option<PathBuf>,
    },
    UntitledRecovery(PathBuf),
}

impl ProjectLocation {
    fn from_environment_or_args() -> Self {
        if let Some(path) = std::env::args_os().nth(1).map(PathBuf::from) {
            match Self::from_positional_path(path) {
                Ok(Some(location)) => return location,
                Ok(None) => {}
                Err(error) => eprintln!("native-canvas event=activation-ignored error={error}"),
            }
        }
        if let Some(path) = std::env::var_os(PROJECT_PATH_ENV) {
            return Self::Explicit {
                path: absolute_activation_path(PathBuf::from(path)),
                bootstrap_png: None,
            };
        }

        // A normal installed launch must reopen its scratchbook on the next
        // launch, and Save must have a visible PNG destination.
        #[cfg(windows)]
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let directory = PathBuf::from(local).join("NyatiDraw").join("Sketchbook");
            if let Err(error) = std::fs::create_dir_all(&directory) {
                // Keep this destination so the ordinary project-open error is
                // visible; do not silently fall back to a disposable document.
                eprintln!("native-canvas event=sketchbook-directory-failed error={error}");
            }
            return Self::Explicit {
                path: directory.join("작업 중.ntdr"),
                bootstrap_png: None,
            };
        }

        let sequence = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-untitled-recovery-{}-{sequence}.ntdr",
            std::process::id(),
        ));
        println!(
            "native-canvas event=untitled-recovery-configured path={} lifetime=canvas-runtime retention=process-exit-no-cleanup",
            path.display(),
        );
        Self::UntitledRecovery(path)
    }

    /// Resolves exactly the positional `.png`/`.ntdr` activation contract.
    /// The only filesystem decision is whether an exact sibling project is
    /// missing/empty; permission and metadata failures are not mistaken for a
    /// missing project and therefore cannot bootstrap over user data.
    pub(crate) fn from_positional_path(path: PathBuf) -> Result<Option<Self>, String> {
        let path = absolute_activation_path(path);
        let extension = path
            .extension()
            .map(|value| value.to_string_lossy().to_ascii_lowercase());
        match extension.as_deref() {
            Some("ntdr") => Ok(Some(Self::Explicit {
                path,
                bootstrap_png: None,
            })),
            Some("png") => {
                let project = path.with_extension("ntdr");
                let bootstrap_png = match std::fs::metadata(&project) {
                    Ok(metadata) if metadata.len() > 0 => None,
                    Ok(_) => Some(path),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(path),
                    Err(error) => {
                        return Err(format!(
                            "activation metadata path={}: error={error}",
                            project.display()
                        ));
                    }
                };
                Ok(Some(Self::Explicit {
                    path: project,
                    bootstrap_png,
                }))
            }
            _ => {
                eprintln!(
                    "native-canvas event=activation-ignored path={} reason=unsupported-extension expected=png-or-ntdr",
                    path.display(),
                );
                Ok(None)
            }
        }
    }

    fn same_storage(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Explicit { path: left, .. }, Self::Explicit { path: right, .. })
            | (Self::UntitledRecovery(left), Self::UntitledRecovery(right)) => left == right,
            _ => false,
        }
    }

    fn title(&self) -> String {
        match self {
            Self::Explicit { path, .. } => path.file_stem().map_or_else(
                || "Untitled".to_owned(),
                |stem| stem.to_string_lossy().into_owned(),
            ),
            Self::UntitledRecovery(_) => "Untitled (Recovery)".to_owned(),
        }
    }

    fn export_path(&self) -> Option<PathBuf> {
        match self {
            Self::Explicit { path, .. } => Some(path.with_extension("png")),
            Self::UntitledRecovery(_) => None,
        }
    }
}

/// Validates an incoming target while the current document is still live.
/// It never creates a missing/empty `.ntdr`; the real open does so only after
/// the current writer has been drained and released. This keeps invalid pair
/// activation from replacing the current document.
fn preflight_project_activation(location: &ProjectLocation) -> Result<(), String> {
    let (path, bootstrap_png) = match location {
        ProjectLocation::Explicit {
            path,
            bootstrap_png,
        } => (path, bootstrap_png.as_ref()),
        ProjectLocation::UntitledRecovery(_) => {
            return Err("untitled recovery is not an external activation target".into());
        }
    };
    if let Some(source) = bootstrap_png {
        nyatidraw_png_io::decode_png(source, LIVE_LAYER)
            .map_err(|error| format!("png activation decode path={}: {error}", source.display()))?;
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > 0 => {
            let db = ProjectDb::open(path).map_err(|error| {
                format!(
                    "project activation preflight path={}: {error}: invalid-non-empty-preserved=true",
                    path.display()
                )
            })?;
            db.load_layer_tree().map_err(|error| {
                format!("activation layer tree path={}: {error}", path.display())
            })?;
            db.load_canvas_spec()
                .map_err(|error| format!("activation canvas path={}: {error}", path.display()))?;
            db.load_reopened()
                .map_err(|error| format!("activation history path={}: {error}", path.display()))?;
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "activation project metadata path={}: {error}",
            path.display()
        )),
    }
}

fn absolute_activation_path(path: PathBuf) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(&path) {
        return canonical;
    }
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or(path.clone(), |directory| directory.join(path))
    }
}

type SaveAsWorker = (JoinHandle<()>, Receiver<Result<PathBuf, String>>);

struct AdmissionGuard(LiveInkBridge);

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        self.0.finish_save_as();
    }
}

pub(crate) struct SharedGpuCanvas {
    live_ink: LiveInkBridge,
    project_location: ProjectLocation,
    state: CanvasState,
    rendered_frames: u64,
    geometry_epoch: u64,
    close_worker: Option<JoinHandle<()>>,
    save_as_worker: Option<SaveAsWorker>,
    performance_probe: Option<crate::performance_probe::PerformanceProbe>,
}

impl SharedGpuCanvas {
    pub(crate) fn set_geometry_epoch(&mut self, epoch: u64) {
        self.geometry_epoch = epoch;
    }

    pub(crate) fn new(live_ink: LiveInkBridge) -> Self {
        let project_location = ProjectLocation::from_environment_or_args();
        #[cfg(windows)]
        match &project_location {
            ProjectLocation::Explicit { path, .. } | ProjectLocation::UntitledRecovery(path) => {
                crate::updates::set_project_path(path.clone());
            }
        }
        Self {
            live_ink,
            project_location,
            state: CanvasState::Suspended,
            rendered_frames: 0,
            geometry_epoch: 0,
            close_worker: None,
            save_as_worker: None,
            performance_probe: None,
        }
    }

    /// Creates the renderer-owned scene against a presentation host's device.
    ///
    /// The caller owns the instance, adapter, device, queue, and surface. This
    /// keeps the editor/durability runtime independent from a particular UI
    /// compositor, including Dioxus Native's former custom-paint callback.
    pub(crate) fn resume(
        &mut self,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let adapter = adapter.get_info();
        println!(
            "native-shell event=gpu-canvas-resume elapsed_ms={} backend={:?} adapter={:?} device_type={:?}",
            elapsed_since_launch(),
            adapter.backend,
            adapter.name,
            adapter.device_type,
        );
        self.state = match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location)
        {
            Ok(canvas) => CanvasState::Active(Box::new(canvas)),
            Err(error) => {
                eprintln!(
                    "native-canvas event=canvas-startup-failed persistence=fatal error={error}"
                );
                self.live_ink.publish_workspace_error(error);
                CanvasState::Failed
            }
        };
    }

    /// Drops presentation-device state while preserving no UI-framework handles.
    pub(crate) fn suspend(&mut self) {
        // Emergency destruction still joins owned work. Ordinary Close keeps
        // this owner and its window alive until poll_close observes completion.
        if let Some(worker) = self.close_worker.take() {
            let _ = worker.join();
        }
        if let Some((worker, _)) = self.save_as_worker.take() {
            let _ = worker.join();
            self.live_ink.finish_save_as();
        }
        self.live_ink.invalidate_canvas_viewport();
        println!("native-shell event=gpu-canvas-suspend");
        self.state = CanvasState::Suspended;
    }

    pub(crate) fn begin_close(&mut self, width: u32, height: u32, scale: f64) {
        self.performance_probe.take();
        if self.close_worker.is_some() {
            return;
        }
        let state = std::mem::replace(&mut self.state, CanvasState::Suspended);
        let CanvasState::Active(mut canvas) = state else {
            self.publish_close_result();
            return;
        };
        // Honor semantic requests admitted before Close, especially Save.
        // Raw samples stay in their bounded native lane for shutdown drain.
        canvas.apply_commands(width, height, scale);
        match thread::Builder::new()
            .name("nyatidraw-close".into())
            .spawn(move || drop(canvas))
        {
            Ok(worker) => self.close_worker = Some(worker),
            Err(error) => {
                self.live_ink
                    .publish_workspace_error(format!("Close worker could not start: {error}"));
                self.publish_close_result();
            }
        }
        println!("native-canvas event=close-started admission=stopped wait=background");
    }

    pub(crate) fn poll_close(&mut self) {
        if self
            .close_worker
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            if self
                .close_worker
                .take()
                .expect("finished close worker")
                .join()
                .is_err()
            {
                self.live_ink
                    .publish_workspace_error("Close worker panicked before completion".into());
            }
            self.publish_close_result();
        }
    }

    fn publish_close_result(&self) {
        let project_saved = !self.live_ink.workspace_failed();
        let export_failed = matches!(
            self.live_ink.export_status_snapshot(),
            ExportStatus::Failed { .. }
        );
        if !project_saved || export_failed {
            let path = match &self.project_location {
                ProjectLocation::Explicit { path, .. }
                | ProjectLocation::UntitledRecovery(path) => path,
            };
            self.live_ink.publish_close_status(CloseStatus::Failed {
                project_saved,
                project_path: path.display().to_string(),
            });
            println!(
                "native-canvas event=close-failed project_saved={project_saved} export_failed={export_failed} window=retained"
            );
        } else {
            self.live_ink.publish_close_status(CloseStatus::Ready);
            println!("native-canvas event=close-ready writer=joined");
        }
    }

    pub(crate) fn reopen_after_close_failure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if !matches!(self.live_ink.close_status(), CloseStatus::Failed { .. })
            || self.close_worker.is_some()
        {
            return;
        }
        self.live_ink.reopen_after_close_failure();
        match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location) {
            Ok(canvas) => self.state = CanvasState::Active(Box::new(canvas)),
            Err(error) => {
                self.live_ink.publish_workspace_error(error);
                self.live_ink.begin_close();
                self.publish_close_result();
            }
        }
    }

    pub(crate) fn save_as(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: PathBuf,
        size: [u32; 2],
        scale: f64,
    ) -> Result<(), String> {
        crate::save_as::validate_target(&path)?;
        if self.save_as_worker.is_some() || self.live_ink.workspace_failed() {
            return Err("저장 중이거나 원본 저장에 실패했습니다".into());
        }
        let CanvasState::Active(canvas) = &mut self.state else {
            return Err("캔버스가 준비되지 않았습니다".into());
        };
        // Drain admitted native samples before same-frame semantic requests.
        canvas.render(size[0], size[1], scale, self.geometry_epoch);
        if canvas.stroke.transform_projection.is_some()
            || canvas.stroke.transform_starting
            || canvas
                .artwork_job
                .as_ref()
                .is_some_and(|job| job.transform.is_some())
        {
            return Err("변형을 확정하거나 취소한 뒤 저장하세요.".into());
        }
        let state = std::mem::replace(&mut self.state, CanvasState::Suspended);
        self.performance_probe.take();
        let source = match &self.project_location {
            ProjectLocation::Explicit { path, .. } | ProjectLocation::UntitledRecovery(path) => {
                path.clone()
            }
        };
        let live_ink = self.live_ink.clone();
        let (sent, received) = sync_channel(1);
        let worker = thread::Builder::new()
            .name("nyatidraw-save-as".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    drop(state); // Joins artwork, durable writer, and independent PNG work.
                    if live_ink.workspace_failed() {
                        Err("원본 프로젝트 저장에 실패해 다른 이름으로 저장을 중단했습니다".into())
                    } else {
                        crate::save_as::copy_closed_project(&source, &path).map(|()| path)
                    }
                }))
                .unwrap_or_else(|_| {
                    let error =
                        "저장 작업이 예기치 않게 종료됐습니다. 원본을 확인해주세요".to_owned();
                    live_ink.publish_workspace_error(error.clone());
                    Err(error)
                });
                let _ = sent.send(result);
                live_ink.request_redraw();
            });
        let worker = match worker {
            Ok(worker) => worker,
            Err(error) => {
                if !self.live_ink.workspace_failed() {
                    let previous_export = self.live_ink.export_status_snapshot();
                    self.live_ink.reset_for_project_activation();
                    self.live_ink
                        .restore_export_failure_after_reopen(previous_export);
                    match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location) {
                        Ok(canvas) => self.state = CanvasState::Active(Box::new(canvas)),
                        Err(error) => {
                            self.live_ink.publish_workspace_error(error);
                            self.state = CanvasState::Failed;
                        }
                    }
                }
                return Err(format!("저장 작업 시작 실패: {error}"));
            }
        };
        self.save_as_worker = Some((worker, received));
        Ok(())
    }

    pub(crate) fn poll_save_as(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some((_, received)) = &self.save_as_worker else {
            return;
        };
        let result = match received.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("저장 작업이 예기치 않게 종료됐습니다".into())
            }
        };
        if let Some((worker, _)) = self.save_as_worker.take() {
            let _ = worker.join();
        }
        let previous = self.project_location.clone();
        // The source export has finished by this join. Keep its failure if
        // copy/open fails and the source remains the active project.
        let previous_export = self.live_ink.export_status_snapshot();
        // Never clear a failed durable-write latch or claim the new document.
        if self.live_ink.workspace_failed() {
            self.state = CanvasState::Failed;
            self.live_ink.finish_save_as();
            self.live_ink
                .publish_activation_notice(result.err().unwrap_or_else(|| "원본 저장 실패".into()));
            return;
        }
        let (target, mut failure) = match result {
            Ok(path) => (
                ProjectLocation::Explicit {
                    path,
                    bootstrap_png: None,
                },
                None,
            ),
            Err(error) => (previous.clone(), Some(error)),
        };
        self.live_ink.reset_for_project_activation();
        match ActiveCanvas::new(device, queue, &self.live_ink, &target) {
            Ok(canvas) => {
                self.state = CanvasState::Active(Box::new(canvas));
                self.project_location = target;
            }
            Err(error) => {
                failure = Some(format!("복사본 열기 실패 (완료된 파일 유지): {error}"));
                self.live_ink.reset_for_project_activation();
                match ActiveCanvas::new(device, queue, &self.live_ink, &previous) {
                    Ok(canvas) => self.state = CanvasState::Active(Box::new(canvas)),
                    Err(error) => {
                        self.state = CanvasState::Failed;
                        self.live_ink
                            .publish_workspace_error(format!("원본 다시 열기 실패: {error}"));
                    }
                }
            }
        }
        if self.project_location.same_storage(&previous) {
            self.live_ink
                .restore_export_failure_after_reopen(previous_export);
        }
        #[cfg(windows)]
        if let ProjectLocation::Explicit { path, .. } = &self.project_location {
            crate::updates::set_project_path(path.clone());
        }
        self.live_ink.finish_save_as();
        if let Some(error) = failure {
            self.live_ink.publish_activation_notice(error);
        }
    }

    /// Replaces the durable document only on the child HWND's UI thread.
    /// Dropping the old active canvas synchronously drains its bounded writer
    /// FIFO and joins export work before its redb lock is released. A target
    /// is preflighted first, and a failed post-drain open restores the prior
    /// project rather than leaving the primary without a document.
    pub(crate) fn activate_project(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: PathBuf,
        size: [u32; 2],
        scale: f64,
    ) -> Result<(), String> {
        if self.live_ink.is_saving_as() || self.live_ink.workspace_failed() {
            return Err("저장 작업 또는 원본 저장 오류를 먼저 확인해주세요".into());
        }
        let target = ProjectLocation::from_positional_path(path)?
            .ok_or_else(|| "unsupported activation extension".to_owned())?;
        if self.project_location.same_storage(&target) {
            println!("native-canvas event=activation-reused-current-project");
            return Ok(());
        }
        preflight_project_activation(&target)?;
        // Reuse the same atomic admission boundary as Save As so neither lane
        // can append behind the final render/drain and disappear on reset.
        let target_path = match &target {
            ProjectLocation::Explicit { path, .. } | ProjectLocation::UntitledRecovery(path) => {
                path.clone()
            }
        };
        self.live_ink.queue_save_as(target_path)?;
        self.live_ink.take_save_as();
        let _admission = AdmissionGuard(self.live_ink.clone());
        if let CanvasState::Active(canvas) = &mut self.state {
            canvas.render(size[0], size[1], scale, self.geometry_epoch);
        }
        self.performance_probe.take();
        let previous = self.project_location.clone();
        println!(
            "native-canvas event=activation-quiescing current={} target={}",
            previous.title(),
            target.title(),
        );
        // Assignment drops `ActiveCanvas` before accepting any target writer.
        self.state = CanvasState::Suspended;
        if self.live_ink.workspace_failed() {
            self.state = CanvasState::Failed;
            return Err("원본 프로젝트 저장에 실패했습니다. 원본 경로와 오류를 유지합니다".into());
        }
        if matches!(
            self.live_ink.export_status_snapshot(),
            ExportStatus::Failed { .. }
        ) {
            // Preserve both the source identity and its independent PNG error.
            self.live_ink
                .restore_export_failure_after_reopen(self.live_ink.export_status_snapshot());
            match ActiveCanvas::new(device, queue, &self.live_ink, &previous) {
                Ok(canvas) => self.state = CanvasState::Active(Box::new(canvas)),
                Err(error) => {
                    self.live_ink.publish_workspace_error(error);
                    self.state = CanvasState::Failed;
                }
            }
            return Err(
                "원본 PNG 저장에 실패했습니다. 재시도하거나 다른 이름으로 저장해주세요".into(),
            );
        }
        self.live_ink.reset_for_project_activation();
        self.project_location = target;
        match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location) {
            Ok(canvas) => {
                self.state = CanvasState::Active(Box::new(canvas));
                #[cfg(windows)]
                if let ProjectLocation::Explicit { path, .. } = &self.project_location {
                    crate::updates::set_project_path(path.clone());
                }
                println!(
                    "native-canvas event=activation-opened project={}",
                    self.project_location.title()
                );
                Ok(())
            }
            Err(error) => {
                eprintln!(
                    "native-canvas event=activation-open-failed target={} error={} recovery=previous-project",
                    self.project_location.title(),
                    error
                );
                self.live_ink.reset_for_project_activation();
                self.project_location = previous;
                match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location) {
                    Ok(canvas) => {
                        self.state = CanvasState::Active(Box::new(canvas));
                        Err(error)
                    }
                    Err(recovery_error) => {
                        let summary = format!(
                            "Activation failed ({error}); reopening the previous project also failed ({recovery_error})"
                        );
                        self.live_ink.publish_workspace_error(summary.clone());
                        self.state = CanvasState::Failed;
                        Err(summary)
                    }
                }
            }
        }
    }

    /// Advances the renderer and returns its disposable RGBA display texture.
    ///
    /// A child-HWND host presents the returned texture to its own surface. The
    /// origin is intentionally `(0, 0)`: Win32 input delivered to that child is
    /// relative to the child, not to the `WebView` parent.
    pub(crate) fn render(&mut self, width: u32, height: u32, scale: f64) -> Option<RenderedCanvas> {
        let CanvasState::Active(canvas) = &mut self.state else {
            return None;
        };
        let display = canvas.render(width, height, scale, self.geometry_epoch)?;
        self.rendered_frames = self.rendered_frames.saturating_add(1);

        if self.rendered_frames == 1 {
            if let ProjectLocation::Explicit { path, .. } = &self.project_location {
                self.performance_probe =
                    crate::performance_probe::PerformanceProbe::start(&self.live_ink, path);
            }
            println!(
                "native-shell event=first-gpu-canvas-submit elapsed_ms={} width={} height={} scale={scale}",
                elapsed_since_launch(),
                width,
                height,
            );
        }

        Some(display)
    }
}

pub(crate) struct RenderedCanvas {
    pub(crate) texture: wgpu::Texture,
    pub(crate) history_timing: Option<HistoryTiming>,
    pub(crate) viewport: Option<ViewportTransform>,
    pub(crate) geometry_epoch: u64,
}

enum CanvasState {
    Active(Box<ActiveCanvas>),
    Suspended,
    Failed,
}

struct ActiveCanvas {
    painter: GpuRoundDabPainter,
    alpha_painter: GpuRoundDabPainter,
    eraser: GpuRoundDabPainter,
    background_painter: GpuRoundDabPainter,
    scene: GpuCompositeScene,
    stroke: StrokePipeline,
    commands: Vec<crate::live_ink::CanvasCommand>,
    view: RendererView,
    active_layer: LayerId,
    next_layer_node_id: u128,
    project_title: String,
    project_export_path: Option<PathBuf>,
    pending_initial_viewport: Option<ViewportCommand>,
    pending_save: Option<u64>,
    artwork_job: Option<ArtworkJob>,
    selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    canvas_spec: CanvasSpec,
    drawing: DrawingConfig,
    projection: ProjectionState,
    cpu_tiles: BTreeMap<TileKey, Vec<u8>>,
    transform_display: Option<TileSnapshot>,
    latest_preview_generation: BTreeMap<TileKey, u64>,
    deferred_uploads: Vec<DeferredTileUpload>,
    gpu_live_generation: Option<(u64, LiveStrokeToken, bool, bool)>,
    async_publication_base_revision: Option<Revision>,
    close_raw_probe_staged: bool,
    save_probe_step: u8,
    protocol_probe_step: Option<u8>,
    history_probe: Option<(EditorCommand, bool)>,
    edit_probe_step: Option<u8>,
    input_safety_probe_step: Option<u8>,
    synthetic_seed_enabled: bool,
    synthetic_seed_submitted: bool,
    navigator_viewport: Option<NavigatorViewport>,
    gpu_batches: u64,
    gpu_batch_logs_remaining: u8,
}

impl ActiveCanvas {
    #[allow(clippy::too_many_lines)]
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        live_ink: &LiveInkBridge,
        project_location: &ProjectLocation,
    ) -> Result<Self, String> {
        let materialization = MaterializationWorker::start(live_ink.clone(), project_location)?;
        let tree = materialization.initial_tree.clone();
        let canvas_spec = materialization.initial_canvas;
        let initial_history = materialization.initial_history.clone();
        let document_size = [canvas_spec.width_px, canvas_spec.height_px];
        let mut scene = GpuCompositeScene::new(device, queue, document_size, tree.clone())
            .map_err(|error| format!("layer-scene:{error:?}"))?;
        let reopened_root = materialization.initial_tiles.root();
        let reopened_tile_count = materialization.initial_tiles.len();
        for (key, tile) in materialization.initial_tiles.iter() {
            scene
                .upload_closed_tile(key, tile.pixels())
                .map_err(|error| format!("reopened-tile:{key:?}:{error:?}"))?;
        }
        println!(
            "native-canvas event=reopened-tiles-uploaded root={:032x} tiles={} source=cpu-authoritative-before-first-render",
            reopened_root.id.0, reopened_tile_count,
        );

        let mut cpu_tiles = BTreeMap::new();
        for (key, tile) in materialization.initial_tiles.iter() {
            cpu_tiles.insert(key, tile.pixels().to_vec());
        }
        let stroke = StrokePipeline::new(live_ink.clone(), materialization.worker);
        if std::env::var_os(DURABILITY_PROBE_ENV).is_some() {
            enqueue_synthetic_durability_probe(live_ink)?;
        }
        if std::env::var_os(ACTIVE_CLOSE_PROBE_ENV).is_some()
            && !ACTIVE_PROBE_USED.swap(true, Ordering::Relaxed)
        {
            for (sequence, phase) in [(1, PointerPhase::Begin), (2, PointerPhase::Move)] {
                live_ink
                    .push(input_probe_sample(sequence, phase))
                    .map_err(|error| format!("active-close-probe-input:{error:?}"))?;
            }
        }

        let (mut authoritative, latest_event) = live_ink.protocol_snapshot();
        authoritative.canvas = canvas_spec;
        let restoring_projection = latest_event.is_some();
        let mut projection = ProjectionState::new(DockTree::safe_default());
        let (view, active_layer) = if restoring_projection {
            let active_layer = authoritative
                .active_layer
                .filter(|layer| {
                    tree.ancestors(nyatidraw_api::LayerTreeNodeId::Raster(*layer))
                        .is_some()
                })
                .unwrap_or_else(|| valid_active_layer(&tree, LIVE_LAYER));
            let view = RendererView::from_projection(authoritative.viewport);
            println!(
                "native-canvas event=projection-restored revision={} source=protocol-mailbox",
                authoritative.revision.0,
            );
            projection.install_authoritative(authoritative);
            projection.stage_history(initial_history.clone());
            projection.stage_edit(nyatidraw_api::EditProjection::default());
            live_ink.resume_after_edit();
            (view, active_layer)
        } else {
            // Project activation clears semantic document fields but preserves
            // this process's monotonic revision. Start the incoming document
            // from that baseline so Dioxus never observes a revision rollback.
            projection.install_authoritative(authoritative);
            projection.stage_history(initial_history);
            let session_dock = projection.current().dock.clone();
            projection
                .publish_editor_state(
                    project_location.title(),
                    false,
                    Some(valid_active_layer(&tree, LIVE_LAYER)),
                    &tree,
                    session_dock,
                    ViewportProjection {
                        zoom_ppm: 1_000_000,
                        ..ViewportProjection::default()
                    },
                )
                .map_err(|error| format!("initial-projection:{error:?}"))?;
            live_ink.publish_editor_event(
                EditorEvent::ProjectionChanged {
                    revision: projection.current().revision,
                },
                Some(projection.current().clone()),
            );
            (
                RendererView::default(),
                valid_active_layer(&tree, LIVE_LAYER),
            )
        };

        let drawing = DrawingConfig::from_projection(projection.current());
        scene
            .set_solo(projection.current().solo_node)
            .map_err(|error| format!("restored-solo:{error:?}"))?;
        let next_layer_node_id = next_layer_node_id(&tree)?;
        live_ink.set_navigation_tool(drawing.tool == DrawingTool::Move);
        let mut canvas = Self {
            painter: GpuRoundDabPainter::new(device, queue, [0.10, 0.78, 0.91, 1.0]),
            alpha_painter: GpuRoundDabPainter::new_alpha_locked(
                device,
                queue,
                [0.10, 0.78, 0.91, 1.0],
            ),
            eraser: GpuRoundDabPainter::new_eraser(device, queue),
            background_painter: GpuRoundDabPainter::new(device, queue, [0.50, 0.20, 0.84, 1.0]),
            scene,
            stroke,
            commands: Vec::with_capacity(64),
            view,
            active_layer,
            next_layer_node_id,
            project_title: project_location.title(),
            project_export_path: project_location.export_path(),
            pending_initial_viewport: (!restoring_projection)
                .then_some(ViewportCommand::FitDocument),
            pending_save: None,
            artwork_job: None,
            selection: None,
            canvas_spec,
            drawing,
            projection,
            cpu_tiles,
            transform_display: None,
            latest_preview_generation: BTreeMap::new(),
            deferred_uploads: Vec::new(),
            gpu_live_generation: None,
            async_publication_base_revision: None,
            close_raw_probe_staged: false,
            save_probe_step: if std::env::var_os(SAVE_PROBE_ENV).is_some()
                && !SAVE_PROBE_USED.swap(true, Ordering::Relaxed)
            {
                0
            } else {
                3
            },
            protocol_probe_step: std::env::var_os(PROTOCOL_PROBE_ENV).map(|_| 0),
            history_probe: history_probe_command().map(|command| (command, false)),
            edit_probe_step: crate::edit_worker::probe_project().map(|_| 0),
            input_safety_probe_step: std::env::var_os(INPUT_SAFETY_PROBE_ENV).map(|_| 0),
            synthetic_seed_enabled: std::env::var_os("NAYATI_SYNTHETIC_INK").is_some(),
            synthetic_seed_submitted: false,
            navigator_viewport: None,
            gpu_batches: 0,
            gpu_batch_logs_remaining: 16,
        };
        if restoring_projection {
            canvas.publish_current_projection();
        } else {
            canvas.enqueue_initial_commands(live_ink);
        }
        live_ink.set_admission_layer(active_layer);
        Ok(canvas)
    }

    fn enqueue_initial_commands(&mut self, live_ink: &LiveInkBridge) {
        let Some(command) = self.pending_initial_viewport else {
            return;
        };
        if live_ink.is_saving_as() || live_ink.is_closing() {
            return;
        }
        let based_on = self.projection.current().revision;
        if live_ink
            .push_editor_command(based_on, EditorCommand::Viewport(command))
            .is_ok()
        {
            self.pending_initial_viewport = None;
            println!("native-canvas event=initial-fit-queued");
        } else if !live_ink.workspace_failed() {
            // Keep one pending initialization request if the bounded lane is
            // full. A later frame retries after the ordinary command drain.
            live_ink.request_redraw();
        }
    }

    fn advance_protocol_probe(&mut self) {
        if self.artwork_job.is_some() {
            return;
        }
        let Some(step) = self.protocol_probe_step else {
            return;
        };
        let revision = self.projection.current().revision;
        let command = match step {
            0 => Some((
                revision,
                EditorCommand::Layer(LayerCommand::SetVisibility {
                    node: nyatidraw_api::LayerTreeNodeId::Group(INK_GROUP),
                    visible: false,
                }),
            )),
            1 => Some((
                revision,
                EditorCommand::Layer(LayerCommand::SetOpacity {
                    node: nyatidraw_api::LayerTreeNodeId::Group(INK_GROUP),
                    opacity_u16: 58_000,
                }),
            )),
            2 => Some((
                revision,
                EditorCommand::Layer(LayerCommand::Reorder {
                    node: nyatidraw_api::LayerTreeNodeId::Group(INK_GROUP),
                    new_parent: ROOT_GROUP,
                    index: 0,
                }),
            )),
            3 => Some((revision, EditorCommand::Layer(LayerCommand::AddRaster))),
            4 => Some((revision, EditorCommand::Layer(LayerCommand::AddGroup))),
            5 => Some((
                revision,
                EditorCommand::Layer(LayerCommand::Rename {
                    node: nyatidraw_api::LayerTreeNodeId::Raster(LayerId(101)),
                    name: "Character".into(),
                }),
            )),
            6 => Some((
                revision,
                EditorCommand::Layer(LayerCommand::ToggleSolo(
                    nyatidraw_api::LayerTreeNodeId::Raster(LayerId(101)),
                )),
            )),
            7 => Some((
                nyatidraw_api::Revision(0),
                EditorCommand::Layer(LayerCommand::SetVisibility {
                    node: nyatidraw_api::LayerTreeNodeId::Group(INK_GROUP),
                    visible: true,
                }),
            )),
            8 => {
                if let Some(layer) = self
                    .projection
                    .current()
                    .layers
                    .iter()
                    .find(|layer| layer.id == nyatidraw_api::LayerTreeNodeId::Group(INK_GROUP))
                {
                    println!(
                        "native-canvas event=protocol-proof-complete revision={} visibility={} opacity={} parent={} layers={} active={} solo={} stale_mutated=false",
                        revision.0,
                        layer.visible,
                        layer.opacity_u16,
                        layer.parent.0,
                        self.projection.current().layers.len(),
                        self.active_layer.0,
                        self.projection.current().solo_node
                            == Some(nyatidraw_api::LayerTreeNodeId::Raster(LayerId(101))),
                    );
                }
                self.protocol_probe_step = None;
                None
            }
            _ => None,
        };
        if let Some((based_on, command)) = command
            && self
                .stroke
                .live_ink
                .push_editor_command(based_on, command)
                .is_ok()
        {
            self.protocol_probe_step = Some(step.saturating_add(1));
        }
    }

    fn advance_history_probe(&mut self) {
        if self.artwork_job.is_some() {
            return;
        }
        let Some((command, submitted)) = self.history_probe.clone() else {
            return;
        };
        if submitted {
            let history = &self.projection.current().history;
            let candidates: Vec<_> = history
                .redo_branches
                .iter()
                .map(|branch| branch.node.0)
                .collect();
            println!(
                "native-canvas event=history-probe-complete candidates={candidates:?} after={:?} more={} active={} layers={} active_valid={} reference_count={} semantic_only=true",
                history.redo_page_after.map(|id| id.0),
                history.has_more_redo_branches,
                self.active_layer.0,
                self.projection.current().layers.len(),
                self.scene
                    .tree()
                    .ancestors(nyatidraw_api::LayerTreeNodeId::Raster(self.active_layer))
                    .is_some(),
                self.projection
                    .current()
                    .layers
                    .iter()
                    .filter(|layer| layer.reference)
                    .count(),
            );
            self.history_probe = None;
        } else if self
            .stroke
            .live_ink
            .push_editor_command(self.projection.current().revision, command.clone())
            .is_ok()
        {
            self.history_probe = Some((command, true));
        }
    }

    fn advance_edit_probe(&mut self) {
        use nyatidraw_api::{EditCommand, EditSource};
        let Some(step) = self.edit_probe_step else {
            return;
        };
        if std::env::var("NAYATI_EDIT_PROBE").ok().as_deref() == Some("close-native-fill") {
            self.advance_close_edit_probe(step);
            return;
        }
        if std::env::var("NAYATI_EDIT_PROBE").ok().as_deref() == Some("lasso-guide") {
            self.advance_lasso_guide_probe(step);
            return;
        }
        if std::env::var("NAYATI_EDIT_PROBE").ok().as_deref() == Some("selected-brush") {
            if step == 0 {
                let command = EditorCommand::Edit(EditCommand::SelectLasso {
                    vertices: vec![[0, 0], [64, 0], [64, 65], [0, 65]],
                });
                if self
                    .stroke
                    .live_ink
                    .push_editor_command(self.projection.current().revision, command)
                    .is_ok()
                {
                    self.edit_probe_step = Some(1);
                }
            } else if self.artwork_job.is_none() && self.selection.is_some() {
                println!(
                    "native-canvas event=selected-brush-probe-ready selected_pixels={} input=native-only",
                    self.projection.current().edit.selected_pixels
                );
                self.edit_probe_step = None;
            }
            return;
        }
        let command = match step {
            0 => EditorCommand::Edit(EditCommand::SelectWand {
                seed: [0, 0],
                tolerance: 0,
                source: EditSource::ActiveLayer,
            }),
            1 if self.artwork_job.is_none() && self.selection.is_some() => {
                EditorCommand::Edit(EditCommand::FillSelection {
                    color: [0, 255, 0, 255],
                })
            }
            2 if self.artwork_job.is_some() => {
                for (sequence, phase) in
                    [(80_001, PointerPhase::Begin), (80_002, PointerPhase::End)]
                {
                    assert_eq!(
                        self.stroke
                            .live_ink
                            .push(input_probe_sample(sequence, phase)),
                        Err(nyatidraw_input_queue::PushError::AdmissionPaused)
                    );
                }
                println!(
                    "native-canvas event=edit-probe-busy raw_gesture=not-admitted save=next-frame"
                );
                EditorCommand::Project(ProjectCommand::Save)
            }
            3 if self.pending_save.is_some() && self.artwork_job.is_some() => {
                println!("native-canvas event=edit-probe-save-retained worker_busy=true");
                self.edit_probe_step = None;
                return;
            }
            _ => return,
        };
        if self
            .stroke
            .live_ink
            .push_editor_command(self.projection.current().revision, command)
            .is_ok()
        {
            self.edit_probe_step = Some(step + 1);
        }
    }

    fn advance_close_edit_probe(&mut self, step: u8) {
        if step == 0 {
            if self
                .stroke
                .live_ink
                .push_editor_command(
                    self.projection.current().revision,
                    EditorCommand::Tool(ToolCommand::Select(DrawingTool::Fill)),
                )
                .is_ok()
            {
                self.edit_probe_step = Some(1);
            }
        } else if step == 1 && self.drawing.tool == DrawingTool::Fill {
            for (sequence, phase) in [(20_000, PointerPhase::Begin), (20_001, PointerPhase::End)] {
                let mut sample = input_probe_sample(sequence, phase);
                sample.position_document = Point { x: 20.0, y: 20.0 };
                sample.viewport_revision = self.stroke.live_ink.canvas_viewport_snapshot().revision;
                if self.stroke.live_ink.push(sample).is_err() {
                    return;
                }
            }
            self.edit_probe_step = Some(2);
            println!(
                "native-canvas event=close-edit-probe-staged input=synthetic raw_samples=2 tool=fill before-next-drain=true"
            );
        }
    }

    fn advance_lasso_guide_probe(&mut self, step: u8) {
        let Some(project) = crate::edit_worker::probe_project() else {
            return;
        };
        if step == 0 {
            if self
                .stroke
                .live_ink
                .push_editor_command(
                    self.projection.current().revision,
                    EditorCommand::Tool(ToolCommand::Select(DrawingTool::Lasso)),
                )
                .is_ok()
            {
                self.edit_probe_step = Some(1);
            }
            return;
        }
        if self.drawing.tool != DrawingTool::Lasso {
            return;
        }
        if step == 5
            && !project
                .parent()
                .is_some_and(|parent| parent.join(".gesture-release").is_file())
        {
            return;
        }
        let (phase, point) = match step {
            1 => (PointerPhase::Begin, [10.0, 10.0]),
            2 => (PointerPhase::Move, [100.0, 10.0]),
            3 => (PointerPhase::Move, [100.0, 55.0]),
            4 => (PointerPhase::Move, [10.0, 55.0]),
            5 => (PointerPhase::End, [10.0, 10.0]),
            _ => return,
        };
        let mut sample = input_probe_sample(10_000 + u64::from(step), phase);
        sample.position_document = Point {
            x: point[0],
            y: point[1],
        };
        sample.viewport_revision = self.stroke.live_ink.canvas_viewport_snapshot().revision;
        if self.stroke.live_ink.push(sample).is_ok() {
            self.edit_probe_step = (step < 5).then_some(step + 1);
            if step == 4 {
                println!(
                    "native-canvas event=lasso-guide-probe-held vertices=4 input=synthetic physical-pen-proof=false"
                );
            }
        }
    }

    fn advance_input_safety_probe_before_drain(&mut self) -> bool {
        if self.input_safety_probe_step != Some(0) {
            return false;
        }
        let started = Instant::now();
        let capacity = self.stroke.live_ink.primary_input_capacity();
        let preserved = capacity.saturating_mul(2);
        for sequence in 1..u64::try_from(preserved).expect("probe capacity fits u64") {
            self.stroke
                .live_ink
                .push(input_probe_sample(sequence, PointerPhase::Cancel))
                .expect("the bounded primary and retry lanes preserve the first 2C transitions");
        }
        let begin_sequence = u64::try_from(preserved).expect("probe capacity fits u64");
        self.stroke
            .live_ink
            .push(input_probe_sample(begin_sequence, PointerPhase::Begin))
            .expect("the final preserved transition fits the retry lane");
        let lost_sequence = begin_sequence.saturating_add(1);
        assert!(
            self.stroke
                .live_ink
                .push(input_probe_sample(lost_sequence, PointerPhase::End))
                .is_err(),
            "transition 2C+1 must latch a discontinuity"
        );
        for (offset, phase) in [(1, PointerPhase::Move), (2, PointerPhase::End)] {
            assert!(
                self.stroke
                    .live_ink
                    .push(input_probe_sample(lost_sequence + offset, phase))
                    .is_err(),
                "samples after a discontinuity stay quarantined until consumer acknowledgement"
            );
        }
        let elapsed_us = started.elapsed().as_micros();
        assert!(
            elapsed_us < 1_000_000,
            "bounded probe admission took over one second"
        );
        let revision = self.projection.current().revision;
        self.stroke
            .live_ink
            .push_editor_command(
                revision,
                EditorCommand::Layer(LayerCommand::SetActive(BACKGROUND_LAYER)),
            )
            .expect("input safety probe command lane has capacity");
        self.input_safety_probe_step = Some(1);
        println!(
            "native-canvas event=input-safety-probe-staged capacity={capacity} preserved={preserved} first_lost_sequence={lost_sequence} elapsed_us={elapsed_us} render_deferred_for_set-active=true"
        );
        true
    }

    fn advance_input_safety_probe_after_drain(&mut self) {
        match self.input_safety_probe_step {
            Some(1) if self.stroke.input_discontinuities == 1 => {
                assert_eq!(self.stroke.last_discontinued_layer, Some(LIVE_LAYER));
                assert!(self.stroke.active.is_none());
                assert!(self.stroke.awaiting_clean_begin);
                assert_eq!(self.active_layer, BACKGROUND_LAYER);
                let base = u64::try_from(self.stroke.live_ink.maximum_drain_len())
                    .expect("probe capacity fits u64")
                    .saturating_add(16);
                for (offset, phase) in [
                    (0, PointerPhase::Move),
                    (1, PointerPhase::End),
                    (2, PointerPhase::Begin),
                    (3, PointerPhase::Move),
                    (4, PointerPhase::End),
                ] {
                    self.stroke
                        .live_ink
                        .push(input_probe_sample(base + offset, phase))
                        .expect("consumer acknowledgement reopens the bounded admission lane");
                }
                self.input_safety_probe_step = Some(2);
            }
            Some(2)
                if self.stroke.closed_strokes == 1
                    && self.stroke.last_closed_layer == Some(BACKGROUND_LAYER) =>
            {
                let stats = self.stroke.live_ink.input_safety_stats();
                assert_eq!(stats.discontinuities_latched, 1);
                assert_eq!(stats.discontinuities_acknowledged, 1);
                assert_eq!(stats.quarantined_after_discontinuity, 2);
                assert_eq!(self.stroke.recovery_quarantined, 2);
                assert!(!self.stroke.awaiting_clean_begin);
                assert!(
                    self.latest_preview_generation
                        .values()
                        .all(|generation| *generation == self.stroke.next_generation - 1),
                    "preview retirement must use the recovered semantic generation, not a device-local token"
                );
                println!(
                    "native-canvas event=input-safety-probe-complete discontinuities={} acknowledged={} producer_quarantined={} consumer_quarantined={} active_cancelled=true cancelled_layer={} recovery_layer={} clean_begin_recovered=true materialized_closed_strokes=1",
                    stats.discontinuities_latched,
                    stats.discontinuities_acknowledged,
                    stats.quarantined_after_discontinuity,
                    self.stroke.recovery_quarantined,
                    LIVE_LAYER.0,
                    BACKGROUND_LAYER.0,
                );
                self.input_safety_probe_step = None;
            }
            Some(_) | None => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn render(
        &mut self,
        width: u32,
        height: u32,
        scale: f64,
        geometry_epoch: u64,
    ) -> Option<RenderedCanvas> {
        if width == 0 || height == 0 {
            return None;
        }
        // Save As and activation construct the replacement while admission
        // remains closed. Queue Fit only after that transaction reopens it.
        if self.pending_initial_viewport.is_some() {
            let live_ink = self.stroke.live_ink.clone();
            self.enqueue_initial_commands(&live_ink);
        }
        self.view.ensure_initialized(
            width,
            height,
            scale,
            [self.canvas_spec.width_px, self.canvas_spec.height_px],
        );
        self.apply_materialized_tiles();

        if self.edit_probe_step == Some(2)
            && std::env::var("NAYATI_EDIT_PROBE").ok().as_deref() == Some("close-native-fill")
        {
            // Scratch-only boundary: keep admitted raw End for the close drain,
            // while allowing the real UI Save request to enter semantic state.
            self.apply_commands(width, height, scale);
            return self.scene.display_texture().map(|texture| RenderedCanvas {
                texture,
                history_timing: None,
                viewport: None,
                geometry_epoch,
            });
        }

        if std::env::var_os(CLOSE_RAW_QUEUE_PROBE_ENV).is_some() {
            if !self.close_raw_probe_staged {
                self.close_raw_probe_staged = true;
                let strokes = if std::env::var_os(ACTIVE_CLOSE_PROBE_ENV).is_some() {
                    1
                } else {
                    32
                };
                println!(
                    "native-canvas event=raw-input-staged-before-drain strokes={strokes} shutdown-drop-must-drain=true"
                );
            }
            return None;
        }

        // Native picker results are adopted after queued Cancel/focus loss and
        // this frame's samples, so a completed read cannot overtake cancellation.
        let history_timing = if self
            .artwork_job
            .as_ref()
            .is_some_and(|job| job.picker_token.is_some())
        {
            None
        } else {
            self.poll_artwork()
        };
        // Native samples already carry the renderer-published document
        // coordinates from their admission moment. Consume their End and put
        // its closed stroke in the writer FIFO before accepting same-frame UI
        // commands, especially Save. This never feeds samples through Dioxus.
        self.stroke.selected_layer_locked = self
            .scene
            .tree()
            .raster(self.active_layer)
            .is_none_or(|layer| layer.locked);
        self.stroke.selected_layer_alpha_locked = self
            .scene
            .tree()
            .raster(self.active_layer)
            .is_some_and(|layer| layer.alpha_locked);
        self.stroke.picker_solo = self.projection.current().solo_node;
        let drained = self.stroke.drain(self.active_layer, self.drawing);
        self.execute_gpu_ops(drained.samples);
        let previous_cancel = self.projection.current().edit.can_cancel;
        self.stage_cancel_availability();
        if previous_cancel != self.projection.current().edit.can_cancel {
            self.async_publication_base_revision
                .get_or_insert(self.projection.current().revision);
            self.publish_current_projection();
        }
        if self.synchronize_workspace_failure() {
            return None;
        }
        // `drain` makes a non-blocking attempt before it consumes raw input.
        // Retry once after End so a newly available writer slot cannot let Save
        // overtake a retained closed stroke.
        self.stroke.flush_pending_materializations();
        let mut recent_usage_changed = false;
        for size in drained.started_brush_sizes.into_iter().flatten().rev() {
            recent_usage_changed |= self.projection.stage_used_brush_size(size);
        }
        for color in drained.started_brush_colors.into_iter().flatten().rev() {
            recent_usage_changed |= self.projection.stage_used_brush_color(color);
        }
        if drained.artwork_closed {
            self.publish_closed_stroke_dirty();
        } else if recent_usage_changed {
            self.async_publication_base_revision
                .get_or_insert(self.projection.current().revision);
            self.publish_current_projection();
        }
        self.apply_materialized_tiles();
        self.adopt_native_edit();
        self.flush_transform_preview();
        self.apply_commands(width, height, scale);
        self.update_picker_preview();
        if self
            .artwork_job
            .as_ref()
            .is_some_and(|job| job.picker_token.is_some())
        {
            let _ = self.poll_artwork();
        }
        if self.synchronize_workspace_failure() {
            return None;
        }
        self.advance_save_probe();
        self.flush_pending_save();
        // Advance only after startup's queued Fit and the previous probe
        // command have run, so the next command uses their resulting revision.
        self.advance_protocol_probe();
        self.advance_history_probe();
        self.advance_edit_probe();
        if self.advance_input_safety_probe_before_drain() {
            return None;
        }
        self.advance_input_safety_probe_after_drain();
        let navigator_viewport =
            navigator_viewport(self.view, width, height, scale, self.canvas_spec);
        if navigator_viewport != self.navigator_viewport {
            self.navigator_viewport = navigator_viewport;
            if let Some(viewport) = navigator_viewport {
                self.stroke.live_ink.publish_navigator_viewport(viewport);
            }
        }
        if self.scene.active_live_stroke().is_none()
            && self.synthetic_seed_enabled
            && !self.synthetic_seed_submitted
        {
            let mut synthetic = Vec::with_capacity(64);
            add_synthetic_seed(
                self.canvas_spec.width_px,
                self.canvas_spec.height_px,
                &mut synthetic,
            );
            let mut background = Vec::with_capacity(18);
            add_synthetic_background(&mut background);
            if let Err(error) =
                self.scene
                    .apply_dabs(&mut self.background_painter, BACKGROUND_LAYER, &background)
            {
                eprintln!("native-canvas event=synthetic-background-failed error={error:?}");
            }
            if let Err(error) =
                self.scene
                    .apply_dabs(&mut self.painter, self.active_layer, &synthetic)
            {
                eprintln!("native-canvas event=synthetic-ink-failed error={error:?}");
            }
            if std::env::var_os("NAYATI_SYNTHETIC_VIEW").is_some() {
                self.view.fit(
                    width,
                    height,
                    scale,
                    [self.canvas_spec.width_px, self.canvas_spec.height_px],
                );
                self.view.rotation_radians = 0.12;
                self.view.zoom *= 1.08;
            }
            self.synthetic_seed_submitted = true;
        }
        let viewport = ViewportTransform {
            revision: self.stroke.live_ink.canvas_viewport_snapshot().revision,
            window_origin_physical: Point { x: 0.0, y: 0.0 },
            physical_size: [width, height],
            dpi_scale: scale,
            pan: self.view.pan,
            zoom: self.view.zoom,
            rotation_radians: self.view.rotation_radians,
            mirrored_horizontal: self.view.mirrored_horizontal,
        };
        let (guide, closed) = self
            .stroke
            .edit_gesture
            .as_ref()
            .map_or((&[][..], false), crate::edit_gesture::EditGesture::preview);
        if let Some(transform) = &self.stroke.transform_projection {
            let points = crate::transform_gesture::guide(transform, 7.0 / self.view.zoom);
            #[allow(clippy::cast_possible_truncation)]
            // Overlay only; worker validates artwork bounds.
            let points: Vec<[i32; 2]> = points
                .iter()
                .map(|p| [p[0].round() as i32, p[1].round() as i32])
                .collect();
            self.scene.set_gesture_preview(&points, false);
        } else {
            self.scene.set_gesture_preview(guide, closed);
        }
        let composite_timing = Span::new(Stage::Composite);
        crate::performance::frame_mark(crate::performance::FrameMark::ViewportBegin);
        let rendered = self.scene.render_viewport(viewport);
        crate::performance::frame_mark(crate::performance::FrameMark::ViewportEnd);
        let stats = match rendered {
            Ok(stats) => stats,
            Err(error) => {
                eprintln!("native-canvas event=viewport-render-failed error={error:?}");
                if error == CompositeRenderError::SparseAtlasExhausted {
                    self.stroke.live_ink.publish_workspace_error(
                        "Visible workspace tiles exceeded the GPU atlas capacity; drawing was stopped to prevent invisible edits"
                            .into(),
                    );
                    self.stroke.fail_closed("sparse-atlas-exhausted");
                }
                return None;
            }
        };
        drop(composite_timing);
        if stats.group_tiles_rebuilt > 0 && self.gpu_batch_logs_remaining > 0 {
            println!(
                "native-canvas event=composite-cache-update rebuilt={} resident={} viewport_passes={}",
                stats.group_tiles_rebuilt, stats.cache_entries, stats.display_passes,
            );
        }
        let texture = self
            .scene
            .display_texture()
            .expect("viewport render created the display texture");
        Some(RenderedCanvas {
            texture,
            history_timing,
            viewport: Some(viewport),
            geometry_epoch,
        })
    }

    fn apply_commands(&mut self, width: u32, height: u32, scale: f64) {
        self.synchronize_workspace_failure();
        self.stroke
            .live_ink
            .drain_editor_commands(&mut self.commands);
        let commands = std::mem::take(&mut self.commands);
        for command in commands {
            let envelope = match command {
                crate::live_ink::CanvasCommand::Ui(envelope) => envelope,
                crate::live_ink::CanvasCommand::NativeViewport { id, command } => CommandEnvelope {
                    id,
                    based_on: self.projection.current().revision,
                    command: EditorCommand::Viewport(command),
                },
            };
            self.apply_command(envelope, width, height, scale);
        }
        // This one-frame compatibility is only for a Save queued just before
        // native stroke closure or async edit publication. Later commands must
        // observe the new semantic revision normally.
        self.async_publication_base_revision = None;
    }

    fn apply_command(&mut self, envelope: CommandEnvelope, width: u32, height: u32, scale: f64) {
        let command_id = envelope.id;
        let persist_dock = matches!(&envelope.command, EditorCommand::Dock(_));
        if self.synchronize_workspace_failure() {
            self.reject_command(command_id, CommandRejectReason::WorkspaceFailed);
            return;
        }
        let save_queued_before_publication = matches!(
            &envelope.command,
            EditorCommand::Project(ProjectCommand::Save)
        ) && self.async_publication_base_revision
            == Some(envelope.based_on);
        // Relative size intent can safely follow earlier size publications, but
        // must never be retargeted to a different brush after tool selection.
        let relative_size_for_current_tool = matches!(
            &envelope.command,
            EditorCommand::Tool(ToolCommand::AdjustSizeSteps { tool, .. })
                if *tool == self.drawing.tool
                    && DrawingConfig::painting_slot(*tool).is_some()
                    && envelope.based_on <= self.projection.current().revision
        );
        let revision_validation =
            if save_queued_before_publication || relative_size_for_current_tool {
                Ok(())
            } else {
                self.projection.validate_command_revision(envelope.based_on)
            };
        let result = revision_validation
            .and_then(|()| {
                self.projection
                    .current()
                    .revision
                    .checked_next()
                    .ok_or(CommandRejectReason::RevisionExhausted)
                    .map(|_| ())
            })
            .and_then(|()| self.apply_editor_command(envelope.command, width, height, scale));

        match result {
            Ok(artwork_changed) => {
                let dirty = self.projection.current().dirty || artwork_changed;
                let publish = self.projection.publish_editor_state(
                    self.project_title.clone(),
                    dirty,
                    Some(self.active_layer),
                    self.scene.tree(),
                    self.projection.current().dock.clone(),
                    self.view.projection(),
                );
                match publish {
                    Ok(_) => {
                        if persist_dock {
                            self.stroke
                                .live_ink
                                .persist_layout(&self.projection.current().dock);
                        }
                        let revision = self.projection.current().revision;
                        self.stroke.live_ink.publish_editor_event(
                            EditorEvent::CommandAccepted {
                                command: command_id,
                                revision,
                            },
                            Some(self.projection.current().clone()),
                        );
                        println!(
                            "native-canvas event=semantic-command-accepted command={} revision={}",
                            command_id.0, revision.0,
                        );
                    }
                    Err(error) => self.reject_command(command_id, projection_rejection(error)),
                }
            }
            Err(reason) => self.reject_command(command_id, reason),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply_editor_command(
        &mut self,
        command: EditorCommand,
        width: u32,
        height: u32,
        scale: f64,
    ) -> Result<bool, CommandRejectReason> {
        if self
            .stroke
            .picker
            .gesture
            .as_ref()
            .is_some_and(|gesture| gesture.released)
            && !matches!(
                command,
                EditorCommand::Tool(ToolCommand::CancelGesture)
                    | EditorCommand::Dock(_)
                    | EditorCommand::Viewport(_)
                    | EditorCommand::Project(ProjectCommand::Save)
            )
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        if self.transform_active() {
            if matches!(command, EditorCommand::Tool(ToolCommand::CancelGesture)) {
                return self.enqueue_free_transform(nyatidraw_api::TransformCommand::Cancel);
            }
            if let EditorCommand::Tool(tool_command) = command
                && let Some(tool) = transform_runtime::selected_tool(
                    self.stroke.pending_tool.unwrap_or(self.drawing.tool),
                    tool_command,
                )
            {
                let template = match tool_command {
                    ToolCommand::SelectPencilTemplate(template) => Some(template),
                    _ => None,
                };
                self.switch_transform_tool(tool, template);
                return Ok(false);
            }
            if !matches!(
                command,
                EditorCommand::Edit(EditCommand::FreeTransform(_))
                    | EditorCommand::Viewport(_)
                    | EditorCommand::Dock(_)
            ) {
                self.stroke
                    .live_ink
                    .publish_activation_notice("변형을 확정하거나 취소한 뒤 작업하세요.".into());
                return Err(CommandRejectReason::CommandQueueBusy);
            }
        }
        if self.artwork_job.is_some()
            && matches!(command, EditorCommand::Layer(_) | EditorCommand::History(_))
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        match command {
            EditorCommand::Edit(command) => self.enqueue_edit(command),
            EditorCommand::Viewport(command) => {
                let previous = self.view;
                match command {
                    ViewportCommand::FitDocument => self.view.fit(
                        width,
                        height,
                        scale,
                        [self.canvas_spec.width_px, self.canvas_spec.height_px],
                    ),
                    ViewportCommand::ActualPixels => self.view.actual_pixels(
                        width,
                        height,
                        scale,
                        [self.canvas_spec.width_px, self.canvas_spec.height_px],
                    ),
                    ViewportCommand::PanBy {
                        logical_x,
                        logical_y,
                    } => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        self.view.pan.x += f64::from(logical_x);
                        self.view.pan.y += f64::from(logical_y);
                    }
                    ViewportCommand::CenterPageAt {
                        page_x_10k,
                        page_y_10k,
                    } => {
                        if !(0..=10_000).contains(&page_x_10k)
                            || !(0..=10_000).contains(&page_y_10k)
                        {
                            return Err(CommandRejectReason::InvalidViewport);
                        }
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        let document = Point {
                            x: f64::from(self.canvas_spec.width_px) * f64::from(page_x_10k)
                                / 10_000.0,
                            y: f64::from(self.canvas_spec.height_px) * f64::from(page_y_10k)
                                / 10_000.0,
                        };
                        let logical_center = Point {
                            x: f64::from(width) / scale * 0.5,
                            y: f64::from(height) / scale * 0.5,
                        };
                        if let Some(view) = self
                            .view
                            .mapping()
                            .with_document_at(document, logical_center)
                        {
                            self.view.pan = view.pan;
                        }
                    }
                    ViewportCommand::ZoomSteps(steps) => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        self.view.zoom_at(
                            Point {
                                x: f64::from(width) / scale * 0.5,
                                y: f64::from(height) / scale * 0.5,
                            },
                            steps,
                        );
                    }
                    ViewportCommand::ZoomAt {
                        steps,
                        logical_x,
                        logical_y,
                    } => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        self.view.zoom_at(
                            Point {
                                x: f64::from(logical_x),
                                y: f64::from(logical_y),
                            },
                            steps,
                        );
                    }
                    ViewportCommand::RotateQuarterSteps(_)
                    | ViewportCommand::ResetRotation
                    | ViewportCommand::ToggleMirrorHorizontal => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        let rotation = match command {
                            ViewportCommand::RotateQuarterSteps(steps) => {
                                (self.view.rotation_radians
                                    + f64::from(steps) * std::f64::consts::FRAC_PI_4)
                                    .rem_euclid(std::f64::consts::TAU)
                            }
                            ViewportCommand::ResetRotation => 0.0,
                            _ => self.view.rotation_radians,
                        };
                        let mirrored = self.view.mirrored_horizontal
                            ^ matches!(command, ViewportCommand::ToggleMirrorHorizontal);
                        self.view.change_at(
                            Point {
                                x: f64::from(width) / scale * 0.5,
                                y: f64::from(height) / scale * 0.5,
                            },
                            self.view.zoom,
                            rotation,
                            mirrored,
                        );
                    }
                }
                let candidate = ViewportTransform {
                    revision: 0,
                    window_origin_physical: Point { x: 0.0, y: 0.0 },
                    physical_size: [width, height],
                    dpi_scale: scale,
                    pan: self.view.pan,
                    zoom: self.view.zoom,
                    rotation_radians: self.view.rotation_radians,
                    mirrored_horizontal: self.view.mirrored_horizontal,
                };
                if candidate.validate().is_err() {
                    self.view = previous;
                    return Err(CommandRejectReason::InvalidViewport);
                }
                Ok(false)
            }
            EditorCommand::Layer(LayerCommand::SetActive(layer)) => {
                if self.stroke.edit_gesture.is_some() || self.artwork_job.is_some() {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                if self
                    .scene
                    .tree()
                    .ancestors(nyatidraw_api::LayerTreeNodeId::Raster(layer))
                    .is_none()
                {
                    return Err(CommandRejectReason::UnknownLayer);
                }
                self.active_layer = layer;
                self.stroke.live_ink.set_admission_layer(layer);
                Ok(false)
            }
            EditorCommand::Layer(LayerCommand::ToggleSolo(node)) => {
                let next = (self.projection.current().solo_node != Some(node)).then_some(node);
                self.scene
                    .set_solo(next)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
                self.projection.stage_solo(next);
                Ok(false)
            }
            EditorCommand::Layer(command) => {
                self.ensure_artwork_command_idle()?;
                if !self.stroke.live_ink.try_pause_for_edit() {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                let result = self.apply_layer_command(command);
                if self.artwork_job.is_none() {
                    self.stroke.live_ink.resume_after_edit();
                }
                result
            }
            EditorCommand::Dock(command) => {
                let mut dock = self.projection.current().dock.clone();
                match command {
                    DockCommand::Replace(next) => dock = next,
                    DockCommand::ActivatePanel(panel) => {
                        dock.activate_panel(panel);
                    }
                    DockCommand::MovePanel {
                        panel,
                        target,
                        position,
                    } => dock
                        .dock_panel(panel, target, position)
                        .map_err(|_| CommandRejectReason::InvalidLayout)?,
                    DockCommand::MoveToolbarToTop { panel, before } => dock
                        .dock_toolbar_top(panel, before)
                        .map_err(|_| CommandRejectReason::InvalidLayout)?,
                    DockCommand::ResetToSafeDefault => dock = DockTree::safe_default(),
                }
                self.projection.stage_dock(dock);
                Ok(false)
            }
            EditorCommand::Project(ProjectCommand::Save) => {
                if self.project_export_path.is_none() {
                    return Err(CommandRejectReason::UnsupportedCommand);
                }
                let generation = self
                    .stroke
                    .materializer
                    .reserve_export()
                    .map_err(|()| CommandRejectReason::CommandQueueBusy)?;
                // One latest-only semantic request waits for the current
                // stroke and retained closed work. Raw input never blocks.
                self.pending_save = Some(generation);
                println!(
                    "native-canvas event=project-save-requested export_generation={generation} waiting_for_stroke={} bounded_pending=1",
                    self.stroke.active.is_some(),
                );
                Ok(false)
            }
            EditorCommand::Tool(ToolCommand::CancelGesture) => {
                if self.stroke.cancel_native_gesture() {
                    self.execute_gpu_ops(0);
                    self.stage_cancel_availability();
                    return Ok(false);
                }
                if self.selection.is_some() {
                    return self.enqueue_edit(EditCommand::ClearSelection);
                }
                Ok(false)
            }
            EditorCommand::Tool(command) => {
                // A pending eyedropper result must not overwrite a newer color
                // command. Serialize color mutations with the artwork worker.
                if (self.artwork_job.is_some() || self.stroke.live_ink.temporary_pick_pending())
                    && matches!(command, ToolCommand::SetColor(_) | ToolCommand::SwapColors)
                {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                if self.scene.active_live_stroke().is_some()
                    || self.stroke.edit_gesture.is_some()
                    || !self.stroke.live_ink.try_pause_for_edit()
                {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                let result = (|| {
                    match command {
                        ToolCommand::Select(tool) => {
                            self.drawing.select_tool(tool);
                            self.stroke
                                .live_ink
                                .set_navigation_tool(tool == DrawingTool::Move);
                        }
                        ToolCommand::SelectPencilTemplate(template) => {
                            self.drawing.remember_current();
                            self.drawing.pencil_template = template;
                            self.drawing.select_tool(DrawingTool::Pencil);
                            self.stroke.live_ink.set_navigation_tool(false);
                        }
                        ToolCommand::CycleBrushFamily => {
                            let tool = match self.drawing.tool {
                                DrawingTool::Pencil => DrawingTool::Pen,
                                DrawingTool::Pen => DrawingTool::Brush,
                                _ => DrawingTool::Pencil,
                            };
                            self.drawing.select_tool(tool);
                            self.stroke.live_ink.set_navigation_tool(false);
                        }
                        ToolCommand::CycleSelectionFamily => {
                            let tool = match self.drawing.tool {
                                DrawingTool::Move => DrawingTool::MoveSelection,
                                DrawingTool::MoveSelection => DrawingTool::Wand,
                                DrawingTool::Wand => DrawingTool::Lasso,
                                DrawingTool::Lasso => DrawingTool::RectangleSelection,
                                _ => DrawingTool::Move,
                            };
                            self.drawing.select_tool(tool);
                            self.stroke
                                .live_ink
                                .set_navigation_tool(self.drawing.tool == DrawingTool::Move);
                        }
                        ToolCommand::CycleFillFamily => {
                            let tool = if self.drawing.tool == DrawingTool::Fill {
                                DrawingTool::Gradient
                            } else {
                                DrawingTool::Fill
                            };
                            self.drawing.select_tool(tool);
                            self.stroke.live_ink.set_navigation_tool(false);
                        }
                        ToolCommand::SetSizeTenths(size) if (1..=2_000).contains(&size) => {
                            self.drawing.size_tenths = size;
                        }
                        ToolCommand::AdjustSizeSteps { tool, steps }
                            if tool == self.drawing.tool
                                && DrawingConfig::painting_slot(tool).is_some() =>
                        {
                            for _ in 0..steps.unsigned_abs() {
                                let size = self.drawing.size_tenths;
                                self.drawing.size_tenths = if steps > 0 {
                                    size.saturating_add((size / 10).max(1)).min(2_000)
                                } else {
                                    size.saturating_sub((size / 11).max(1)).max(1)
                                };
                            }
                        }
                        ToolCommand::SetOpacityU16(opacity) if opacity > 0 => {
                            self.drawing.opacity_u16 = opacity;
                        }
                        ToolCommand::SetSizePressure(enabled) => {
                            self.drawing.brush_settings.size_pressure = enabled;
                        }
                        ToolCommand::SetOpacityPressure(enabled) => {
                            self.drawing.brush_settings.opacity_pressure = enabled;
                        }
                        ToolCommand::SetSizeMinimumU16(minimum) => {
                            self.drawing.brush_settings.size_minimum_u16 = minimum;
                        }
                        ToolCommand::SetOpacityMinimumU16(minimum) => {
                            self.drawing.brush_settings.opacity_minimum_u16 = minimum;
                        }
                        ToolCommand::SetHardnessU16(hardness) => {
                            self.drawing.brush_settings.hardness_u16 = hardness;
                        }
                        ToolCommand::SetSmoothing(strength) => {
                            self.drawing.brush_settings.smoothing = strength.min(100);
                        }
                        ToolCommand::SetColor(color) if color[3] > 0 => self.drawing.color = color,
                        ToolCommand::SwapColors => {
                            std::mem::swap(
                                &mut self.drawing.color,
                                &mut self.drawing.background_color,
                            );
                        }
                        ToolCommand::SetEditSource(source) => {
                            self.drawing.edit_settings.source = source;
                        }
                        ToolCommand::SetEditTolerance(tolerance) => {
                            self.drawing.edit_settings.tolerance = tolerance;
                        }
                        ToolCommand::SetFillSettings(settings) => {
                            if settings.gap_close_px > 8 || settings.expand_px > 64 {
                                return Err(CommandRejectReason::UnsupportedCommand);
                            }
                            self.drawing.edit_settings.fill = settings;
                        }
                        ToolCommand::SetFillGapClose(radius) => {
                            if radius > 8 {
                                return Err(CommandRejectReason::UnsupportedCommand);
                            }
                            self.drawing.edit_settings.fill.gap_close_px = radius;
                        }
                        ToolCommand::SetFillExpansion(radius) => {
                            if radius > 64 {
                                return Err(CommandRejectReason::UnsupportedCommand);
                            }
                            self.drawing.edit_settings.fill.expand_px = radius;
                        }
                        ToolCommand::SetFillAntialias(enabled) => {
                            self.drawing.edit_settings.fill.antialias = enabled;
                        }
                        ToolCommand::SetSelectionMode(mode) => {
                            self.drawing.edit_settings.selection_mode = mode;
                        }
                        ToolCommand::CancelGesture
                        | ToolCommand::SetSizeTenths(_)
                        | ToolCommand::AdjustSizeSteps { .. }
                        | ToolCommand::SetOpacityU16(_)
                        | ToolCommand::SetColor(_) => {
                            return Err(CommandRejectReason::UnsupportedCommand);
                        }
                    }
                    self.drawing.remember_current();
                    self.projection
                        .stage_pencil_template(self.drawing.pencil_template);
                    self.projection
                        .stage_brush_settings(self.drawing.brush_settings);
                    self.projection.stage_drawing_controls(
                        self.drawing.tool,
                        self.drawing.size_tenths,
                        self.drawing.opacity_u16,
                        self.drawing.color,
                        self.drawing.background_color,
                    );
                    self.projection
                        .stage_edit_settings(self.drawing.edit_settings);
                    Ok(false)
                })();
                if self.artwork_job.is_none() {
                    self.stroke.live_ink.resume_after_edit();
                }
                result
            }
            EditorCommand::History(command) => {
                self.ensure_artwork_command_idle()?;
                if !self.stroke.live_ink.try_pause_for_edit() {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                let (reply, received) = sync_channel(1);
                self.enqueue_artwork(
                    WorkerRequest::MoveHistory { command, reply },
                    received,
                    None,
                )
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn apply_layer_command(&mut self, command: LayerCommand) -> Result<bool, CommandRejectReason> {
        let mut tree = self.scene.tree().clone();
        let mut active = self.active_layer;
        let next_id = self.next_layer_node_id;
        let mut pixel_change = None;
        match command {
            LayerCommand::AddRaster | LayerCommand::AddGroup => {
                let is_group = matches!(command, LayerCommand::AddGroup);
                let (parent, index) = tree
                    .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(active))
                    .map_or(
                        (tree.root_id(), tree.root().children.len()),
                        |(parent, index)| (parent, index + 1),
                    );
                let name = next_default_node_name(&tree, is_group);
                let node = if is_group {
                    LayerTreeNode::Group(GroupNode {
                        clip_to_below: false,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        id: GroupId(next_id),
                        name,
                        visible: true,
                        opacity_u16: u16::MAX,
                        children: Vec::new(),
                    })
                } else {
                    active = LayerId(next_id);
                    LayerTreeNode::Raster(empty_raster(active, name))
                };
                let _ = next_id
                    .checked_add(1)
                    .ok_or(CommandRejectReason::RevisionExhausted)?;
                tree.insert(parent, index, node)
                    .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
            }
            LayerCommand::DuplicateRaster(source) => {
                let _ = next_id
                    .checked_add(1)
                    .ok_or(CommandRejectReason::RevisionExhausted)?;
                active = LayerId(next_id);
                tree.duplicate_raster(source, active)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
                pixel_change = Some(LayerPixelChange::Duplicate {
                    source,
                    destination: active,
                });
            }
            LayerCommand::SetReference { layer, reference } => {
                tree.set_reference(layer, reference)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetLocked { layer, locked } => {
                tree.set_locked(layer, locked)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetAlphaLocked {
                layer,
                alpha_locked,
            } => {
                tree.set_alpha_locked(layer, alpha_locked)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetClipToBelow {
                node,
                clip_to_below,
            } => {
                tree.set_clip_to_below(node, clip_to_below)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetBlendMode { node, blend_mode } => {
                tree.set_blend_mode(node, blend_mode)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::Delete(node) => {
                tree.remove(node).map_err(|error| match error {
                    nyatidraw_document::LayerTreeError::LockedNode(_) => {
                        CommandRejectReason::LayerLocked
                    }
                    _ => CommandRejectReason::UnknownLayer,
                })?;
                if raster_layer_ids(&tree).is_empty() {
                    active = LayerId(next_id);
                    let _ = next_id
                        .checked_add(1)
                        .ok_or(CommandRejectReason::RevisionExhausted)?;
                    tree.insert(
                        tree.root_id(),
                        tree.root().children.len(),
                        LayerTreeNode::Raster(empty_raster(active, "레이어 1".into())),
                    )
                    .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
                }
                active = valid_active_layer(&tree, active);
            }
            LayerCommand::Rename { node, name } => {
                tree.rename(node, &name)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetVisibility { node, visible } => {
                tree.set_visibility(node, visible)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::SetOpacity { node, opacity_u16 } => {
                tree.set_opacity(node, opacity_u16)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
            }
            LayerCommand::Reorder {
                node,
                new_parent,
                index,
            } => {
                tree.reorder(node, new_parent, index)
                    .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
            }
            LayerCommand::AddWhiteBackground => {
                let _ = next_id
                    .checked_add(1)
                    .ok_or(CommandRejectReason::RevisionExhausted)?;
                let layer = LayerId(next_id);
                tree.insert(
                    tree.root_id(),
                    0,
                    LayerTreeNode::Raster(empty_raster(layer, "White".into())),
                )
                .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
                pixel_change = Some(LayerPixelChange::White(layer));
            }
            LayerCommand::SetActive(_) | LayerCommand::ToggleSolo(_) => {
                unreachable!("session commands handled separately")
            }
        }
        if &tree == self.scene.tree() && pixel_change.is_none() {
            return Ok(false);
        }
        self.scene
            .validate_tree_replacement(&tree)
            .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        let (reply, received) = sync_channel(1);
        self.enqueue_artwork(
            WorkerRequest::CommitLayerTree {
                tree,
                pixel_change,
                reply,
            },
            received,
            Some(active),
        )
    }

    fn ensure_artwork_command_idle(&mut self) -> Result<(), CommandRejectReason> {
        if self.artwork_job.is_some()
            || self.stroke.edit_gesture.is_some()
            || self.scene.active_live_stroke().is_some()
            || self.stroke.active.is_some()
            || !self.stroke.pending_materializations.is_empty()
            || self.stroke.materializer.pending.load(Ordering::Acquire) != 0
            || matches!(
                self.stroke.live_ink.export_status_snapshot(),
                ExportStatus::Queued { .. } | ExportStatus::Running { .. }
            )
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let based_on = self.projection.current().revision;
        self.apply_materialized_tiles();
        if self.projection.current().revision != based_on {
            return Err(CommandRejectReason::StaleProjection {
                based_on,
                current: self.projection.current().revision,
            });
        }
        Ok(())
    }

    fn install_history_move(&mut self, moved: HistoryMove) -> Result<(), CommandRejectReason> {
        let result = self.install_history_move_inner(moved);
        if let Err(error) = result {
            self.stroke.live_ink.publish_workspace_error(format!(
                "Artwork was saved but the canvas could not adopt its history state: {error:?}; reopen the project before drawing"
            ));
        }
        result
    }

    fn install_history_move_inner(
        &mut self,
        moved: HistoryMove,
    ) -> Result<(), CommandRejectReason> {
        let _timing = Span::new(Stage::HistoryAdoption);
        self.projection.stage_history(moved.history);
        let Some(snapshot) = moved.tiles else {
            return Ok(());
        };
        let clear_selection = moved.tree.is_some() || moved.canvas.is_some();
        let tree = moved.tree.unwrap_or_else(|| self.scene.tree().clone());
        // Only reuse closed pixels while their disposable surfaces survive.
        // Tree/page changes keep the conservative full upload path, including
        // restored layers and artwork that becomes visible after page growth.
        let rebuild_surfaces = tree != *self.scene.tree()
            || moved.canvas.is_some_and(|canvas| {
                [canvas.width_px, canvas.height_px] != self.scene.document_size()
            });
        if let Some(canvas) = moved.canvas {
            let size = [canvas.width_px, canvas.height_px];
            if size != self.scene.document_size() {
                self.scene
                    .replace_document(size, tree)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
            } else if rebuild_surfaces {
                self.scene
                    .replace_tree(tree)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
            }
            self.canvas_spec = canvas;
            let mut current = self.projection.current().clone();
            current.canvas = canvas;
            self.projection.install_authoritative(current);
        } else if rebuild_surfaces {
            self.scene
                .replace_tree(tree)
                .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        }
        if clear_selection {
            self.selection = None;
            self.stroke.selection = None;
            self.painter.set_selection_mask(None);
            self.alpha_painter.set_selection_mask(None);
            self.eraser.set_selection_mask(None);
            self.scene.set_selection_overlay(None);
            self.projection
                .stage_edit(nyatidraw_api::EditProjection::default());
        }
        self.active_layer = valid_active_layer(self.scene.tree(), self.active_layer);
        self.stroke.live_ink.set_admission_layer(self.active_layer);
        self.next_layer_node_id = self.next_layer_node_id.max(
            next_layer_node_id(self.scene.tree())
                .map_err(|_| CommandRejectReason::RevisionExhausted)?,
        );
        if self
            .projection
            .current()
            .solo_node
            .is_some_and(|node| self.scene.tree().ancestors(node).is_none())
        {
            self.projection.stage_solo(None);
        }
        let adoption = CpuSnapshotAdoption::new(&mut self.cpu_tiles, &snapshot);
        let retained = if rebuild_surfaces {
            0
        } else {
            adoption
                .tiles
                .iter()
                .filter(|(_, changed)| !changed)
                .count()
        };
        let mut uploaded = 0;
        for key in adoption.upload_keys(rebuild_surfaces) {
            if self
                .scene
                .tree()
                .ancestors(nyatidraw_api::LayerTreeNodeId::Raster(key.layer))
                .is_none()
            {
                continue;
            }
            let pixels = snapshot.get(key).map_or(
                &TRANSPARENT_HISTORY_TILE[..],
                nyatidraw_tiles::TileObject::pixels,
            );
            self.scene
                .upload_closed_tile(key, pixels)
                .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
            uploaded += 1;
        }
        if crate::performance::start().is_some() {
            println!(
                "performance-history-upload rebuilt_surfaces={rebuild_surfaces} retained={retained} uploaded={uploaded}"
            );
        }
        adoption.apply();
        self.latest_preview_generation.clear();
        if self.artwork_job.is_none() {
            self.stroke.live_ink.resume_after_edit();
        }
        Ok(())
    }

    fn enqueue_edit(
        &mut self,
        command: nyatidraw_api::EditCommand,
    ) -> Result<bool, CommandRejectReason> {
        if let EditCommand::FreeTransform(command) = command {
            return self.enqueue_free_transform(command);
        }
        if command == EditCommand::PasteSelection {
            return self.enqueue_free_transform(nyatidraw_api::TransformCommand::Paste);
        }
        if self.stroke.transform_projection.is_some() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        self.ensure_artwork_command_idle()?;
        if let EditCommand::ResizePage { size } = &command {
            self.scene
                .validate_document_replacement(*size, self.scene.tree())
                .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        }
        if command == EditCommand::PasteSelection {
            // Preflight one added surface before the worker commits a layer.
            // The worker chooses the durable ID; GPU capacity depends on shape,
            // not the numerical ID used for this temporary validation tree.
            let mut candidate = self.scene.tree().clone();
            let (parent, index) = candidate
                .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(self.active_layer))
                .ok_or(CommandRejectReason::UnknownLayer)?;
            let id = next_layer_node_id(&candidate)
                .map_err(|_| CommandRejectReason::RevisionExhausted)?;
            candidate
                .insert(
                    parent,
                    index + 1,
                    LayerTreeNode::Raster(empty_raster(LayerId(id), "붙여넣기".into())),
                )
                .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
            self.scene
                .validate_tree_replacement(&candidate)
                .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        }
        if !self.stroke.live_ink.try_pause_for_edit() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let (reply, received) = sync_channel(1);
        let request = WorkerRequest::Edit {
            command,
            target: self.active_layer,
            reply,
        };
        self.enqueue_artwork(request, received, None)
    }

    fn enqueue_artwork(
        &mut self,
        request: WorkerRequest,
        received: Receiver<Result<ArtworkOutcome, EditFailure>>,
        active_layer: Option<LayerId>,
    ) -> Result<bool, CommandRejectReason> {
        let kind = match &request {
            WorkerRequest::Edit { .. } => "edit",
            WorkerRequest::MoveHistory { .. } => "history",
            WorkerRequest::CommitLayerTree { .. } => "layer",
            _ => unreachable!("only artwork requests use the artwork job"),
        };
        let temporary_picker = match &request {
            WorkerRequest::Edit {
                command: EditCommand::PickDisplayColor { input_sequence, .. },
                ..
            } => Some(*input_sequence),
            _ => None,
        };
        let transform = match &request {
            WorkerRequest::Edit {
                command: EditCommand::FreeTransform(command),
                ..
            } => Some(command.clone()),
            _ => None,
        };
        let queued = self
            .stroke
            .materializer
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send(request).is_ok());
        if !queued {
            self.stroke.live_ink.resume_after_edit();
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        self.artwork_job = Some(ArtworkJob {
            transform: transform.clone(),
            temporary_picker,
            picker_token: None,
            received,
            active_layer,
            history_timing: (kind == "history").then(HistoryTiming::queued).flatten(),
        });
        let mut edit = self.projection.current().edit.clone();
        edit.busy = true;
        edit.can_cancel = matches!(
            transform,
            Some(
                nyatidraw_api::TransformCommand::Begin
                    | nyatidraw_api::TransformCommand::Paste
                    | nyatidraw_api::TransformCommand::Preview(_)
            )
        );
        edit.error = None;
        self.projection.stage_edit(edit);
        println!(
            "native-canvas event=artwork-queued kind={kind} pending=1 response_capacity=1 wait=nonblocking"
        );
        Ok(false)
    }

    fn adopt_native_edit(&mut self) {
        let Some(result) = self.stroke.completed_edit.take() else {
            return;
        };
        let temporary_picker = std::mem::take(&mut self.stroke.completed_temporary_picker);
        let picker_token = self
            .stroke
            .picker
            .gesture
            .as_ref()
            .filter(|gesture| gesture.released)
            .map(|gesture| gesture.token);
        let result = result.and_then(|command| {
            if matches!(
                command,
                EditCommand::PickColor { .. } | EditCommand::PickDisplayColor { .. }
            ) && picker_token.is_none_or(|token| !self.stroke.picker_token_valid(token))
            {
                self.stroke.cancel_picker();
                if let Some(sequence) = temporary_picker {
                    self.stroke.live_ink.complete_temporary_pick(sequence);
                }
                return Ok(());
            }
            let command =
                match command {
                    EditCommand::PickColor { point, .. } if temporary_picker.is_some() => {
                        EditCommand::PickDisplayColor {
                            point,
                            solo: self.stroke.picker.gesture.as_ref().and_then(|gesture| {
                                match gesture.source {
                                    crate::edit_worker::PickerSource::Display(solo) => solo,
                                    crate::edit_worker::PickerSource::Artwork(_) => None,
                                }
                            }),
                            input_sequence: temporary_picker.expect("matched temporary read"),
                        }
                    }
                    other => other,
                };
            if let EditCommand::Transform(legacy) = &command
                && self.drawing.tool == DrawingTool::MoveSelection
            {
                // An idle move gesture may predate successful draft admission.
                // It must still become a preview, never an immediate old-style
                // destructive transform that clears the retained selection.
                self.enqueue_free_transform(nyatidraw_api::TransformCommand::Begin)
                    .map_err(|error| format!("변형을 시작하지 못했습니다: {error:?}"))?;
                self.stroke.pending_transform = Some(nyatidraw_api::AffineTransform {
                    offset_milli: legacy.offset.map(|value| i64::from(value) * 1000),
                    ..Default::default()
                });
                return Ok(());
            }
            let queued = self.enqueue_edit(command.clone());
            if (temporary_picker.is_some() || picker_token.is_some())
                && matches!(
                    queued,
                    Err(CommandRejectReason::CommandQueueBusy
                        | CommandRejectReason::StaleProjection { .. })
                )
            {
                self.stroke.completed_edit = Some(Ok(command));
                self.stroke.completed_temporary_picker = temporary_picker;
                return Ok(());
            }
            if queued.is_ok()
                && let Some(job) = &mut self.artwork_job
            {
                job.picker_token = picker_token;
                if picker_token.is_some() {
                    self.stage_cancel_availability();
                }
            }
            queued
                .map(|_| ())
                .map_err(|error| format!("편집을 적용하지 못했습니다. 다시 시도하세요: {error:?}"))
        });
        if let Err(error) = result {
            if picker_token.is_some() {
                self.stroke.cancel_picker();
            }
            if let Some(sequence) = temporary_picker {
                self.stroke.live_ink.complete_temporary_pick(sequence);
                self.stroke.live_ink.resume_after_edit();
            }
            let mut edit = self.projection.current().edit.clone();
            edit.error = Some(error);
            self.projection.stage_edit(edit);
        }
        self.async_publication_base_revision
            .get_or_insert(self.projection.current().revision);
        self.publish_current_projection();
    }

    // Keep outcome adoption and release of its owned input barrier together.
    #[allow(clippy::too_many_lines)]
    fn poll_artwork(&mut self) -> Option<HistoryTiming> {
        let Some(job) = &self.artwork_job else {
            return None;
        };
        let result = match job.received.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(EditFailure::Fatal("Artwork worker disconnected".into()))
            }
        };
        let active_layer = job.active_layer;
        let temporary_picker = job.temporary_picker;
        let picker_token = job.picker_token;
        let picker_cancelled =
            picker_token.is_some_and(|token| !self.stroke.picker_token_valid(token));
        let transform_command = job.transform.clone();
        let mut history_timing = job.history_timing;
        let mut history_changed = false;
        let mut error = None;
        match result {
            Ok(ArtworkOutcome::Edit(_)) | Err(EditFailure::Rejected(_)) if picker_cancelled => {}
            Ok(ArtworkOutcome::Transform(outcome)) => {
                if let Err(reason) = self.install_transform_outcome(outcome) {
                    self.stroke.live_ink.publish_workspace_error(reason);
                    return None;
                }
            }
            Ok(ArtworkOutcome::Edit(outcome)) => {
                if let Some(layer) = outcome.active_layer {
                    self.active_layer = layer;
                }
                if let Some(color) = outcome.sampled_color {
                    self.drawing.color = color;
                    self.projection.stage_drawing_controls(
                        self.drawing.tool,
                        self.drawing.size_tenths,
                        self.drawing.opacity_u16,
                        self.drawing.color,
                        self.drawing.background_color,
                    );
                }
                let changed = outcome.tiles.is_some();
                if changed
                    && self
                        .install_history_move(HistoryMove {
                            tiles: outcome.tiles,
                            canvas: outcome.canvas,
                            tree: outcome.tree,
                            history: outcome.history,
                            worker_timing: None,
                        })
                        .is_err()
                {
                    return None;
                }
                if let Err(error) = self.install_selection(outcome.selection) {
                    self.stroke.live_ink.publish_workspace_error(error);
                    return None;
                }
                if changed {
                    let mut current = self.projection.current().clone();
                    current.dirty = true;
                    self.projection.install_authoritative(current);
                }
            }
            Ok(ArtworkOutcome::History(moved)) => {
                if let Some(timing) = &mut history_timing {
                    timing.received(moved.worker_timing);
                }
                let changed = moved.tiles.is_some();
                if let Some(active) = active_layer {
                    self.active_layer = active;
                }
                if self.install_history_move(moved).is_err() {
                    return None;
                }
                history_changed = changed;
                if changed {
                    let mut current = self.projection.current().clone();
                    current.dirty = true;
                    self.projection.install_authoritative(current);
                }
            }
            Err(EditFailure::Rejected(reason)) => {
                self.reject_transform_outcome(transform_command.as_ref());
                if let Some(transform) = &mut self.stroke.transform_projection {
                    transform.can_commit = false;
                }
                error = Some(reason);
            }
            Err(EditFailure::Fatal(reason)) => {
                self.stroke.live_ink.publish_workspace_error(reason);
                return None;
            }
        }
        self.artwork_job = None;
        if picker_token.is_some() {
            self.stroke.cancel_picker();
        }
        if let Some(sequence) = temporary_picker {
            self.stroke.live_ink.complete_temporary_pick(sequence);
        }
        let selected_pixels = self
            .selection
            .as_ref()
            .map_or(0, |mask| mask.selected_pixels());
        self.projection.stage_edit(nyatidraw_api::EditProjection {
            busy: false,
            can_cancel: self.stroke.transform_projection.is_some(),
            has_selection: self.selection.is_some(),
            selected_pixels,
            error,
            transform: self.stroke.transform_projection.clone(),
        });
        self.stroke.live_ink.resume_after_edit();
        self.async_publication_base_revision
            .get_or_insert(self.projection.current().revision);
        self.publish_current_projection();
        println!("native-canvas event=artwork-adopted selected_pixels={selected_pixels} pending=0");
        let mut timing = history_timing.filter(|_| history_changed);
        if let Some(timing) = &mut timing {
            timing.adopted();
        }
        timing
    }

    fn install_selection(
        &mut self,
        selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    ) -> Result<(), String> {
        let unchanged = match (&self.selection, &selection) {
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            let mask = selection
                .as_ref()
                .map(|mask| {
                    self.painter.create_selection_mask_at(
                        mask.origin(),
                        mask.dimensions(),
                        &mask.packed_bits(),
                    )
                })
                .transpose()
                .map_err(|error| format!("Selection could not be adopted by the GPU: {error:?}"))?;
            self.painter.set_selection_mask(mask.as_ref());
            self.alpha_painter.set_selection_mask(mask.as_ref());
            self.eraser.set_selection_mask(mask.as_ref());
            self.scene.set_selection_overlay(mask.as_ref());
        }
        self.stroke.selection = selection.clone();
        self.selection = selection;
        Ok(())
    }

    fn advance_save_probe(&mut self) {
        let Ok(mode) = std::env::var(SAVE_PROBE_ENV) else {
            return;
        };
        if self.save_probe_step < 2 {
            self.stroke
                .live_ink
                .push_editor_command(
                    self.projection.current().revision,
                    EditorCommand::Project(ProjectCommand::Save),
                )
                .expect("scratch Save probe command fits bounded lane");
            self.save_probe_step += 1;
        } else if self.save_probe_step == 2 && mode == "end" && self.pending_save.is_some() {
            let mut end = input_probe_sample(2, PointerPhase::End);
            end.sequence = 3;
            self.stroke
                .live_ink
                .push(end)
                .expect("scratch End fits bounded lane");
            self.save_probe_step = 3;
        }
    }

    fn flush_pending_save(&mut self) {
        let Some(generation) = self.pending_save else {
            return;
        };
        if self.artwork_job.is_some()
            || self.stroke.active.is_some()
            || !self.stroke.pending_materializations.is_empty()
        {
            return;
        }
        let Some(path) = self.project_export_path.clone() else {
            return;
        };
        match self
            .stroke
            .materializer
            .enqueue_export(generation, path, false)
        {
            Ok(()) => {
                self.pending_save = None;
                // The FIFO now includes every closed stroke represented by
                // this export. Completion remains a separate worker status.
                let mut saved = self.projection.current().clone();
                saved.dirty = false;
                self.projection.install_authoritative(saved);
                self.publish_current_projection();
                println!(
                    "native-canvas event=project-save-accepted export_generation={generation} project_durability=independent export=queued"
                );
            }
            Err(ExportEnqueueError::Busy) => {}
            Err(ExportEnqueueError::Disconnected) => {
                self.pending_save = None;
                self.stroke
                    .live_ink
                    .publish_export_status(ExportStatus::Failed { generation });
            }
        }
    }

    fn reject_command(&self, command: nyatidraw_api::CommandId, reason: CommandRejectReason) {
        eprintln!(
            "native-canvas event=semantic-command-rejected command={} reason={reason:?}",
            command.0,
        );
        self.stroke.live_ink.publish_editor_event(
            EditorEvent::CommandRejected { command, reason },
            (!self.stroke.live_ink.workspace_failed()).then(|| self.projection.current().clone()),
        );
    }

    fn synchronize_workspace_failure(&mut self) -> bool {
        if !self.stroke.live_ink.workspace_failed() {
            return false;
        }
        let (authoritative, _) = self.stroke.live_ink.protocol_snapshot();
        self.projection.install_authoritative(authoritative);
        self.stroke.fail_closed("workspace-error-latched");
        self.stroke.live_ink.fail_incomplete_export();
        true
    }

    /// A closed valid stroke has changed the durable project state, and its
    /// sibling PNG is now stale until the next accepted Save/export. Publish
    /// this semantic boundary once for the whole native drain, never per raw
    /// sample or GPU dab.
    fn publish_closed_stroke_dirty(&mut self) {
        let previous_revision = self.projection.current().revision;
        let publish = self.projection.publish_editor_state(
            self.project_title.clone(),
            true,
            Some(self.active_layer),
            self.scene.tree(),
            self.projection.current().dock.clone(),
            self.view.projection(),
        );
        match publish {
            Ok(event) => {
                self.async_publication_base_revision
                    .get_or_insert(previous_revision);
                self.stroke
                    .live_ink
                    .publish_editor_event(event, Some(self.projection.current().clone()));
            }
            Err(error) => {
                eprintln!(
                    "native-canvas event=closed-stroke-dirty-publication-failed error={error:?}"
                );
                self.stroke.live_ink.publish_workspace_error(format!(
                    "Closed stroke was saved but its semantic state could not be published: {error:?}"
                ));
            }
        }
    }

    fn apply_materialized_tiles(&mut self) {
        let mut trace = crate::performance::MaterializedTrace::begin();
        let mut completed = Vec::new();
        self.stroke.materializer.drain_completed(&mut completed);
        trace.add_completed(completed.len());
        let latest_history = adopt_materialized_completions(
            &mut self.cpu_tiles,
            &mut self.deferred_uploads,
            completed,
            |bytes| trace.add_cpu_clone(bytes),
        );
        let active_layer = self.scene.active_live_stroke().map(LiveStrokeToken::layer);
        let mut deferred = Vec::new();
        for completion in self.deferred_uploads.drain(..) {
            let mut remaining = Vec::new();
            for (key, pixels) in completion.tiles {
                let newer_preview = self
                    .latest_preview_generation
                    .get(&key)
                    .is_some_and(|generation| *generation > completion.stroke_generation);
                if newer_preview {
                    continue;
                }
                if active_layer == Some(key.layer) {
                    trace.add_deferred();
                    remaining.push((key, pixels));
                    continue;
                }
                trace.add_upload_attempt();
                match self.scene.upload_closed_tile(key, &pixels) {
                    Ok(()) => {
                        trace.add_upload_success();
                        if self.latest_preview_generation.get(&key)
                            == Some(&completion.stroke_generation)
                        {
                            self.latest_preview_generation.remove(&key);
                        }
                    }
                    Err(LayerUploadError::LiveStrokeActive(_)) => {
                        trace.add_deferred();
                        remaining.push((key, pixels));
                    }
                    Err(error) => eprintln!(
                        "live-ink event=closed-tile-display-upload-failed stroke={} error={error:?}",
                        completion.ordinal,
                    ),
                }
            }
            if !remaining.is_empty() {
                deferred.push(DeferredTileUpload {
                    ordinal: completion.ordinal,
                    stroke_generation: completion.stroke_generation,
                    tiles: remaining,
                });
            }
        }
        self.deferred_uploads = deferred;
        if let Some(history) = latest_history {
            self.projection.stage_history(history);
            self.publish_current_projection();
        }
    }

    /// Publishes staged editor/session metadata without creating artwork history.
    /// History is staged separately after writer confirmation; a usage-only
    /// publication must not claim an active stroke is durably committed.
    fn publish_current_projection(&mut self) {
        self.stage_cancel_availability();
        let _timing = Span::new(Stage::Projection);
        let publish = self.projection.publish_editor_state(
            self.project_title.clone(),
            self.projection.current().dirty,
            Some(self.active_layer),
            self.scene.tree(),
            self.projection.current().dock.clone(),
            self.view.projection(),
        );
        match publish {
            Ok(event) => self
                .stroke
                .live_ink
                .publish_editor_event(event, Some(self.projection.current().clone())),
            Err(error) => {
                eprintln!("native-canvas event=projection-publication-failed error={error:?}");
                self.stroke.live_ink.publish_workspace_error(format!(
                    "The editor state could not be published: {error:?}"
                ));
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Keep generation checks and fail-closed Begin/Dabs/Finish handling together"
    )]
    fn execute_gpu_ops(&mut self, drained_samples: usize) {
        let _timing = (!self.stroke.gpu_ops.is_empty()).then(|| Span::new(Stage::GpuOps));
        for op in std::mem::take(&mut self.stroke.gpu_ops) {
            match op {
                GpuStrokeOp::Replace {
                    generation,
                    layer,
                    color,
                    eraser,
                    alpha_locked,
                } => {
                    if !eraser {
                        self.painter.set_brush_rgba(color);
                        self.alpha_painter.set_brush_rgba(color);
                    }
                    match self.scene.replace_live_stroke(layer) {
                        Ok(replacement) => {
                            if replacement.cancelled.is_some()
                                && let Some((previous_generation, _, _, _)) =
                                    self.gpu_live_generation
                            {
                                self.latest_preview_generation
                                    .retain(|_, latest| *latest != previous_generation);
                            }
                            // A discontinuity can discard an admitted Begin
                            // before GPU submission. Semantic generations and
                            // device-local tokens therefore need not coincide.
                            self.gpu_live_generation =
                                Some((generation, replacement.active, eraser, alpha_locked));
                        }
                        Err(error) => {
                            self.fail_gpu_transaction("begin", generation, None, &error);
                            break;
                        }
                    }
                }
                GpuStrokeOp::Dabs { generation, dabs } => {
                    let Some((active_generation, token, eraser, alpha_locked)) =
                        self.gpu_live_generation
                    else {
                        continue;
                    };
                    if active_generation != generation {
                        continue;
                    }
                    let painter = if eraser {
                        &mut self.eraser
                    } else if alpha_locked {
                        &mut self.alpha_painter
                    } else {
                        &mut self.painter
                    };
                    match self.scene.apply_live_dabs(painter, token, &dabs) {
                        Ok(submission) => {
                            if let Some(dirty) = submission.document_dirty {
                                for key in paint_dirty_tiles(token.layer(), dirty) {
                                    self.latest_preview_generation.insert(key, generation);
                                }
                            }
                            if submission.dab_count > 0 {
                                self.gpu_batches = self.gpu_batches.saturating_add(1);
                                if self.gpu_batch_logs_remaining > 0 {
                                    self.gpu_batch_logs_remaining -= 1;
                                    println!(
                                        "live-ink event=gpu-dab-batch source=native-input batch={} layer={} generation={} samples={} dabs={} dirty={:?} elapsed_ms={}",
                                        self.gpu_batches,
                                        token.layer().0,
                                        generation,
                                        drained_samples,
                                        submission.dab_count,
                                        submission.dirty,
                                        elapsed_since_launch(),
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            self.fail_gpu_transaction("apply", generation, Some(token), &error);
                            break;
                        }
                    }
                }
                GpuStrokeOp::Finish {
                    generation,
                    disposition,
                } => {
                    let Some((active_generation, token, eraser, alpha_locked)) =
                        self.gpu_live_generation.take()
                    else {
                        continue;
                    };
                    if active_generation != generation {
                        self.gpu_live_generation =
                            Some((active_generation, token, eraser, alpha_locked));
                        continue;
                    }
                    match self.scene.finish_live_stroke(token, disposition) {
                        Ok(_) if disposition == LiveStrokeDisposition::Cancel => {
                            self.latest_preview_generation
                                .retain(|_, preview_generation| *preview_generation != generation);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.fail_gpu_transaction("finish", generation, Some(token), &error);
                            break;
                        }
                    }
                }
            }
        }
        if let Some(active) = self.stroke.active.as_mut() {
            // The pending vector was consumed above. A later discontinuity
            // must truncate only ops accumulated in its new drain.
            active.gpu_op_start = 0;
        }
    }

    fn fail_gpu_transaction(
        &mut self,
        stage: &str,
        generation: u64,
        failed_token: Option<LiveStrokeToken>,
        error: &impl std::fmt::Debug,
    ) {
        let token = failed_token
            .or_else(|| self.gpu_live_generation.map(|(_, token, _, _)| token))
            .or_else(|| self.scene.active_live_stroke());
        let failed_generation = self
            .gpu_live_generation
            .map_or(generation, |(active, _, _, _)| active);
        self.gpu_live_generation = None;
        if let Some(token) = token {
            self.latest_preview_generation
                .retain(|_, preview_generation| *preview_generation != failed_generation);
            if let Err(cancel_error) = self
                .scene
                .finish_live_stroke(token, LiveStrokeDisposition::Cancel)
            {
                eprintln!(
                    "native-canvas event=gpu-live-cancel-after-failure-failed generation={generation} error={cancel_error:?}"
                );
            }
        }
        self.stroke.fail_closed("gpu-transaction-failed");
        self.stroke.gpu_ops.clear();
        let summary =
            format!("GPU live stroke failed at {stage}; drawing input was quarantined: {error:?}");
        eprintln!(
            "native-canvas event=gpu-live-fatal stage={stage} generation={generation} error={error:?} admission=quarantine"
        );
        self.stroke.live_ink.publish_workspace_error(summary);
    }
}

#[derive(Clone, Copy, Debug)]
struct RendererView {
    pan: Point,
    zoom: f64,
    rotation_radians: f64,
    mirrored_horizontal: bool,
    initialized: bool,
}

impl Default for RendererView {
    fn default() -> Self {
        Self {
            pan: Point { x: 0.0, y: 0.0 },
            zoom: 1.0,
            rotation_radians: 0.0,
            mirrored_horizontal: false,
            initialized: false,
        }
    }
}

impl RendererView {
    // This reverses the bounded fixed-point UI projection produced from the
    // renderer's f64 view; the protocol validation keeps values finite.
    #[allow(clippy::cast_precision_loss)]
    fn from_projection(projection: ViewportProjection) -> Self {
        Self {
            pan: Point {
                x: projection.pan_x_milli as f64 / 1_000.0,
                y: projection.pan_y_milli as f64 / 1_000.0,
            },
            zoom: f64::from(projection.zoom_ppm) / 1_000_000.0,
            rotation_radians: (f64::from(projection.rotation_millidegrees) / 1_000.0).to_radians(),
            mirrored_horizontal: projection.mirrored_horizontal,
            initialized: true,
        }
    }

    fn ensure_initialized(&mut self, width: u32, height: u32, scale: f64, document_size: [u32; 2]) {
        if !self.initialized {
            self.fit(width, height, scale, document_size);
        }
    }

    fn fit(&mut self, width: u32, height: u32, scale: f64, document_size: [u32; 2]) {
        let logical_width = f64::from(width) / scale;
        let logical_height = f64::from(height) / scale;
        // Tiny imported PNGs can otherwise fit above the input transform's
        // maximum zoom and prevent the first canvas frame from rendering.
        self.zoom = ((logical_width / f64::from(document_size[0]))
            .min(logical_height / f64::from(document_size[1]))
            * 0.9)
            .clamp(ViewportTransform::MIN_ZOOM, ViewportTransform::MAX_ZOOM);
        self.pan = Point {
            x: (logical_width - f64::from(document_size[0]) * self.zoom) * 0.5,
            y: (logical_height - f64::from(document_size[1]) * self.zoom) * 0.5,
        };
        self.rotation_radians = 0.0;
        self.mirrored_horizontal = false;
        self.initialized = true;
    }

    fn actual_pixels(&mut self, width: u32, height: u32, scale: f64, document_size: [u32; 2]) {
        let logical_width = f64::from(width) / scale;
        let logical_height = f64::from(height) / scale;
        self.zoom = 1.0 / scale;
        self.pan = Point {
            x: (logical_width - f64::from(document_size[0]) * self.zoom) * 0.5,
            y: (logical_height - f64::from(document_size[1]) * self.zoom) * 0.5,
        };
        self.rotation_radians = 0.0;
        self.mirrored_horizontal = false;
        self.initialized = true;
    }

    fn zoom_at(&mut self, focus: Point, steps: i8) {
        let next_zoom = (self.zoom * 1.25_f64.powi(i32::from(steps)))
            .clamp(ViewportTransform::MIN_ZOOM, ViewportTransform::MAX_ZOOM);
        self.change_at(
            focus,
            next_zoom,
            self.rotation_radians,
            self.mirrored_horizontal,
        );
    }

    /// Logical-only helper; the actual window mapping is validated on publication.
    fn mapping(self) -> ViewportTransform {
        ViewportTransform {
            revision: 0,
            window_origin_physical: Point::default(),
            physical_size: [1, 1],
            dpi_scale: 1.0,
            pan: self.pan,
            zoom: self.zoom,
            rotation_radians: self.rotation_radians,
            mirrored_horizontal: self.mirrored_horizontal,
        }
    }

    fn change_at(&mut self, focus: Point, zoom: f64, rotation: f64, mirrored: bool) {
        if let Some(view) = self.mapping().with_view_at(focus, zoom, rotation, mirrored) {
            self.pan = view.pan;
            self.zoom = view.zoom;
            self.rotation_radians = view.rotation_radians;
            self.mirrored_horizontal = view.mirrored_horizontal;
        }
    }

    // RendererView has already passed ViewportTransform validation and zoom is
    // clamped positive, so these fixed-point UI casts cannot change sign.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn projection(self) -> ViewportProjection {
        ViewportProjection {
            pan_x_milli: (self.pan.x * 1_000.0).round() as i64,
            pan_y_milli: (self.pan.y * 1_000.0).round() as i64,
            zoom_ppm: (self.zoom * 1_000_000.0).round() as u32,
            rotation_millidegrees: (self.rotation_radians.to_degrees() * 1_000.0).round() as i32,
            mirrored_horizontal: self.mirrored_horizontal,
        }
    }
}

fn navigator_viewport(
    view: RendererView,
    width: u32,
    height: u32,
    scale: f64,
    canvas: CanvasSpec,
) -> Option<NavigatorViewport> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let transform = ViewportTransform {
        revision: 0,
        window_origin_physical: Point { x: 0.0, y: 0.0 },
        physical_size: [width, height],
        dpi_scale: scale,
        pan: view.pan,
        zoom: view.zoom,
        rotation_radians: view.rotation_radians,
        mirrored_horizontal: view.mirrored_horizontal,
    };
    if transform.validate().is_err() {
        return None;
    }
    let logical_width = f64::from(width) / scale;
    let logical_height = f64::from(height) / scale;
    let corners = [
        Point { x: 0.0, y: 0.0 },
        Point {
            x: logical_width,
            y: 0.0,
        },
        Point {
            x: logical_width,
            y: logical_height,
        },
        Point {
            x: 0.0,
            y: logical_height,
        },
    ];
    let mut quad = [[0; 2]; 4];
    for (target, corner) in quad.iter_mut().zip(corners) {
        let document = transform.logical_to_document(corner)?;
        *target = [
            normalized_page_coordinate(document.x, canvas.width_px),
            normalized_page_coordinate(document.y, canvas.height_px),
        ];
    }
    Some(NavigatorViewport {
        quad_page_10k: quad,
    })
}

#[allow(clippy::cast_possible_truncation)]
fn normalized_page_coordinate(coordinate: f64, extent: u32) -> i32 {
    let value = coordinate / f64::from(extent) * 10_000.0;
    if value.is_finite() {
        value
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    } else {
        0
    }
}

fn projection_rejection(error: ProjectionError) -> CommandRejectReason {
    match error {
        ProjectionError::RevisionExhausted => CommandRejectReason::RevisionExhausted,
        ProjectionError::UnknownActiveLayer(_) => CommandRejectReason::UnknownLayer,
    }
}

fn empty_raster(id: LayerId, name: String) -> LayerNode {
    LayerNode {
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id,
        name,
        visible: true,
        locked: false,
        reference: false,
        opacity_u16: u16::MAX,
        content_root: ContentRootId(0),
    }
}

// Compare pixels once for GPU upload and CPU adoption. Borrowing both maps
// keeps that decision valid until all GPU uploads succeed and apply consumes
// the plan. Dropping a failed upload's plan leaves the CPU cache unchanged.
struct CpuSnapshotAdoption<'a> {
    cache: &'a mut BTreeMap<TileKey, Vec<u8>>,
    snapshot: &'a TileSnapshot,
    tiles: Vec<(TileKey, bool)>,
}

impl<'a> CpuSnapshotAdoption<'a> {
    fn new(cache: &'a mut BTreeMap<TileKey, Vec<u8>>, snapshot: &'a TileSnapshot) -> Self {
        let keys: BTreeSet<_> = cache
            .keys()
            .copied()
            .chain(snapshot.iter().map(|(key, _)| key))
            .collect();
        let tiles = keys
            .into_iter()
            .map(|key| {
                let changed = cache.get(&key).map(Vec::as_slice)
                    != snapshot.get(key).map(nyatidraw_tiles::TileObject::pixels);
                (key, changed)
            })
            .collect();
        Self {
            cache,
            snapshot,
            tiles,
        }
    }

    fn upload_keys(&self, rebuild_surfaces: bool) -> impl Iterator<Item = TileKey> + '_ {
        self.tiles
            .iter()
            .filter(move |(_, changed)| rebuild_surfaces || *changed)
            .map(|(key, _)| *key)
    }

    fn apply(self) {
        let _timing = Span::new(Stage::HistoryCpuSnapshot);
        for (key, changed) in self.tiles {
            if !changed {
                continue;
            }
            if let Some(tile) = self.snapshot.get(key) {
                // Reuse surviving allocations; a new key starts with an empty Vec.
                tile.pixels().clone_into(self.cache.entry(key).or_default());
            } else {
                // Undo to an empty layer must not retain deleted artwork.
                self.cache.remove(&key);
            }
        }
    }
}

fn valid_active_layer(tree: &LayerTree, preferred: LayerId) -> LayerId {
    if tree
        .ancestors(nyatidraw_api::LayerTreeNodeId::Raster(preferred))
        .is_some()
    {
        preferred
    } else {
        raster_layer_ids(tree)
            .first()
            .copied()
            .unwrap_or(LIVE_LAYER)
    }
}

pub(crate) fn default_layer_tree() -> LayerTree {
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT_GROUP,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(empty_raster(
            LIVE_LAYER,
            "Layer 1".into(),
        ))],
    })
    .expect("new document has one transparent raster layer")
}

/// Older projects without layer metadata used this topology. Never reinterpret
/// their existing pixels using the new-document default (which would hide layer 2).
pub(crate) fn legacy_layer_tree() -> LayerTree {
    let raster = |id, name: &str| {
        LayerTreeNode::Raster(LayerNode {
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id,
            name: name.into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    };
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT_GROUP,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![
            raster(BACKGROUND_LAYER, "Background"),
            LayerTreeNode::Group(GroupNode {
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: INK_GROUP,
                name: "Ink".into(),
                visible: true,
                opacity_u16: u16::MAX,
                children: vec![raster(LIVE_LAYER, "Ink 1")],
            }),
        ],
    })
    .expect("built-in layer tree is valid")
}

pub(crate) fn next_layer_node_id(tree: &LayerTree) -> Result<u128, String> {
    fn visit(group: &GroupNode, maximum: &mut u128) {
        *maximum = (*maximum).max(group.id.0);
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => *maximum = (*maximum).max(layer.id.0),
                LayerTreeNode::Group(child_group) => visit(child_group, maximum),
            }
        }
    }

    let mut maximum = 0;
    visit(tree.root(), &mut maximum);
    maximum
        .checked_add(1)
        .ok_or_else(|| "layer identifier space is exhausted".to_owned())
}

fn next_default_node_name(tree: &LayerTree, group_name: bool) -> String {
    fn count(group: &GroupNode, groups: &mut usize, rasters: &mut usize) {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(_) => *rasters = rasters.saturating_add(1),
                LayerTreeNode::Group(child_group) => {
                    *groups = groups.saturating_add(1);
                    count(child_group, groups, rasters);
                }
            }
        }
    }

    let mut groups = 0;
    let mut rasters = 0;
    count(tree.root(), &mut groups, &mut rasters);
    if group_name {
        format!("Group {}", groups.saturating_add(1))
    } else {
        format!("Layer {}", rasters.saturating_add(1))
    }
}

fn opaque_white_layer_tiles(canvas: CanvasSpec, layer: LayerId) -> TileSnapshot {
    let columns = canvas.width_px.div_ceil(TILE_EDGE);
    let rows = canvas.height_px.div_ceil(TILE_EDGE);
    TileSnapshot::from_tiles((0..rows).flat_map(|y| {
        (0..columns).map(move |x| {
            let mut pixels = vec![0; TILE_BYTE_LEN];
            let width = (canvas.width_px - x * TILE_EDGE).min(TILE_EDGE) as usize;
            let height = (canvas.height_px - y * TILE_EDGE).min(TILE_EDGE) as usize;
            for row in 0..height {
                let start = row * TILE_EDGE as usize * 4;
                pixels[start..start + width * 4].fill(u8::MAX);
            }
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: i32::try_from(x).expect("document tile column fits i32"),
                    y: i32::try_from(y).expect("document tile row fits i32"),
                },
                pixels,
            )
        })
    }))
    .expect("white raster tiles are canonical")
}

fn enqueue_synthetic_durability_probe(live_ink: &LiveInkBridge) -> Result<(), String> {
    const STROKE_COUNT: u64 = 32;
    let storage = if std::env::var_os(PROJECT_PATH_ENV).is_some() {
        "explicit-project"
    } else {
        "untitled-recovery"
    };
    for stroke in 0..STROKE_COUNT {
        let eraser = stroke >= 24;
        let base_sequence = stroke * 3 + 1;
        let column = u32::try_from(stroke % 8).expect("probe column is in 0..8");
        let offset = f64::from(column) * 18.0;
        let fixtures = [
            (PointerPhase::Begin, base_sequence, 180.0 + offset, 210.0),
            (PointerPhase::Move, base_sequence + 1, 240.0 + offset, 270.0),
            (PointerPhase::End, base_sequence + 2, 300.0 + offset, 230.0),
        ];
        for (phase, sequence, x, y) in fixtures {
            live_ink
                .push(StylusSample {
                    sequence,
                    timestamp_ns: sequence * 1_000_000,
                    device_id: u64::MAX,
                    phase,
                    position_document: Point { x, y },
                    pressure: 0.75,
                    tilt: None,
                    twist_radians: None,
                    tangential_pressure: None,
                    buttons: PenButtons::default(),
                    eraser,
                    viewport_revision: 0,
                })
                .map_err(|error| format!("durability-probe-input:{error:?}"))?;
        }
    }
    println!(
        "native-canvas event=durability-probe-enqueued input=synthetic strokes={STROKE_COUNT} eraser_strokes=8 storage={storage} claim=desktop-pipeline-and-close-drain-only physical-pen-proof=false"
    );
    Ok(())
}

fn input_probe_sample(sequence: u64, phase: PointerPhase) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence.saturating_mul(1_000_000),
        device_id: u64::MAX - 1,
        phase,
        position_document: Point {
            x: 256.0 + f64::from(u32::try_from(sequence % 64).expect("remainder fits u32")),
            y: 256.0,
        },
        pressure: 0.75,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    }
}

// The cached layer lock is document metadata, independent of the existing
// discontinuity/fatal/shutdown lifecycle flags; it is not another lifecycle state.
#[allow(clippy::struct_excessive_bools)]
struct StrokePipeline {
    /// Native-only pending Begin and one latest target; no sample enters UI.
    transform_starting: bool,
    transform_paste: bool,
    pending_tool: Option<DrawingTool>,
    pending_pencil_template: Option<nyatidraw_api::PencilTemplate>,
    revalidating_transform: bool,
    transform_projection: Option<nyatidraw_api::TransformProjection>,
    transform_gesture: Option<(
        crate::transform_gesture::TransformGesture,
        u64,
        nyatidraw_api::AffineTransform,
    )>,
    pending_transform: Option<nyatidraw_api::AffineTransform>,
    pending_transform_cancel: bool,
    edit_gesture: Option<crate::edit_gesture::EditGesture>,
    temporary_picker_active: bool,
    completed_temporary_picker: Option<(u64, u64)>,
    picker: picker_preview::PickerRuntime,
    picker_solo: Option<nyatidraw_api::LayerTreeNodeId>,
    completed_edit: Option<Result<EditCommand, String>>,
    selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    live_ink: LiveInkBridge,
    evaluator: RoundBrushEvaluator,
    active: Option<LiveStroke>,
    queued: Vec<AdmittedSample>,
    pending_materializations: VecDeque<ClosedStrokeRequest>,
    materialization_backlog_capacity: usize,
    gpu_ops: Vec<GpuStrokeOp>,
    scratch_dabs: Vec<BrushDab>,
    next_generation: u64,
    selected_layer: LayerId,
    selected_layer_locked: bool,
    selected_layer_alpha_locked: bool,
    drawing: DrawingConfig,
    closed_strokes: u64,
    awaiting_clean_begin: bool,
    input_discontinuities: u64,
    recovery_quarantined: u64,
    last_discontinued_layer: Option<LayerId>,
    last_closed_layer: Option<LayerId>,
    fatal: bool,
    shutdown_prepared: bool,
    materializer: MaterializationWorker,
}

#[derive(Clone, Copy, Debug, Default)]
struct StrokeDrain {
    samples: usize,
    artwork_closed: bool,
    /// Bounded newest-first Begin usage; never carries samples through the UI.
    started_brush_sizes: [Option<u16>; 4],
    /// Source sRGB colors captured with the accepted immutable stroke config.
    /// Erasing consumes no painting color. This lane has the same bound as MRU.
    started_brush_colors: [Option<[u8; 4]>; 10],
}

impl StrokeDrain {
    fn record_brush_begin(&mut self, size: u16, color: Option<[u8; 4]>) {
        let index = self
            .started_brush_sizes
            .iter()
            .position(|entry| *entry == Some(size))
            .unwrap_or(self.started_brush_sizes.len() - 1);
        self.started_brush_sizes[..=index].rotate_right(1);
        self.started_brush_sizes[0] = Some(size);
        if let Some(color) = color {
            let index = self
                .started_brush_colors
                .iter()
                .position(|entry| *entry == Some(color))
                .unwrap_or(self.started_brush_colors.len() - 1);
            self.started_brush_colors[..=index].rotate_right(1);
            self.started_brush_colors[0] = Some(color);
        }
    }
}

impl StrokePipeline {
    fn new(live_ink: LiveInkBridge, materializer: MaterializationWorker) -> Self {
        let materialization_backlog_capacity = live_ink.maximum_drain_len();
        Self {
            live_ink,
            transform_starting: false,
            transform_paste: false,
            pending_tool: None,
            pending_pencil_template: None,
            revalidating_transform: false,
            transform_projection: None,
            transform_gesture: None,
            pending_transform: None,
            pending_transform_cancel: false,
            selection: None,
            edit_gesture: None,
            temporary_picker_active: false,
            completed_temporary_picker: None,
            picker: picker_preview::PickerRuntime::default(),
            picker_solo: None,
            completed_edit: None,
            evaluator: RoundBrushEvaluator::new(0x4e41_5941_5449),
            active: None,
            queued: Vec::with_capacity(materialization_backlog_capacity),
            pending_materializations: VecDeque::with_capacity(materialization_backlog_capacity),
            materialization_backlog_capacity,
            gpu_ops: Vec::with_capacity(256),
            scratch_dabs: Vec::with_capacity(256),
            next_generation: 1,
            selected_layer: LIVE_LAYER,
            selected_layer_locked: false,
            selected_layer_alpha_locked: false,
            drawing: DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty()),
            closed_strokes: 0,
            awaiting_clean_begin: false,
            input_discontinuities: 0,
            recovery_quarantined: 0,
            last_discontinued_layer: None,
            last_closed_layer: None,
            fatal: false,
            shutdown_prepared: false,
            materializer,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn drain(&mut self, active_layer: LayerId, drawing: DrawingConfig) -> StrokeDrain {
        self.selected_layer = active_layer;
        self.drawing = drawing;
        if self.live_ink.workspace_failed() {
            self.fail_closed("workspace-error-latched");
        }
        self.flush_pending_materializations();

        let mut queued_samples = std::mem::take(&mut self.queued);
        queued_samples.clear();
        let discontinuity = self.live_ink.drain_into(&mut queued_samples);
        let drained = queued_samples.len();
        let _timing = (drained > 0).then(|| Span::new(Stage::DrainBrush));

        if self.fatal {
            if drained > 0 || discontinuity.is_some() {
                eprintln!(
                    "live-ink event=input-quarantined reason=workspace-failed drained={} discontinuity={}",
                    drained,
                    discontinuity.is_some(),
                );
            }
            self.queued = queued_samples;
            return StrokeDrain {
                samples: drained,
                artwork_closed: false,
                ..StrokeDrain::default()
            };
        }

        let mut artwork_closed = false;
        let mut report = StrokeDrain::default();
        for admitted in queued_samples.drain(..) {
            let mut sample = admitted.queued.sample;
            if sample.phase == PointerPhase::Begin {
                self.cancel_picker();
                self.temporary_picker_active = admitted.temporary_picker;
            }
            let temporary_picker = self.temporary_picker_active;
            if matches!(sample.phase, PointerPhase::End | PointerPhase::Cancel) {
                self.temporary_picker_active = false;
            }
            let gesture_tool = if temporary_picker {
                DrawingTool::Eyedropper
            } else {
                drawing.tool
            };
            // Lock changes are admitted only at an idle artwork boundary.
            // Still drain every phase, but never create preview or durable ink.
            // Selection-only gestures remain available on locked artwork.
            if self.selected_layer_locked
                && !matches!(
                    gesture_tool,
                    DrawingTool::Wand
                        | DrawingTool::Lasso
                        | DrawingTool::RectangleSelection
                        | DrawingTool::Eyedropper
                )
            {
                continue;
            }
            if self.awaiting_clean_begin && sample.phase != PointerPhase::Begin {
                self.recovery_quarantined = self.recovery_quarantined.saturating_add(1);
                if temporary_picker && sample.phase == PointerPhase::End {
                    self.live_ink
                        .complete_temporary_pick((sample.device_id, sample.sequence));
                }
                continue;
            }
            if self.awaiting_clean_begin {
                self.awaiting_clean_begin = false;
                println!(
                    "live-ink event=input-discontinuity-recovered clean_begin_sequence={} quarantined_after_ack={}",
                    sample.sequence, self.recovery_quarantined,
                );
            }
            if self.transform_projection.is_some() || self.transform_starting {
                if !temporary_picker {
                    self.observe_transform_sample(sample);
                }
                if temporary_picker && sample.phase == PointerPhase::End {
                    self.live_ink
                        .complete_temporary_pick((sample.device_id, sample.sequence));
                }
                continue;
            }
            if gesture_tool == DrawingTool::MoveSelection && !temporary_picker {
                if sample.phase != PointerPhase::Begin || admitted.begin_layer == Some(active_layer)
                {
                    self.begin_native_transform(sample);
                }
                continue;
            }
            if gesture_tool.is_edit() {
                match sample.phase {
                    PointerPhase::Begin if admitted.begin_layer != Some(active_layer) => {
                        self.edit_gesture = None;
                        self.completed_edit = Some(Err(
                            "대상 레이어가 변경되어 선택 제스처를 취소했습니다.".into(),
                        ));
                    }
                    PointerPhase::Begin => {
                        // A clean new Begin also cancels any incomplete paint
                        // preview; switching to a read must never close it.
                        if let Some(previous) = self.active.take() {
                            self.scratch_dabs.clear();
                            let _ = self.evaluator.end(previous.token, &mut self.scratch_dabs);
                            self.gpu_ops.push(GpuStrokeOp::Finish {
                                generation: previous.generation,
                                disposition: LiveStrokeDisposition::Cancel,
                            });
                        }
                        let settings = if temporary_picker {
                            nyatidraw_api::EditSettings {
                                source: nyatidraw_api::EditSource::AllVisible,
                                ..drawing.edit_settings
                            }
                        } else {
                            drawing.edit_settings
                        };
                        self.edit_gesture = Some(crate::edit_gesture::EditGesture::begin(
                            gesture_tool,
                            settings,
                            drawing.stroke_color().0,
                            self.selection.is_some(),
                            sample,
                        ));
                        if let Some(point) = self
                            .edit_gesture
                            .as_ref()
                            .and_then(|gesture| gesture.picker_point(sample))
                        {
                            let source = if temporary_picker {
                                crate::edit_worker::PickerSource::Display(self.picker_solo)
                            } else {
                                crate::edit_worker::PickerSource::Artwork(settings.source)
                            };
                            self.begin_picker(sample, point, admitted.picker_epoch, source);
                        }
                    }
                    PointerPhase::Move => {
                        if let Some(gesture) = &mut self.edit_gesture {
                            gesture.push(sample);
                        }
                        let point = self
                            .edit_gesture
                            .as_ref()
                            .and_then(|gesture| gesture.picker_point(sample));
                        self.move_picker(sample, point);
                    }
                    PointerPhase::Cancel => {
                        self.edit_gesture = None;
                        self.cancel_picker();
                    }
                    PointerPhase::End => {
                        if let Some(gesture) = self.edit_gesture.take() {
                            self.completed_temporary_picker =
                                temporary_picker.then_some((sample.device_id, sample.sequence));
                            let result = gesture.finish(sample);
                            if let Some(picker) = &mut self.picker.gesture {
                                picker.released = true;
                                self.live_ink.publish_picker_snapshot(None);
                            }
                            self.completed_edit = Some(if self.completed_edit.is_none() {
                                result
                            } else {
                                Err(
                                    "여러 선택 제스처가 겹쳐 적용하지 않았습니다. 다시 시도하세요."
                                        .into(),
                                )
                            });
                        } else if temporary_picker {
                            self.live_ink
                                .complete_temporary_pick((sample.device_id, sample.sequence));
                        }
                    }
                }
                continue;
            }
            // Correct only accepted brush positions, once. Raw input diagnostics
            // are upstream; live dabs and retained replay samples below receive
            // this identical evaluated stream. Settings are captured at Begin.
            let mut begin_smoother = (sample.phase == PointerPhase::Begin)
                .then(|| StrokeSmoother::new(drawing.smoothing_for_stroke(sample.eraser)));
            let corrected = if let Some(smoother) = &mut begin_smoother {
                smoother.process(sample)
            } else if let Some(active) = &mut self.active {
                active.smoother.process(sample)
            } else {
                Ok(sample)
            };
            let Ok(corrected) = corrected else {
                self.cancel_invalid_smoothed_stroke(sample.sequence);
                continue;
            };
            sample = corrected;
            match sample.phase {
                PointerPhase::Begin => {
                    if self.pending_materializations.len() >= self.materialization_backlog_capacity
                    {
                        self.live_ink.publish_workspace_error(
                            "Drawing stopped because the durable stroke backlog is full".into(),
                        );
                        self.fail_closed("materialization-backlog-full");
                        continue;
                    }
                    if let Some(previous) = self.active.take() {
                        self.scratch_dabs.clear();
                        let record = self.evaluator.end(previous.token, &mut self.scratch_dabs);
                        self.gpu_ops.push(GpuStrokeOp::Finish {
                            generation: previous.generation,
                            disposition: LiveStrokeDisposition::Cancel,
                        });
                        println!(
                            "live-ink event=stroke-aborted reason=replaced-by-begin samples={} first_sequence={} last_sequence={}",
                            previous.samples.len(),
                            record.first_sample_sequence,
                            record.last_sample_sequence,
                        );
                    }
                    self.scratch_dabs.clear();
                    let eraser = drawing.tool == DrawingTool::Eraser || sample.eraser;
                    if self.selected_layer_alpha_locked && eraser {
                        self.completed_edit = Some(Err(
                            "투명도 잠금 중에는 지울 수 없습니다. 잠금을 해제하세요.".into(),
                        ));
                        self.awaiting_clean_begin = true;
                        continue;
                    }
                    sample.eraser = eraser;
                    let brush = drawing.preset_for_stroke(sample.eraser);
                    let color = drawing.stroke_color();
                    let gpu_color = drawing.gpu_color();
                    let token = begin_round_stroke(
                        &mut self.evaluator,
                        &brush,
                        sample,
                        &mut self.scratch_dabs,
                    );
                    let generation = self.next_generation;
                    self.next_generation = self.next_generation.saturating_add(1);
                    let layer = admitted.begin_layer.unwrap_or(active_layer);
                    let gpu_op_start = self.gpu_ops.len();
                    self.gpu_ops.push(GpuStrokeOp::Replace {
                        generation,
                        layer,
                        color: gpu_color,
                        eraser,
                        alpha_locked: self.selected_layer_alpha_locked,
                    });
                    append_gpu_dabs(&mut self.gpu_ops, generation, &mut self.scratch_dabs);
                    self.active = Some(LiveStroke {
                        token,
                        smoother: begin_smoother.expect("Begin owns a fresh smoother"),
                        generation,
                        layer,
                        gpu_op_start,
                        samples: vec![sample],
                        sample_limit_exceeded: false,
                        brush: BrushSnapshot { preset: brush },
                        color,
                        eraser,
                        alpha_locked: self.selected_layer_alpha_locked,
                        selection: self.selection.clone(),
                    });
                    // The preset may be the remembered hardware eraser, not
                    // the UI's selected brush. Record the accepted Begin only.
                    let used_size = if sample.eraser && drawing.tool != DrawingTool::Eraser {
                        drawing.remembered[3].size_tenths
                    } else {
                        drawing.size_tenths
                    };
                    // Preserve the exact sRGB source chosen for this accepted
                    // stroke, rather than round-tripping quantized tile bytes.
                    let used_color = (!eraser
                        && matches!(
                            drawing.tool,
                            DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush
                        ))
                    .then_some(drawing.color);
                    report.record_brush_begin(used_size, used_color);
                }
                PointerPhase::Move => {
                    if let Some(active) = self.active.as_mut() {
                        sample.eraser = active.eraser;
                        active.retain_sample(sample);
                        self.scratch_dabs.clear();
                        self.evaluator
                            .push(&mut active.token, &[sample], &mut self.scratch_dabs);
                        if active.token.is_discontinuous() {
                            self.cancel_native_brush_discontinuity(
                                sample.sequence,
                                "brush-dab-budget",
                                "입력 간격이 너무 커 현재 선을 취소했습니다. 다시 그려주세요.",
                            );
                            continue;
                        }
                        append_gpu_dabs(
                            &mut self.gpu_ops,
                            active.generation,
                            &mut self.scratch_dabs,
                        );
                    }
                }
                PointerPhase::End => {
                    if let Some(mut active) = self.active.take() {
                        sample.eraser = active.eraser;
                        active.retain_sample(sample);
                        self.scratch_dabs.clear();
                        self.evaluator
                            .push(&mut active.token, &[sample], &mut self.scratch_dabs);
                        if active.token.is_discontinuous() {
                            self.active = Some(active);
                            self.cancel_native_brush_discontinuity(
                                sample.sequence,
                                "brush-dab-budget",
                                "입력 간격이 너무 커 현재 선을 취소했습니다. 다시 그려주세요.",
                            );
                            continue;
                        }
                        append_gpu_dabs(
                            &mut self.gpu_ops,
                            active.generation,
                            &mut self.scratch_dabs,
                        );
                        self.scratch_dabs.clear();
                        let record = self.evaluator.end(active.token, &mut self.scratch_dabs);
                        append_gpu_dabs(
                            &mut self.gpu_ops,
                            active.generation,
                            &mut self.scratch_dabs,
                        );
                        self.closed_strokes = self.closed_strokes.saturating_add(1);
                        self.last_closed_layer = Some(active.layer);
                        println!(
                            "live-ink event=stroke-closed count={} samples={} first_sequence={} last_sequence={}",
                            self.closed_strokes,
                            record.sample_count,
                            record.first_sample_sequence,
                            record.last_sample_sequence,
                        );
                        if active.sample_limit_exceeded {
                            self.gpu_ops.push(GpuStrokeOp::Finish {
                                generation: active.generation,
                                disposition: LiveStrokeDisposition::Cancel,
                            });
                            eprintln!(
                                "live-ink event=stroke-materialization-rejected source=native-input count={} reason=sample-limit limit={MAX_SAMPLES_PER_STROKE}",
                                self.closed_strokes,
                            );
                        } else {
                            self.gpu_ops.push(GpuStrokeOp::Finish {
                                generation: active.generation,
                                disposition: LiveStrokeDisposition::Commit,
                            });
                            let request = ClosedStrokeRequest {
                                ordinal: self.closed_strokes,
                                stroke_generation: active.generation,
                                layer: active.layer,
                                recorded: record,
                                samples: active.samples,
                                brush: active.brush,
                                color: active.color,
                                alpha_locked: active.alpha_locked,
                                selection: active.selection,
                                end_timestamp_ns: sample.timestamp_ns,
                                enqueued_at: Instant::now(),
                            };
                            self.queue_closed_stroke(request);
                            artwork_closed = true;
                        }
                    } else {
                        eprintln!(
                            "live-ink event=stroke-materialization-rejected source=native-input reason=end-without-active sequence={}",
                            sample.sequence,
                        );
                    }
                }
                PointerPhase::Cancel => {
                    if let Some(active) = self.active.take() {
                        self.scratch_dabs.clear();
                        let record = self.evaluator.end(active.token, &mut self.scratch_dabs);
                        self.gpu_ops.push(GpuStrokeOp::Finish {
                            generation: active.generation,
                            disposition: LiveStrokeDisposition::Cancel,
                        });
                        println!(
                            "live-ink event=stroke-cancelled samples={} first_sequence={} last_sequence={}",
                            record.sample_count,
                            record.first_sample_sequence,
                            record.last_sample_sequence,
                        );
                    }
                }
            }
        }

        if let Some(discontinuity) = discontinuity {
            self.consume_discontinuity(discontinuity);
        }

        self.queued = queued_samples;
        StrokeDrain {
            samples: drained,
            artwork_closed,
            ..report
        }
    }

    fn cancel_invalid_smoothed_stroke(&mut self, sequence: u64) {
        self.cancel_native_brush_discontinuity(
            sequence,
            "invalid-smoothing-position",
            "잘못된 입력 좌표로 현재 선을 취소했습니다. 다시 그려주세요.",
        );
    }

    fn cancel_native_brush_discontinuity(&mut self, sequence: u64, reason: &str, message: &str) {
        if let Some(active) = self.active.take() {
            self.gpu_ops.truncate(active.gpu_op_start);
            self.scratch_dabs.clear();
            let _ = self.evaluator.end(active.token, &mut self.scratch_dabs);
            self.scratch_dabs.clear();
            self.gpu_ops.push(GpuStrokeOp::Finish {
                generation: active.generation,
                disposition: LiveStrokeDisposition::Cancel,
            });
        }
        self.awaiting_clean_begin = true;
        self.live_ink.publish_activation_notice(message.into());
        eprintln!(
            "live-ink event=stroke-cancelled reason={reason} sequence={sequence} materialized=false recovery=await-clean-begin"
        );
    }

    fn consume_discontinuity(&mut self, discontinuity: InputDiscontinuity) {
        self.cancel_picker();
        if matches!(
            self.completed_edit,
            Some(Ok(
                EditCommand::PickColor { .. } | EditCommand::PickDisplayColor { .. }
            ))
        ) {
            self.completed_edit = None;
            if let Some(sequence) = self.completed_temporary_picker.take() {
                self.live_ink.complete_temporary_pick(sequence);
            }
        }
        if let Some((_, _, original)) = self.transform_gesture.take() {
            self.pending_transform = Some(original);
        }
        self.temporary_picker_active = false;
        if self.edit_gesture.take().is_some() {
            self.completed_edit =
                Some(Err("입력 연속성이 끊겨 선택 제스처를 취소했습니다.".into()));
        }
        self.input_discontinuities = self.input_discontinuities.saturating_add(1);
        let cancelled = self.active.take().map(|active| {
            self.gpu_ops.truncate(active.gpu_op_start);
            let layer = active.layer;
            self.scratch_dabs.clear();
            let record = self.evaluator.end(active.token, &mut self.scratch_dabs);
            self.scratch_dabs.clear();
            self.gpu_ops.push(GpuStrokeOp::Finish {
                generation: active.generation,
                disposition: LiveStrokeDisposition::Cancel,
            });
            self.last_discontinued_layer = Some(layer);
            (layer, record.sample_count)
        });
        self.awaiting_clean_begin = true;
        let (active_cancelled, layer, samples) =
            cancelled.map_or((false, 0, 0), |(layer, count)| (true, layer.0, count));
        eprintln!(
            "live-ink event=input-discontinuity-consumed count={} first_lost_sequence={} phase={:?} quarantined_before_ack={} active_cancelled={} layer={} samples={} materialized=false recovery=await-clean-begin",
            self.input_discontinuities,
            discontinuity.first_lost_sequence,
            discontinuity.first_lost_phase,
            discontinuity.quarantined_before_ack,
            active_cancelled,
            layer,
            samples,
        );
    }

    fn fail_closed(&mut self, reason: &str) {
        self.cancel_picker();
        self.temporary_picker_active = false;
        self.edit_gesture = None;
        self.completed_edit = None;
        if self.fatal {
            return;
        }
        self.fatal = true;
        if let Some(active) = self.active.take() {
            self.gpu_ops.truncate(active.gpu_op_start);
            self.scratch_dabs.clear();
            let _ = self.evaluator.end(active.token, &mut self.scratch_dabs);
            self.scratch_dabs.clear();
            self.gpu_ops.push(GpuStrokeOp::Finish {
                generation: active.generation,
                disposition: LiveStrokeDisposition::Cancel,
            });
        }
        self.awaiting_clean_begin = true;
        eprintln!(
            "live-ink event=input-fail-closed reason={reason} active_materialized=false admission=quarantine"
        );
    }

    fn queue_closed_stroke(&mut self, request: ClosedStrokeRequest) {
        // A worker may free capacity during a native drain. New strokes must
        // still follow the older semantic backlog, especially paint/erase.
        if !self.pending_materializations.is_empty() {
            self.defer_materialization(request);
            return;
        }
        let ordinal = request.ordinal;
        match self.materializer.try_enqueue(request) {
            TryEnqueueStatus::Enqueued(outcome) => println!(
                "live-ink event=stroke-materialization-enqueued source=native-input count={ordinal} queue_full=false stroke_end_block_us=0 pending={}",
                outcome.pending,
            ),
            TryEnqueueStatus::Full(request) => self.defer_materialization(request),
            TryEnqueueStatus::Disconnected(request) => {
                self.defer_materialization(request);
                eprintln!(
                    "live-ink event=stroke-materialization-stalled source=native-input count={ordinal} reason=worker-disconnected retained=true"
                );
            }
        }
    }

    fn flush_pending_materializations(&mut self) {
        while let Some(request) = self.pending_materializations.pop_front() {
            let ordinal = request.ordinal;
            match self.materializer.try_enqueue(request) {
                TryEnqueueStatus::Enqueued(outcome) => println!(
                    "live-ink event=stroke-materialization-enqueued source=bounded-retry count={ordinal} queue_full=false stroke_end_block_us=0 pending={}",
                    outcome.pending,
                ),
                TryEnqueueStatus::Full(request) | TryEnqueueStatus::Disconnected(request) => {
                    self.pending_materializations.push_front(request);
                    break;
                }
            }
        }
    }

    fn defer_materialization(&mut self, request: ClosedStrokeRequest) {
        assert!(
            self.pending_materializations.len() < self.materialization_backlog_capacity,
            "one bounded input drain cannot exceed its matching closed-stroke backlog"
        );
        let ordinal = request.ordinal;
        self.pending_materializations.push_back(request);
        println!(
            "live-ink event=stroke-materialization-deferred source=native-input count={ordinal} reason=worker-busy backlog={} capacity={} stroke_end_block_us=0",
            self.pending_materializations.len(),
            self.materialization_backlog_capacity,
        );
    }

    /// Preserves a healthy in-progress stroke at its last received sample when
    /// its owner is closing. The semantic End is explicitly synthetic; it is
    /// never sent through the native recorder or counted as physical pen input.
    fn seal_active_for_shutdown(&mut self) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        if self.fatal || self.awaiting_clean_begin {
            return;
        }
        let mut end = *active.samples.last().expect("active stroke has a Begin");
        let Some(sequence) = end.sequence.checked_add(1).filter(|_| {
            !active.sample_limit_exceeded && active.samples.len() < MAX_SAMPLES_PER_STROKE
        }) else {
            self.live_ink.publish_workspace_error(
                "The active stroke exceeded its safe recording limits and could not be sealed on close".into(),
            );
            eprintln!(
                "live-ink event=active-stroke-close-rejected reason=recording-limit materialized=false"
            );
            return;
        };
        end.sequence = sequence;
        end.phase = PointerPhase::End;
        active.samples.push(end);
        self.scratch_dabs.clear();
        self.evaluator
            .push(&mut active.token, &[end], &mut self.scratch_dabs);
        let recorded = self.evaluator.end(active.token, &mut self.scratch_dabs);
        self.scratch_dabs.clear();
        self.closed_strokes = self.closed_strokes.saturating_add(1);
        let sample_count = recorded.sample_count;
        let request = ClosedStrokeRequest {
            ordinal: self.closed_strokes,
            stroke_generation: active.generation,
            layer: active.layer,
            recorded,
            samples: active.samples,
            brush: active.brush,
            color: active.color,
            alpha_locked: active.alpha_locked,
            selection: active.selection,
            end_timestamp_ns: end.timestamp_ns,
            enqueued_at: Instant::now(),
        };
        if self.materializer.enqueue_for_shutdown(request).is_err() {
            self.live_ink.publish_workspace_error(
                "The project writer stopped before the active stroke could be saved on close"
                    .into(),
            );
            eprintln!("live-ink event=active-stroke-close-rejected reason=worker-disconnected");
        } else {
            println!(
                "live-ink event=active-stroke-close-sealed source=shutdown-policy synthetic_end=true samples={} last_received_sequence={} physical-pen-proof=false",
                sample_count,
                sequence - 1,
            );
        }
    }
}

impl StrokePipeline {
    fn prepare_shutdown(&mut self) {
        if self.shutdown_prepared {
            return;
        }
        self.shutdown_prepared = true;
        // The owner will never consume another GPU retirement payload. Retire
        // that disposable display before blocking on any writer admission;
        // otherwise its bounded completion lane can stop the durable drain.
        self.materializer.retire_display();
        while let Some(request) = self.pending_materializations.pop_front() {
            if self.materializer.enqueue_for_shutdown(request).is_err() {
                break;
            }
        }
        let drained_on_close = self.drain(self.selected_layer, self.drawing);
        if drained_on_close.samples > 0 {
            println!(
                "live-ink event=input-close-drain samples={} invented_end=false",
                drained_on_close.samples,
            );
        }
        let queued = self.pending_materializations.len();
        if queued > 0 {
            println!(
                "live-ink event=stroke-materialization-close-drain queued={queued} mode=blocking-shutdown-only"
            );
        }
        while let Some(request) = self.pending_materializations.pop_front() {
            if self.materializer.enqueue_for_shutdown(request).is_err() {
                eprintln!(
                    "live-ink event=stroke-materialization-close-drain-failed reason=worker-disconnected"
                );
                break;
            }
        }
        self.seal_active_for_shutdown();
    }
}

impl Drop for StrokePipeline {
    fn drop(&mut self) {
        self.prepare_shutdown();
    }
}

impl Drop for ActiveCanvas {
    fn drop(&mut self) {
        // Semantic controls can change after the last render drain. Pending
        // Begin samples must use those accepted controls even if Close wins
        // the race with the next frame (for example Fill selected after Brush).
        self.stroke.selected_layer = self.active_layer;
        self.stroke.selected_layer_locked = self
            .scene
            .tree()
            .raster(self.active_layer)
            .is_none_or(|layer| layer.locked);
        self.stroke.selected_layer_alpha_locked = self
            .scene
            .tree()
            .raster(self.active_layer)
            .is_some_and(|layer| layer.alpha_locked);
        self.stroke.drawing = self.drawing;
        self.stroke.prepare_shutdown();
        // Temporary sampling changes UI color only. A transparent read cannot
        // become a closing artwork failure, and has nothing durable to flush.
        if let Some(sequence) = self.stroke.completed_temporary_picker.take() {
            self.stroke.completed_edit = None;
            self.stroke.live_ink.complete_temporary_pick(sequence);
        }
        // A native End received before Close must reach the writer FIFO before
        // export. Blocking here is confined to the dedicated close worker.
        if let Some(result) = self.stroke.completed_edit.take() {
            let result = result.and_then(|command| {
                let (reply, received) = sync_channel(1);
                self.stroke
                    .materializer
                    .sender
                    .as_ref()
                    .ok_or("Project writer unavailable")?
                    .send(WorkerRequest::Edit {
                        command,
                        target: self.active_layer,
                        reply,
                    })
                    .map_err(|_| "Project writer stopped")?;
                match received.recv().map_err(|_| "Edit result unavailable")? {
                    Ok(_) => Ok(()),
                    Err(EditFailure::Rejected(error) | EditFailure::Fatal(error)) => Err(error),
                }
            });
            if let Err(error) = result {
                self.stroke
                    .live_ink
                    .publish_workspace_error(format!("Closing edit was not applied: {error}"));
            }
        }
        if let Some(generation) = self.pending_save.take() {
            if self.stroke.live_ink.workspace_failed() {
                self.stroke.live_ink.fail_incomplete_export();
                self.stroke.live_ink.flush_layout();
                return;
            }
            if let Some(path) = self.project_export_path.clone()
                && self
                    .stroke
                    .materializer
                    .enqueue_export(generation, path, true)
                    .is_err()
            {
                self.stroke
                    .live_ink
                    .publish_export_status(ExportStatus::Failed { generation });
            }
        }
        self.stroke.live_ink.flush_layout();
    }
}

struct LiveStroke {
    alpha_locked: bool,
    selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    token: RoundBrushStroke,
    smoother: StrokeSmoother,
    generation: u64,
    layer: LayerId,
    gpu_op_start: usize,
    samples: Vec<StylusSample>,
    sample_limit_exceeded: bool,
    brush: BrushSnapshot,
    color: StrokeColor,
    eraser: bool,
}

enum GpuStrokeOp {
    Replace {
        generation: u64,
        layer: LayerId,
        color: [f32; 4],
        eraser: bool,
        alpha_locked: bool,
    },
    Dabs {
        generation: u64,
        dabs: Vec<BrushDab>,
    },
    Finish {
        generation: u64,
        disposition: LiveStrokeDisposition,
    },
}

fn append_gpu_dabs(
    operations: &mut Vec<GpuStrokeOp>,
    generation: u64,
    scratch: &mut Vec<BrushDab>,
) {
    if scratch.is_empty() {
        return;
    }
    if let Some(GpuStrokeOp::Dabs {
        generation: pending_generation,
        dabs,
    }) = operations.last_mut()
        && *pending_generation == generation
    {
        dabs.append(scratch);
        return;
    }
    operations.push(GpuStrokeOp::Dabs {
        generation,
        dabs: std::mem::take(scratch),
    });
}

fn paint_dirty_tiles(layer: LayerId, dirty: SignedDirtyRect) -> impl Iterator<Item = TileKey> {
    let tile_edge = i64::from(TILE_EDGE);
    let min_x = dirty.min_x.div_euclid(tile_edge);
    let min_y = dirty.min_y.div_euclid(tile_edge);
    let max_x = dirty.max_x.saturating_sub(1).div_euclid(tile_edge);
    let max_y = dirty.max_y.saturating_sub(1).div_euclid(tile_edge);
    (min_y..=max_y).flat_map(move |y| {
        (min_x..=max_x).map(move |x| TileKey {
            layer,
            mip: 0,
            x: i32::try_from(x).expect("signed dirty tile x fits i32"),
            y: i32::try_from(y).expect("signed dirty tile y fits i32"),
        })
    })
}

impl LiveStroke {
    fn retain_sample(&mut self, sample: StylusSample) {
        if self.samples.len() < MAX_SAMPLES_PER_STROKE {
            self.samples.push(sample);
        } else if !self.sample_limit_exceeded {
            self.sample_limit_exceeded = true;
            eprintln!(
                "live-ink event=stroke-sample-limit source=native-input limit={MAX_SAMPLES_PER_STROKE} admission=reject-on-end"
            );
        }
    }
}

struct ClosedStrokeRequest {
    alpha_locked: bool,
    selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    ordinal: u64,
    stroke_generation: u64,
    layer: LayerId,
    recorded: RecordedStroke,
    samples: Vec<StylusSample>,
    brush: BrushSnapshot,
    color: StrokeColor,
    end_timestamp_ns: u64,
    enqueued_at: Instant,
}

enum TryEnqueueStatus {
    Enqueued(EnqueueOutcome),
    Full(ClosedStrokeRequest),
    Disconnected(ClosedStrokeRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EnqueueOutcome {
    pending: usize,
}

struct MaterializationWorker {
    sender: Option<SyncSender<WorkerRequest>>,
    completed: Receiver<ClosedStrokeCompletion>,
    handle: Option<JoinHandle<WorkerExit>>,
    pending: Arc<AtomicUsize>,
    display_retired: Arc<AtomicBool>,
    export_generations: Arc<Mutex<ExportGenerationGate>>,
    live_ink: LiveInkBridge,
}

enum ExportEnqueueError {
    Busy,
    Disconnected,
}

/// Serializes export admission with the final replacement. Holding this gate
/// while replacing means a Save accepted before the check is never overwritten
/// by a prior PNG, and a Save accepted afterwards will replace it in turn.
#[derive(Debug, Default)]
struct ExportGenerationGate {
    next: u64,
    latest: u64,
}

impl ExportGenerationGate {
    fn next_generation(&self) -> Result<u64, ()> {
        self.next.checked_add(1).ok_or(())
    }

    fn accept_submission(&mut self, generation: u64) {
        debug_assert!(generation > self.latest);
        self.next = generation;
        self.latest = generation;
    }

    fn is_current(&self, generation: u64) -> bool {
        generation == self.latest
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerExit {
    Drained,
    FailStop,
    Panicked,
}

struct MaterializationBootstrap {
    worker: MaterializationWorker,
    initial_tiles: TileSnapshot,
    initial_tree: LayerTree,
    initial_canvas: CanvasSpec,
    initial_history: HistoryProjection,
}

enum LayerPixelChange {
    White(LayerId),
    Duplicate {
        source: LayerId,
        destination: LayerId,
    },
}

impl LayerPixelChange {
    fn apply(self, tiles: &TileSnapshot, canvas: CanvasSpec) -> Result<TileSnapshot, String> {
        match self {
            Self::White(layer) => tiles
                .with_replacements(
                    opaque_white_layer_tiles(canvas, layer)
                        .iter()
                        .map(|(key, tile)| (key, tile.pixels().to_vec())),
                )
                .map_err(|error| format!("background tiles: {error:?}")),
            Self::Duplicate {
                source,
                destination,
            } => {
                if source == destination || tiles.iter().any(|(key, _)| key.layer == destination) {
                    return Err("duplicate destination already contains artwork".into());
                }
                // Keep signed off-page coordinates and shared immutable pixel objects.
                TileSnapshot::from_objects(
                    tiles.iter().map(|(key, tile)| (key, tile.clone())).chain(
                        tiles
                            .iter()
                            .filter(|(key, _)| key.layer == source)
                            .map(|(key, tile)| {
                                (
                                    TileKey {
                                        layer: destination,
                                        ..key
                                    },
                                    tile.clone(),
                                )
                            }),
                    ),
                )
                .map_err(|error| format!("duplicate tiles: {error:?}"))
            }
        }
    }
}

enum WorkerRequest {
    PickerPreview {
        request: picker_preview::PickerRead,
        reply: SyncSender<Result<picker_preview::PickerFrame, String>>,
    },
    Stroke(ClosedStrokeRequest),
    Edit {
        command: nyatidraw_api::EditCommand,
        target: LayerId,
        reply: SyncSender<Result<ArtworkOutcome, EditFailure>>,
    },
    CommitLayerTree {
        tree: LayerTree,
        pixel_change: Option<LayerPixelChange>,
        reply: SyncSender<Result<ArtworkOutcome, EditFailure>>,
    },
    ExportPng {
        generation: u64,
        path: PathBuf,
    },
    MoveHistory {
        command: nyatidraw_api::HistoryCommand,
        reply: SyncSender<Result<ArtworkOutcome, EditFailure>>,
    },
}

enum ArtworkOutcome {
    Edit(EditOutcome),
    Transform(crate::transform_worker::TransformOutcome),
    History(HistoryMove),
}

struct ArtworkJob {
    transform: Option<nyatidraw_api::TransformCommand>,
    temporary_picker: Option<(u64, u64)>,
    picker_token: Option<picker_preview::PickerToken>,
    received: Receiver<Result<ArtworkOutcome, EditFailure>>,
    active_layer: Option<LayerId>,
    history_timing: Option<HistoryTiming>,
}

struct HistoryMove {
    tiles: Option<TileSnapshot>,
    canvas: Option<CanvasSpec>,
    tree: Option<LayerTree>,
    history: HistoryProjection,
    worker_timing: Option<HistoryWorkerTiming>,
}

enum ProjectSink {
    UntitledRecovery { db: ProjectDb, path: PathBuf },
    ExplicitProject { db: ProjectDb, path: PathBuf },
}

#[derive(Debug)]
enum WorkerCommitError {
    Project(ProjectOpenError),
    Session(nyatidraw_editor::HeadlessStrokeError),
}

impl std::fmt::Display for WorkerCommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project(error) => write!(formatter, "project:{error}"),
            Self::Session(error) => write!(formatter, "session:{error:?}"),
        }
    }
}

#[allow(clippy::too_many_lines)]
fn open_materialization_session(
    project_location: &ProjectLocation,
) -> Result<
    (
        HeadlessStrokeSession,
        TileSnapshot,
        LayerTree,
        CanvasSpec,
        u128,
        ProjectSink,
    ),
    String,
> {
    let (path, mode) = match project_location {
        ProjectLocation::Explicit { path, .. } => (path, "durable"),
        ProjectLocation::UntitledRecovery(path) => (path, "untitled-recovery"),
    };
    let imported = match project_location {
        ProjectLocation::Explicit {
            bootstrap_png: Some(source),
            ..
        } => Some(
            nyatidraw_png_io::decode_png(source, LIVE_LAYER)
                .map_err(|error| format!("png-import:path={}:error={error}", source.display()))?,
        ),
        ProjectLocation::Explicit {
            bootstrap_png: None,
            ..
        }
        | ProjectLocation::UntitledRecovery(_) => None,
    };
    let db = ProjectDb::open(path).map_err(|error| {
        format!(
            "project-open:mode={mode}:path={}:error={error}:invalid-non-empty-preserved=true",
            path.display()
        )
    })?;
    let stored_layer_tree = db
        .load_layer_tree()
        .map_err(|error| format!("layer-tree-reopen:path={}:error={error}", path.display()))?;
    let persisted_canvas = db
        .load_canvas_spec()
        .map_err(|error| format!("canvas-reopen:path={}:error={error}", path.display()))?;
    let reopened = db.load_reopened().map_err(|error| {
        format!(
            "project-reopen:path={}:error={error}:original-preserved=true",
            path.display()
        )
    })?;
    let layer_tree = if let Some(tree) = stored_layer_tree {
        tree
    } else if reopened.is_some() {
        legacy_layer_tree()
    } else {
        // Explicit synthetic probes exercise group and cross-layer invariants;
        // their fixture must not dictate the user-facing new-document defaults.
        let tree = if [
            PROTOCOL_PROBE_ENV,
            HISTORY_PROBE_ENV,
            INPUT_SAFETY_PROBE_ENV,
            "NAYATI_SYNTHETIC_INK",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
        {
            legacy_layer_tree()
        } else {
            default_layer_tree()
        };
        // Persist the initial topology before the first stroke so undo-to-empty
        // and later process reopen use the same baseline, not a future default.
        db.persist_layer_tree(&tree)
            .map_err(|error| format!("initial-layer-tree:{error}"))?;
        tree
    };

    if let Some(reopened) = reopened {
        let initial_tiles = reopened.current_tiles().clone();
        let next_id = next_project_id(&reopened)?;
        println!(
            "native-canvas event=project-open mode={mode}-reopened path={} snapshot={} root={:032x} tiles={} history_nodes={} next_id={next_id}",
            path.display(),
            reopened.current_snapshot().0,
            initial_tiles.root().id.0,
            initial_tiles.len(),
            reopened.history().node_count(),
        );
        Ok((
            HeadlessStrokeSession::from_reopened(reopened),
            initial_tiles,
            layer_tree,
            persisted_canvas,
            next_id,
            project_sink(project_location, db),
        ))
    } else {
        let (session, tiles, canvas, next_id) = if let Some(imported) = imported {
            let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
            let batch = session
                .prepare_structural_change(
                    SnapshotId(1),
                    HistoryNodeId(1),
                    system_timestamp_ns(),
                    imported.tiles,
                )
                .map_err(|error| format!("png-import-prepare:{error:?}"))?;
            db.persist_canvas_spec(imported.canvas)
                .map_err(|error| format!("png-import-canvas:{error}"))?;
            db.persist_layer_tree(&layer_tree)
                .map_err(|error| format!("png-import-layers:{error}"))?;
            db.commit_structural(&batch)
                .map_err(|error| format!("png-import-commit:{error}"))?;
            session
                .accept_structural_change(&batch)
                .map_err(|error| format!("png-import-accept:{error:?}"))?;
            println!(
                "native-canvas event=png-imported path={} project={} width={} height={} tiles={} snapshot=1",
                match project_location {
                    ProjectLocation::Explicit {
                        bootstrap_png: Some(source),
                        ..
                    } => source.display().to_string(),
                    _ => String::new(),
                },
                path.display(),
                imported.canvas.width_px,
                imported.canvas.height_px,
                session.tiles().len(),
            );
            let tiles = session.tiles().clone();
            (session, tiles, imported.canvas, 2)
        } else {
            let tiles = TileSnapshot::empty();
            (
                HeadlessStrokeSession::new(SnapshotId(0), tiles.clone()),
                tiles,
                persisted_canvas,
                1,
            )
        };
        println!(
            "native-canvas event=project-open mode={mode}-empty path={} snapshot={} root={:032x} tiles={} next_id={next_id}",
            path.display(),
            session.current_snapshot().0,
            tiles.root().id.0,
            tiles.len(),
        );
        Ok((
            session,
            tiles,
            layer_tree,
            canvas,
            next_id,
            project_sink(project_location, db),
        ))
    }
}

fn project_sink(project_location: &ProjectLocation, db: ProjectDb) -> ProjectSink {
    match project_location {
        ProjectLocation::Explicit { path, .. } => ProjectSink::ExplicitProject {
            db,
            path: path.clone(),
        },
        ProjectLocation::UntitledRecovery(path) => ProjectSink::UntitledRecovery {
            db,
            path: path.clone(),
        },
    }
}

fn next_project_id(reopened: &ReopenedProject) -> Result<u128, String> {
    let parts = reopened.clone().into_parts();
    let mut maximum = parts.current_snapshot.0;
    for (head, cursor) in parts.cursors {
        maximum = maximum.max(cursor.snapshot_id.0);
        if let Some(head) = head {
            maximum = maximum.max(head.0);
        }
    }
    maximum
        .checked_add(1)
        .ok_or_else(|| "project identifier space is exhausted".to_owned())
}

impl MaterializationWorker {
    fn start(
        live_ink: LiveInkBridge,
        project_location: &ProjectLocation,
    ) -> Result<MaterializationBootstrap, String> {
        let (session, initial_tiles, initial_tree, initial_canvas, next_id, sink) =
            open_materialization_session(project_location)?;
        let initial_history = session.history_projection();
        publish_navigator_frame(&live_ink, &initial_tiles, &initial_tree, initial_canvas);
        publish_layer_thumbnail_frames(
            &live_ink,
            1,
            &initial_tiles,
            raster_layer_ids(&initial_tree),
            initial_canvas,
        );
        let (sender, receiver) = sync_channel(MATERIALIZATION_QUEUE_CAPACITY);
        let (completed_sender, completed) = sync_channel(CLOSED_STROKE_COMPLETION_QUEUE_CAPACITY);
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_pending = Arc::clone(&pending);
        let display_retired = Arc::new(AtomicBool::new(false));
        let worker_display_retired = Arc::clone(&display_retired);
        let export_generations = Arc::new(Mutex::new(ExportGenerationGate::default()));
        let worker_export_generations = Arc::clone(&export_generations);
        let worker_preview_tree = initial_tree.clone();
        let worker_live_ink = live_ink.clone();
        let handle = thread::Builder::new()
            .name("nyatidraw-project-writer".into())
            .spawn(move || {
                let exit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    materialization_loop(
                        &receiver,
                        &completed_sender,
                        worker_display_retired.as_ref(),
                        worker_pending.as_ref(),
                        session,
                        worker_preview_tree,
                        initial_canvas,
                        next_id,
                        &sink,
                        &worker_live_ink,
                        worker_export_generations.as_ref(),
                    )
                }));
                match exit {
                    Ok(WorkerExit::Drained) => WorkerExit::Drained,
                    Ok(WorkerExit::FailStop) => {
                        worker_live_ink.fail_incomplete_export();
                        WorkerExit::FailStop
                    }
                    Ok(WorkerExit::Panicked) => unreachable!("worker loop cannot return panic"),
                    Err(_) => {
                        worker_live_ink.fail_incomplete_export();
                        WorkerExit::Panicked
                    }
                }
            })
            .map_err(|error| format!("project-writer-thread:{error}"))?;
        Ok(MaterializationBootstrap {
            worker: Self {
                sender: Some(sender),
                completed,
                handle: Some(handle),
                pending,
                display_retired,
                export_generations,
                live_ink,
            },
            initial_tiles,
            initial_tree,
            initial_canvas,
            initial_history,
        })
    }

    fn try_enqueue(&self, request: ClosedStrokeRequest) -> TryEnqueueStatus {
        let Some(sender) = self.sender.as_ref() else {
            return TryEnqueueStatus::Disconnected(request);
        };
        self.pending.fetch_add(1, Ordering::Relaxed);
        match sender.try_send(WorkerRequest::Stroke(request)) {
            Ok(()) => TryEnqueueStatus::Enqueued(EnqueueOutcome {
                pending: self.pending.load(Ordering::Relaxed),
            }),
            Err(TrySendError::Full(WorkerRequest::Stroke(request))) => {
                self.pending.fetch_sub(1, Ordering::Relaxed);
                TryEnqueueStatus::Full(request)
            }
            Err(TrySendError::Disconnected(WorkerRequest::Stroke(request))) => {
                self.pending.fetch_sub(1, Ordering::Relaxed);
                TryEnqueueStatus::Disconnected(request)
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                unreachable!("this send always wraps a stroke")
            }
        }
    }

    fn reserve_export(&self) -> Result<u64, ()> {
        // Acceptance supersedes older work even while this request waits for
        // an active stroke. Final replacement uses the same generation gate.
        let generation = {
            let mut generations = self
                .export_generations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let generation = generations.next_generation()?;
            generations.accept_submission(generation);
            generation
        };
        self.live_ink
            .publish_export_status(ExportStatus::Waiting { generation });
        Ok(generation)
    }

    fn enqueue_export(
        &self,
        generation: u64,
        path: PathBuf,
        shutdown: bool,
    ) -> Result<(), ExportEnqueueError> {
        let sender = self
            .sender
            .as_ref()
            .ok_or(ExportEnqueueError::Disconnected)?;
        let request = WorkerRequest::ExportPng { generation, path };
        // Never hold the replacement gate while waiting on worker capacity:
        // an older encoder may need it before it can free that capacity.
        if shutdown {
            sender
                .send(request)
                .map_err(|_| ExportEnqueueError::Disconnected)?;
        } else {
            sender.try_send(request).map_err(|error| match error {
                TrySendError::Full(_) => ExportEnqueueError::Busy,
                TrySendError::Disconnected(_) => ExportEnqueueError::Disconnected,
            })?;
        }
        self.live_ink
            .publish_export_status(ExportStatus::Queued { generation });
        Ok(())
    }

    /// Only the retiring canvas owner may waive display delivery. Closed
    /// strokes still pass through CPU replay and the immediate transaction.
    fn retire_display(&self) {
        self.display_retired.store(true, Ordering::Release);
    }

    fn enqueue_for_shutdown(&self, request: ClosedStrokeRequest) -> Result<(), ()> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(());
        };
        self.pending.fetch_add(1, Ordering::Relaxed);
        if sender.send(WorkerRequest::Stroke(request)).is_err() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            return Err(());
        }
        Ok(())
    }

    fn drain_completed(&self, output: &mut Vec<ClosedStrokeCompletion>) {
        output.extend(self.completed.try_iter());
    }
}

impl Drop for MaterializationWorker {
    fn drop(&mut self) {
        self.retire_display();
        drop(self.sender.take());
        let Some(handle) = self.handle.take() else {
            return;
        };
        let pending_before_join = self.pending.load(Ordering::Relaxed);
        let (status, close_guarantee) = match handle.join() {
            Ok(WorkerExit::Drained) => ("drained-and-joined", "latest-queued-closed-stroke"),
            Ok(WorkerExit::FailStop) => ("fail-stop-joined", "not-established"),
            Ok(WorkerExit::Panicked) | Err(_) => ("panicked", "not-established"),
        };
        let pending_after_join = self.pending.load(Ordering::Relaxed);
        if status != "drained-and-joined" || pending_after_join != 0 {
            self.live_ink.publish_workspace_error(
                "The project writer stopped before all accepted artwork was saved".into(),
            );
        }
        println!(
            "live-ink event=materialization-worker-shutdown mode={status} pending_before_join={pending_before_join} pending_after_join={pending_after_join} close_guarantee={close_guarantee}"
        );
    }
}

struct ClosedStrokeCompletion {
    ordinal: u64,
    stroke_generation: u64,
    tiles: Vec<(TileKey, Vec<u8>)>,
    history: Option<HistoryProjection>,
}

/// GPU-only retirement work; its CPU payload and history were already adopted.
struct DeferredTileUpload {
    ordinal: u64,
    stroke_generation: u64,
    tiles: Vec<(TileKey, Vec<u8>)>,
}

/// Adopt every fresh durable payload, even when a newer GPU preview hides it.
/// Append after older GPU work: uploading a new generation first could remove
/// its preview guard and let an older deferred upload overwrite it afterward.
fn adopt_materialized_completions(
    cpu_tiles: &mut BTreeMap<TileKey, Vec<u8>>,
    pending_uploads: &mut Vec<DeferredTileUpload>,
    completed: impl IntoIterator<Item = ClosedStrokeCompletion>,
    mut record_cpu_clone: impl FnMut(usize),
) -> Option<HistoryProjection> {
    let mut latest_history = None;
    for completion in completed {
        if let Some(history) = completion.history {
            latest_history = Some(history);
        }
        for (key, pixels) in &completion.tiles {
            record_cpu_clone(pixels.len());
            cpu_tiles.insert(*key, pixels.clone());
        }
        pending_uploads.push(DeferredTileUpload {
            ordinal: completion.ordinal,
            stroke_generation: completion.stroke_generation,
            tiles: completion.tiles,
        });
    }
    latest_history
}

/// Publishes the CPU payload which retires a committed live GPU preview.
///
/// This must never wait: a saturated UI-facing lane can occur while the event
/// loop is closing, and waiting here would prevent the writer from exiting and
/// `Drop` from joining it. Once the owner explicitly retires the display, CPU
/// durability no longer depends on delivering its disposable GPU payload.
/// While the display is live, a full/disconnected lane still quarantines input
/// and fail-stops the worker rather than silently losing a preview update.
fn publish_closed_completion(
    completed: &SyncSender<ClosedStrokeCompletion>,
    display_retired: &AtomicBool,
    live_ink: &LiveInkBridge,
    completion: ClosedStrokeCompletion,
) -> bool {
    let ordinal = completion.ordinal;
    let tile_count = completion.tiles.len();
    if display_retired.load(Ordering::Acquire) {
        println!(
            "live-ink event=closed-tile-completion-retired stroke={ordinal} tiles={tile_count} reason=display-owner-shutdown durable-cpu-authority=true"
        );
        return true;
    }
    match completed.try_send(completion) {
        Ok(()) => {
            live_ink.request_redraw();
            true
        }
        Err(_) if display_retired.load(Ordering::Acquire) => {
            // Shutdown may race the non-blocking send. A retired display
            // cannot turn an otherwise successful commit into a drain failure.
            println!(
                "live-ink event=closed-tile-completion-retired stroke={ordinal} tiles={tile_count} reason=display-owner-shutdown durable-cpu-authority=true"
            );
            true
        }
        Err(TrySendError::Full(_)) => {
            eprintln!(
                "live-ink event=closed-tile-completion-failed stroke={ordinal} tiles={tile_count} reason=completion-queue-full durable-cpu-authority=true worker=fail-stop"
            );
            live_ink.publish_workspace_error(format!(
                "Drawing stopped after stroke {ordinal} was saved because its exact tile update could not retire the GPU preview"
            ));
            false
        }
        Err(TrySendError::Disconnected(_)) => {
            eprintln!(
                "live-ink event=closed-tile-completion-failed stroke={ordinal} tiles={tile_count} reason=completion-receiver-disconnected durable-cpu-authority=true worker=fail-stop"
            );
            live_ink.publish_workspace_error(format!(
                "Drawing stopped after stroke {ordinal} was saved because its exact tile update lost its display receiver"
            ));
            false
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn materialization_loop(
    receiver: &Receiver<WorkerRequest>,
    completed: &SyncSender<ClosedStrokeCompletion>,
    display_retired: &AtomicBool,
    pending: &AtomicUsize,
    mut session: HeadlessStrokeSession,
    mut preview_tree: LayerTree,
    mut preview_canvas: CanvasSpec,
    mut next_id: u128,
    sink: &ProjectSink,
    live_ink: &LiveInkBridge,
    export_generations: &Mutex<ExportGenerationGate>,
) -> WorkerExit {
    let mut processed = 0_u64;
    // This is intentionally independent of the project snapshot id: undo can
    // select an older durable snapshot while still needing a newer mailbox
    // publication than the frame currently visible in Dioxus.
    let mut thumbnail_generation = 1_u64;
    let mut exit = WorkerExit::Drained;
    let mut selection = None;
    let mut transform_worker = crate::transform_worker::TransformWorker::default();
    while let Ok(work) = receiver.recv() {
        // Dequeue freed a bounded writer slot. Wake a Save retained by the
        // canvas even when this work only changes metadata or exports a PNG.
        live_ink.request_redraw();
        let probe = match &work {
            WorkerRequest::Edit {
                command: EditCommand::ResizePage { .. } | EditCommand::CropPageToSelection,
                reply,
                ..
            } => Some(("page", reply)),
            WorkerRequest::MoveHistory { reply, .. } => Some(("history", reply)),
            WorkerRequest::CommitLayerTree { reply, .. } => Some(("layer", reply)),
            _ => None,
        };
        if let Some((kind, reply)) = probe
            && let Err(reason) = crate::edit_worker::pause_artwork_probe(kind)
        {
            let _ = reply.send(Err(EditFailure::Rejected(reason)));
            live_ink.request_redraw();
            continue;
        }
        let request = match work {
            WorkerRequest::PickerPreview { request, reply } => {
                let result = picker_preview::sample_patch(request, session.tiles(), &preview_tree);
                let _ = reply.send(result);
                live_ink.request_redraw();
                continue;
            }
            WorkerRequest::Stroke(request) => request,
            WorkerRequest::Edit {
                command,
                target,
                reply,
            } => {
                let started = Instant::now();
                let db = match sink {
                    ProjectSink::UntitledRecovery { db, .. }
                    | ProjectSink::ExplicitProject { db, .. } => db,
                };
                if let EditCommand::FreeTransform(command) = command {
                    let result = transform_worker.execute(
                        command,
                        target,
                        preview_canvas,
                        &preview_tree,
                        &mut selection,
                        &mut session,
                        &mut next_id,
                        db,
                    );
                    if let Ok(outcome) = &result
                        && outcome.committed
                    {
                        preview_tree = outcome.tree.clone();
                        publish_navigator_frame(
                            live_ink,
                            session.tiles(),
                            &preview_tree,
                            preview_canvas,
                        );
                        thumbnail_generation = thumbnail_generation.saturating_add(1);
                        publish_layer_thumbnail_frames(
                            live_ink,
                            thumbnail_generation,
                            session.tiles(),
                            raster_layer_ids(&preview_tree),
                            preview_canvas,
                        );
                    }
                    let failed = matches!(result, Err(EditFailure::Fatal(_)));
                    if let Err(EditFailure::Fatal(reason)) = &result {
                        live_ink.publish_workspace_error(reason.clone());
                    }
                    let _ = reply.send(result.map(ArtworkOutcome::Transform));
                    live_ink.request_redraw();
                    if failed {
                        exit = WorkerExit::FailStop;
                        break;
                    }
                    continue;
                }
                let result = crate::edit_worker::execute(
                    command,
                    target,
                    preview_canvas,
                    &preview_tree,
                    &mut selection,
                    &mut session,
                    &mut next_id,
                    db,
                );
                let failed = matches!(result, Err(EditFailure::Fatal(_)));
                match &result {
                    Ok(outcome) if outcome.tiles.is_some() => {
                        if let Some(tree) = &outcome.tree {
                            preview_tree = tree.clone();
                        }
                        if let Some(canvas) = outcome.canvas {
                            preview_canvas = canvas;
                        }
                        publish_navigator_frame(
                            live_ink,
                            session.tiles(),
                            &preview_tree,
                            preview_canvas,
                        );
                        thumbnail_generation = thumbnail_generation.saturating_add(1);
                        publish_layer_thumbnail_frames(
                            live_ink,
                            thumbnail_generation,
                            session.tiles(),
                            raster_layer_ids(&preview_tree),
                            preview_canvas,
                        );
                    }
                    Err(EditFailure::Fatal(reason)) => {
                        live_ink.publish_workspace_error(reason.clone());
                    }
                    Err(EditFailure::Rejected(reason)) => {
                        eprintln!("native-canvas event=edit-rejected reason={reason}");
                    }
                    Ok(_) => {}
                }
                println!(
                    "native-canvas event=edit-finished elapsed_us={} snapshot={} fatal={failed}",
                    started.elapsed().as_micros(),
                    session.current_snapshot().0
                );
                let _ = reply.send(result.map(ArtworkOutcome::Edit));
                live_ink.request_redraw();
                if failed {
                    exit = WorkerExit::FailStop;
                    break;
                }
                continue;
            }
            WorkerRequest::ExportPng { generation, path } => {
                let current = export_generations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_current(generation);
                if !current {
                    println!(
                        "native-canvas event=png-export-skipped generation={} path={} reason=superseded-before-start",
                        generation,
                        path.display(),
                    );
                    continue;
                }
                live_ink.publish_export_status(ExportStatus::Running { generation });
                if live_ink.is_closing() {
                    live_ink.publish_close_status(CloseStatus::Exporting);
                }
                match export_current_png(
                    &path,
                    session.tiles(),
                    &preview_tree,
                    preview_canvas,
                    generation,
                    export_generations,
                ) {
                    Ok(ExportPngOutcome::Replaced) => {
                        live_ink.publish_export_status(ExportStatus::Current { generation });
                        println!(
                            "native-canvas event=png-export-finished generation={} path={} snapshot={} root={:032x}",
                            generation,
                            path.display(),
                            session.current_snapshot().0,
                            session.tiles().root().id.0,
                        );
                    }
                    Ok(ExportPngOutcome::Superseded { latest }) => println!(
                        "native-canvas event=png-export-skipped generation={} latest_generation={} path={} reason=superseded-before-replace",
                        generation,
                        latest,
                        path.display(),
                    ),
                    Err(error) => {
                        live_ink.publish_export_status(ExportStatus::Failed { generation });
                        eprintln!(
                            "native-canvas event=png-export-failed generation={} path={} snapshot={} error={} project_durability=unaffected",
                            generation,
                            path.display(),
                            session.current_snapshot().0,
                            error,
                        );
                    }
                }
                continue;
            }
            WorkerRequest::MoveHistory { command, reply } => {
                let mut worker_timing = HistoryWorkerTiming::begin();
                if let nyatidraw_api::HistoryCommand::ShowRedoBranches { after } = command {
                    let _ = reply.send(Ok(ArtworkOutcome::History(HistoryMove {
                        tiles: None,
                        canvas: None,
                        tree: None,
                        history: session.history_projection_after(after),
                        worker_timing: None,
                    })));
                    live_ink.request_redraw();
                    continue;
                }
                let prepared = match command {
                    nyatidraw_api::HistoryCommand::Undo => session.prepare_undo_cursor(),
                    nyatidraw_api::HistoryCommand::Redo => session.prepare_redo_cursor(),
                    nyatidraw_api::HistoryCommand::RedoTo(candidate) => {
                        session.prepare_redo_to_cursor(candidate)
                    }
                    nyatidraw_api::HistoryCommand::ShowRedoBranches { .. } => {
                        unreachable!("handled above")
                    }
                };
                let mut result = prepared
                    .map_err(|error| EditFailure::Rejected(format!("history-prepare:{error:?}")))
                    .and_then(|prepared| {
                        HistoryWorkerTiming::mark(&mut worker_timing, HistoryWorkerStep::Prepared);
                        let target = prepared.target();
                        let db = match sink {
                            ProjectSink::UntitledRecovery { db, .. }
                            | ProjectSink::ExplicitProject { db, .. } => db,
                        };
                        let tiles = db
                            .load_cursor_tiles(target)
                            .map_err(|error| EditFailure::Fatal(format!("history-load:{error}")))?;
                        HistoryWorkerTiming::mark(
                            &mut worker_timing,
                            HistoryWorkerStep::TilesLoaded,
                        );
                        let tree = db
                            .load_cursor_layer_tree(target)
                            .map_err(|error| {
                                EditFailure::Fatal(format!("history-tree-load:{error}"))
                            })?
                            .unwrap_or_else(legacy_layer_tree);
                        let canvas = db.load_cursor_canvas_spec(target).map_err(|error| {
                            EditFailure::Fatal(format!("history-page-load:{error}"))
                        })?;
                        HistoryWorkerTiming::mark(
                            &mut worker_timing,
                            HistoryWorkerStep::MetadataLoaded,
                        );
                        db.persist_history_cursor(target).map_err(|error| {
                            EditFailure::Fatal(format!("history-persist:{error}"))
                        })?;
                        HistoryWorkerTiming::mark(
                            &mut worker_timing,
                            HistoryWorkerStep::CursorPersisted,
                        );
                        session
                            .accept_history_cursor_move(prepared, tiles.clone())
                            .map_err(|error| {
                                EditFailure::Fatal(format!("history-accept:{error:?}"))
                            })?;
                        HistoryWorkerTiming::mark(
                            &mut worker_timing,
                            HistoryWorkerStep::SessionAccepted,
                        );
                        preview_tree = tree.clone();
                        preview_canvas = canvas;
                        selection = None;
                        Ok(HistoryMove {
                            tiles: Some(tiles),
                            canvas: Some(canvas),
                            tree: Some(tree),
                            history: session.history_projection(),
                            worker_timing: None,
                        })
                    });
                let failed = matches!(result, Err(EditFailure::Fatal(_)));
                if let Err(EditFailure::Fatal(error)) = &result {
                    live_ink
                        .publish_workspace_error(format!("History could not be restored: {error}"));
                } else if let Err(EditFailure::Rejected(error)) = &result {
                    eprintln!("native-canvas event=history-move-rejected error={error}");
                } else {
                    publish_navigator_frame(
                        live_ink,
                        session.tiles(),
                        &preview_tree,
                        preview_canvas,
                    );
                    thumbnail_generation = thumbnail_generation.saturating_add(1);
                    publish_layer_thumbnail_frames(
                        live_ink,
                        thumbnail_generation,
                        session.tiles(),
                        raster_layer_ids(&preview_tree),
                        preview_canvas,
                    );
                }
                HistoryWorkerTiming::mark(&mut worker_timing, HistoryWorkerStep::ReplyReady);
                if let Ok(moved) = &mut result {
                    moved.worker_timing = worker_timing;
                }
                let _ = reply.send(result.map(ArtworkOutcome::History));
                live_ink.request_redraw();
                if failed {
                    exit = WorkerExit::FailStop;
                    break;
                }
                continue;
            }
            WorkerRequest::CommitLayerTree {
                tree,
                pixel_change,
                reply,
            } => {
                let result = (|| -> Result<HistoryMove, String> {
                    let next = next_id
                        .checked_add(1)
                        .ok_or("project identifier space exhausted")?;
                    let kept: BTreeSet<_> = raster_layer_ids(&tree).into_iter().collect();
                    let removed: BTreeSet<_> = raster_layer_ids(&preview_tree)
                        .into_iter()
                        .filter(|id| !kept.contains(id))
                        .collect();
                    let mut after = if removed.is_empty() {
                        session.tiles().clone()
                    } else {
                        session
                            .tiles()
                            .with_replacements(
                                session
                                    .tiles()
                                    .iter()
                                    .filter(|(key, _)| removed.contains(&key.layer))
                                    .map(|(key, _)| (key, vec![0; TILE_BYTE_LEN])),
                            )
                            .map_err(|error| format!("subtree tiles: {error:?}"))?
                    };
                    if let Some(change) = pixel_change {
                        after = change.apply(&after, preview_canvas)?;
                    }
                    let batch = session
                        .prepare_structural_change(
                            SnapshotId(next_id),
                            HistoryNodeId(next_id),
                            system_timestamp_ns(),
                            after.clone(),
                        )
                        .map_err(|error| format!("layer history prepare: {error:?}"))?;
                    let db = match sink {
                        ProjectSink::UntitledRecovery { db, .. }
                        | ProjectSink::ExplicitProject { db, .. } => db,
                    };
                    db.commit_structural_with_layer_tree(&batch, &tree)
                        .map_err(|error| format!("layer history commit: {error}"))?;
                    session
                        .accept_structural_change(&batch)
                        .map_err(|error| format!("layer history accept: {error:?}"))?;
                    next_id = next;
                    preview_tree = tree.clone();
                    selection = None;
                    Ok(HistoryMove {
                        tiles: Some(after),
                        canvas: None,
                        tree: Some(tree),
                        history: session.history_projection(),
                        worker_timing: None,
                    })
                })();
                if let Err(error) = &result {
                    live_ink.publish_workspace_error(format!(
                        "Layer change could not be saved: {error}"
                    ));
                } else {
                    publish_navigator_frame(
                        live_ink,
                        session.tiles(),
                        &preview_tree,
                        preview_canvas,
                    );
                    thumbnail_generation = thumbnail_generation.saturating_add(1);
                    publish_layer_thumbnail_frames(
                        live_ink,
                        thumbnail_generation,
                        session.tiles(),
                        raster_layer_ids(&preview_tree),
                        preview_canvas,
                    );
                    println!(
                        "native-canvas event=layer-tree-persisted mode=redb-immediate snapshot={} root={:032x} nodes=metadata",
                        session.current_snapshot().0,
                        session.tiles().root().id.0
                    );
                }
                let failed = result.is_err();
                let _ = reply.send(
                    result
                        .map(ArtworkOutcome::History)
                        .map_err(EditFailure::Fatal),
                );
                live_ink.request_redraw();
                if failed {
                    exit = WorkerExit::FailStop;
                    break;
                }
                continue;
            }
        };
        let started_at = Instant::now();
        if preview_tree
            .raster(request.layer)
            .is_none_or(|layer| layer.locked || layer.alpha_locked != request.alpha_locked)
        {
            // Defense in depth: a stale/invalid producer must never commit ink
            // to a locked or removed layer, even if it bypassed live admission.
            live_ink.publish_workspace_error(
                "Closed stroke targeted a locked or missing layer; reopen before drawing".into(),
            );
            pending.fetch_sub(1, Ordering::Release);
            exit = WorkerExit::FailStop;
            break;
        }
        let queue_wait_micros = started_at.duration_since(request.enqueued_at).as_micros();
        let snapshot_id = SnapshotId(next_id);
        let replay_timing = Span::new(Stage::Replay);
        let result = session.prepare_round_stroke_with_options(
            snapshot_id,
            HistoryNodeId(next_id),
            request.end_timestamp_ns,
            request.layer,
            request.brush,
            request.recorded,
            request.color,
            request.samples,
            request.selection,
            request.alpha_locked,
        );
        drop(replay_timing);
        let keep_running = match result {
            Ok(batch) => {
                let closed_tiles = batch
                    .materialized
                    .changed_tiles
                    .iter()
                    .map(|(key, _)| {
                        let pixels = batch
                            .materialized
                            .after
                            .get(*key)
                            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
                        (*key, pixels)
                    })
                    .collect();
                let commit_timing = Span::new(Stage::Commit);
                let durable_result = match sink {
                    ProjectSink::UntitledRecovery { db, .. }
                    | ProjectSink::ExplicitProject { db, .. } => db
                        .commit(&batch)
                        .map_err(WorkerCommitError::Project)
                        .and_then(|()| {
                            session
                                .accept_committed(&batch)
                                .map_err(WorkerCommitError::Session)
                        }),
                };
                drop(commit_timing);
                match durable_result {
                    Ok(()) => {
                        publish_navigator_frame(
                            live_ink,
                            session.tiles(),
                            &preview_tree,
                            preview_canvas,
                        );
                        let changed_thumbnail_layers: BTreeSet<_> = batch
                            .materialized
                            .changed_tiles
                            .iter()
                            .map(|(key, _)| key.layer)
                            .collect();
                        thumbnail_generation = thumbnail_generation.saturating_add(1);
                        publish_layer_thumbnail_frames(
                            live_ink,
                            thumbnail_generation,
                            session.tiles(),
                            changed_thumbnail_layers,
                            preview_canvas,
                        );
                        processed = processed.saturating_add(1);
                        let durability = match sink {
                            ProjectSink::UntitledRecovery { .. } => {
                                "untitled-recovery-redb-immediate"
                            }
                            ProjectSink::ExplicitProject { .. } => "redb-immediate",
                        };
                        println!(
                            "live-ink event=stroke-materialized source=native-input durability={durability} stroke={} snapshot={} root={:032x} changed_tiles={} queue_wait_us={} materialization_us={} end_to_complete_us={}",
                            request.ordinal,
                            snapshot_id.0,
                            batch.materialized.after.root().id.0,
                            batch.materialized.changed_tiles.len(),
                            queue_wait_micros,
                            started_at.elapsed().as_micros(),
                            request.enqueued_at.elapsed().as_micros(),
                        );
                        let completion_delivered = publish_closed_completion(
                            completed,
                            display_retired,
                            live_ink,
                            ClosedStrokeCompletion {
                                ordinal: request.ordinal,
                                stroke_generation: request.stroke_generation,
                                tiles: closed_tiles,
                                history: Some(session.history_projection()),
                            },
                        );
                        if completion_delivered {
                            let Some(next) = next_id.checked_add(1) else {
                                eprintln!(
                                    "live-ink event=stroke-materialization-failed source=native-input stroke={} stage=id-allocation error=identifier-exhausted",
                                    request.ordinal,
                                );
                                pending.fetch_sub(1, Ordering::Relaxed);
                                exit = WorkerExit::FailStop;
                                break;
                            };
                            next_id = next;
                            true
                        } else {
                            false
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "live-ink event=stroke-materialization-failed source=native-input stroke={} stage=commit-before-accept error={error} worker=fail-stop",
                            request.ordinal,
                        );
                        live_ink.publish_workspace_error(format!(
                            "Closed stroke could not be saved: {error}"
                        ));
                        false
                    }
                }
            }
            Err(error) => {
                eprintln!(
                    "live-ink event=stroke-materialization-failed source=native-input stroke={} stage=cpu-replay error={error:?} worker=fail-stop",
                    request.ordinal,
                );
                live_ink.publish_workspace_error(format!(
                    "Closed stroke could not be materialized: {error:?}"
                ));
                false
            }
        };
        pending.fetch_sub(1, Ordering::Release);
        if !keep_running {
            exit = WorkerExit::FailStop;
            break;
        }
    }
    let mode = match sink {
        ProjectSink::UntitledRecovery { path, .. } => {
            println!(
                "live-ink event=untitled-recovery-repository-retained path={} processed={processed} retention=process-exit-no-cleanup",
                path.display(),
            );
            "untitled-recovery"
        }
        ProjectSink::ExplicitProject { path, .. } => {
            println!(
                "live-ink event=project-repository-closed path={} processed={processed}",
                path.display(),
            );
            "durable"
        }
    };
    println!("live-ink event=materialization-worker-exit mode={mode} processed={processed}");
    crate::performance::flush("writer");
    exit
}

fn publish_navigator_frame(
    live_ink: &LiveInkBridge,
    tiles: &TileSnapshot,
    tree: &LayerTree,
    canvas: CanvasSpec,
) {
    let _timing = Span::new(Stage::Navigator);
    let frame = render_page_preview_rgba8(
        tiles,
        tree,
        canvas,
        NAVIGATOR_MAX_WIDTH,
        NAVIGATOR_MAX_HEIGHT,
    )
    .map_err(|error| format!("navigator-preview-render:{error:?}"))
    .and_then(|surface| navigator_frame(tiles.root().id, &surface));
    match frame {
        Ok(frame) => live_ink.publish_navigator_frame(frame),
        Err(error) => eprintln!(
            "native-canvas event=navigator-preview-skipped error={error} prior-frame-retained=true"
        ),
    }
}

/// Publishes only the raster rows whose durable source pixels changed. The
/// CPU renderer samples the finite page directly; it never flattens the full
/// page or asks the GPU/UI path for image data.
fn publish_layer_thumbnail_frames(
    live_ink: &LiveInkBridge,
    generation: u64,
    tiles: &TileSnapshot,
    layers: impl IntoIterator<Item = LayerId>,
    canvas: CanvasSpec,
) {
    let _timing = Span::new(Stage::Thumbnails);
    let frames = layers
        .into_iter()
        .take(MAX_LAYER_THUMBNAILS)
        .filter_map(|layer| {
            render_raster_page_preview_rgba8(
                tiles,
                layer,
                canvas,
                LAYER_THUMBNAIL_MAX_WIDTH,
                LAYER_THUMBNAIL_MAX_HEIGHT,
            )
            .map_err(|error| format!("layer-thumbnail-render:{error:?}"))
            .and_then(|surface| layer_thumbnail_frame(layer, &surface))
            .map_err(|error| {
                eprintln!(
                    "native-canvas event=layer-thumbnail-skipped layer={} generation={} error={} prior-frame-retained=true",
                    layer.0, generation, error
                );
            })
            .ok()
        })
        .collect::<Vec<_>>();
    live_ink.publish_layer_thumbnails(generation, frames);
}

fn raster_layer_ids(tree: &LayerTree) -> Vec<LayerId> {
    fn visit(group: &GroupNode, layers: &mut Vec<LayerId>) {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => layers.push(layer.id),
                LayerTreeNode::Group(group) => visit(group, layers),
            }
        }
    }

    let mut layers = Vec::new();
    visit(tree.root(), &mut layers);
    // Rows render topmost first, so retain previews for the first visible
    // rows if the desktop-only cache hits its explicit cap.
    layers.into_iter().rev().collect()
}

enum ExportPngOutcome {
    Replaced,
    Superseded { latest: u64 },
}

fn export_current_png(
    path: &std::path::Path,
    tiles: &TileSnapshot,
    tree: &LayerTree,
    canvas: CanvasSpec,
    generation: u64,
    generations: &Mutex<ExportGenerationGate>,
) -> Result<ExportPngOutcome, String> {
    let _export_interval = crate::performance::ExportInterval::enter();
    let composite_timing = Span::new(Stage::ExportComposite);
    let surface = flatten_layer_tree_rgba8(tiles, tree, canvas)
        .map_err(|error| format!("composite:{error:?}"))?;
    drop(composite_timing);
    let sequence = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let file_name = path
        .file_name()
        .map_or_else(|| "export.png".into(), |name| name.to_string_lossy());
    let temporary = path.with_file_name(format!(
        ".{file_name}.~tmp-{}-{sequence}",
        std::process::id()
    ));
    let encode_timing = Span::new(Stage::ExportEncode);
    if let Err(error) = nyatidraw_png_io::encode_png(&temporary, &surface) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("encode:{error}"));
    }
    drop(encode_timing);
    let _replace_timing = Span::new(Stage::ExportReplace);
    export_probe_boundary(path, "encoded");
    if let Err(error) = std::fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .and_then(|file| file.sync_all())
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("sync:{}:{error}", temporary.display()));
    }
    export_probe_boundary(path, "synced");
    // This lock pairs the immediately-preceding check with replacement. A
    // successful Save cannot interleave after the check and before MoveFileExW
    // to let an obsolete encoded file become the final PNG.
    let generations = generations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !generations.is_current(generation) {
        let latest = generations.latest;
        drop(generations);
        let _ = std::fs::remove_file(&temporary);
        return Ok(ExportPngOutcome::Superseded { latest });
    }
    export_probe_boundary(path, "before-replace");
    if let Err(error) = replace_file(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("replace:{}:{error}", path.display()));
    }
    export_probe_boundary(path, "after-replace");
    Ok(ExportPngOutcome::Replaced)
}

/// Opt-in, scratch-only acceptance boundary. The harness may terminate its own
/// child here or release it; ordinary user projects cannot enable this by name.
fn export_probe_boundary(path: &std::path::Path, stage: &str) {
    use std::io::Write as _;

    if std::env::var("NAYATI_EXPORT_PAUSE").as_deref() != Ok(stage) {
        return;
    }
    let Some(directory) = std::env::var_os("NAYATI_EXPORT_PROBE_DIR").map(PathBuf::from) else {
        return;
    };
    let Ok(directory) = directory.canonicalize() else {
        return;
    };
    let Ok(temp_root) = std::env::temp_dir().canonicalize() else {
        return;
    };
    if !directory.starts_with(temp_root)
        || path
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .as_ref()
            != Some(&directory)
        || std::fs::read(directory.join(".nyatidraw-scratch-export-probe"))
            .ok()
            .as_deref()
            != Some(b"scratch-only")
    {
        return;
    }
    println!("native-canvas event=export-probe-paused stage={stage} scratch_only=true");
    let _ = std::io::stdout().flush();
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    while Instant::now() < deadline && !directory.join("release-export").exists() {
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both buffers are live, NUL-terminated UTF-16 paths. The flags
    // request one same-volume replace after the complete sibling file exists.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(std::io::Error::other)
    }
}

#[cfg(not(windows))]
fn replace_file(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[allow(clippy::cast_possible_truncation)]
fn add_synthetic_seed(width: u32, height: u32, output: &mut Vec<BrushDab>) {
    let width = f64::from(width);
    let height = f64::from(height);
    for index in 0..=48_u32 {
        let t = f64::from(index) / 48.0;
        output.push(BrushDab {
            grain: None,
            center: nyatidraw_input::Point {
                x: width * t.mul_add(0.68, 0.16),
                y: height * (0.5 + (t * std::f64::consts::TAU).sin() * 0.18),
            },
            radius_px: (8.0 + 12.0 * (t * std::f64::consts::PI).sin()) as f32,
            opacity: 0.86,
            flow: 0.42,
            hardness: 1.0,
        });
    }
}

fn add_synthetic_background(output: &mut Vec<BrushDab>) {
    for index in 0..18_u32 {
        let t = f64::from(index) / 17.0;
        output.push(BrushDab {
            grain: None,
            center: Point {
                x: 170.0 + t * 680.0,
                y: 570.0 - t * 360.0,
            },
            radius_px: 54.0,
            opacity: 0.72,
            flow: 0.5,
            hardness: 1.0,
        });
    }
}

pub(crate) fn system_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flipped_pen_eraser_snapshot_preserves_the_next_painted_stroke() {
        // Product risk: flipping a soft/low-opacity pen must not erase with
        // that brush or overwrite the brush used after flipping back.
        for tool in [
            DrawingTool::Pencil,
            DrawingTool::Pen,
            DrawingTool::Brush,
            DrawingTool::Eraser,
        ] {
            let mut drawing = DrawingConfig::from_projection(&nyatidraw_api::UiProjection::empty());
            drawing.select_tool(DrawingTool::Eraser);
            drawing.size_tenths = 120;
            drawing.opacity_u16 = 40_000;
            drawing.brush_settings.smoothing = 90;
            let remembered_eraser = drawing.preset();
            drawing.select_tool(tool);
            drawing.size_tenths = 57;
            drawing.opacity_u16 = 22_000;
            drawing.brush_settings.hardness_u16 = 8_000;
            drawing.brush_settings.smoothing = 20;
            let selected = drawing.preset();
            for hardware_eraser in [false, true] {
                let expected = if hardware_eraser && tool != DrawingTool::Eraser {
                    remembered_eraser
                } else {
                    selected
                };
                assert_eq!(drawing.preset_for_stroke(hardware_eraser), expected);
                assert_eq!(
                    drawing.smoothing_for_stroke(hardware_eraser),
                    if hardware_eraser && tool != DrawingTool::Eraser {
                        90
                    } else {
                        20
                    },
                );
                assert_eq!(drawing.tool, tool);
                assert_eq!(drawing.preset_for_stroke(false), selected);
            }
        }
    }

    #[test]
    fn white_raster_cannot_overwrite_other_layers_or_leak_beyond_page_edges() {
        // Product risk: a reserved layer ID can overwrite ink, and opaque tile
        // padding can reveal unwanted white artwork after canvas expansion.
        for (width_px, height_px, layer) in [
            (1, 1, LayerId(101)),
            (128, 128, LayerId(203)),
            (129, 257, LayerId(999)),
        ] {
            let snapshot = opaque_white_layer_tiles(
                CanvasSpec {
                    width_px,
                    height_px,
                    pixels_per_inch: 96,
                },
                layer,
            );
            let mut white_pixels = 0_u64;
            for (key, tile) in snapshot.iter() {
                assert_eq!(key.layer, layer);
                assert_eq!(key.mip, 0);
                let origin_x = u32::try_from(key.x).unwrap() * TILE_EDGE;
                let origin_y = u32::try_from(key.y).unwrap() * TILE_EDGE;
                for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
                    let x = origin_x + u32::try_from(index).unwrap() % TILE_EDGE;
                    let y = origin_y + u32::try_from(index).unwrap() / TILE_EDGE;
                    if x < width_px && y < height_px {
                        assert_eq!(pixel, [255; 4]);
                        white_pixels += 1;
                    } else {
                        assert_eq!(pixel, [0; 4]);
                    }
                }
            }
            assert_eq!(white_pixels, u64::from(width_px) * u64::from(height_px));
        }
    }

    #[test]
    fn deferred_gpu_work_cannot_recopy_or_roll_back_durable_tiles_and_history() {
        // Product risk: retrying old GPU work must not overwrite newer CPU
        // artwork/history, or run after a new upload clears its preview guard.
        for fresh_count in 0_u64..=2 {
            let key = TileKey {
                layer: LIVE_LAYER,
                mip: 0,
                x: -1,
                y: 2,
            };
            let mut cpu = BTreeMap::from([(key, vec![1; TILE_BYTE_LEN])]);
            let mut pending = vec![DeferredTileUpload {
                ordinal: 1,
                stroke_generation: 1,
                tiles: vec![(key, vec![1; TILE_BYTE_LEN])],
            }];
            let fresh = (2..2 + fresh_count).map(|generation| {
                let mut history = HistoryProjection::initial();
                history.has_older_entries = generation == 3;
                ClosedStrokeCompletion {
                    ordinal: generation,
                    stroke_generation: generation,
                    tiles: vec![(key, vec![u8::try_from(generation).unwrap(); TILE_BYTE_LEN])],
                    history: Some(history),
                }
            });
            let mut copied = Vec::new();
            let history = adopt_materialized_completions(&mut cpu, &mut pending, fresh, |bytes| {
                copied.push(bytes);
            });
            assert_eq!(
                copied,
                vec![TILE_BYTE_LEN; usize::try_from(fresh_count).unwrap()]
            );
            assert_eq!(
                cpu[&key],
                vec![u8::try_from(1 + fresh_count).unwrap(); TILE_BYTE_LEN]
            );
            assert_eq!(
                history.map(|history| history.has_older_entries),
                (fresh_count != 0).then_some(fresh_count == 2)
            );
            assert_eq!(
                pending
                    .iter()
                    .map(|upload| upload.stroke_generation)
                    .collect::<Vec<_>>(),
                (1..=1 + fresh_count).collect::<Vec<_>>()
            );

            // Another render can retry all pending GPU work; none of it is
            // accepted as fresh CPU/history authority or copied a second time.
            let before = cpu.clone();
            let copied_before = copied.len();
            assert!(
                adopt_materialized_completions(&mut cpu, &mut pending, [], |bytes| copied
                    .push(bytes))
                .is_none()
            );
            assert_eq!(cpu, before);
            assert_eq!(copied.len(), copied_before);
            assert_eq!(pending.first().unwrap().tiles[0].1, vec![1; TILE_BYTE_LEN]);
        }
    }

    #[test]
    fn save_as_preserves_source_and_every_redo_snapshot_after_closed_copy() {
        // Product risk: saving the visible head alone silently destroys redo
        // artwork; copying while a writer is open can publish a torn database.
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-save-as-test-{}-{}",
            std::process::id(),
            system_timestamp_ns()
        ));
        std::fs::create_dir(&directory).unwrap();
        let source = directory.join("원본.ntdr");
        let target = directory.join("다른 이름.ntdr");
        let live_ink = LiveInkBridge::with_capacity(512, LIVE_LAYER);
        let bootstrap = MaterializationWorker::start(
            live_ink.clone(),
            &ProjectLocation::UntitledRecovery(source.clone()),
        )
        .unwrap();
        enqueue_synthetic_durability_probe(&live_ink).unwrap();
        drop(StrokePipeline::new(live_ink.clone(), bootstrap.worker));
        assert!(!live_ink.workspace_failed());
        let database = ProjectDb::open(&source).unwrap();
        let reopened = database.load_reopened().unwrap().unwrap();
        let session = HeadlessStrokeSession::from_reopened(reopened);
        let undo = session.prepare_undo_cursor().unwrap();
        database.persist_history_cursor(undo.target()).unwrap();
        database
            .persist_canvas_spec(CanvasSpec {
                width_px: 32,
                height_px: 32,
                pixels_per_inch: 96,
            })
            .unwrap();
        let expected = database.load_reopened().unwrap().unwrap().into_parts();
        drop(database);
        let original = std::fs::read(&source).unwrap();
        crate::save_as::copy_closed_project(&source, &target).unwrap();
        assert_eq!(
            std::fs::read(&source).unwrap(),
            original,
            "Save As must not alter original bytes"
        );
        let copied = ProjectDb::open(&target).unwrap();
        let actual = copied.load_reopened().unwrap().unwrap().into_parts();
        assert_eq!(actual.current_cursor, expected.current_cursor);
        assert_eq!(
            actual.cursors, expected.cursors,
            "all redo snapshots remain reachable"
        );
        assert_eq!(actual.history.node_count(), 32);
        assert_eq!(actual.current_tiles, expected.current_tiles);
        assert!(target.with_extension("png").is_file());
        drop(copied);
        for path in [&source, &target, &target.with_extension("png")] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn cpu_snapshot_adoption_cannot_retain_deleted_artwork_or_lose_restored_layers() {
        let signed = TileKey::from_pixel(LayerId(1), 128, -129, -1);
        let retained = TileKey::from_pixel(LayerId(2), 128, 0, 0);
        let restored = TileKey::from_pixel(LayerId(3), 128, 129, 129);
        let mut cache = BTreeMap::new();
        // Add, modify/retain/remove, restore a removed layer, then undo all ink.
        for (entries, changed_keys, all_keys) in [
            (
                vec![(signed, 16), (retained, 32), (restored, 64)],
                vec![signed, retained, restored],
                vec![signed, retained, restored],
            ),
            (
                vec![(signed, 24), (retained, 32)],
                vec![signed, restored],
                vec![signed, retained, restored],
            ),
            (
                vec![(retained, 32), (restored, 64)],
                vec![signed, restored],
                vec![signed, retained, restored],
            ),
            (vec![], vec![retained, restored], vec![retained, restored]),
        ] {
            let expected: BTreeMap<_, _> = entries
                .into_iter()
                .map(|(key, value)| (key, vec![value; TILE_BYTE_LEN]))
                .collect();
            let snapshot = TileSnapshot::from_tiles(expected.clone()).unwrap();
            let adoption = CpuSnapshotAdoption::new(&mut cache, &snapshot);
            assert_eq!(
                adoption.upload_keys(false).collect::<Vec<_>>(),
                changed_keys,
                "surviving surfaces must receive every changed or deleted tile"
            );
            assert_eq!(
                adoption.upload_keys(true).collect::<Vec<_>>(),
                all_keys,
                "recreated surfaces must also receive unchanged artwork"
            );
            adoption.apply();
            assert_eq!(
                cache, expected,
                "renderer cache must equal all authoritative pixels"
            );
        }
    }

    fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 1_000_000,
            device_id: 7,
            phase,
            position_document: Point { x, y },
            pressure: 0.75,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        }
    }

    #[test]
    fn escape_quarantines_gesture_tail_without_committing_artwork() {
        // Product risk: the End following Escape commits a cancelled line or
        // selection, or usage-only metadata changes dirty/history state. An
        // accepted cancelled stroke still used its immutable source color.
        for (tool, hardware_eraser, uses_color) in [
            (DrawingTool::Pencil, false, true),
            (DrawingTool::Pen, false, true),
            (DrawingTool::Brush, false, true),
            (DrawingTool::Brush, true, false),
            (DrawingTool::Eraser, false, false),
            (DrawingTool::Lasso, false, false),
            (DrawingTool::Fill, false, false),
            (DrawingTool::Eyedropper, false, false),
        ] {
            let path = std::env::temp_dir().join(format!(
                "nyatidraw-cancel-gesture-{}-{}.ntdr",
                std::process::id(),
                system_timestamp_ns()
            ));
            let ink = LiveInkBridge::with_capacity(16, LIVE_LAYER);
            let bootstrap = MaterializationWorker::start(
                ink.clone(),
                &ProjectLocation::UntitledRecovery(path.clone()),
            )
            .unwrap();
            let mut pipeline = StrokePipeline::new(ink.clone(), bootstrap.worker);
            let mut drawing = pipeline.drawing;
            drawing.select_tool(tool);
            drawing.color = if tool == DrawingTool::Pencil {
                [0, 0, 0, 255]
            } else {
                [17, 83, 211, 255]
            };
            let mut projection = ProjectionState::new(DockTree::safe_default());
            let original_history = projection.current().history.clone();
            projection.stage_drawing_controls(
                tool,
                drawing.size_tenths,
                drawing.opacity_u16,
                drawing.color,
                drawing.background_color,
            );
            assert!(
                projection.current().recent_colors.is_empty(),
                "selecting a color is not an artwork Begin"
            );
            let mut begin = sample(1, PointerPhase::Begin, 4.0, 4.0);
            begin.eraser = hardware_eraser;
            ink.push(begin).unwrap();
            let started = pipeline.drain(LIVE_LAYER, drawing);
            let expected_color = uses_color.then_some(drawing.color);
            assert_eq!(started.started_brush_colors[0], expected_color);
            if uses_color {
                assert_eq!(
                    pipeline.active.as_ref().unwrap().color,
                    drawing.stroke_color()
                );
            }
            for color in started.started_brush_colors.into_iter().flatten().rev() {
                projection.stage_used_brush_color(color);
            }
            assert!(pipeline.cancel_native_gesture());
            assert!(
                !pipeline.cancel_native_gesture(),
                "one Escape consumes only one gesture"
            );
            drawing.select_tool(DrawingTool::Pen);
            ink.push(sample(2, PointerPhase::Move, 8.0, 4.0)).unwrap();
            ink.push(sample(3, PointerPhase::End, 12.0, 4.0)).unwrap();
            let drained = pipeline.drain(LIVE_LAYER, drawing);
            assert!(!drained.artwork_closed);
            assert!(pipeline.active.is_none());
            assert!(pipeline.completed_edit.is_none());
            assert!(pipeline.pending_materializations.is_empty());
            assert_eq!(pipeline.materializer.pending.load(Ordering::Acquire), 0);
            assert_eq!(
                projection.current().recent_colors,
                expected_color.into_iter().collect::<Vec<_>>()
            );
            assert_eq!(projection.current().history, original_history);
            assert!(
                !projection.current().dirty,
                "painting usage must not mark cancelled artwork dirty"
            );
            ink.push(sample(4, PointerPhase::Begin, 5.0, 5.0)).unwrap();
            pipeline.drain(LIVE_LAYER, drawing);
            assert!(
                pipeline.active.is_some(),
                "new contact must recover after cancellation"
            );
            ink.push(sample(5, PointerPhase::Cancel, 5.0, 5.0)).unwrap();
            pipeline.drain(LIVE_LAYER, drawing);
            drop(pipeline);
            let db = ProjectDb::open(&path).unwrap();
            assert!(db.load_reopened().unwrap().is_none());
            drop(db);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn temporary_picker_cannot_paint_on_release_cancel_or_queued_next_begin() {
        // Product risk: releasing Alt or draining the next Begin before the
        // asynchronous read can paint unexpected marks with the previous color.
        for (phase, locked) in [
            (PointerPhase::End, false),
            (PointerPhase::End, true),
            (PointerPhase::Cancel, false),
        ] {
            let path = std::env::temp_dir().join(format!(
                "nyatidraw-temp-picker-{}-{}.ntdr",
                std::process::id(),
                system_timestamp_ns()
            ));
            let ink = LiveInkBridge::with_capacity(16, LIVE_LAYER);
            let bootstrap = MaterializationWorker::start(
                ink.clone(),
                &ProjectLocation::UntitledRecovery(path.clone()),
            )
            .unwrap();
            let mut pipeline = StrokePipeline::new(ink.clone(), bootstrap.worker);
            pipeline.selected_layer_locked = locked;
            let original = pipeline.drawing;
            ink.push_with_temporary_picker(sample(1, PointerPhase::Begin, -1.0, 4.0), true)
                .unwrap();
            let started = pipeline.drain(LIVE_LAYER, original);
            assert!(started.started_brush_colors.iter().all(Option::is_none));
            assert!(pipeline.active.is_none());
            assert!(pipeline.edit_gesture.is_some());
            let picker_token = pipeline.picker.gesture.as_ref().unwrap().token;
            assert!(pipeline.picker_token_valid(picker_token));
            // Alt has been released; only Begin owns its latched tool intent.
            ink.push(sample(2, PointerPhase::Move, -2.0, 4.0)).unwrap();
            ink.push(sample(3, phase, -2.0, 4.0)).unwrap();
            if phase == PointerPhase::End {
                assert!(ink.push(sample(4, PointerPhase::Begin, 4.0, 4.0)).is_err());
                assert!(ink.push(sample(5, PointerPhase::End, 5.0, 4.0)).is_err());
            }
            pipeline.drain(LIVE_LAYER, original);
            assert!(pipeline.active.is_none());
            assert!(pipeline.edit_gesture.is_none());
            assert!(pipeline.gpu_ops.is_empty());
            assert_eq!(pipeline.materializer.pending.load(Ordering::Acquire), 0);
            assert_eq!(pipeline.drawing.tool, original.tool);
            assert_eq!(pipeline.drawing.size_tenths, original.size_tenths);
            assert_eq!(pipeline.drawing.color, original.color);
            if phase == PointerPhase::End {
                assert!(pipeline.picker.gesture.as_ref().unwrap().released);
                assert!(pipeline.picker_token_valid(picker_token));
                assert!(matches!(
                    pipeline.completed_edit.take(),
                    Some(Ok(EditCommand::PickColor {
                        point: [-2, 4],
                        source: nyatidraw_api::EditSource::AllVisible
                    }))
                ));
                assert_eq!(pipeline.completed_temporary_picker, Some((7, 3)));
                assert!(ink.push(sample(6, PointerPhase::Begin, 4.0, 4.0)).is_err());
                ink.resume_after_edit();
                assert!(
                    ink.push(sample(6, PointerPhase::Begin, 4.0, 4.0)).is_err(),
                    "ordinary edit resume cannot release a pending color read"
                );
                ink.complete_temporary_pick((7, 3));
                // End may already be in the worker when focus leaves. Its
                // old read must no longer be eligible to change foreground.
                ink.invalidate_picker();
                assert!(!pipeline.picker_token_valid(picker_token));
            } else {
                assert!(pipeline.completed_edit.is_none());
                assert!(!pipeline.picker_token_valid(picker_token));
            }
            pipeline.selected_layer_locked = false;
            ink.push(sample(7, PointerPhase::Begin, 4.0, 4.0)).unwrap();
            pipeline.drain(LIVE_LAYER, original);
            assert!(
                pipeline.active.is_some(),
                "next clean Begin resumes original brush"
            );
            ink.push(sample(8, PointerPhase::Cancel, 4.0, 4.0)).unwrap();
            pipeline.drain(LIVE_LAYER, original);
            drop(pipeline);
            let db = ProjectDb::open(&path).unwrap();
            assert!(db.load_reopened().unwrap().is_none());
            drop(db);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn locked_native_input_cannot_preview_save_or_resurrect_on_close() {
        // Product risk: GPU preview/close-drain can bypass CPU edit protection.
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-lock-input-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let ink = LiveInkBridge::with_capacity(16, LIVE_LAYER);
        let bootstrap = MaterializationWorker::start(
            ink.clone(),
            &ProjectLocation::UntitledRecovery(path.clone()),
        )
        .unwrap();
        let mut pipeline = StrokePipeline::new(ink.clone(), bootstrap.worker);
        pipeline.selected_layer_locked = true;
        let mut sequence = 0;
        for tool in [
            DrawingTool::Pencil,
            DrawingTool::Pen,
            DrawingTool::Brush,
            DrawingTool::Eraser,
            DrawingTool::MoveSelection,
            DrawingTool::Fill,
            DrawingTool::Gradient,
        ] {
            pipeline.drawing.tool = tool;
            for phase in [PointerPhase::Begin, PointerPhase::Move, PointerPhase::End] {
                sequence += 1;
                ink.push(sample(sequence, phase, 4.0, 4.0)).unwrap();
            }
            let started = pipeline.drain(LIVE_LAYER, pipeline.drawing);
            assert!(started.started_brush_colors.iter().all(Option::is_none));
            assert!(pipeline.active.is_none());
            assert!(pipeline.gpu_ops.is_empty());
            assert!(pipeline.completed_edit.is_none());
            assert!(pipeline.pending_materializations.is_empty());
            assert_eq!(pipeline.materializer.pending.load(Ordering::Acquire), 0);
        }
        // Raw input arriving immediately before Close must remain protected.
        pipeline.drawing.tool = DrawingTool::Brush;
        ink.push(sample(sequence + 1, PointerPhase::Begin, 8.0, 8.0))
            .unwrap();
        ink.push(sample(sequence + 2, PointerPhase::End, 8.0, 8.0))
            .unwrap();
        drop(pipeline);
        assert!(!ink.workspace_failed());
        let db = ProjectDb::open(&path).unwrap();
        assert!(db.load_reopened().unwrap().is_none());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Persisted protection, rejection and undo are one invariant.
    fn layer_lock_blocks_edits_and_group_deletion_and_survives_restart() {
        // Product risk: metadata-only locks must not be lost on restart or
        // bypassed by structural actions; rejected edits cannot consume history.
        const REOPEN: &str = "NYATIDRAW_TEST_LOCK_REOPEN";
        let canvas = CanvasSpec {
            width_px: 8,
            height_px: 8,
            pixels_per_inch: 96,
        };
        let mut tree = default_layer_tree();
        let group = GroupId(800);
        tree.insert(
            tree.root_id(),
            1,
            LayerTreeNode::Group(GroupNode {
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: group,
                name: "Sketch".into(),
                visible: true,
                opacity_u16: u16::MAX,
                children: Vec::new(),
            }),
        )
        .unwrap();
        tree.reorder(nyatidraw_api::LayerTreeNodeId::Raster(LIVE_LAYER), group, 0)
            .unwrap();
        let unlocked = tree.clone();
        tree.set_locked(LIVE_LAYER, true).unwrap();
        let tiles = TileSnapshot::from_tiles([-1, 0].map(|x| {
            (
                TileKey {
                    layer: LIVE_LAYER,
                    mip: 0,
                    x,
                    y: 0,
                },
                vec![64; TILE_BYTE_LEN],
            )
        }))
        .unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, actual, actual_tree, _, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .unwrap();
            assert_eq!(actual, tiles);
            assert_eq!(actual_tree, tree);
            assert!(actual_tree.raster(LIVE_LAYER).unwrap().locked);
            return;
        }
        for node in [
            nyatidraw_api::LayerTreeNodeId::Raster(LIVE_LAYER),
            nyatidraw_api::LayerTreeNodeId::Group(group),
        ] {
            let original = tree.clone();
            assert_eq!(
                tree.remove(node),
                Err(nyatidraw_document::LayerTreeError::LockedNode(node))
            );
            assert_eq!(tree, original);
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-lock-history-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let (mut session, _, _, _, mut next_id, sink) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        for metadata in [&unlocked, &tree] {
            let batch = session
                .prepare_structural_change(
                    SnapshotId(next_id),
                    HistoryNodeId(next_id),
                    1,
                    tiles.clone(),
                )
                .unwrap();
            db.commit_structural_with_layer_tree(&batch, metadata)
                .unwrap();
            session.accept_structural_change(&batch).unwrap();
            next_id += 1;
        }
        let mut selection = Some(Arc::new(
            nyatidraw_paint_cpu::lasso_selection(
                canvas,
                &[[0, 0], [4, 0], [4, 4], [0, 4]],
                nyatidraw_paint_cpu::EditLimits::default(),
            )
            .unwrap(),
        ));
        let head = session.current_snapshot();
        let expected_next = next_id;
        for command in [
            EditCommand::ClearActiveLayer,
            EditCommand::Transform(nyatidraw_api::RasterTransform {
                offset: [1, 0],
                ..Default::default()
            }),
            EditCommand::FillSelection { color: [255; 4] },
            EditCommand::FloodFill {
                seed: [0, 0],
                tolerance: 0,
                source: nyatidraw_api::EditSource::ActiveLayer,
                color: [255; 4],
            },
            EditCommand::GradientSelection {
                start: [0, 0],
                end: [3, 3],
                start_color: [255; 4],
                end_color: [0; 4],
            },
        ] {
            let before_selection = selection.clone();
            assert!(matches!(
                crate::edit_worker::execute(
                    command,
                    LIVE_LAYER,
                    canvas,
                    &tree,
                    &mut selection,
                    &mut session,
                    &mut next_id,
                    db
                ),
                Err(EditFailure::Rejected(_))
            ));
            assert_eq!(session.tiles(), &tiles);
            assert_eq!(session.current_snapshot(), head);
            assert_eq!(next_id, expected_next);
            assert_eq!(selection, before_selection);
        }
        // Unlock is itself undoable; after undoing a clear and the unlock,
        // both the pixels and the protection must be restored together.
        let batch = session
            .prepare_structural_change(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                2,
                tiles.clone(),
            )
            .unwrap();
        db.commit_structural_with_layer_tree(&batch, &unlocked)
            .unwrap();
        session.accept_structural_change(&batch).unwrap();
        next_id += 1;
        crate::edit_worker::execute(
            EditCommand::ClearActiveLayer,
            LIVE_LAYER,
            canvas,
            &unlocked,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        )
        .ok()
        .unwrap();
        assert!(session.tiles().is_empty());
        for _ in 0..2 {
            let undo = session.prepare_undo_cursor().unwrap();
            let restored = db.load_cursor_tiles(undo.target()).unwrap();
            db.persist_history_cursor(undo.target()).unwrap();
            session.accept_history_cursor_move(undo, restored).unwrap();
        }
        drop(session);
        drop(sink);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::tests::layer_lock_blocks_edits_and_group_deletion_and_survives_restart"])
            .env(REOPEN, &path).status().unwrap();
        assert!(status.success());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One complete gesture/save/restart artwork invariant.
    fn rectangle_move_preserves_pixels_undo_and_off_page_reopen() {
        // Product risk: a selection move must cut exactly once, preserve other
        // artwork, and keep moved off-page pixels durable but out of the PNG.
        const REOPEN: &str = "NYATIDRAW_TEST_RECTANGLE_MOVE_REOPEN";
        let canvas = CanvasSpec {
            width_px: 16,
            height_px: 16,
            pixels_per_inch: 96,
        };
        let tree = default_layer_tree();
        let fixture = |moved| {
            let mut pixels = vec![0; TILE_BYTE_LEN];
            for (x, y, color) in [
                (if moved { 33 } else { 1 }, 1, [255, 0, 0, 255]),
                (if moved { 34 } else { 2 }, 2, [0, 0, 255, 255]),
                (7, 7, [0, 255, 0, 255]),
            ] {
                let offset = (y * 128 + x) * 4;
                pixels[offset..offset + 4].copy_from_slice(&color);
            }
            TileSnapshot::from_tiles([
                (
                    TileKey {
                        layer: LIVE_LAYER,
                        mip: 0,
                        x: 0,
                        y: 0,
                    },
                    pixels,
                ),
                (
                    TileKey {
                        layer: LIVE_LAYER,
                        mip: 0,
                        x: -1,
                        y: 0,
                    },
                    vec![64; TILE_BYTE_LEN],
                ),
            ])
            .unwrap()
        };
        let before = fixture(false);
        let expected = fixture(true);
        let expected_png = nyatidraw_png_io::encode_png_bytes(
            &flatten_layer_tree_rgba8(&expected, &tree, canvas).unwrap(),
        )
        .unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, tiles, reopened_tree, reopened_canvas, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .unwrap();
            assert_eq!(tiles, expected);
            assert_eq!(reopened_tree, tree);
            assert_eq!(reopened_canvas, canvas);
            let actual_png = nyatidraw_png_io::encode_png_bytes(
                &flatten_layer_tree_rgba8(&tiles, &reopened_tree, reopened_canvas).unwrap(),
            )
            .unwrap();
            assert_eq!(actual_png, expected_png);
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-rectangle-move-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let (mut session, _, _, _, mut next_id, sink) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        db.persist_canvas_spec(canvas).unwrap();
        let seed = session
            .prepare_structural_change(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                1,
                before.clone(),
            )
            .unwrap();
        db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
        session.accept_structural_change(&seed).unwrap();
        next_id += 1;
        let mut selection = None;
        for (tool, start, end, has_selection) in [
            (
                DrawingTool::RectangleSelection,
                [2.0, 2.0],
                [1.0, 1.0],
                false,
            ),
            (DrawingTool::MoveSelection, [1.0, 1.0], [33.0, 1.0], true),
        ] {
            let gesture = crate::edit_gesture::EditGesture::begin(
                tool,
                nyatidraw_api::EditSettings::default(),
                [0; 4],
                has_selection,
                sample(1, PointerPhase::Begin, start[0], start[1]),
            );
            let command = gesture
                .finish(sample(2, PointerPhase::End, end[0], end[1]))
                .unwrap();
            crate::edit_worker::execute(
                command,
                LIVE_LAYER,
                canvas,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db,
            )
            .ok()
            .expect("gesture applied");
            if tool == DrawingTool::RectangleSelection {
                assert_eq!(selection.as_ref().unwrap().selected_pixels(), 4);
                assert_eq!(
                    session.tiles(),
                    &before,
                    "selection must not modify artwork"
                );
            }
        }
        assert_eq!(session.tiles(), &expected);
        assert!(
            selection.is_none(),
            "existing transform semantics clear selection after commit"
        );
        let undo = session.prepare_undo_cursor().unwrap();
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, before);
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        let redo = session.prepare_redo_cursor().unwrap();
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, expected);
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
        drop(session);
        drop(sink);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_canvas::tests::rectangle_move_preserves_pixels_undo_and_off_page_reopen",
            ])
            .env(REOPEN, &path)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Keep the parent/child durability sequence together.
    fn duplicate_layer_preserves_off_page_artwork_history_and_reopen() {
        // Product risk: copying only preview pixels loses off-page artwork;
        // split metadata/pixel commits or shared mutable copies corrupt undo.
        const REOPEN: &str = "NYATIDRAW_TEST_DUPLICATE_REOPEN";
        let source = LIVE_LAYER;
        let destination = LayerId(991);
        let mut original_tree = default_layer_tree();
        original_tree
            .rename(
                nyatidraw_api::LayerTreeNodeId::Raster(source),
                &"선".repeat(128),
            )
            .unwrap();
        original_tree.set_reference(source, true).unwrap();
        original_tree
            .set_opacity(nyatidraw_api::LayerTreeNodeId::Raster(source), 31_337)
            .unwrap();
        let mut tree = original_tree.clone();
        tree.duplicate_raster(source, destination).unwrap();
        assert_eq!(
            tree.parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(destination)),
            Some((tree.root_id(), 1))
        );
        let LayerTreeNode::Raster(copy) = &tree.root().children[1] else {
            panic!("raster copy")
        };
        assert_eq!(copy.name.chars().count(), 128);
        assert!(copy.name.ends_with(" 복사"));
        assert!(copy.reference);
        assert_eq!(copy.opacity_u16, 31_337);
        let accepted_tree = tree.clone();
        assert!(tree.duplicate_raster(source, destination).is_err());
        assert!(
            tree.duplicate_raster(LayerId(999_999), LayerId(992))
                .is_err()
        );
        assert_eq!(tree, accepted_tree, "rejected copies are failure-atomic");
        let before = TileSnapshot::from_tiles([-20, 0, 300].map(|x| {
            (
                TileKey {
                    layer: source,
                    mip: 0,
                    x,
                    y: 0,
                },
                vec![128; TILE_BYTE_LEN],
            )
        }))
        .unwrap();
        let duplicate = || LayerPixelChange::Duplicate {
            source,
            destination,
        };
        let after = duplicate().apply(&before, CanvasSpec::DEFAULT).unwrap();
        assert_eq!(after.len(), before.len() * 2);
        for (key, tile) in before.iter() {
            assert_eq!(after.get(key), Some(tile));
            assert_eq!(
                after.get(TileKey {
                    layer: destination,
                    ..key
                }),
                Some(tile)
            );
        }
        assert!(duplicate().apply(&after, CanvasSpec::DEFAULT).is_err());
        assert!(
            duplicate()
                .apply(&TileSnapshot::empty(), CanvasSpec::DEFAULT)
                .unwrap()
                .is_empty()
        );
        let copy_key = TileKey {
            layer: destination,
            mip: 0,
            x: -20,
            y: 0,
        };
        let edited_copy = after
            .with_replacements([(copy_key, vec![255; TILE_BYTE_LEN])])
            .unwrap();
        assert_eq!(
            edited_copy.get(TileKey {
                layer: source,
                ..copy_key
            }),
            before.get(TileKey {
                layer: source,
                ..copy_key
            })
        );
        assert_eq!(
            after.get(copy_key).unwrap().pixels(),
            vec![128; TILE_BYTE_LEN]
        );
        // Include non-preset opacity in the persisted structural operation.
        tree.set_opacity(nyatidraw_api::LayerTreeNodeId::Raster(destination), 17_777)
            .unwrap();
        let canvas = CanvasSpec {
            width_px: 32,
            height_px: 32,
            pixels_per_inch: 96,
        };
        let expected_export = flatten_layer_tree_rgba8(&after, &tree, canvas).unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, tiles, reopened_tree, _, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .unwrap();
            assert_eq!(tiles, after);
            assert_eq!(reopened_tree, tree);
            assert_eq!(
                flatten_layer_tree_rgba8(&tiles, &reopened_tree, canvas).unwrap(),
                expected_export
            );
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-duplicate-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let (mut session, _, _, _, next_id, sink) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        for (offset, tiles, metadata) in [
            (0, before.clone(), &original_tree),
            (1, after.clone(), &tree),
        ] {
            let batch = session
                .prepare_structural_change(
                    SnapshotId(next_id + offset),
                    HistoryNodeId(next_id + offset),
                    u64::try_from(offset + 1).unwrap(),
                    tiles,
                )
                .unwrap();
            db.commit_structural_with_layer_tree(&batch, metadata)
                .unwrap();
            session.accept_structural_change(&batch).unwrap();
        }
        let undo = session.prepare_undo_cursor().unwrap();
        assert_eq!(
            db.load_cursor_layer_tree(undo.target()).unwrap(),
            Some(original_tree)
        );
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, before);
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        let redo = session.prepare_redo_cursor().unwrap();
        assert_eq!(
            db.load_cursor_layer_tree(redo.target()).unwrap(),
            Some(tree)
        );
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, after);
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
        drop(session);
        drop(sink);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::tests::duplicate_layer_preserves_off_page_artwork_history_and_reopen"])
            .env(REOPEN, &path).status().unwrap();
        assert!(status.success());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn clipboard_cut_paste_preserves_artwork_on_failure_undo_and_process_restart() {
        // Product risk: clipboard contention must not cut artwork, and paste
        // must save its new layer and exact alpha atomically with pixel history.
        use crate::artwork_clipboard::{ArtworkClipboard, MemoryClipboard};
        use crate::edit_worker::execute_with_clipboard;
        const REOPEN: &str = "NYATIDRAW_TEST_CLIPBOARD_REOPEN";
        let tree = default_layer_tree();
        let canvas = CanvasSpec {
            width_px: 3,
            height_px: 1,
            pixels_per_inch: 96,
        };
        let key = |layer, x| TileKey {
            layer,
            mip: 0,
            x,
            y: 0,
        };
        let mut pixels = vec![0; TILE_BYTE_LEN];
        pixels[..12].copy_from_slice(&[128, 0, 0, 128, 0, 255, 0, 255, 0, 0, 64, 64]);
        let before = TileSnapshot::from_tiles([
            (key(LIVE_LAYER, 0), pixels),
            (key(LIVE_LAYER, -1), vec![64; TILE_BYTE_LEN]),
        ])
        .unwrap();
        let expected_png = nyatidraw_png_io::encode_png_bytes(
            &flatten_layer_tree_rgba8(&before, &tree, canvas).unwrap(),
        )
        .unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, tiles, reopened_tree, reopened_canvas, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .unwrap();
            assert_eq!(reopened_canvas, canvas);
            let pasted = raster_layer_ids(&reopened_tree)
                .into_iter()
                .find(|id| *id != LIVE_LAYER)
                .unwrap();
            assert_eq!(reopened_tree.raster(pasted).unwrap().name, "붙여넣기");
            assert_eq!(
                tiles.get(key(LIVE_LAYER, -1)),
                before.get(key(LIVE_LAYER, -1))
            );
            assert_eq!(
                &tiles.get(key(pasted, 0)).unwrap().pixels()[..12],
                &[128, 0, 0, 128, 0, 0, 0, 0, 0, 0, 64, 64]
            );
            assert_eq!(
                nyatidraw_png_io::encode_png_bytes(
                    &flatten_layer_tree_rgba8(&tiles, &reopened_tree, canvas).unwrap()
                )
                .unwrap(),
                expected_png
            );
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-clipboard-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let (mut session, _, _, _, mut next_id, sink) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        db.persist_canvas_spec(canvas).unwrap();
        let seed = session
            .prepare_structural_change(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                1,
                before.clone(),
            )
            .unwrap();
        db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
        session.accept_structural_change(&seed).unwrap();
        next_id += 1;
        let mut clipboard = MemoryClipboard::default();
        let mut selection = Some(Arc::new(
            nyatidraw_paint_cpu::SelectionMask::from_packed_bits(3, 1, &[0b101]).unwrap(),
        ));
        let head = session.current_snapshot();
        let id_before = next_id;
        let copied = execute_with_clipboard(
            EditCommand::CopySelection,
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
            &mut clipboard,
        )
        .ok()
        .unwrap();
        assert!(copied.tiles.is_none());
        assert_eq!(session.current_snapshot(), head);
        assert_eq!(next_id, id_before);
        assert_eq!(session.tiles(), &before);
        assert_eq!(clipboard.read().unwrap().origin(), [0, 0]);
        let bytes_before = clipboard.bytes.clone();
        clipboard.reject_write = true;
        assert!(
            execute_with_clipboard(
                EditCommand::CutSelection,
                LIVE_LAYER,
                canvas,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db,
                &mut clipboard
            )
            .is_err()
        );
        assert_eq!(session.tiles(), &before);
        assert_eq!(session.current_snapshot(), head);
        assert_eq!(clipboard.bytes, bytes_before);
        clipboard.reject_write = false;
        let mut locked = tree.clone();
        locked.set_locked(LIVE_LAYER, true).unwrap();
        assert!(
            execute_with_clipboard(
                EditCommand::CutSelection,
                LIVE_LAYER,
                canvas,
                &locked,
                &mut selection,
                &mut session,
                &mut next_id,
                db,
                &mut clipboard
            )
            .is_err()
        );
        assert_eq!(next_id, id_before);
        let cut = execute_with_clipboard(
            EditCommand::CutSelection,
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
            &mut clipboard,
        )
        .ok()
        .unwrap()
        .tiles
        .unwrap();
        assert_eq!(
            &cut.get(key(LIVE_LAYER, 0)).unwrap().pixels()[..12],
            &[0, 0, 0, 0, 0, 255, 0, 255, 0, 0, 0, 0]
        );
        assert_eq!(
            cut.get(key(LIVE_LAYER, -1)),
            before.get(key(LIVE_LAYER, -1))
        );
        clipboard.bytes = Some(vec![0; 24]);
        let cut_id = next_id;
        assert!(
            execute_with_clipboard(
                EditCommand::PasteSelection,
                LIVE_LAYER,
                canvas,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db,
                &mut clipboard
            )
            .is_err()
        );
        assert_eq!(next_id, cut_id);
        assert_eq!(session.tiles(), &cut);
        clipboard.bytes = bytes_before;
        let pasted = execute_with_clipboard(
            EditCommand::PasteSelection,
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
            &mut clipboard,
        )
        .ok()
        .unwrap();
        let pasted_tree = pasted.tree.unwrap();
        let pasted_id = pasted.active_layer.unwrap();
        assert_eq!(
            pasted_tree
                .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(pasted_id))
                .unwrap()
                .1,
            1
        );
        assert!(selection.is_none());
        let final_tiles = session.tiles().clone();
        assert_eq!(
            nyatidraw_png_io::encode_png_bytes(
                &flatten_layer_tree_rgba8(&final_tiles, &pasted_tree, canvas).unwrap()
            )
            .unwrap(),
            expected_png
        );
        let undo = session.prepare_undo_cursor().unwrap();
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, cut);
        assert_eq!(
            db.load_cursor_layer_tree(undo.target()).unwrap().unwrap(),
            tree
        );
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        let redo = session.prepare_redo_cursor().unwrap();
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, final_tiles);
        assert_eq!(
            db.load_cursor_layer_tree(redo.target()).unwrap().unwrap(),
            pasted_tree
        );
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
        drop(session);
        drop(sink);
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::tests::clipboard_cut_paste_preserves_artwork_on_failure_undo_and_process_restart"])
            .env(REOPEN, &path).status().unwrap().success());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn eyedropper_is_read_only_and_picked_color_survives_paint_save_reopen() {
        // Product risk: sampling must not create history or alter pixels, and
        // sampled translucent RGB must not darken subsequent saved artwork.
        const REOPEN: &str = "NYATIDRAW_TEST_PICK_COLOR_REOPEN";
        let tree = default_layer_tree();
        let canvas = nyatidraw_api::CanvasSpec {
            width_px: 2,
            height_px: 1,
            pixels_per_inch: 96,
        };
        let key = |x| TileKey {
            layer: LIVE_LAYER,
            mip: 0,
            x,
            y: 0,
        };
        let outside = [128, 0, 0, 128].repeat(TILE_BYTE_LEN / 4);
        let before = TileSnapshot::from_tiles([(key(-1), outside.clone())]).unwrap();
        let mut painted = vec![0; TILE_BYTE_LEN];
        painted[..4].copy_from_slice(&[255, 0, 0, 255]);
        let expected = TileSnapshot::from_tiles([(key(-1), outside), (key(0), painted)]).unwrap();
        let expected_png = nyatidraw_png_io::encode_png_bytes(
            &flatten_layer_tree_rgba8(&expected, &tree, canvas).unwrap(),
        )
        .unwrap();
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, tiles, reopened_tree, reopened_canvas, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .unwrap();
            assert_eq!(tiles, expected);
            assert_eq!(reopened_tree, tree);
            assert_eq!(reopened_canvas, canvas);
            assert_eq!(
                nyatidraw_png_io::encode_png_bytes(
                    &flatten_layer_tree_rgba8(&tiles, &tree, canvas).unwrap(),
                )
                .unwrap(),
                expected_png
            );
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-pick-color-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns(),
        ));
        let (mut session, _, _, _, mut next_id, sink) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        db.persist_canvas_spec(canvas).unwrap();
        let seed = session
            .prepare_structural_change(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                1,
                before.clone(),
            )
            .unwrap();
        db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
        session.accept_structural_change(&seed).unwrap();
        next_id += 1;
        let id_before = next_id;
        let mut selection = Some(Arc::new(
            nyatidraw_paint_cpu::lasso_selection(
                canvas,
                &[[0, 0], [1, 0], [1, 1], [0, 1]],
                nyatidraw_paint_cpu::EditLimits::default(),
            )
            .unwrap(),
        ));
        let mask_before = selection.clone().unwrap();
        let gesture = crate::edit_gesture::EditGesture::begin(
            DrawingTool::Eyedropper,
            nyatidraw_api::EditSettings {
                source: nyatidraw_api::EditSource::ActiveLayer,
                ..Default::default()
            },
            [0; 4],
            true,
            sample(1, PointerPhase::Begin, -1.0, 0.0),
        );
        let command = gesture
            .finish(sample(2, PointerPhase::End, -1.0, 0.0))
            .unwrap();
        let picked = crate::edit_worker::execute(
            command,
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        )
        .ok()
        .expect("sample signed translucent artwork");
        assert_eq!(picked.sampled_color, Some([255, 0, 0, 255]));
        assert!(picked.tiles.is_none());
        assert_eq!(next_id, id_before);
        assert_eq!(session.tiles(), &before);
        assert!(Arc::ptr_eq(selection.as_ref().unwrap(), &mask_before));
        let displayed = crate::edit_worker::execute(
            EditCommand::PickDisplayColor {
                point: [-1, 0],
                solo: Some(nyatidraw_api::LayerTreeNodeId::Raster(LIVE_LAYER)),
                input_sequence: (7, 2),
            },
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        )
        .ok()
        .expect("temporary display sampling preserves the saved source");
        assert_eq!(displayed.sampled_color, picked.sampled_color);
        assert!(displayed.tiles.is_none());
        assert_eq!(next_id, id_before);
        assert_eq!(session.tiles(), &before);
        assert!(Arc::ptr_eq(selection.as_ref().unwrap(), &mask_before));
        assert!(
            crate::edit_worker::execute(
                EditCommand::PickColor {
                    point: [0, 0],
                    source: nyatidraw_api::EditSource::AllVisible
                },
                LIVE_LAYER,
                canvas,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db,
            )
            .is_err(),
            "transparent sampling must leave the foreground unchanged"
        );
        assert_eq!(next_id, id_before);
        let color =
            nyatidraw_tiles::color::srgb8_to_linear_premultiplied(displayed.sampled_color.unwrap());
        crate::edit_worker::execute(
            EditCommand::FillSelection { color },
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        )
        .ok()
        .expect("paint with sampled RGB");
        assert_eq!(session.tiles(), &expected);
        let undo = session.prepare_undo_cursor().unwrap();
        assert_eq!(db.load_cursor_tiles(undo.target()).unwrap(), before);
        drop(session);
        drop(sink);
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::tests::eyedropper_is_read_only_and_picked_color_survives_paint_save_reopen"])
            .env(REOPEN, &path).status().unwrap().success());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn clear_layer_keeps_undo_and_reopens_empty_pixels_without_deleting_layer() {
        // Product risk: clear must be a durable, undoable pixel operation,
        // including outside-page pixels, never deletion of the layer itself.
        const REOPEN: &str = "NYATIDRAW_TEST_CLEAR_REOPEN";
        if let Some(path) = std::env::var_os(REOPEN) {
            let (_, tiles, tree, _, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .expect("fresh process reopens cleared document");
            assert!(tiles.is_empty());
            assert_eq!(tree, default_layer_tree());
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-clear-test-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns(),
        ));
        let location = ProjectLocation::UntitledRecovery(path.clone());
        let (mut session, _, tree, _, mut next_id, sink) =
            open_materialization_session(&location).expect("scratch project");
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        let before = TileSnapshot::from_tiles([-20, 0, 300].map(|x| {
            (
                TileKey {
                    layer: LIVE_LAYER,
                    mip: 0,
                    x,
                    y: 0,
                },
                vec![255; TILE_BYTE_LEN],
            )
        }))
        .expect("signed artwork fixture");
        let seed = session
            .prepare_structural_change(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                1,
                before.clone(),
            )
            .expect("seed artwork");
        db.commit_structural_with_layer_tree(&seed, &tree)
            .expect("save seed");
        session
            .accept_structural_change(&seed)
            .expect("accept seed");
        next_id += 1;
        let outcome = crate::edit_worker::execute(
            EditCommand::ClearActiveLayer,
            LIVE_LAYER,
            nyatidraw_api::CanvasSpec::DEFAULT,
            &tree,
            &mut None,
            &mut session,
            &mut next_id,
            db,
        )
        .ok()
        .expect("clear commits successfully");
        assert!(outcome.tiles.expect("clear changed pixels").is_empty());
        let undo = session.prepare_undo_cursor().expect("clear undo exists");
        let restored = db.load_cursor_tiles(undo.target()).expect("undo pixels");
        assert_eq!(restored.root(), before.root());
        db.persist_history_cursor(undo.target()).expect("save undo");
        session
            .accept_history_cursor_move(undo, restored)
            .expect("accept undo");
        let redo = session.prepare_redo_cursor().expect("clear redo exists");
        let cleared = db.load_cursor_tiles(redo.target()).expect("redo pixels");
        assert!(cleared.is_empty());
        db.persist_history_cursor(redo.target()).expect("save redo");
        session
            .accept_history_cursor_move(redo, cleared)
            .expect("accept redo");
        drop(session);
        drop(sink);
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "native_canvas::tests::clear_layer_keeps_undo_and_reopens_empty_pixels_without_deleting_layer"])
            .env(REOPEN, &path).status().expect("start fresh verifier");
        assert!(status.success());
        std::fs::remove_file(path).expect("remove scratch fixture");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn smoothing_live_dabs_and_saved_replay_remain_identical_after_process_restart() {
        // Product risk: filtering only preview or filtering replay a second time
        // changes the artwork when a stroke closes or the project is reopened.
        const REOPEN: &str = "NYATIDRAW_TEST_SMOOTHING_REOPEN";
        const ROOT: &str = "NYATIDRAW_TEST_SMOOTHING_ROOT";
        let received = [
            sample(1, PointerPhase::Begin, 30.0, 32.0),
            sample(2, PointerPhase::Move, 60.0, 70.0),
            sample(3, PointerPhase::Move, 90.0, 30.0),
            sample(4, PointerPhase::Move, 120.0, 68.0),
            sample(5, PointerPhase::End, 150.0, 40.0),
        ];
        let mut smoother = StrokeSmoother::new(100);
        let expected: Vec<_> = received
            .iter()
            .map(|sample| smoother.process(*sample).unwrap())
            .collect();
        if let Some(path) = std::env::var_os(REOPEN).map(PathBuf::from) {
            let database = ProjectDb::open(&path).unwrap();
            let reopened = database.load_reopened().unwrap().unwrap();
            assert_eq!(reopened.current().unwrap().stroke.samples(), expected);
            assert_eq!(
                format!("{:?}", reopened.current_tiles().root()),
                std::env::var(ROOT).unwrap()
            );
            let output = flatten_layer_tree_rgba8(
                reopened.current_tiles(),
                &default_layer_tree(),
                CanvasSpec::default(),
            )
            .unwrap();
            let png =
                nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
            let imported =
                flatten_layer_tree_rgba8(&png.tiles, &default_layer_tree(), png.canvas).unwrap();
            assert!(
                output.pixels == imported.pixels,
                "fresh-process PNG decode must match the pre-close artwork exactly"
            );
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-smoothing-{}-{}.ntdr",
            std::process::id(),
            system_timestamp_ns()
        ));
        let live_ink = LiveInkBridge::with_capacity(16, LIVE_LAYER);
        let bootstrap = MaterializationWorker::start(
            live_ink.clone(),
            &ProjectLocation::UntitledRecovery(path.clone()),
        )
        .unwrap();
        let mut pipeline = StrokePipeline::new(live_ink.clone(), bootstrap.worker);
        let mut drawing = pipeline.drawing;
        drawing.brush_settings.smoothing = 100;
        live_ink.push(received[0]).unwrap();
        pipeline.drain(LIVE_LAYER, drawing);
        // Mid-stroke UI settings are not allowed to change captured correction.
        drawing.brush_settings.smoothing = 0;
        for sample in &received[1..] {
            live_ink.push(*sample).unwrap();
        }
        assert!(pipeline.drain(LIVE_LAYER, drawing).artwork_closed);
        let live_dabs: Vec<_> = pipeline
            .gpu_ops
            .iter()
            .filter_map(|op| match op {
                GpuStrokeOp::Dabs { dabs, .. } => Some(dabs.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        assert!(!live_dabs.is_empty());
        drop(pipeline);
        assert!(!live_ink.workspace_failed());
        let database = ProjectDb::open(&path).unwrap();
        let reopened = database.load_reopened().unwrap().unwrap();
        let stroke = &reopened.current().unwrap().stroke;
        assert_eq!(stroke.samples(), expected);
        assert_ne!(
            stroke.samples()[1].position_document,
            received[1].position_document
        );
        let mut replay_dabs = Vec::new();
        nyatidraw_brush::replay_round_stroke(
            &stroke.brush,
            &stroke.recorded,
            stroke.samples(),
            &mut replay_dabs,
        )
        .unwrap();
        assert_eq!(
            live_dabs, replay_dabs,
            "GPU command stream and durable CPU replay use identical evaluated positions"
        );
        let root = format!("{:?}", reopened.current_tiles().root());
        let page = flatten_layer_tree_rgba8(
            reopened.current_tiles(),
            &default_layer_tree(),
            CanvasSpec::default(),
        )
        .unwrap();
        assert!(page.pixels.chunks_exact(4).any(|pixel| pixel[3] != 0));
        nyatidraw_png_io::encode_png(&path.with_extension("png"), &page).unwrap();
        drop(database);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::tests::smoothing_live_dabs_and_saved_replay_remain_identical_after_process_restart"])
            .env(REOPEN, &path)
            .env(ROOT, root)
            .status().unwrap();
        assert!(status.success());
        std::fs::remove_file(path.with_extension("png")).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn untitled_recovery_reopens_exact_cpu_tiles_after_lifecycle_restart() {
        const REOPEN_PATH: &str = "NYATIDRAW_TEST_NEW_DOCUMENT_REOPEN";
        const REOPEN_ROOT: &str = "NYATIDRAW_TEST_NEW_DOCUMENT_ROOT";
        if let Some(path) = std::env::var_os(REOPEN_PATH) {
            // A fresh test process must see the same baseline and artwork;
            // dropping handles in the original process alone is not that proof.
            let (_, tiles, tree, _, _, _) =
                open_materialization_session(&ProjectLocation::UntitledRecovery(path.into()))
                    .expect("new process reopens committed document");
            assert_eq!(tree, default_layer_tree());
            assert_eq!(
                format!("{:?}", tiles.root()),
                std::env::var(REOPEN_ROOT).expect("parent supplied expected content root"),
            );
            return;
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-untitled-recovery-test-{}-{nonce}.ntdr",
            std::process::id(),
        ));
        let location = ProjectLocation::UntitledRecovery(path.clone());

        let (mut session, initial_tiles, initial_tree, _, next_id, sink) =
            open_materialization_session(&location).expect("fresh untitled recovery store opens");
        // Product risk: changing defaults must not introduce hidden/background
        // artwork or change the baseline hierarchy on undo and reopen.
        assert!(initial_tiles.is_empty());
        assert_eq!(initial_tree, default_layer_tree());
        let begin = sample(1, PointerPhase::Begin, 128.0, 128.0);
        let end = sample(2, PointerPhase::End, 260.0, 196.0);
        let mut evaluator = RoundBrushEvaluator::new(0x4e41_5941_5449);
        let mut dabs = Vec::new();
        let mut stroke = begin_round_stroke(&mut evaluator, &LIVE_BRUSH, begin, &mut dabs);
        dabs.clear();
        evaluator.push(&mut stroke, &[end], &mut dabs);
        dabs.clear();
        let recorded = evaluator.end(stroke, &mut dabs);
        let batch = session
            .prepare_round_stroke(
                SnapshotId(next_id),
                HistoryNodeId(next_id),
                end.timestamp_ns,
                LIVE_LAYER,
                BrushSnapshot { preset: LIVE_BRUSH },
                recorded,
                LIVE_BRUSH_COLOR,
                vec![begin, end],
            )
            .expect("fixture stroke materializes");
        match &sink {
            ProjectSink::UntitledRecovery { db, .. } => {
                db.commit(&batch)
                    .expect("untitled recovery commits immediately");
            }
            ProjectSink::ExplicitProject { .. } => panic!("fixture must use untitled recovery"),
        }
        session
            .accept_committed(&batch)
            .expect("durably committed fixture advances session");
        let expected_tiles = batch.materialized.after.clone();
        drop(session);
        drop(sink);

        let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "native_canvas::tests::untitled_recovery_reopens_exact_cpu_tiles_after_lifecycle_restart",
                "--nocapture",
            ])
            .env(REOPEN_PATH, &path)
            .env(REOPEN_ROOT, format!("{:?}", expected_tiles.root()))
            .status()
            .expect("spawn independent document reopen verifier");
        assert!(
            child.success(),
            "new process must preserve artwork and initial layer tree"
        );

        let (_, reopened_tiles, reopened_tree, _, _, reopened_sink) =
            open_materialization_session(&location)
                .expect("same process-lifetime recovery path reopens after suspend/resume");
        assert_eq!(reopened_tree, initial_tree);
        assert_eq!(reopened_tiles.root(), expected_tiles.root());
        assert_eq!(reopened_tiles.len(), expected_tiles.len());
        for (key, expected) in expected_tiles.iter() {
            assert_eq!(
                reopened_tiles
                    .get(key)
                    .map(nyatidraw_tiles::TileObject::pixels),
                Some(expected.pixels()),
                "reopened tile {key:?} differs from its committed CPU authority"
            );
        }
        drop(reopened_sink);
        std::fs::remove_file(path).expect("test-only recovery fixture is removable");
    }

    #[test]
    fn newer_export_generation_blocks_a_stale_final_replacement() {
        // Product risk: an earlier PNG encoder must never replace the target
        // after a later Save has already been accepted.
        let mut generations = ExportGenerationGate::default();
        generations.accept_submission(1);
        generations.accept_submission(2);

        assert!(!generations.is_current(1));
        assert!(generations.is_current(2));
    }

    #[test]
    fn full_completion_lane_quarantines_input_and_fail_stops_delivery() {
        // Product risk: a closed stroke's exact GPU-preview retirement payload
        // must never be silently lost when the bounded completion lane fills.
        let live_ink = LiveInkBridge::with_capacity(1, LIVE_LAYER);
        let (sender, receiver) = sync_channel(1);
        sender
            .try_send(ClosedStrokeCompletion {
                ordinal: 1,
                stroke_generation: 1,
                tiles: Vec::new(),
                history: None,
            })
            .expect("fixture fills the completion lane");

        assert!(!publish_closed_completion(
            &sender,
            &AtomicBool::new(false),
            &live_ink,
            ClosedStrokeCompletion {
                ordinal: 2,
                stroke_generation: 2,
                tiles: Vec::new(),
                history: None,
            },
        ));
        assert!(live_ink.workspace_failed());
        assert_eq!(
            receiver
                .try_recv()
                .expect("the already accepted payload remains available")
                .ordinal,
            1
        );
    }

    #[test]
    fn shutdown_preserves_all_closed_artwork_beyond_display_completion_capacity() {
        // Product risk: closing the canvas must not lose pending closed
        // strokes merely because nobody can consume GPU completions anymore.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-close-drain-test-{}-{nonce}.ntdr",
            std::process::id(),
        ));
        let location = ProjectLocation::UntitledRecovery(path.clone());
        let live_ink = LiveInkBridge::with_capacity(512, LIVE_LAYER);
        let bootstrap = MaterializationWorker::start(live_ink.clone(), &location)
            .expect("scratch writer starts");
        let pending = Arc::clone(&bootstrap.worker.pending);
        enqueue_synthetic_durability_probe(&live_ink).expect("32 closed strokes fit native lane");
        drop(StrokePipeline::new(live_ink.clone(), bootstrap.worker));

        assert!(
            !live_ink.workspace_failed(),
            "retired display cannot stop persistence"
        );
        assert_eq!(
            pending.load(Ordering::Relaxed),
            0,
            "every accepted stroke drained"
        );
        let database = ProjectDb::open(&path).expect("shutdown released writer lock");
        let reopened = database
            .load_reopened()
            .expect("valid durable project")
            .expect("closed artwork persisted");
        assert_eq!(reopened.current_snapshot(), SnapshotId(32));
        assert_eq!(reopened.history().node_count(), 32);
        assert!(!reopened.current_tiles().is_empty());
        drop(database);
        std::fs::remove_file(path).expect("remove owned scratch project");
    }

    #[test]
    fn close_preserves_received_samples_without_resurrecting_cancelled_or_invalid_strokes() {
        let nonce = system_timestamp_ns();
        for case in [
            "tap",
            "line",
            "cancel",
            "discontinuity",
            "limit",
            "sequence",
            "smoothing-invalid",
        ] {
            let path = std::env::temp_dir().join(format!(
                "nyatidraw-active-close-{}-{nonce}-{case}.ntdr",
                std::process::id(),
            ));
            let live_ink = LiveInkBridge::with_capacity(8, LIVE_LAYER);
            let bootstrap = MaterializationWorker::start(
                live_ink.clone(),
                &ProjectLocation::UntitledRecovery(path.clone()),
            )
            .expect("scratch writer");
            let mut pipeline = StrokePipeline::new(live_ink.clone(), bootstrap.worker);
            if case == "smoothing-invalid" {
                pipeline.drawing.brush_settings.smoothing = 100;
            }
            let mut received = vec![sample(1, PointerPhase::Begin, -4.0, 12.0)];
            if case != "tap" {
                received.push(sample(2, PointerPhase::Move, 150.0, 120.0));
            }
            for sample in &received {
                live_ink.push(*sample).expect("bounded fixture admission");
            }
            pipeline.drain(LIVE_LAYER, pipeline.drawing);
            match case {
                "cancel" => {
                    live_ink
                        .push(sample(3, PointerPhase::Cancel, 150.0, 120.0))
                        .unwrap();
                }
                "discontinuity" => pipeline.consume_discontinuity(InputDiscontinuity {
                    first_lost_sequence: 3,
                    first_lost_phase: PointerPhase::End,
                    quarantined_before_ack: 0,
                }),
                "limit" => pipeline.active.as_mut().unwrap().sample_limit_exceeded = true,
                "sequence" => {
                    pipeline
                        .active
                        .as_mut()
                        .unwrap()
                        .samples
                        .last_mut()
                        .unwrap()
                        .sequence = u64::MAX;
                }
                "smoothing-invalid" => {
                    live_ink
                        .push(sample(3, PointerPhase::Move, f64::NAN, 120.0))
                        .unwrap();
                    pipeline.drain(LIVE_LAYER, pipeline.drawing);
                    assert!(pipeline.active.is_none());
                    assert!(pipeline.awaiting_clean_begin);
                }
                _ => {}
            }
            drop(pipeline);
            let database = ProjectDb::open(&path).expect("closed writer unlocked project");
            let reopened = database.load_reopened().expect("valid recovery state");
            if matches!(case, "tap" | "line") {
                let reopened = reopened.expect("healthy active stroke was saved");
                let saved = reopened.current().unwrap().stroke.samples();
                let mut end = *received.last().unwrap();
                end.sequence += 1;
                end.phase = PointerPhase::End;
                received.push(end);
                assert_eq!(
                    saved, received,
                    "{case}: every received sample and last position survive"
                );
                assert_eq!(reopened.history().node_count(), 1);
                assert!(!reopened.current_tiles().is_empty());
            } else {
                assert!(
                    reopened.is_none(),
                    "{case}: shutdown must not resurrect unsafe artwork"
                );
            }
            assert_eq!(
                live_ink.workspace_failed(),
                matches!(case, "limit" | "sequence")
            );
            drop(database);
            std::fs::remove_file(path).expect("remove owned scratch project");
        }
    }

    #[test]
    fn freed_writer_capacity_cannot_reorder_closed_strokes_ahead_of_the_semantic_backlog() {
        // Product risk: swapping queued paint/erase operations changes artwork.
        // The real bounded channel is empty, but an older semantic request
        // still owns the next writer position.
        let request = |ordinal| {
            let begin = sample(ordinal * 2, PointerPhase::Begin, 10.0, 10.0);
            let end = sample(ordinal * 2 + 1, PointerPhase::End, 12.0, 10.0);
            let mut evaluator = RoundBrushEvaluator::new(1);
            let mut dabs = Vec::new();
            let mut token = begin_round_stroke(&mut evaluator, &LIVE_BRUSH, begin, &mut dabs);
            evaluator.push(&mut token, &[end], &mut dabs);
            ClosedStrokeRequest {
                selection: None,
                alpha_locked: false,
                ordinal,
                stroke_generation: ordinal,
                layer: LIVE_LAYER,
                recorded: evaluator.end(token, &mut dabs),
                samples: vec![begin, end],
                brush: BrushSnapshot { preset: LIVE_BRUSH },
                color: LIVE_BRUSH_COLOR,
                end_timestamp_ns: end.timestamp_ns,
                enqueued_at: Instant::now(),
            }
        };
        let live_ink = LiveInkBridge::with_capacity(2, LIVE_LAYER);
        let (sender, receiver) = sync_channel(2);
        let (_completed_sender, completed) = sync_channel(2);
        let worker = MaterializationWorker {
            sender: Some(sender),
            completed,
            handle: None,
            pending: Arc::new(AtomicUsize::new(0)),
            display_retired: Arc::new(AtomicBool::new(false)),
            export_generations: Arc::new(Mutex::new(ExportGenerationGate::default())),
            live_ink: live_ink.clone(),
        };
        let mut pipeline = StrokePipeline::new(live_ink, worker);
        pipeline.defer_materialization(request(1));
        pipeline.queue_closed_stroke(request(2));
        assert!(
            matches!(
                receiver.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ),
            "a newly free slot must not admit the newer stroke first"
        );
        pipeline.flush_pending_materializations();
        for expected in [1, 2] {
            let WorkerRequest::Stroke(actual) = receiver.try_recv().expect("closed request") else {
                panic!("unexpected worker request");
            };
            assert_eq!(
                actual.ordinal, expected,
                "semantic order must survive backpressure"
            );
        }
        assert!(pipeline.pending_materializations.is_empty());
    }

    #[test]
    fn png_pair_import_export_and_project_reopen_preserve_pixels() {
        // Product risk: Open With pairing must never replace source artwork
        // until a complete export exists, and the paired project must reopen.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-png-pair-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create scratch pair directory");
        let png = directory.join("player.png");
        let project = directory.join("player.ntdr");
        let exported = directory.join("exported.png");
        let fixture = nyatidraw_tiles::FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 1,
            height: 1,
            pixels: vec![64, 32, 16, 128],
        };
        nyatidraw_png_io::encode_png(&png, &fixture).expect("write source PNG");

        let location = ProjectLocation::Explicit {
            path: project.clone(),
            bootstrap_png: Some(png.clone()),
        };
        let (_, tiles, tree, canvas, _, sink) =
            open_materialization_session(&location).expect("PNG initializes paired project");
        assert_eq!((canvas.width_px, canvas.height_px), (1, 1));
        assert!(project.is_file());
        let mut generations = ExportGenerationGate::default();
        generations.accept_submission(1);
        assert!(matches!(
            export_current_png(
                &exported,
                &tiles,
                &tree,
                canvas,
                1,
                &Mutex::new(generations)
            )
            .expect("atomic PNG export"),
            ExportPngOutcome::Replaced
        ));
        let exported_pixels = nyatidraw_png_io::decode_png(&exported, LIVE_LAYER)
            .expect("decode exported PNG")
            .tiles
            .crop_base_layer_rgba8_to_canvas(LIVE_LAYER, canvas)
            .expect("crop exported PNG");
        assert_eq!(exported_pixels.pixels, fixture.pixels);
        drop(sink);

        let reopened = ProjectLocation::Explicit {
            path: project,
            bootstrap_png: None,
        };
        let (_, reopened_tiles, _, reopened_canvas, _, reopened_sink) =
            open_materialization_session(&reopened).expect("paired project reopens");
        assert_eq!(reopened_canvas, canvas);
        assert_eq!(reopened_tiles.root(), tiles.root());
        drop(reopened_sink);
        std::fs::remove_dir_all(directory).expect("remove scratch pair directory");
    }

    #[test]
    fn non_empty_invalid_png_sibling_is_preserved_without_import_fallback() {
        // Product risk: a damaged paired project must never be silently
        // replaced by importing its PNG again, which would destroy history.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-invalid-png-pair-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create scratch pair directory");
        let png = directory.join("player.png");
        let project = directory.join("player.ntdr");
        let fixture = nyatidraw_tiles::FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 1,
            height: 1,
            pixels: vec![64, 32, 16, 128],
        };
        nyatidraw_png_io::encode_png(&png, &fixture).expect("write source PNG");
        let invalid_project = b"not a NyatiDraw project";
        std::fs::write(&project, invalid_project).expect("write invalid sibling project");

        let location = ProjectLocation::Explicit {
            path: project.clone(),
            bootstrap_png: None,
        };
        let Err(error) = open_materialization_session(&location) else {
            panic!("non-empty invalid sibling must reject PNG import fallback");
        };
        assert!(error.contains("invalid-non-empty-preserved=true"));
        assert_eq!(
            std::fs::read(&project).expect("read preserved invalid project"),
            invalid_project,
        );
        assert!(png.is_file(), "source PNG remains untouched");
        std::fs::remove_dir_all(directory).expect("remove scratch pair directory");
    }
}

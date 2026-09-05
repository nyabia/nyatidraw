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

use nyatidraw_api::{
    CanvasSpec, CommandEnvelope, CommandRejectReason, ContentRootId, DockCommand, DockTree,
    DrawingTool, EditorCommand, EditorEvent, GroupId, HistoryNodeId, HistoryProjection,
    LayerCommand, LayerId, ProjectCommand, Revision, SnapshotId, ToolCommand, ViewportCommand,
    ViewportProjection,
};
use nyatidraw_brush::{
    BrushDab, BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot,
    ROUND_BRUSH_ENGINE_VERSION, RecordedStroke, RoundBrushEvaluator, RoundBrushStroke,
    begin_round_stroke,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::{HeadlessStrokeSession, ProjectionError, ProjectionState};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample, ViewportTransform};
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
    elapsed_since_launch,
    live_ink::{AdmittedSample, CloseStatus, ExportStatus, InputDiscontinuity, LiveInkBridge},
    preview::{
        LAYER_THUMBNAIL_MAX_HEIGHT, LAYER_THUMBNAIL_MAX_WIDTH, MAX_LAYER_THUMBNAILS,
        NAVIGATOR_MAX_HEIGHT, NAVIGATOR_MAX_WIDTH, NavigatorViewport, layer_thumbnail_frame,
        navigator_frame,
    },
};

const LIVE_BRUSH: BrushPreset = BrushPreset {
    id: BrushPresetId(1),
    schema_version: 1,
    engine_version: ROUND_BRUSH_ENGINE_VERSION,
    size_px: 28.0,
    opacity: 0.92,
    flow: 0.38,
    spacing_ratio: 0.12,
};
const LIVE_BRUSH_COLOR: StrokeColor = StrokeColor([26, 199, 232, 255]);
const LIVE_LAYER: LayerId = LayerId(1);
const BACKGROUND_LAYER: LayerId = LayerId(2);
const ROOT_GROUP: GroupId = GroupId(100);
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
const INPUT_SAFETY_PROBE_ENV: &str = "NAYATI_INPUT_SAFETY_PROBE";
static ACTIVE_PROBE_USED: AtomicBool = AtomicBool::new(false);
static SAVE_PROBE_USED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
struct DrawingConfig {
    tool: DrawingTool,
    size_tenths: u16,
    opacity_u16: u16,
    color: [u8; 4],
}

impl DrawingConfig {
    fn from_projection(projection: &nyatidraw_api::UiProjection) -> Self {
        Self {
            tool: projection.drawing_tool,
            size_tenths: projection.brush_size_tenths,
            opacity_u16: projection.brush_opacity_u16,
            color: projection.brush_color,
        }
    }

    fn preset(self) -> BrushPreset {
        let (id, flow, spacing_ratio) = match self.tool {
            DrawingTool::Pencil => (BrushPresetId(2), 0.82, 0.08),
            DrawingTool::Pen => (BrushPresetId(3), 0.62, 0.10),
            DrawingTool::Move | DrawingTool::Brush | DrawingTool::Eraser => {
                (BrushPresetId(1), 0.38, 0.12)
            }
        };
        BrushPreset {
            id,
            size_px: f32::from(self.size_tenths) / 10.0,
            opacity: f32::from(self.opacity_u16) / f32::from(u16::MAX),
            flow,
            spacing_ratio,
            ..LIVE_BRUSH
        }
    }

    fn stroke_color(self) -> StrokeColor {
        let alpha = u16::from(self.color[3]);
        let channel = |value: u8| {
            u8::try_from((u16::from(value) * alpha + 127) / 255)
                .expect("premultiplied color channel is bounded")
        };
        StrokeColor([
            channel(self.color[0]),
            channel(self.color[1]),
            channel(self.color[2]),
            self.color[3],
        ])
    }

    fn gpu_color(self) -> [f32; 4] {
        self.color.map(|channel| f32::from(channel) / 255.0)
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

pub(crate) struct SharedGpuCanvas {
    live_ink: LiveInkBridge,
    project_location: ProjectLocation,
    state: CanvasState,
    rendered_frames: u64,
    close_worker: Option<JoinHandle<()>>,
}

impl SharedGpuCanvas {
    pub(crate) fn new(live_ink: LiveInkBridge) -> Self {
        Self {
            live_ink,
            project_location: ProjectLocation::from_environment_or_args(),
            state: CanvasState::Suspended,
            rendered_frames: 0,
            close_worker: None,
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
        self.live_ink.invalidate_canvas_viewport();
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
        self.live_ink.invalidate_canvas_viewport();
        println!("native-shell event=gpu-canvas-suspend");
        self.state = CanvasState::Suspended;
    }

    pub(crate) fn begin_close(&mut self, width: u32, height: u32, scale: f64) {
        if self.close_worker.is_some() {
            return;
        }
        let state = std::mem::replace(&mut self.state, CanvasState::Suspended);
        let CanvasState::Active(mut canvas) = state else {
            self.live_ink.publish_close_status(CloseStatus::Ready);
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
    ) -> Result<(), String> {
        let target = ProjectLocation::from_positional_path(path)?
            .ok_or_else(|| "unsupported activation extension".to_owned())?;
        if self.project_location.same_storage(&target) {
            println!("native-canvas event=activation-reused-current-project");
            return Ok(());
        }
        preflight_project_activation(&target)?;
        let previous = self.project_location.clone();
        println!(
            "native-canvas event=activation-quiescing current={} target={}",
            previous.title(),
            target.title(),
        );
        // Assignment drops `ActiveCanvas` before accepting any target writer.
        self.state = CanvasState::Suspended;
        self.live_ink.reset_for_project_activation();
        self.project_location = target;
        match ActiveCanvas::new(device, queue, &self.live_ink, &self.project_location) {
            Ok(canvas) => {
                self.state = CanvasState::Active(Box::new(canvas));
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
    pub(crate) fn render(&mut self, width: u32, height: u32, scale: f64) -> Option<wgpu::Texture> {
        let CanvasState::Active(canvas) = &mut self.state else {
            return None;
        };
        if self.live_ink.publish_canvas_surface(width, height, scale) {
            let viewport = self.live_ink.canvas_viewport_snapshot();
            println!(
                "native-input event=canvas-surface-updated revision={} width={} height={} scale={} input_origin=child-local",
                viewport.revision, viewport.width_px, viewport.height_px, viewport.scale,
            );
        }
        if let Err(error) =
            self.live_ink
                .publish_canvas_layout(Point { x: 0.0, y: 0.0 }, width, height, scale)
        {
            eprintln!(
                "native-input event=canvas-child-layout-rejected error={error:?} admission=reject"
            );
            return None;
        }
        let display = canvas.render(width, height, scale)?;
        self.rendered_frames = self.rendered_frames.saturating_add(1);

        if self.rendered_frames == 1 {
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

enum CanvasState {
    Active(Box<ActiveCanvas>),
    Suspended,
    Failed,
}

struct ActiveCanvas {
    painter: GpuRoundDabPainter,
    eraser: GpuRoundDabPainter,
    background_painter: GpuRoundDabPainter,
    scene: GpuCompositeScene,
    stroke: StrokePipeline,
    commands: Vec<CommandEnvelope>,
    view: RendererView,
    active_layer: LayerId,
    next_layer_node_id: u128,
    project_title: String,
    project_export_path: Option<PathBuf>,
    pending_save: Option<u64>,
    canvas_spec: CanvasSpec,
    drawing: DrawingConfig,
    projection: ProjectionState,
    cpu_tiles: BTreeMap<TileKey, Vec<u8>>,
    latest_preview_generation: BTreeMap<TileKey, u64>,
    deferred_completions: Vec<ClosedStrokeCompletion>,
    gpu_live_generation: Option<(u64, LiveStrokeToken, bool)>,
    closed_stroke_dirty_base_revision: Option<Revision>,
    close_raw_probe_staged: bool,
    save_probe_step: u8,
    protocol_probe_step: Option<u8>,
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

        let (authoritative, latest_event) = live_ink.protocol_snapshot();
        let restoring_projection = latest_event.is_some();
        let mut projection = ProjectionState::new(DockTree::safe_default());
        let (view, active_layer) = if restoring_projection {
            let active_layer = authoritative
                .active_layer
                .filter(|layer| {
                    tree.ancestors(nyatidraw_api::LayerTreeNodeId::Raster(*layer))
                        .is_some()
                })
                .unwrap_or(LIVE_LAYER);
            let view = RendererView::from_projection(authoritative.viewport);
            println!(
                "native-canvas event=projection-restored revision={} source=protocol-mailbox",
                authoritative.revision.0,
            );
            projection.install_authoritative(authoritative);
            projection.stage_history(initial_history.clone());
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
                    Some(LIVE_LAYER),
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
            (RendererView::default(), LIVE_LAYER)
        };

        let drawing = DrawingConfig::from_projection(projection.current());
        scene
            .set_solo(projection.current().solo_node)
            .map_err(|error| format!("restored-solo:{error:?}"))?;
        let next_layer_node_id = next_layer_node_id(&tree)?;
        live_ink.set_navigation_tool(drawing.tool == DrawingTool::Move);
        let mut canvas = Self {
            painter: GpuRoundDabPainter::new(device, queue, [0.10, 0.78, 0.91, 1.0]),
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
            pending_save: None,
            canvas_spec,
            drawing,
            projection,
            cpu_tiles,
            latest_preview_generation: BTreeMap::new(),
            deferred_completions: Vec::new(),
            gpu_live_generation: None,
            closed_stroke_dirty_base_revision: None,
            close_raw_probe_staged: false,
            save_probe_step: if std::env::var_os(SAVE_PROBE_ENV).is_some()
                && !SAVE_PROBE_USED.swap(true, Ordering::Relaxed)
            {
                0
            } else {
                3
            },
            protocol_probe_step: std::env::var_os(PROTOCOL_PROBE_ENV).map(|_| 0),
            input_safety_probe_step: std::env::var_os(INPUT_SAFETY_PROBE_ENV).map(|_| 0),
            synthetic_seed_enabled: std::env::var_os("NAYATI_SYNTHETIC_INK").is_some(),
            synthetic_seed_submitted: false,
            navigator_viewport: None,
            gpu_batches: 0,
            gpu_batch_logs_remaining: 16,
        };
        if !restoring_projection {
            canvas.enqueue_initial_commands(live_ink);
        }
        live_ink.set_admission_layer(active_layer);
        Ok(canvas)
    }

    fn enqueue_initial_commands(&mut self, live_ink: &LiveInkBridge) {
        let based_on = self.projection.current().revision;
        if live_ink
            .push_editor_command(
                based_on,
                EditorCommand::Viewport(ViewportCommand::FitDocument),
            )
            .is_err()
        {
            eprintln!("native-canvas event=initial-fit-rejected reason=queue-full");
        }
    }

    fn advance_protocol_probe(&mut self) {
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
    fn render(&mut self, width: u32, height: u32, scale: f64) -> Option<wgpu::Texture> {
        if width == 0 || height == 0 {
            return None;
        }
        self.view.ensure_initialized(
            width,
            height,
            scale,
            [self.canvas_spec.width_px, self.canvas_spec.height_px],
        );
        self.apply_materialized_tiles();

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

        // Native samples already carry the renderer-published document
        // coordinates from their admission moment. Consume their End and put
        // its closed stroke in the writer FIFO before accepting same-frame UI
        // commands, especially Save. This never feeds samples through Dioxus.
        let drained = self.stroke.drain(self.active_layer, self.drawing);
        self.execute_gpu_ops(drained.samples);
        if self.synchronize_workspace_failure() {
            return None;
        }
        // `drain` makes a non-blocking attempt before it consumes raw input.
        // Retry once after End so a newly available writer slot cannot let Save
        // overtake a retained closed stroke.
        self.stroke.flush_pending_materializations();
        if drained.artwork_closed {
            self.publish_closed_stroke_dirty();
        }
        self.apply_materialized_tiles();
        self.apply_commands(width, height, scale);
        if self.synchronize_workspace_failure() {
            return None;
        }
        self.advance_save_probe();
        self.flush_pending_save();
        // Advance only after startup's queued Fit and the previous probe
        // command have run, so the next command uses their resulting revision.
        self.advance_protocol_probe();
        if self.advance_input_safety_probe_before_drain() {
            return None;
        }
        self.advance_input_safety_probe_after_drain();
        if let Err(error) = self.stroke.live_ink.publish_renderer_view(
            self.view.pan,
            self.view.zoom,
            self.view.rotation_radians,
        ) {
            eprintln!("native-canvas event=viewport-publish-failed error={error:?}");
            return None;
        }
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
                let _ = self.stroke.live_ink.publish_renderer_view(
                    self.view.pan,
                    self.view.zoom,
                    self.view.rotation_radians,
                );
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
        };
        let stats = match self.scene.render_viewport(viewport) {
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
        Some(texture)
    }

    fn apply_commands(&mut self, width: u32, height: u32, scale: f64) {
        self.synchronize_workspace_failure();
        self.stroke
            .live_ink
            .drain_editor_commands(&mut self.commands);
        let commands = std::mem::take(&mut self.commands);
        for envelope in commands {
            self.apply_command(envelope, width, height, scale);
        }
        // This one-frame compatibility is only for a Save that was queued by
        // the UI just before native input closed a stroke. Later commands must
        // observe the new semantic revision normally.
        self.closed_stroke_dirty_base_revision = None;
    }

    fn apply_command(&mut self, envelope: CommandEnvelope, width: u32, height: u32, scale: f64) {
        let command_id = envelope.id;
        if self.synchronize_workspace_failure() {
            self.reject_command(command_id, CommandRejectReason::WorkspaceFailed);
            return;
        }
        let save_queued_before_closed_stroke = matches!(
            &envelope.command,
            EditorCommand::Project(ProjectCommand::Save)
        ) && self.closed_stroke_dirty_base_revision
            == Some(envelope.based_on);
        let revision_validation = if save_queued_before_closed_stroke {
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
        match command {
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
                        let (sin, cos) = self.view.rotation_radians.sin_cos();
                        self.view.pan = Point {
                            x: logical_center.x
                                - (cos * document.x - sin * document.y) * self.view.zoom,
                            y: logical_center.y
                                - (sin * document.x + cos * document.y) * self.view.zoom,
                        };
                    }
                    ViewportCommand::ZoomSteps(steps) => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        self.view.zoom = (self.view.zoom * 1.25_f64.powi(i32::from(steps)))
                            .clamp(ViewportTransform::MIN_ZOOM, ViewportTransform::MAX_ZOOM);
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
                    ViewportCommand::RotateQuarterSteps(steps) => {
                        self.view.ensure_initialized(
                            width,
                            height,
                            scale,
                            [self.canvas_spec.width_px, self.canvas_spec.height_px],
                        );
                        self.view.rotation_radians +=
                            f64::from(steps) * std::f64::consts::FRAC_PI_4;
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
                };
                if candidate.validate().is_err() {
                    self.view = previous;
                    return Err(CommandRejectReason::InvalidViewport);
                }
                Ok(false)
            }
            EditorCommand::Layer(LayerCommand::SetActive(layer)) => {
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
            EditorCommand::Layer(LayerCommand::AddWhiteBackground) => {
                // A white background is real editable raster content. It is
                // added beneath existing ink through one structural content
                // transition; it never alters any non-background tile.
                if self.scene.active_live_stroke().is_some()
                    || self
                        .cpu_tiles
                        .keys()
                        .any(|key| key.layer == BACKGROUND_LAYER)
                {
                    return Err(CommandRejectReason::UnsupportedCommand);
                }
                let tiles = opaque_white_background_tiles(self.canvas_spec);
                let history = self
                    .stroke
                    .materializer
                    .persist_white_background(tiles.clone())
                    .map_err(|()| CommandRejectReason::CommandQueueBusy)?;
                self.projection.stage_history(history);
                for (key, tile) in tiles.iter() {
                    self.scene
                        .upload_closed_tile(key, tile.pixels())
                        .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
                    self.cpu_tiles.insert(key, tile.pixels().to_vec());
                }
                println!(
                    "native-canvas event=white-background-materialized layer={} tiles={} durability=redb-immediate",
                    BACKGROUND_LAYER.0,
                    self.cpu_tiles.len(),
                );
                Ok(true)
            }
            EditorCommand::Layer(command @ (LayerCommand::AddRaster | LayerCommand::AddGroup)) => {
                if self.scene.active_live_stroke().is_some() {
                    return Err(CommandRejectReason::UnsupportedCommand);
                }
                let node_id = self.next_layer_node_id;
                let next_id = node_id
                    .checked_add(1)
                    .ok_or(CommandRejectReason::RevisionExhausted)?;
                let active_node = nyatidraw_api::LayerTreeNodeId::Raster(self.active_layer);
                let (parent, index) = self.scene.tree().parent_and_index(active_node).map_or(
                    (
                        self.scene.tree().root_id(),
                        self.scene.tree().root().children.len(),
                    ),
                    |(parent, index)| (parent, index.saturating_add(1)),
                );
                let (node, new_active) = match command {
                    LayerCommand::AddRaster => {
                        let id = LayerId(node_id);
                        (
                            LayerTreeNode::Raster(LayerNode {
                                id,
                                name: next_default_node_name(self.scene.tree(), false),
                                visible: true,
                                locked: false,
                                opacity_u16: u16::MAX,
                                content_root: ContentRootId(0),
                            }),
                            Some(id),
                        )
                    }
                    LayerCommand::AddGroup => (
                        LayerTreeNode::Group(GroupNode {
                            id: GroupId(node_id),
                            name: next_default_node_name(self.scene.tree(), true),
                            visible: true,
                            opacity_u16: u16::MAX,
                            children: Vec::new(),
                        }),
                        None,
                    ),
                    _ => unreachable!("match arm only accepts add commands"),
                };
                let mut candidate = self.scene.tree().clone();
                candidate
                    .insert(parent, index, node.clone())
                    .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
                self.scene
                    .validate_empty_node_insert(parent, index, &node)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
                self.stroke
                    .materializer
                    .try_persist_layer_tree(candidate)
                    .map_err(|()| CommandRejectReason::CommandQueueBusy)?;
                self.scene
                    .insert_empty_node(parent, index, node)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
                self.next_layer_node_id = next_id;
                if let Some(layer) = new_active {
                    self.active_layer = layer;
                    self.stroke.live_ink.set_admission_layer(layer);
                }
                Ok(true)
            }
            EditorCommand::Layer(LayerCommand::Rename { node, name }) => {
                let mut candidate = self.scene.tree().clone();
                candidate
                    .rename(node, &name)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
                self.stroke
                    .materializer
                    .try_persist_layer_tree(candidate)
                    .map_err(|()| CommandRejectReason::CommandQueueBusy)?;
                self.scene
                    .rename(node, &name)
                    .map_err(|_| CommandRejectReason::UnknownLayer)?;
                Ok(true)
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
                let mut candidate = self.scene.tree().clone();
                match command {
                    LayerCommand::SetVisibility { node, visible } => candidate
                        .set_visibility(node, visible)
                        .map_err(|_| CommandRejectReason::UnknownLayer)?,
                    LayerCommand::SetOpacity { node, opacity_u16 } => candidate
                        .set_opacity(node, opacity_u16)
                        .map_err(|_| CommandRejectReason::UnknownLayer)?,
                    LayerCommand::Reorder {
                        node,
                        new_parent,
                        index,
                    } => candidate
                        .reorder(node, new_parent, index)
                        .map_err(|_| CommandRejectReason::InvalidLayerMove)?,
                    LayerCommand::AddRaster
                    | LayerCommand::AddGroup
                    | LayerCommand::Rename { .. }
                    | LayerCommand::ToggleSolo(_)
                    | LayerCommand::AddWhiteBackground
                    | LayerCommand::SetActive(_) => {
                        unreachable!("handled above")
                    }
                };
                self.stroke
                    .materializer
                    .try_persist_layer_tree(candidate)
                    .map_err(|()| CommandRejectReason::CommandQueueBusy)?;
                match command {
                    LayerCommand::SetVisibility { node, visible } => self
                        .scene
                        .set_visibility(node, visible)
                        .map_err(|_| CommandRejectReason::UnknownLayer)?,
                    LayerCommand::SetOpacity { node, opacity_u16 } => self
                        .scene
                        .set_opacity(node, opacity_u16)
                        .map_err(|_| CommandRejectReason::UnknownLayer)?,
                    LayerCommand::Reorder {
                        node,
                        new_parent,
                        index,
                    } => self
                        .scene
                        .reorder(node, new_parent, index)
                        .map_err(|_| CommandRejectReason::InvalidLayerMove)?,
                    LayerCommand::AddRaster
                    | LayerCommand::AddGroup
                    | LayerCommand::Rename { .. }
                    | LayerCommand::ToggleSolo(_)
                    | LayerCommand::AddWhiteBackground
                    | LayerCommand::SetActive(_) => {
                        unreachable!("handled above")
                    }
                }
                Ok(true)
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
            EditorCommand::Tool(command) => {
                if self.scene.active_live_stroke().is_some() {
                    return Err(CommandRejectReason::UnsupportedCommand);
                }
                match command {
                    ToolCommand::Select(tool) => {
                        self.drawing.tool = tool;
                        self.stroke
                            .live_ink
                            .set_navigation_tool(tool == DrawingTool::Move);
                    }
                    ToolCommand::CycleBrushFamily => {
                        self.drawing.tool = match self.drawing.tool {
                            DrawingTool::Pencil => DrawingTool::Pen,
                            DrawingTool::Pen => DrawingTool::Brush,
                            DrawingTool::Move | DrawingTool::Brush | DrawingTool::Eraser => {
                                DrawingTool::Pencil
                            }
                        };
                        self.stroke.live_ink.set_navigation_tool(false);
                    }
                    ToolCommand::SetSizeTenths(size) if (1..=2_000).contains(&size) => {
                        self.drawing.size_tenths = size;
                    }
                    ToolCommand::SetOpacityU16(opacity) if opacity > 0 => {
                        self.drawing.opacity_u16 = opacity;
                    }
                    ToolCommand::SetColor(color) if color[3] > 0 => self.drawing.color = color,
                    ToolCommand::SetSizeTenths(_)
                    | ToolCommand::SetOpacityU16(_)
                    | ToolCommand::SetColor(_) => {
                        return Err(CommandRejectReason::UnsupportedCommand);
                    }
                }
                self.projection.stage_drawing_controls(
                    self.drawing.tool,
                    self.drawing.size_tenths,
                    self.drawing.opacity_u16,
                    self.drawing.color,
                );
                Ok(false)
            }
            EditorCommand::History(command) => {
                if self.scene.active_live_stroke().is_some() {
                    return Err(CommandRejectReason::UnsupportedCommand);
                }
                let moved = self
                    .stroke
                    .materializer
                    .move_history(command)
                    .map_err(|()| CommandRejectReason::UnsupportedCommand)?;
                self.projection.stage_history(moved.history);
                let snapshot = moved.tiles;
                let keys: std::collections::BTreeSet<_> = self
                    .cpu_tiles
                    .keys()
                    .copied()
                    .chain(snapshot.iter().map(|(key, _)| key))
                    .collect();
                for key in keys {
                    let pixels = snapshot
                        .get(key)
                        .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
                    self.scene
                        .upload_closed_tile(key, &pixels)
                        .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
                }
                self.cpu_tiles.clear();
                self.cpu_tiles.extend(
                    snapshot
                        .iter()
                        .map(|(key, tile)| (key, tile.pixels().to_vec())),
                );
                Ok(true)
            }
        }
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
        if self.stroke.active.is_some() || !self.stroke.pending_materializations.is_empty() {
            return;
        }
        let Some(path) = self.project_export_path.clone() else {
            return;
        };
        match self.stroke.materializer.enqueue_export(
            generation,
            path,
            self.scene.tree().clone(),
            self.canvas_spec,
            false,
        ) {
            Ok(()) => {
                self.pending_save = None;
                // The FIFO now includes every closed stroke represented by
                // this export. Completion remains a separate worker status.
                let mut saved = self.projection.current().clone();
                saved.dirty = false;
                self.projection.install_authoritative(saved);
                self.publish_committed_history();
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
                self.closed_stroke_dirty_base_revision = Some(previous_revision);
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
        self.stroke
            .materializer
            .drain_completed(&mut self.deferred_completions);
        let active_layer = self.scene.active_live_stroke().map(LiveStrokeToken::layer);
        let mut deferred = Vec::new();
        let mut latest_history = None;
        for completion in self.deferred_completions.drain(..) {
            if let Some(history) = completion.history {
                latest_history = Some(history);
            }
            let mut remaining = Vec::new();
            for (key, pixels) in completion.tiles {
                self.cpu_tiles.insert(key, pixels.clone());
                let newer_preview = self
                    .latest_preview_generation
                    .get(&key)
                    .is_some_and(|generation| *generation > completion.stroke_generation);
                if newer_preview {
                    continue;
                }
                if active_layer == Some(key.layer) {
                    remaining.push((key, pixels));
                    continue;
                }
                match self.scene.upload_closed_tile(key, &pixels) {
                    Ok(()) => {
                        if self.latest_preview_generation.get(&key)
                            == Some(&completion.stroke_generation)
                        {
                            self.latest_preview_generation.remove(&key);
                        }
                    }
                    Err(LayerUploadError::LiveStrokeActive(_)) => remaining.push((key, pixels)),
                    Err(error) => eprintln!(
                        "live-ink event=closed-tile-display-upload-failed stroke={} error={error:?}",
                        completion.ordinal,
                    ),
                }
            }
            if !remaining.is_empty() {
                deferred.push(ClosedStrokeCompletion {
                    ordinal: completion.ordinal,
                    stroke_generation: completion.stroke_generation,
                    tiles: remaining,
                    history: None,
                });
            }
        }
        self.deferred_completions = deferred;
        if let Some(history) = latest_history {
            self.projection.stage_history(history);
            self.publish_committed_history();
        }
    }

    /// Publishes the writer-confirmed history after a closed stroke reaches
    /// durable storage. This is deliberately separate from the raw-input
    /// close boundary, so the panel cannot claim a stroke before commit.
    fn publish_committed_history(&mut self) {
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
                eprintln!(
                    "native-canvas event=committed-history-publication-failed error={error:?}"
                );
                self.stroke.live_ink.publish_workspace_error(format!(
                    "A saved stroke's history could not be published: {error:?}"
                ));
            }
        }
    }

    fn execute_gpu_ops(&mut self, drained_samples: usize) {
        for op in std::mem::take(&mut self.stroke.gpu_ops) {
            match op {
                GpuStrokeOp::Replace {
                    generation,
                    layer,
                    color,
                    eraser,
                } => {
                    if !eraser {
                        self.painter.set_brush_rgba(color);
                    }
                    match self.scene.replace_live_stroke(layer) {
                        Ok(replacement) => {
                            if replacement.cancelled.is_some()
                                && let Some((previous_generation, _, _)) = self.gpu_live_generation
                            {
                                self.latest_preview_generation
                                    .retain(|_, latest| *latest != previous_generation);
                            }
                            // A discontinuity can discard an admitted Begin
                            // before GPU submission. Semantic generations and
                            // device-local tokens therefore need not coincide.
                            self.gpu_live_generation =
                                Some((generation, replacement.active, eraser));
                        }
                        Err(error) => {
                            self.fail_gpu_transaction("begin", generation, None, &error);
                            break;
                        }
                    }
                }
                GpuStrokeOp::Dabs { generation, dabs } => {
                    let Some((active_generation, token, eraser)) = self.gpu_live_generation else {
                        continue;
                    };
                    if active_generation != generation {
                        continue;
                    }
                    let painter = if eraser {
                        &mut self.eraser
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
                    let Some((active_generation, token, eraser)) = self.gpu_live_generation.take()
                    else {
                        continue;
                    };
                    if active_generation != generation {
                        self.gpu_live_generation = Some((active_generation, token, eraser));
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
            .or_else(|| self.gpu_live_generation.map(|(_, token, _)| token))
            .or_else(|| self.scene.active_live_stroke());
        let failed_generation = self
            .gpu_live_generation
            .map_or(generation, |(active, _, _)| active);
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
    initialized: bool,
}

impl Default for RendererView {
    fn default() -> Self {
        Self {
            pan: Point { x: 0.0, y: 0.0 },
            zoom: 1.0,
            rotation_radians: 0.0,
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
        self.initialized = true;
    }

    fn zoom_at(&mut self, focus: Point, steps: i8) {
        let previous_zoom = self.zoom;
        let next_zoom = (previous_zoom * 1.25_f64.powi(i32::from(steps)))
            .clamp(ViewportTransform::MIN_ZOOM, ViewportTransform::MAX_ZOOM);
        let x = focus.x - self.pan.x;
        let y = focus.y - self.pan.y;
        let (sin, cos) = self.rotation_radians.sin_cos();
        let document_x = (cos * x + sin * y) / previous_zoom;
        let document_y = (-sin * x + cos * y) / previous_zoom;
        self.pan = Point {
            x: focus.x - (cos * document_x - sin * document_y) * next_zoom,
            y: focus.y - (sin * document_x + cos * document_y) * next_zoom,
        };
        self.zoom = next_zoom;
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

fn default_layer_tree() -> LayerTree {
    let raster = |id, name: &str| {
        LayerTreeNode::Raster(LayerNode {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    };
    LayerTree::new(GroupNode {
        id: ROOT_GROUP,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![
            raster(BACKGROUND_LAYER, "Background"),
            LayerTreeNode::Group(GroupNode {
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

fn next_layer_node_id(tree: &LayerTree) -> Result<u128, String> {
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

fn opaque_white_background_tiles(canvas: CanvasSpec) -> TileSnapshot {
    let columns = canvas.width_px.div_ceil(TILE_EDGE);
    let rows = canvas.height_px.div_ceil(TILE_EDGE);
    let white_tile = vec![u8::MAX; TILE_BYTE_LEN];
    TileSnapshot::from_tiles((0..rows).flat_map(|y| {
        let white_tile = white_tile.clone();
        (0..columns).map(move |x| {
            (
                TileKey {
                    layer: BACKGROUND_LAYER,
                    mip: 0,
                    x: i32::try_from(x).expect("document tile column fits i32"),
                    y: i32::try_from(y).expect("document tile row fits i32"),
                },
                white_tile.clone(),
            )
        })
    }))
    .expect("built-in white background tiles are canonical")
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

struct StrokePipeline {
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
}

impl StrokePipeline {
    fn new(live_ink: LiveInkBridge, materializer: MaterializationWorker) -> Self {
        let materialization_backlog_capacity = live_ink.maximum_drain_len();
        Self {
            live_ink,
            evaluator: RoundBrushEvaluator::new(0x4e41_5941_5449),
            active: None,
            queued: Vec::with_capacity(materialization_backlog_capacity),
            pending_materializations: VecDeque::with_capacity(materialization_backlog_capacity),
            materialization_backlog_capacity,
            gpu_ops: Vec::with_capacity(256),
            scratch_dabs: Vec::with_capacity(256),
            next_generation: 1,
            selected_layer: LIVE_LAYER,
            drawing: DrawingConfig {
                tool: DrawingTool::Brush,
                size_tenths: 280,
                opacity_u16: 60_292,
                color: LIVE_BRUSH_COLOR.0,
            },
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
            };
        }

        let mut artwork_closed = false;
        for admitted in queued_samples.drain(..) {
            let mut sample = admitted.queued.sample;
            if self.awaiting_clean_begin && sample.phase != PointerPhase::Begin {
                self.recovery_quarantined = self.recovery_quarantined.saturating_add(1);
                continue;
            }
            if self.awaiting_clean_begin {
                self.awaiting_clean_begin = false;
                println!(
                    "live-ink event=input-discontinuity-recovered clean_begin_sequence={} quarantined_after_ack={}",
                    sample.sequence, self.recovery_quarantined,
                );
            }
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
                    sample.eraser = eraser;
                    let brush = drawing.preset();
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
                    });
                    append_gpu_dabs(&mut self.gpu_ops, generation, &mut self.scratch_dabs);
                    self.active = Some(LiveStroke {
                        token,
                        generation,
                        layer,
                        gpu_op_start,
                        samples: vec![sample],
                        sample_limit_exceeded: false,
                        brush: BrushSnapshot { preset: brush },
                        color,
                        eraser,
                    });
                }
                PointerPhase::Move => {
                    if let Some(active) = self.active.as_mut() {
                        sample.eraser = active.eraser;
                        active.retain_sample(sample);
                        self.scratch_dabs.clear();
                        self.evaluator
                            .push(&mut active.token, &[sample], &mut self.scratch_dabs);
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
        }
    }

    fn consume_discontinuity(&mut self, discontinuity: InputDiscontinuity) {
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
        self.stroke.prepare_shutdown();
        if let Some(generation) = self.pending_save.take() {
            if self.stroke.live_ink.workspace_failed() {
                self.stroke.live_ink.fail_incomplete_export();
                return;
            }
            if let Some(path) = self.project_export_path.clone()
                && self
                    .stroke
                    .materializer
                    .enqueue_export(
                        generation,
                        path,
                        self.scene.tree().clone(),
                        self.canvas_spec,
                        true,
                    )
                    .is_err()
            {
                self.stroke
                    .live_ink
                    .publish_export_status(ExportStatus::Failed { generation });
            }
        }
    }
}

struct LiveStroke {
    token: RoundBrushStroke,
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

enum WorkerRequest {
    Stroke(ClosedStrokeRequest),
    PersistLayerTree(LayerTree),
    ExportPng {
        generation: u64,
        path: PathBuf,
        tree: LayerTree,
        canvas: CanvasSpec,
    },
    MoveHistory {
        command: nyatidraw_api::HistoryCommand,
        reply: SyncSender<Result<HistoryMove, String>>,
    },
    PersistInitialBackground {
        tiles: TileSnapshot,
        reply: SyncSender<Result<HistoryProjection, String>>,
    },
}

struct HistoryMove {
    tiles: TileSnapshot,
    history: HistoryProjection,
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
    let layer_tree = db
        .load_layer_tree()
        .map_err(|error| format!("layer-tree-reopen:path={}:error={error}", path.display()))?
        .unwrap_or_else(default_layer_tree);
    let persisted_canvas = db
        .load_canvas_spec()
        .map_err(|error| format!("canvas-reopen:path={}:error={error}", path.display()))?;
    let reopened = db.load_reopened().map_err(|error| {
        format!(
            "project-reopen:path={}:error={error}:original-preserved=true",
            path.display()
        )
    })?;

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

    fn try_persist_layer_tree(&self, tree: LayerTree) -> Result<(), ()> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(());
        };
        sender
            .try_send(WorkerRequest::PersistLayerTree(tree))
            .map_err(|_| ())
    }

    fn persist_white_background(&self, tiles: TileSnapshot) -> Result<HistoryProjection, ()> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(());
        };
        let (reply, received) = sync_channel(0);
        sender
            .try_send(WorkerRequest::PersistInitialBackground { tiles, reply })
            .map_err(|_| ())?;
        received.recv().ok().and_then(Result::ok).ok_or(())
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
        tree: LayerTree,
        canvas: CanvasSpec,
        shutdown: bool,
    ) -> Result<(), ExportEnqueueError> {
        let sender = self
            .sender
            .as_ref()
            .ok_or(ExportEnqueueError::Disconnected)?;
        let request = WorkerRequest::ExportPng {
            generation,
            path,
            tree,
            canvas,
        };
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

    fn move_history(&self, command: nyatidraw_api::HistoryCommand) -> Result<HistoryMove, ()> {
        let Some(sender) = self.sender.as_ref() else {
            return Err(());
        };
        let (reply, received) = sync_channel(0);
        sender
            .try_send(WorkerRequest::MoveHistory { command, reply })
            .map_err(|_| ())?;
        received.recv().ok().and_then(Result::ok).ok_or(())
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
    preview_canvas: CanvasSpec,
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
    while let Ok(work) = receiver.recv() {
        // Dequeue freed a bounded writer slot. Wake a Save retained by the
        // canvas even when this work only changes metadata or exports a PNG.
        live_ink.request_redraw();
        let request = match work {
            WorkerRequest::Stroke(request) => request,
            WorkerRequest::ExportPng {
                generation,
                path,
                tree,
                canvas,
            } => {
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
                    &tree,
                    canvas,
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
                let prepared = match command {
                    nyatidraw_api::HistoryCommand::Undo => session.prepare_undo_cursor(),
                    nyatidraw_api::HistoryCommand::Redo => session.prepare_redo_cursor(),
                };
                let result = prepared
                    .map_err(|error| format!("history-prepare:{error:?}"))
                    .and_then(|prepared| {
                        let target = prepared.target();
                        let db = match sink {
                            ProjectSink::UntitledRecovery { db, .. }
                            | ProjectSink::ExplicitProject { db, .. } => db,
                        };
                        db.persist_history_cursor(target)
                            .map_err(|error| format!("history-persist:{error}"))?;
                        let tiles = db
                            .load_cursor_tiles(target)
                            .map_err(|error| format!("history-load:{error}"))?;
                        session
                            .accept_history_cursor_move(prepared, tiles.clone())
                            .map_err(|error| format!("history-accept:{error:?}"))?;
                        Ok(HistoryMove {
                            tiles,
                            history: session.history_projection(),
                        })
                    });
                if let Err(error) = &result {
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
                let _ = reply.send(result);
                continue;
            }
            WorkerRequest::PersistInitialBackground { tiles, reply } => {
                let after = match session.tiles().with_replacements(
                    tiles
                        .iter()
                        .map(|(key, tile)| (key, tile.pixels().to_vec())),
                ) {
                    Ok(after) => after,
                    Err(error) => {
                        let _ = reply.send(Err(format!("white background tiles: {error:?}")));
                        continue;
                    }
                };
                let Some(next) = next_id.checked_add(1) else {
                    let _ = reply.send(Err("project identifier space is exhausted".to_owned()));
                    continue;
                };
                let result = session
                    .prepare_structural_change(
                        SnapshotId(next_id),
                        HistoryNodeId(next_id),
                        system_timestamp_ns(),
                        after,
                    )
                    .map_err(|error| format!("structural background: {error:?}"))
                    .and_then(|batch| {
                        let persisted = match sink {
                            ProjectSink::UntitledRecovery { db, .. }
                            | ProjectSink::ExplicitProject { db, .. } => {
                                db.commit_structural(&batch)
                            }
                        };
                        persisted.map_err(|error| error.to_string()).and_then(|()| {
                            session
                                .accept_structural_change(&batch)
                                .map_err(|error| format!("session:{error:?}"))
                                .map(|()| session.history_projection())
                        })
                    });
                let failed = result.is_err();
                if !failed {
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
                        [BACKGROUND_LAYER],
                        preview_canvas,
                    );
                }
                let _ = reply.send(result);
                if failed {
                    live_ink.publish_workspace_error("White background could not be saved".into());
                    exit = WorkerExit::FailStop;
                    break;
                }
                next_id = next;
                println!(
                    "native-canvas event=white-background-persisted mode=redb-immediate tiles={}",
                    session.tiles().len(),
                );
                continue;
            }
            WorkerRequest::PersistLayerTree(tree) => {
                match sink {
                    ProjectSink::UntitledRecovery { db, .. }
                    | ProjectSink::ExplicitProject { db, .. } => {
                        if let Err(error) = db.persist_layer_tree(&tree) {
                            eprintln!(
                                "native-canvas event=layer-tree-persist-failed error={error} worker=fail-stop"
                            );
                            live_ink.publish_workspace_error(format!(
                                "Layer metadata could not be saved: {error}"
                            ));
                            exit = WorkerExit::FailStop;
                            break;
                        }
                        println!(
                            "native-canvas event=layer-tree-persisted mode=redb-immediate root={} nodes=metadata",
                            tree.root().id.0,
                        );
                        preview_tree = tree;
                        publish_navigator_frame(
                            live_ink,
                            session.tiles(),
                            &preview_tree,
                            preview_canvas,
                        );
                    }
                }
                continue;
            }
        };
        let started_at = Instant::now();
        let queue_wait_micros = started_at.duration_since(request.enqueued_at).as_micros();
        let snapshot_id = SnapshotId(next_id);
        let result = session.prepare_round_stroke(
            snapshot_id,
            HistoryNodeId(next_id),
            request.end_timestamp_ns,
            request.layer,
            request.brush,
            request.recorded,
            request.color,
            request.samples,
        );
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
        pending.fetch_sub(1, Ordering::Relaxed);
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
    exit
}

fn publish_navigator_frame(
    live_ink: &LiveInkBridge,
    tiles: &TileSnapshot,
    tree: &LayerTree,
    canvas: CanvasSpec,
) {
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
    let surface = flatten_layer_tree_rgba8(tiles, tree, canvas)
        .map_err(|error| format!("composite:{error:?}"))?;
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
    if let Err(error) = nyatidraw_png_io::encode_png(&temporary, &surface) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("encode:{error}"));
    }
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
            center: nyatidraw_input::Point {
                x: width * t.mul_add(0.68, 0.16),
                y: height * (0.5 + (t * std::f64::consts::TAU).sin() * 0.18),
            },
            radius_px: (8.0 + 12.0 * (t * std::f64::consts::PI).sin()) as f32,
            opacity: 0.86,
            flow: 0.42,
        });
    }
}

fn add_synthetic_background(output: &mut Vec<BrushDab>) {
    for index in 0..18_u32 {
        let t = f64::from(index) / 17.0;
        output.push(BrushDab {
            center: Point {
                x: 170.0 + t * 680.0,
                y: 570.0 - t * 360.0,
            },
            radius_px: 54.0,
            opacity: 0.72,
            flow: 0.5,
        });
    }
}

fn system_timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn untitled_recovery_reopens_exact_cpu_tiles_after_lifecycle_restart() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-untitled-recovery-test-{}-{nonce}.ntdr",
            std::process::id(),
        ));
        let location = ProjectLocation::UntitledRecovery(path.clone());

        let (mut session, _, _, _, next_id, sink) =
            open_materialization_session(&location).expect("fresh untitled recovery store opens");
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

        let (_, reopened_tiles, _, _, _, reopened_sink) = open_materialization_session(&location)
            .expect("same process-lifetime recovery path reopens after suspend/resume");
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

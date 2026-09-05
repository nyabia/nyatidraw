use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use nyatidraw_api::{
    CommandEnvelope, CommandId, EditorCommand, EditorEvent, EventEnvelope, LayerId, Revision,
    UiProjection, WorkspaceProjection,
};
use nyatidraw_input::{Point, PointerPhase, StylusSample, ViewportTransform};
use nyatidraw_input_queue::{InputQueue, PushError, QueuedSample};

use crate::preview::{
    LayerThumbnailFrame, LayerThumbnailSnapshot, NavigatorFrame, NavigatorSnapshot,
    NavigatorViewport,
};

/// Canvas geometry shared between the compositor callback and native input.
///
/// The custom-paint callback owns the physical surface metrics. The app-owned
/// Blitz shell publishes the matching renderer layout origin only after it has
/// resolved the DOM and cross-checked those metrics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CanvasViewportSnapshot {
    pub(crate) origin_client_px: Option<Point>,
    pub(crate) width_px: u32,
    pub(crate) height_px: u32,
    pub(crate) scale: f64,
    pub(crate) pan: Point,
    pub(crate) zoom: f64,
    pub(crate) rotation_radians: f64,
    pub(crate) revision: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CanvasCommandQueueFull;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CanvasViewportPublishError {
    InvalidGeometry,
    SurfaceUnavailable,
    SurfaceMismatch {
        surface_width_px: u32,
        surface_height_px: u32,
        surface_scale: f64,
        layout_width_px: u32,
        layout_height_px: u32,
        layout_scale: f64,
    },
}

impl CanvasViewportSnapshot {
    fn unavailable() -> Self {
        Self {
            origin_client_px: None,
            width_px: 0,
            height_px: 0,
            scale: 1.0,
            pan: Point { x: 0.0, y: 0.0 },
            zoom: 1.0,
            rotation_radians: 0.0,
            revision: 0,
        }
    }

    pub(crate) fn recorder_mapping(self) -> Option<nyatidraw_input_platform::ViewportSnapshot> {
        let transform = self.window_viewport_transform()?;
        let origin = transform.window_origin_physical;
        let p0 = transform.window_to_document(origin)?;
        let px = transform.window_to_document(Point {
            x: origin.x + 1.0,
            y: origin.y,
        })?;
        let py = transform.window_to_document(Point {
            x: origin.x,
            y: origin.y + 1.0,
        })?;

        Some(nyatidraw_input_platform::ViewportSnapshot {
            revision: self.revision,
            xx: px.x - p0.x,
            xy: py.x - p0.x,
            yx: px.y - p0.y,
            yy: py.y - p0.y,
            tx: p0.x - (px.x - p0.x) * origin.x - (py.x - p0.x) * origin.y,
            ty: p0.y - (px.y - p0.y) * origin.x - (py.y - p0.y) * origin.y,
        })
    }

    pub(crate) fn contains_document_point(self, point: Point) -> bool {
        let Some(transform) = self.window_viewport_transform() else {
            return false;
        };
        let Some(logical) = transform.document_to_logical(point) else {
            return false;
        };
        logical.x >= 0.0
            && logical.y >= 0.0
            && logical.x * self.scale < f64::from(self.width_px)
            && logical.y * self.scale < f64::from(self.height_px)
    }

    pub(crate) fn contains_client_point(self, point: Point) -> bool {
        let Some(origin) = self.origin_client_px else {
            return false;
        };
        point.x >= origin.x
            && point.y >= origin.y
            && point.x < origin.x + f64::from(self.width_px)
            && point.y < origin.y + f64::from(self.height_px)
    }

    pub(crate) fn document_point_for_client(self, point: Point) -> Option<Point> {
        if !self.contains_client_point(point) {
            return None;
        }
        Some(self.recorder_mapping()?.document_point(point))
    }

    fn window_viewport_transform(self) -> Option<ViewportTransform> {
        let transform = ViewportTransform {
            revision: self.revision,
            window_origin_physical: self.origin_client_px?,
            physical_size: [self.width_px, self.height_px],
            dpi_scale: self.scale,
            pan: self.pan,
            zoom: self.zoom,
            rotation_radians: self.rotation_radians,
        };
        transform.validate().ok()
    }
}

/// Bounded native-input handoff shared by the Win32 hook and GPU paint source.
#[derive(Clone)]
pub(crate) struct LiveInkBridge {
    inner: Arc<LiveInkInner>,
}

struct LiveInkInner {
    layout: OnceLock<crate::layout_store::LayoutStore>,
    raw_input: Mutex<RawInputState>,
    canvas_viewport: Mutex<CanvasViewportSnapshot>,
    editor_commands: Mutex<VecDeque<CommandEnvelope>>,
    command_capacity: usize,
    next_command_id: AtomicU64,
    protocol: Mutex<ProtocolMailbox>,
    export: Mutex<ExportMailbox>,
    navigator: Mutex<NavigatorSnapshot>,
    layer_thumbnails: Mutex<LayerThumbnailSnapshot>,
    activation_notice: Mutex<Option<String>>,
    redraw_notifier: Mutex<Option<Notifier>>,
    ui_notifier: Mutex<Option<Notifier>>,
    redraw_requested: AtomicBool,
    fatal_input_quarantine: AtomicBool,
    navigation_tool: AtomicBool,
    closing: AtomicBool,
    close_status: Mutex<CloseStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CloseStatus {
    Open,
    SavingProject,
    Exporting,
    Failed {
        project_saved: bool,
        project_path: String,
    },
    Ready,
}

type Notifier = Arc<dyn Fn() + Send + Sync + 'static>;

struct ProtocolMailbox {
    projection: UiProjection,
    latest_event: Option<EventEnvelope>,
    next_event_sequence: u64,
}

/// Desktop-private export state. It is intentionally separate from the
/// semantic editor projection: export completion must wake the UI, but it must
/// not make a command rendered against the document projection stale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExportStatus {
    Idle,
    Waiting { generation: u64 },
    Queued { generation: u64 },
    Running { generation: u64 },
    Current { generation: u64 },
    Failed { generation: u64 },
}

impl ExportStatus {
    pub(crate) fn generation(self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Waiting { generation }
            | Self::Queued { generation }
            | Self::Running { generation }
            | Self::Current { generation }
            | Self::Failed { generation } => Some(generation),
        }
    }

    fn progression(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Waiting { .. } => 1,
            Self::Queued { .. } => 2,
            Self::Running { .. } => 3,
            Self::Current { .. } | Self::Failed { .. } => 4,
        }
    }
}

struct ExportMailbox {
    status: ExportStatus,
}

struct RawInputState {
    performance_batch_start: Option<std::time::Instant>,
    queue: InputQueue,
    /// Bounded retry lane for transitions encountered while the primary queue
    /// contains only transitions. The producer never waits for the consumer.
    pending_transitions: VecDeque<StylusSample>,
    transition_capacity: usize,
    begin_layers: VecDeque<(u64, LayerId)>,
    admission_layer: LayerId,
    admitted_active: bool,
    edit_paused: bool,
    skipped_edit_gesture: bool,
    discontinuity: Option<InputDiscontinuity>,
    stats: InputSafetyStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InputDiscontinuity {
    pub(crate) first_lost_sequence: u64,
    pub(crate) first_lost_phase: PointerPhase,
    pub(crate) quarantined_before_ack: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct InputSafetyStats {
    pub(crate) discontinuities_latched: u64,
    pub(crate) discontinuities_acknowledged: u64,
    pub(crate) quarantined_after_discontinuity: u64,
    pub(crate) quarantined_after_fatal: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AdmittedSample {
    pub(crate) queued: QueuedSample,
    /// The authoritative active layer observed when this Begin was admitted.
    /// Non-Begin samples intentionally carry no UI/editor state.
    pub(crate) begin_layer: Option<LayerId>,
}

impl LiveInkBridge {
    /// Called once by the primary shell before creating the canvas or UI.
    pub(crate) fn enable_layout_persistence(&self) {
        let weak = Arc::downgrade(&self.inner);
        let (store, tree) = crate::layout_store::LayoutStore::open(Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                Self { inner }.notify_ui();
            }
        }));
        if self.inner.layout.set(store).is_ok() {
            self.inner
                .protocol
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .projection
                .dock = tree;
        }
    }

    pub(crate) fn persist_layout(&self, tree: &nyatidraw_api::DockTree) {
        if let Some(store) = self.inner.layout.get() {
            store.submit(tree);
        }
    }

    pub(crate) fn layout_notice(&self) -> Option<String> {
        self.inner
            .layout
            .get()
            .and_then(crate::layout_store::LayoutStore::notice)
    }

    pub(crate) fn flush_layout(&self) {
        if let Some(store) = self.inner.layout.get() {
            store.flush();
        }
    }

    pub(crate) fn with_capacity(capacity: usize, initial_layer: LayerId) -> Self {
        Self {
            inner: Arc::new(LiveInkInner {
                layout: OnceLock::new(),
                raw_input: Mutex::new(RawInputState {
                    performance_batch_start: None,
                    queue: InputQueue::with_capacity(capacity),
                    pending_transitions: VecDeque::with_capacity(capacity),
                    transition_capacity: capacity,
                    begin_layers: VecDeque::with_capacity(capacity.saturating_mul(2)),
                    admission_layer: initial_layer,
                    admitted_active: false,
                    edit_paused: false,
                    skipped_edit_gesture: false,
                    discontinuity: None,
                    stats: InputSafetyStats::default(),
                }),
                canvas_viewport: Mutex::new(CanvasViewportSnapshot::unavailable()),
                editor_commands: Mutex::new(VecDeque::with_capacity(64)),
                command_capacity: 64,
                next_command_id: AtomicU64::new(1),
                protocol: Mutex::new(ProtocolMailbox {
                    projection: UiProjection::empty(),
                    latest_event: None,
                    next_event_sequence: 1,
                }),
                export: Mutex::new(ExportMailbox {
                    status: ExportStatus::Idle,
                }),
                navigator: Mutex::new(NavigatorSnapshot::default()),
                layer_thumbnails: Mutex::new(LayerThumbnailSnapshot::default()),
                activation_notice: Mutex::new(None),
                redraw_notifier: Mutex::new(None),
                ui_notifier: Mutex::new(None),
                redraw_requested: AtomicBool::new(false),
                fatal_input_quarantine: AtomicBool::new(false),
                navigation_tool: AtomicBool::new(false),
                closing: AtomicBool::new(false),
                close_status: Mutex::new(CloseStatus::Open),
            }),
        }
    }

    /// Enqueues one sample and reports whether the event loop needs one wake-up.
    pub(crate) fn push(&self, sample: StylusSample) -> Result<bool, PushError> {
        let performance_start = crate::performance::start();
        let mut raw = self.raw_input();
        if self.is_closing() {
            return Err(PushError::TransitionQueueFull);
        }
        if self.inner.fatal_input_quarantine.load(Ordering::Acquire) {
            raw.stats.quarantined_after_fatal = raw.stats.quarantined_after_fatal.saturating_add(1);
            return Err(PushError::TransitionQueueFull);
        }
        if let Some(mut discontinuity) = raw.discontinuity {
            discontinuity.quarantined_before_ack =
                discontinuity.quarantined_before_ack.saturating_add(1);
            raw.discontinuity = Some(discontinuity);
            raw.stats.quarantined_after_discontinuity =
                raw.stats.quarantined_after_discontinuity.saturating_add(1);
            return Err(PushError::TransitionQueueFull);
        }

        if raw.edit_paused {
            if sample.phase == PointerPhase::Begin {
                raw.skipped_edit_gesture = true;
                eprintln!(
                    "native-input event=gesture-not-admitted reason=selection-edit sequence={} resume=clean-begin",
                    sample.sequence
                );
            }
            return Err(PushError::AdmissionPaused);
        }
        if raw.skipped_edit_gesture {
            if sample.phase != PointerPhase::Begin {
                return Err(PushError::AdmissionPaused);
            }
            raw.skipped_edit_gesture = false;
        }
        let admission_layer = raw.admission_layer;
        match raw.queue.push(sample) {
            Ok(()) => {}
            Err(error @ PushError::TransitionQueueFull) if sample.phase != PointerPhase::Move => {
                if raw.pending_transitions.len() >= raw.transition_capacity {
                    raw.discontinuity = Some(InputDiscontinuity {
                        first_lost_sequence: sample.sequence,
                        first_lost_phase: sample.phase,
                        quarantined_before_ack: 0,
                    });
                    raw.stats.discontinuities_latched =
                        raw.stats.discontinuities_latched.saturating_add(1);
                    let total = raw.stats.discontinuities_latched;
                    drop(raw);
                    eprintln!(
                        "native-input event=input-discontinuity-latched first_lost_sequence={} phase={:?} total={} admission=quarantine-until-consumer-ack",
                        sample.sequence, sample.phase, total,
                    );
                    self.request_redraw();
                    return Err(error);
                }
                raw.pending_transitions.push_back(sample);
            }
            Err(error) => return Err(error),
        }
        if raw.performance_batch_start.is_none() {
            raw.performance_batch_start = performance_start;
        }
        if sample.phase == PointerPhase::Begin {
            raw.admitted_active = true;
            raw.begin_layers
                .push_back((sample.sequence, admission_layer));
        } else if matches!(sample.phase, PointerPhase::End | PointerPhase::Cancel) {
            raw.admitted_active = false;
        }
        drop(raw);
        Ok(self.request_redraw())
    }

    /// Atomically excludes an in-flight or newly admitted stroke before a CPU edit.
    /// Resume only after CPU/GPU selection state is ready for the next Begin.
    pub(crate) fn try_pause_for_edit(&self) -> bool {
        let mut raw = self.raw_input();
        if self.is_closing()
            || self.inner.fatal_input_quarantine.load(Ordering::Acquire)
            || raw.admitted_active
            || !raw.queue.is_empty()
            || !raw.pending_transitions.is_empty()
            || raw.discontinuity.is_some()
        {
            return false;
        }
        raw.edit_paused = true;
        true
    }

    pub(crate) fn resume_after_edit(&self) {
        self.raw_input().edit_paused = false;
    }

    /// Enqueues one versioned semantic command. This bounded lane cannot carry
    /// raw input samples and is never touched per stylus sample by Dioxus.
    pub(crate) fn push_editor_command(
        &self,
        based_on: Revision,
        command: EditorCommand,
    ) -> Result<CommandId, CanvasCommandQueueFull> {
        if matches!(&command, EditorCommand::Edit(nyatidraw_api::EditCommand::SelectLasso { vertices }) if !(3..=4096).contains(&vertices.len()))
        {
            return Err(CanvasCommandQueueFull);
        }
        if self.workspace_failed() {
            return Err(CanvasCommandQueueFull);
        }
        let id = CommandId(u128::from(
            self.inner.next_command_id.fetch_add(1, Ordering::Relaxed),
        ));
        let mut commands = self
            .inner
            .editor_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_closing() {
            return Err(CanvasCommandQueueFull);
        }
        if commands.len() >= self.inner.command_capacity {
            return Err(CanvasCommandQueueFull);
        }
        commands.push_back(CommandEnvelope {
            id,
            based_on,
            command,
        });
        drop(commands);
        self.request_redraw();
        Ok(id)
    }

    /// Enqueues a UI command against the authoritative projection observed at
    /// the instant the event handler runs. Dioxus callbacks must not carry a
    /// rendered projection revision across later semantic updates.
    pub(crate) fn push_ui_editor_command(
        &self,
        command: EditorCommand,
    ) -> Result<CommandId, CanvasCommandQueueFull> {
        let based_on = self
            .inner
            .protocol
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .projection
            .revision;
        self.push_editor_command(based_on, command)
    }

    pub(crate) fn drain_editor_commands(&self, output: &mut Vec<CommandEnvelope>) {
        output.clear();
        let mut commands = self
            .inner
            .editor_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        output.extend(commands.drain(..));
    }

    /// Publishes the renderer/editor authority's response and immutable UI
    /// projection, then schedules the root Dioxus component exactly once.
    pub(crate) fn publish_editor_event(
        &self,
        event: EditorEvent,
        projection: Option<UiProjection>,
    ) {
        if self.inner.fatal_input_quarantine.load(Ordering::Acquire)
            && projection.as_ref().is_some_and(|projection| {
                !matches!(projection.workspace, WorkspaceProjection::Error { .. })
            })
        {
            self.notify_ui();
            return;
        }
        {
            let mut protocol = self
                .inner
                .protocol
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(projection) = projection {
                if projection.revision < protocol.projection.revision {
                    eprintln!(
                        "native-ui event=projection-publication-ignored reason=revision-regression incoming={} current={}",
                        projection.revision.0, protocol.projection.revision.0,
                    );
                    return;
                }
                protocol.projection = projection;
            }
            let sequence = protocol.next_event_sequence;
            protocol.next_event_sequence = protocol.next_event_sequence.saturating_add(1);
            protocol.latest_event = Some(EventEnvelope { sequence, event });
        }
        self.notify_ui();
    }

    pub(crate) fn protocol_snapshot(&self) -> (UiProjection, Option<EventEnvelope>) {
        let protocol = self
            .inner
            .protocol
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (protocol.projection.clone(), protocol.latest_event.clone())
    }

    /// Clears only process-local presentation and admission state before the
    /// canvas replaces one already-drained durable project with another.
    ///
    /// The last revision is retained so a newly opened document can publish a
    /// strictly newer projection. No raw sample, queued command, preview, or
    /// export state from the previous document can cross this boundary.
    pub(crate) fn reset_for_project_activation(&self) {
        self.inner
            .fatal_input_quarantine
            .store(false, Ordering::Release);
        self.inner.navigation_tool.store(false, Ordering::Release);
        {
            let mut raw = self.raw_input();
            while raw.queue.pop().is_some() {}
            raw.pending_transitions.clear();
            raw.begin_layers.clear();
            raw.performance_batch_start = None;
            raw.discontinuity = None;
            raw.admitted_active = false;
            raw.edit_paused = false;
            raw.skipped_edit_gesture = false;
        }
        self.inner
            .editor_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        {
            let mut protocol = self
                .inner
                .protocol
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = protocol.projection.clone();
            let mut next = UiProjection::empty();
            next.revision = previous.revision;
            next.dock = previous.dock;
            next.drawing_tool = previous.drawing_tool;
            next.brush_size_tenths = previous.brush_size_tenths;
            next.brush_opacity_u16 = previous.brush_opacity_u16;
            next.brush_color = previous.brush_color;
            next.recent_colors = previous.recent_colors;
            protocol.projection = next;
            protocol.latest_event = None;
        }
        self.inner
            .export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status = ExportStatus::Idle;
        *self
            .inner
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = NavigatorSnapshot::default();
        *self
            .inner
            .layer_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = LayerThumbnailSnapshot::default();
        *self
            .inner
            .activation_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.request_redraw();
        self.notify_ui();
    }

    /// A failed secondary activation must be visible, but unlike a writer or
    /// GPU failure it must not quarantine the still-open current document.
    pub(crate) fn publish_activation_notice(&self, summary: String) {
        *self
            .inner
            .activation_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(summary);
        self.notify_ui();
    }

    pub(crate) fn activation_notice_snapshot(&self) -> Option<String> {
        self.inner
            .activation_notice
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Returns the latest result for the colocated PNG only. This mailbox has
    /// no raw samples, document pixels, or GPU objects.
    pub(crate) fn export_status_snapshot(&self) -> ExportStatus {
        self.inner
            .export
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
    }

    /// Publishes monotonic export progress. A delayed worker from an older
    /// generation, or a late Queued notification after Running, cannot replace
    /// a newer visible result.
    pub(crate) fn publish_export_status(&self, incoming: ExportStatus) {
        let changed = {
            let mut export = self
                .inner
                .export
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let replace = match (export.status.generation(), incoming.generation()) {
                (None, Some(_)) => true,
                (Some(current), Some(next)) if next > current => true,
                (Some(current), Some(next)) if next == current => {
                    incoming.progression() >= export.status.progression()
                }
                _ => false,
            };
            if replace && export.status != incoming {
                export.status = incoming;
                true
            } else {
                false
            }
        };
        if changed {
            self.notify_ui();
        }
    }

    /// A writer that stops unexpectedly cannot leave the UI reporting an
    /// export that will never finish. A completed PNG remains a completed PNG;
    /// only work still waiting on that writer becomes failed.
    pub(crate) fn fail_incomplete_export(&self) {
        let changed = {
            let mut export = self
                .inner
                .export
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match export.status {
                ExportStatus::Waiting { generation }
                | ExportStatus::Queued { generation }
                | ExportStatus::Running { generation } => {
                    export.status = ExportStatus::Failed { generation };
                    true
                }
                ExportStatus::Idle | ExportStatus::Current { .. } | ExportStatus::Failed { .. } => {
                    false
                }
            }
        };
        if changed {
            self.notify_ui();
        }
    }

    /// Returns the single latest disposable navigator artifact. It lives next
    /// to, rather than inside, the semantic UI projection so pixel payloads
    /// cannot enter the command protocol.
    pub(crate) fn navigator_snapshot(&self) -> NavigatorSnapshot {
        self.inner
            .navigator
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces the one-frame mailbox after a durable CPU state change.
    pub(crate) fn publish_navigator_frame(&self, frame: NavigatorFrame) {
        let changed = {
            let mut navigator = self
                .inner
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let frame = std::sync::Arc::new(frame);
            if navigator.frame.as_ref() == Some(&frame) {
                false
            } else {
                navigator.frame = Some(frame);
                true
            }
        };
        if changed {
            self.notify_ui();
        }
    }

    /// Returns the bounded latest-only raster thumbnail cache. Like the
    /// navigator, it is desktop-private and never crosses the UI protocol.
    pub(crate) fn layer_thumbnail_snapshot(&self) -> LayerThumbnailSnapshot {
        self.inner
            .layer_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Merges a writer-owned durable thumbnail update. The worker's separate
    /// generation is monotonic even when undo moves to an older snapshot, so a
    /// delayed render cannot overwrite the visible current cache.
    pub(crate) fn publish_layer_thumbnails(
        &self,
        generation: u64,
        frames: impl IntoIterator<Item = LayerThumbnailFrame>,
    ) {
        let changed = {
            let mut thumbnails = self
                .inner
                .layer_thumbnails
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if generation < thumbnails.generation {
                return;
            }
            let mut changed = generation != thumbnails.generation;
            for frame in frames {
                let layer = frame.layer;
                let frame = Arc::new(frame);
                if thumbnails.frames.contains_key(&layer)
                    || thumbnails.frames.len() < crate::preview::MAX_LAYER_THUMBNAILS
                {
                    if thumbnails.frames.get(&layer) == Some(&frame) {
                        continue;
                    }
                    thumbnails.frames.insert(layer, frame);
                    changed = true;
                }
            }
            thumbnails.generation = generation;
            changed
        };
        if changed {
            self.notify_ui();
        }
    }

    /// Publishes only changed viewport geometry. Raw stylus samples never
    /// enter this mailbox; callers update it from resolved render view/size.
    pub(crate) fn publish_navigator_viewport(&self, viewport: NavigatorViewport) {
        let changed = {
            let mut navigator = self
                .inner
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if navigator.viewport == Some(viewport) {
                false
            } else {
                navigator.viewport = Some(viewport);
                true
            }
        };
        if changed {
            self.notify_ui();
        }
    }

    pub(crate) fn clear_navigator_viewport(&self) {
        let changed = {
            let mut navigator = self
                .inner
                .navigator
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            navigator.viewport.take().is_some()
        };
        if changed {
            self.notify_ui();
        }
    }

    /// Latches the first fatal workspace error. The same mailbox projection is
    /// the renderer and UI authority until process restart/recovery.
    pub(crate) fn publish_workspace_error(&self, summary: String) -> bool {
        self.inner
            .fatal_input_quarantine
            .store(true, Ordering::Release);
        let published = {
            let mut protocol = self
                .inner
                .protocol
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(
                protocol.projection.workspace,
                WorkspaceProjection::Error { .. }
            ) {
                false
            } else {
                protocol.projection.workspace = WorkspaceProjection::Error { summary };
                protocol.projection.dirty = true;
                let revision = protocol
                    .projection
                    .advance_revision()
                    .unwrap_or(protocol.projection.revision);
                let sequence = protocol.next_event_sequence;
                protocol.next_event_sequence = protocol.next_event_sequence.saturating_add(1);
                protocol.latest_event = Some(EventEnvelope {
                    sequence,
                    event: EditorEvent::ProjectionChanged { revision },
                });
                true
            }
        };
        self.request_redraw();
        self.notify_ui();
        published
    }

    pub(crate) fn workspace_failed(&self) -> bool {
        self.inner.fatal_input_quarantine.load(Ordering::Acquire)
    }

    pub(crate) fn is_closing(&self) -> bool {
        self.inner.closing.load(Ordering::Acquire)
    }

    pub(crate) fn begin_close(&self) -> bool {
        // Lock both admission lanes so the returned boundary includes every
        // previously accepted sample/command, with no late admission behind it.
        let raw = self.raw_input();
        let commands = self
            .inner
            .editor_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = !self.inner.closing.swap(true, Ordering::AcqRel);
        drop(commands);
        drop(raw);
        if first {
            self.publish_close_status(CloseStatus::SavingProject);
        }
        first
    }

    pub(crate) fn close_status(&self) -> CloseStatus {
        self.inner
            .close_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn publish_close_status(&self, status: CloseStatus) {
        *self
            .inner
            .close_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = status;
        self.notify_ui();
        self.request_redraw();
    }

    pub(crate) fn reopen_after_close_failure(&self) {
        self.reset_for_project_activation();
        self.inner.closing.store(false, Ordering::Release);
        self.publish_close_status(CloseStatus::Open);
    }

    pub(crate) fn set_admission_layer(&self, layer: LayerId) {
        self.raw_input().admission_layer = layer;
    }

    pub(crate) fn set_navigation_tool(&self, enabled: bool) {
        self.inner.navigation_tool.store(enabled, Ordering::Release);
    }

    pub(crate) fn navigation_tool(&self) -> bool {
        self.inner.navigation_tool.load(Ordering::Acquire)
    }

    pub(crate) fn set_redraw_notifier(&self, notifier: Notifier) {
        *self
            .inner
            .redraw_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notifier);
        if self.inner.redraw_requested.load(Ordering::Acquire) {
            self.notify_redraw();
        }
    }

    pub(crate) fn set_ui_notifier(&self, notifier: Notifier) {
        *self
            .inner
            .ui_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notifier);
    }

    /// Marks renderer work pending and wakes the event loop without waiting.
    pub(crate) fn request_redraw(&self) -> bool {
        let was_pending = self.inner.redraw_requested.swap(true, Ordering::AcqRel);
        if !was_pending {
            self.notify_redraw();
        }
        !was_pending
    }

    /// Consumes the wakeup represented by the render currently beginning.
    /// Work requested during that render can then post one fresh wakeup.
    pub(crate) fn begin_redraw(&self) {
        self.inner.redraw_requested.store(false, Ordering::Release);
    }

    fn notify_redraw(&self) {
        let notifier = self
            .inner
            .redraw_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    fn notify_ui(&self) {
        let notifier = self
            .inner
            .ui_notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(notifier) = notifier {
            notifier();
        }
    }

    pub(crate) fn drain_into(
        &self,
        output: &mut Vec<AdmittedSample>,
    ) -> Option<InputDiscontinuity> {
        let mut raw = self.raw_input();
        let first = output.len();
        while let Some(queued) = raw.queue.pop() {
            output.push(AdmittedSample {
                queued,
                begin_layer: None,
            });
        }
        while let Some(sample) = raw.pending_transitions.pop_front() {
            output.push(AdmittedSample {
                queued: QueuedSample {
                    pressure_min: sample.pressure,
                    pressure_max: sample.pressure,
                    sample,
                },
                begin_layer: None,
            });
        }
        for admitted in &mut output[first..] {
            if admitted.queued.sample.phase != PointerPhase::Begin {
                continue;
            }
            let Some((sequence, layer)) = raw.begin_layers.pop_front() else {
                debug_assert!(false, "every admitted Begin retains its admission layer");
                continue;
            };
            debug_assert_eq!(sequence, admitted.queued.sample.sequence);
            admitted.begin_layer = Some(layer);
        }
        let discontinuity = raw.discontinuity.take();
        if discontinuity.is_some() {
            raw.admitted_active = false;
            raw.stats.discontinuities_acknowledged =
                raw.stats.discontinuities_acknowledged.saturating_add(1);
        }
        let performance_start = raw.performance_batch_start.take();
        drop(raw);
        if discontinuity.is_none() && output.len() > first {
            crate::performance::input_dequeued(performance_start);
        }
        discontinuity
    }

    /// Maximum number of samples one drain can transfer from the primary
    /// queue and its transition-only retry lane together.
    pub(crate) fn maximum_drain_len(&self) -> usize {
        self.raw_input().transition_capacity.saturating_mul(2)
    }

    pub(crate) fn primary_input_capacity(&self) -> usize {
        self.raw_input().transition_capacity
    }

    pub(crate) fn input_safety_stats(&self) -> InputSafetyStats {
        self.raw_input().stats
    }

    /// Publishes the physical surface metrics exposed by `CustomPaintSource`.
    ///
    /// A size or scale change invalidates the independently measured layout
    /// origin until the event-loop owner publishes a matching complete view.
    pub(crate) fn publish_canvas_surface(&self, width_px: u32, height_px: u32, scale: f64) -> bool {
        let mut viewport = self.canvas_viewport();
        let changed = viewport.width_px != width_px
            || viewport.height_px != height_px
            || viewport.scale.to_bits() != scale.to_bits();
        if changed {
            viewport.width_px = width_px;
            viewport.height_px = height_px;
            viewport.scale = scale;
            viewport.origin_client_px = None;
            viewport.revision = viewport.revision.wrapping_add(1);
        }
        changed
    }

    /// Publishes the affine that the renderer actually used for the display
    /// texture. Input admission observes the same revisioned mapping.
    pub(crate) fn publish_renderer_view(
        &self,
        pan: Point,
        zoom: f64,
        rotation_radians: f64,
    ) -> Result<CanvasViewportSnapshot, CanvasViewportPublishError> {
        let mut viewport = self.canvas_viewport();
        let candidate = ViewportTransform {
            revision: viewport.revision,
            window_origin_physical: Point { x: 0.0, y: 0.0 },
            physical_size: [viewport.width_px, viewport.height_px],
            dpi_scale: viewport.scale,
            pan,
            zoom,
            rotation_radians,
        };
        if candidate.validate().is_err() {
            return Err(CanvasViewportPublishError::InvalidGeometry);
        }
        let changed = viewport.pan != pan
            || viewport.zoom.to_bits() != zoom.to_bits()
            || viewport.rotation_radians.to_bits() != rotation_radians.to_bits();
        if changed {
            viewport.pan = pan;
            viewport.zoom = zoom;
            viewport.rotation_radians = rotation_radians;
            viewport.revision = viewport.revision.wrapping_add(1);
        }
        Ok(*viewport)
    }

    /// Publishes renderer-owned layout geometry after matching it to the most
    /// recent custom-paint surface.
    pub(crate) fn publish_canvas_layout(
        &self,
        origin_client_px: Point,
        width_px: u32,
        height_px: u32,
        scale: f64,
    ) -> Result<CanvasViewportSnapshot, CanvasViewportPublishError> {
        let mut viewport = self.canvas_viewport();
        if !origin_client_px.x.is_finite()
            || !origin_client_px.y.is_finite()
            || !scale.is_finite()
            || scale <= 0.0
            || width_px == 0
            || height_px == 0
        {
            invalidate_origin(&mut viewport);
            return Err(CanvasViewportPublishError::InvalidGeometry);
        }

        if viewport.width_px == 0 || viewport.height_px == 0 {
            invalidate_origin(&mut viewport);
            return Err(CanvasViewportPublishError::SurfaceUnavailable);
        }

        if viewport.width_px != width_px
            || viewport.height_px != height_px
            || viewport.scale.to_bits() != scale.to_bits()
        {
            let error = CanvasViewportPublishError::SurfaceMismatch {
                surface_width_px: viewport.width_px,
                surface_height_px: viewport.height_px,
                surface_scale: viewport.scale,
                layout_width_px: width_px,
                layout_height_px: height_px,
                layout_scale: scale,
            };
            invalidate_origin(&mut viewport);
            return Err(error);
        }

        if viewport.origin_client_px != Some(origin_client_px) {
            viewport.origin_client_px = Some(origin_client_px);
            viewport.revision = viewport.revision.wrapping_add(1);
        }
        Ok(*viewport)
    }

    pub(crate) fn invalidate_canvas_viewport(&self) {
        {
            let mut viewport = self.canvas_viewport();
            viewport.width_px = 0;
            viewport.height_px = 0;
            viewport.origin_client_px = None;
            viewport.revision = viewport.revision.wrapping_add(1);
        }
        self.clear_navigator_viewport();
    }

    pub(crate) fn canvas_viewport_snapshot(&self) -> CanvasViewportSnapshot {
        *self.canvas_viewport()
    }

    fn raw_input(&self) -> MutexGuard<'_, RawInputState> {
        self.inner
            .raw_input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn canvas_viewport(&self) -> MutexGuard<'_, CanvasViewportSnapshot> {
        self.inner
            .canvas_viewport
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn invalidate_origin(viewport: &mut CanvasViewportSnapshot) {
    if viewport.origin_client_px.take().is_some() {
        viewport.revision = viewport.revision.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_input::PenButtons;

    #[test]
    fn edit_admission_never_splits_a_stroke_or_adopts_a_paused_gesture_tail() {
        let bridge = LiveInkBridge::with_capacity(8, LayerId(7));
        let mut received = Vec::new();
        bridge.push(sample(1, PointerPhase::Begin)).unwrap();
        bridge.drain_into(&mut received);
        assert!(
            !bridge.try_pause_for_edit(),
            "an empty queue can still have an active stroke"
        );
        bridge.push(sample(2, PointerPhase::End)).unwrap();
        assert!(
            !bridge.try_pause_for_edit(),
            "queued End must be consumed before editing"
        );
        bridge.drain_into(&mut received);
        assert!(bridge.try_pause_for_edit());
        for (sequence, phase) in [(3, PointerPhase::Begin), (4, PointerPhase::Move)] {
            assert_eq!(
                bridge.push(sample(sequence, phase)),
                Err(PushError::AdmissionPaused)
            );
        }
        bridge.resume_after_edit();
        assert_eq!(
            bridge.push(sample(5, PointerPhase::End)),
            Err(PushError::AdmissionPaused)
        );
        bridge.push(sample(6, PointerPhase::Begin)).unwrap();
        bridge.push(sample(7, PointerPhase::End)).unwrap();
        assert!(bridge.drain_into(&mut received).is_none());
        assert_eq!(
            received
                .iter()
                .map(|item| item.queued.sample.sequence)
                .collect::<Vec<_>>(),
            [1, 2, 6, 7],
            "only complete admitted gestures reach replay"
        );
        assert!(bridge.try_pause_for_edit());
        bridge.reset_for_project_activation();
        bridge.push(sample(8, PointerPhase::Begin)).unwrap();
    }

    #[test]
    fn close_boundary_preserves_admitted_input_and_rejects_late_artwork() {
        let bridge = LiveInkBridge::with_capacity(2, LayerId(7));
        bridge.push(sample(1, PointerPhase::Begin)).unwrap();
        bridge.push(sample(2, PointerPhase::Move)).unwrap();
        bridge
            .push_editor_command(
                Revision(0),
                EditorCommand::Project(nyatidraw_api::ProjectCommand::Save),
            )
            .unwrap();
        assert!(bridge.begin_close());
        assert!(!bridge.begin_close());
        assert!(bridge.push(sample(3, PointerPhase::End)).is_err());
        assert!(
            bridge
                .push_editor_command(
                    Revision(0),
                    EditorCommand::Project(nyatidraw_api::ProjectCommand::Save)
                )
                .is_err()
        );
        let mut received = Vec::new();
        assert!(bridge.drain_into(&mut received).is_none());
        assert_eq!(
            received
                .iter()
                .map(|entry| entry.queued.sample.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let mut commands = Vec::new();
        bridge.drain_editor_commands(&mut commands);
        assert_eq!(commands.len(), 1, "an admitted Save must survive Close");
        assert!(
            !bridge.workspace_failed(),
            "intentional close is not input discontinuity"
        );
    }

    #[test]
    fn fatal_workspace_latch_preserves_error_projection_and_quarantines_art_input() {
        let bridge = LiveInkBridge::with_capacity(2, LayerId(7));
        let mut ready = UiProjection::empty();
        ready.workspace = WorkspaceProjection::Ready;
        ready.revision = Revision(4);
        bridge.publish_editor_event(
            EditorEvent::ProjectionChanged {
                revision: ready.revision,
            },
            Some(ready.clone()),
        );

        assert!(bridge.publish_workspace_error("writer failed".into()));
        bridge.publish_editor_event(
            EditorEvent::CommandAccepted {
                command: CommandId(1),
                revision: Revision(6),
            },
            Some(ready),
        );
        bridge.publish_editor_event(
            EditorEvent::CommandRejected {
                command: CommandId(2),
                reason: nyatidraw_api::CommandRejectReason::WorkspaceFailed,
            },
            None,
        );

        let (projection, event) = bridge.protocol_snapshot();
        assert!(projection.dirty);
        assert!(matches!(
            projection.workspace,
            WorkspaceProjection::Error { ref summary } if summary == "writer failed"
        ));
        assert_eq!(projection.revision, Revision(5));
        assert!(matches!(
            event.map(|event| event.event),
            Some(EditorEvent::CommandRejected {
                reason: nyatidraw_api::CommandRejectReason::WorkspaceFailed,
                ..
            })
        ));
        assert!(bridge.push(sample(9, PointerPhase::Begin)).is_err());
        assert!(
            bridge
                .push_editor_command(
                    Revision(5),
                    EditorCommand::Viewport(nyatidraw_api::ViewportCommand::FitDocument)
                )
                .is_err()
        );
        assert_eq!(bridge.input_safety_stats().quarantined_after_fatal, 1);
    }

    #[test]
    fn writer_exit_fails_only_an_incomplete_png_export() {
        // Product risk: a writer fail-stop/panic/disconnect must not leave the
        // UI waiting forever, nor erase evidence that an older PNG is current.
        let bridge = LiveInkBridge::with_capacity(2, LayerId(7));
        bridge.publish_export_status(ExportStatus::Current { generation: 4 });
        bridge.fail_incomplete_export();
        assert_eq!(
            bridge.export_status_snapshot(),
            ExportStatus::Current { generation: 4 }
        );

        bridge.publish_export_status(ExportStatus::Queued { generation: 5 });
        bridge.fail_incomplete_export();
        assert_eq!(
            bridge.export_status_snapshot(),
            ExportStatus::Failed { generation: 5 }
        );
        bridge.publish_export_status(ExportStatus::Waiting { generation: 6 });
        bridge.publish_export_status(ExportStatus::Current { generation: 5 });
        assert_eq!(
            bridge.export_status_snapshot(),
            ExportStatus::Waiting { generation: 6 }
        );
        bridge.fail_incomplete_export();
        assert_eq!(
            bridge.export_status_snapshot(),
            ExportStatus::Failed { generation: 6 }
        );
    }

    fn sample(sequence: u64, phase: PointerPhase) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence,
            device_id: 1,
            phase,
            position_document: Point { x: 1.0, y: 1.0 },
            pressure: 0.5,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        }
    }
}

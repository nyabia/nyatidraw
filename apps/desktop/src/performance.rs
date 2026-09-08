//! Opt-in CPU timing. Thread-local fixed histograms; no hot-path I/O or locks.
//! Present means the API request returned, never a visible-pixel measurement.
use std::{
    cell::RefCell,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const BINS: usize = 1024;
const HISTORY_SAMPLE_CAPACITY: usize = 64;
const STAGES: [&str; 20] = [
    "input_batch_to_dequeue",
    "input_batch_to_present_request",
    "drain_brush",
    "gpu_ops_encode_submit",
    "composite_encode_submit",
    "frame_cpu",
    "surface_acquire",
    "surface_blit_present_request",
    "history_adoption",
    "projection_publish",
    "stroke_replay",
    "stroke_commit_accept",
    "navigator",
    "layer_thumbnails",
    "export_composite",
    "export_encode",
    "export_sync_replace",
    "history_cpu_snapshot",
    "history_queue_to_adoption",
    "history_queue_to_present_request",
];

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    InputDequeue,
    InputPresent,
    DrainBrush,
    GpuOps,
    Composite,
    Frame,
    SurfaceAcquire,
    SurfacePresent,
    HistoryAdoption,
    Projection,
    Replay,
    Commit,
    Navigator,
    Thumbnails,
    ExportComposite,
    ExportEncode,
    ExportReplace,
    HistoryCpuSnapshot,
    HistoryQueueToAdoption,
    HistoryQueueToPresent,
}

/// One accepted history worker request, carried only by its adoption frame.
/// A failed/no-op request or a dropped frame never borrows a later frame's time.
#[derive(Clone, Copy)]
pub(crate) struct HistoryTiming {
    at: Instant,
    exporting: bool,
    sequence: u64,
    worker: Option<HistoryWorkerTiming>,
    received: Option<Instant>,
    adopted: Option<Instant>,
}

impl HistoryTiming {
    pub(crate) fn queued() -> Option<Self> {
        let at = start()?;
        let mut sequence = 0;
        with_recorder(|r| {
            r.history_queued = r.history_queued.saturating_add(1);
            sequence = r.history_queued;
        });
        Some(Self {
            at,
            exporting: EXPORTING.load(Ordering::Relaxed),
            sequence,
            worker: None,
            received: None,
            adopted: None,
        })
    }

    pub(crate) fn received(&mut self, worker: Option<HistoryWorkerTiming>) {
        self.received = Some(Instant::now());
        self.worker = worker;
    }

    pub(crate) fn adopted(&mut self) {
        self.adopted = Some(Instant::now());
        record(Stage::HistoryQueueToAdoption, self.at, self.exporting);
        with_recorder(|r| r.history_adopted = r.history_adopted.saturating_add(1));
    }

    pub(crate) fn presented(self) {
        let presented = Instant::now();
        record(Stage::HistoryQueueToPresent, self.at, self.exporting);
        with_recorder(|r| {
            r.history_presented = r.history_presented.saturating_add(1);
            if let (Some(worker), Some(received), Some(adopted)) =
                (self.worker, self.received, self.adopted)
            {
                if r.history_samples.len() < HISTORY_SAMPLE_CAPACITY {
                    r.history_samples.push(HistorySample {
                        sequence: self.sequence,
                        exporting: self.exporting,
                        admission: self.at,
                        worker,
                        received,
                        adopted,
                        presented,
                    });
                } else {
                    r.history_samples_omitted = r.history_samples_omitted.saturating_add(1);
                }
            }
        });
    }
}

/// Seven boundaries belonging to one successful history worker response.
#[derive(Clone, Copy)]
pub(crate) struct HistoryWorkerTiming([Instant; 7]);

pub(crate) enum HistoryWorkerStep {
    Prepared = 1,
    TilesLoaded,
    MetadataLoaded,
    CursorPersisted,
    SessionAccepted,
    ReplyReady,
}

impl HistoryWorkerTiming {
    pub(crate) fn begin() -> Option<Self> {
        start().map(|at| Self([at; 7]))
    }

    pub(crate) fn mark(timing: &mut Option<Self>, step: HistoryWorkerStep) {
        if let Some(timing) = timing {
            timing.0[step as usize] = Instant::now();
        }
    }
}

struct HistorySample {
    sequence: u64,
    exporting: bool,
    admission: Instant,
    worker: HistoryWorkerTiming,
    received: Instant,
    adopted: Instant,
    presented: Instant,
}

impl HistorySample {
    fn print(&self, owner: &str) {
        let [
            started,
            prepared,
            loaded,
            metadata,
            persisted,
            accepted,
            ready,
        ] = self.worker.0;
        let micros = |end: Instant, begin: Instant| {
            end.saturating_duration_since(begin)
                .as_nanos()
                .div_ceil(1000)
        };
        let export = if self.exporting { "active" } else { "inactive" };
        // Admission is stamped after try_send succeeds. A worker may begin
        // before that stamp; expose the overlap instead of claiming queue wait.
        println!(
            "performance-history-sample owner={owner} sequence={} export={export} worker_before_admission={} queue_wait_us={} prepare_us={} tile_load_us={} metadata_us={} persist_us={} session_accept_us={} preview_reply_us={} worker_total_us={} reply_wait_us={} adoption_publish_us={} after_adoption_us={} total_us={} claim=cpu-api-only",
            self.sequence,
            started < self.admission,
            micros(started, self.admission),
            micros(prepared, started),
            micros(loaded, prepared),
            micros(metadata, loaded),
            micros(persisted, metadata),
            micros(accepted, persisted),
            micros(ready, accepted),
            micros(ready, started),
            micros(self.received, ready),
            micros(self.adopted, self.received),
            micros(self.presented, self.adopted),
            micros(self.presented, self.admission),
        );
    }
}

static ENABLED: OnceLock<bool> = OnceLock::new();
static EXPORTING: AtomicBool = AtomicBool::new(false);

pub(crate) fn start() -> Option<Instant> {
    (*ENABLED.get_or_init(|| std::env::var("NAYATI_PERFORMANCE").as_deref() == Ok("1")))
        .then(Instant::now)
}

pub(crate) struct Span(Option<(Instant, Stage, bool)>);

impl Span {
    pub(crate) fn new(stage: Stage) -> Self {
        Self(start().map(|at| (at, stage, EXPORTING.load(Ordering::Relaxed))))
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((at, stage, exporting)) = self.0 {
            record(stage, at, exporting);
        }
    }
}

// A single active document has one export writer. This flag classifies CPU
// spans by whether that writer was inside export at the start of the span.
pub(crate) struct ExportInterval;

impl ExportInterval {
    pub(crate) fn enter() -> Self {
        if start().is_some() {
            EXPORTING.store(true, Ordering::Relaxed);
        }
        Self
    }
}

impl Drop for ExportInterval {
    fn drop(&mut self) {
        EXPORTING.store(false, Ordering::Relaxed);
    }
}

struct Histogram {
    bins: Box<[u64]>,
    count: u64,
    min: u64,
    max: u64,
}

impl Histogram {
    fn new() -> Self {
        Self {
            bins: vec![0; BINS].into_boxed_slice(),
            count: 0,
            min: u64::MAX,
            max: 0,
        }
    }

    fn add(&mut self, micros: u64) {
        let index = if micros < 16 {
            usize::try_from(micros).expect("less than 16")
        } else {
            let exponent = micros.ilog2();
            let fraction = (micros - (1_u64 << exponent)) >> (exponent - 4);
            ((exponent - 3) * 16) as usize + usize::try_from(fraction).expect("four mantissa bits")
        };
        self.bins[index] = self.bins[index].saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.min = self.min.min(micros);
        self.max = self.max.max(micros);
    }

    fn percentile_upper(&self, percent: u64) -> u64 {
        let target = (u128::from(self.count) * u128::from(percent)).div_ceil(100);
        let mut count = 0_u128;
        for (index, &observations) in self.bins.iter().enumerate() {
            count += u128::from(observations);
            if count >= target {
                let upper = if index < 16 {
                    index as u64
                } else {
                    let exponent = index / 16 + 3;
                    (1_u64 << exponent).saturating_add(
                        ((index as u64 % 16 + 1) << (exponent - 4)).saturating_sub(1),
                    )
                };
                return upper.min(self.max);
            }
        }
        self.max
    }
}

struct Recorder {
    histograms: Vec<Histogram>,
    pending_input: Option<(Instant, bool)>,
    history_queued: u64,
    history_adopted: u64,
    history_presented: u64,
    history_samples: Vec<HistorySample>,
    history_samples_omitted: u64,
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

fn with_recorder(f: impl FnOnce(&mut Recorder)) {
    RECORDER.with(|cell| {
        let mut recorder = cell.borrow_mut();
        f(recorder.get_or_insert_with(|| Recorder {
            histograms: (0..STAGES.len() * 2).map(|_| Histogram::new()).collect(),
            pending_input: None,
            history_queued: 0,
            history_adopted: 0,
            history_presented: 0,
            history_samples: Vec::with_capacity(HISTORY_SAMPLE_CAPACITY),
            history_samples_omitted: 0,
        }));
    });
}

fn record(stage: Stage, at: Instant, exporting: bool) {
    let micros = u64::try_from(at.elapsed().as_nanos().div_ceil(1000)).unwrap_or(u64::MAX);
    with_recorder(|recorder| {
        recorder.histograms[stage as usize * 2 + usize::from(exporting)].add(micros);
    });
}

pub(crate) fn input_dequeued(at: Option<Instant>) {
    if let Some(at) = at {
        trace_dequeued(at);
        let exporting = EXPORTING.load(Ordering::Relaxed);
        record(Stage::InputDequeue, at, exporting);
        with_recorder(|recorder| {
            if recorder.pending_input.is_none() {
                recorder.pending_input = Some((at, exporting));
            }
        });
    }
}

pub(crate) fn presented() {
    if start().is_none() {
        return;
    }
    let pending = RECORDER.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .and_then(|r| r.pending_input.take())
    });
    if let Some((at, exporting)) = pending {
        record(Stage::InputPresent, at, exporting);
    }
}

pub(crate) fn flush(owner: &str) {
    flush_frame_trace(owner);
    let recorder = RECORDER.with(|cell| cell.borrow_mut().take());
    let Some(recorder) = recorder else {
        return;
    };
    for (stage, name) in STAGES.iter().enumerate() {
        for phase in 0..2 {
            let h = &recorder.histograms[stage * 2 + phase];
            if h.count == 0 {
                continue;
            }
            let export = if phase == 0 { "inactive" } else { "active" };
            println!(
                "performance owner={owner} stage={name} export={export} count={} min_us={} p50_upper_us={} p95_upper_us={} p99_upper_us={} max_us={} histogram=log2-16-sub-bins claim=cpu-api-only",
                h.count,
                h.min,
                h.percentile_upper(50),
                h.percentile_upper(95),
                h.percentile_upper(99),
                h.max
            );
        }
    }
    if recorder.pending_input.is_some() {
        println!("performance owner={owner} input_batch_without_present=1");
    }
    if recorder.history_queued > 0 {
        println!(
            "performance-history owner={owner} queued={} changed_adopted={} adoption_frame_presented={} claim=cpu-api-only",
            recorder.history_queued, recorder.history_adopted, recorder.history_presented
        );
        for sample in &recorder.history_samples {
            sample.print(owner);
        }
        println!(
            "performance-history-samples owner={owner} recorded={} omitted={} capacity={HISTORY_SAMPLE_CAPACITY} reserved_bytes={} scope=changed-presented-history",
            recorder.history_samples.len(),
            recorder.history_samples_omitted,
            recorder.history_samples.capacity() * std::mem::size_of::<HistorySample>()
        );
    }
}

// Independent of the histograms: bounded, opt-in correlation, not a frame
// profiler or proof that a submitted image contains a particular input batch.
const FRAME_TRACE_CAPACITY: usize = 128;
const FRAME_TRACE_TAIL_US: u128 = 8_000;
static FRAME_TRACE_ENABLED: OnceLock<bool> = OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) enum FrameMark {
    Render,
    SceneBegin,
    SceneEnd,
    AcquireBegin,
    AcquireEnd,
    PresentBegin,
    PresentEnd,
    ViewportBegin,
    ViewportEnd,
}

#[derive(Clone, Copy)]
struct TraceInput {
    admission: Instant,
    first_dequeue: Instant,
    last_dequeue: Instant,
    first_batch: u64,
    batches: u64,
    exporting_at_first_dequeue: bool,
    crossed_control_boundary: bool,
}

#[derive(Clone, Copy)]
struct FrameSample {
    id: u64,
    wake: Instant,
    end: Instant,
    marks: [Option<Instant>; 9],
    viewport_calls: u64,
    materialized: MaterializedCounters,
    input: Option<TraceInput>,
    dequeues: u64,
    exporting_at_wake: bool,
    control_boundary: bool,
}

struct FrameTrace {
    materialized_totals: MaterializedCounters,
    sequence: u64,
    current: Option<FrameSample>,
    pending: Option<TraceInput>,
    samples: Box<[Option<FrameSample>]>,
    stored: usize,
    omitted: u64,
    below_threshold: u64,
    // Only this recorded thread is observed; helper-thread tails have no trace.
    unframed_dequeues_in_recorded_thread: u64,
    presented: u64,
    skipped: u64,
    control_only: u64,
    coalesced_presentations: u64,
}

thread_local! {
    static FRAME_TRACE: RefCell<Option<Box<FrameTrace>>> = const { RefCell::new(None) };
}

/// One actor mailbox batch, including control-only and skipped render batches.
/// Wake is observed AFTER the mailbox lock is released, not producer wake time.
pub(crate) struct FrameBatch(bool);

impl FrameBatch {
    pub(crate) fn begin(control_boundary: bool) -> Self {
        if !*FRAME_TRACE_ENABLED.get_or_init(|| {
            start().is_some() && std::env::var("NAYATI_FRAME_TRACE").as_deref() == Ok("1")
        }) {
            return Self(false);
        }
        let now = Instant::now();
        FRAME_TRACE.with(|cell| {
            let mut trace = cell.borrow_mut();
            let trace = trace.get_or_insert_with(|| {
                Box::new(FrameTrace {
                    materialized_totals: MaterializedCounters::default(),
                    sequence: 0,
                    current: None,
                    pending: None,
                    samples: vec![None; FRAME_TRACE_CAPACITY].into_boxed_slice(),
                    stored: 0,
                    omitted: 0,
                    below_threshold: 0,
                    unframed_dequeues_in_recorded_thread: 0,
                    presented: 0,
                    skipped: 0,
                    control_only: 0,
                    coalesced_presentations: 0,
                })
            });
            if control_boundary && let Some(pending) = trace.pending.as_mut() {
                pending.crossed_control_boundary = true;
            }
            trace.sequence = trace.sequence.saturating_add(1);
            trace.current = Some(FrameSample {
                id: trace.sequence,
                wake: now,
                end: now,
                marks: [None; 9],
                viewport_calls: 0,
                materialized: MaterializedCounters::default(),
                input: None,
                dequeues: 0,
                exporting_at_wake: EXPORTING.load(Ordering::Relaxed),
                control_boundary,
            });
        });
        Self(true)
    }
}

pub(crate) fn frame_mark(mark: FrameMark) {
    if FRAME_TRACE_ENABLED.get() != Some(&true) {
        return;
    }
    FRAME_TRACE.with(|cell| {
        if let Some(trace) = cell.borrow_mut().as_mut()
            && let Some(current) = trace.current.as_mut()
        {
            if matches!(mark, FrameMark::ViewportBegin) {
                current.viewport_calls = current.viewport_calls.saturating_add(1);
                current.marks[FrameMark::ViewportEnd as usize] = None;
            }
            current.marks[mark as usize] = Some(Instant::now());
        }
    });
}

#[derive(Clone, Copy, Default)]
struct MaterializedCounters {
    nanos: u128,
    calls: u64,
    completed: u64,
    cpu_clones: u64,
    cpu_bytes: u64,
    upload_attempts: u64,
    upload_successes: u64,
    deferred: u64,
    outside_scene_calls: u64,
}

impl MaterializedCounters {
    fn accumulate(&mut self, add: Self) {
        self.nanos = self.nanos.saturating_add(add.nanos);
        self.calls = self.calls.saturating_add(add.calls);
        self.completed = self.completed.saturating_add(add.completed);
        self.cpu_clones = self.cpu_clones.saturating_add(add.cpu_clones);
        self.cpu_bytes = self.cpu_bytes.saturating_add(add.cpu_bytes);
        self.upload_attempts = self.upload_attempts.saturating_add(add.upload_attempts);
        self.upload_successes = self.upload_successes.saturating_add(add.upload_successes);
        self.deferred = self.deferred.saturating_add(add.deferred);
        self.outside_scene_calls = self
            .outside_scene_calls
            .saturating_add(add.outside_scene_calls);
    }
}

/// Local counters and elapsed time for each materialization adoption call.
/// Multiple non-overlapping calls accumulate, never use their outer envelope.
pub(crate) struct MaterializedTrace {
    started: Option<(Instant, u64)>,
    counts: MaterializedCounters,
}

impl MaterializedTrace {
    pub(crate) fn begin() -> Self {
        let mut result = Self {
            started: None,
            counts: MaterializedCounters::default(),
        };
        if FRAME_TRACE_ENABLED.get() != Some(&true) {
            return result;
        }
        FRAME_TRACE.with(|cell| {
            if let Some(trace) = cell.borrow().as_ref()
                && let Some(current) = trace.current.as_ref()
            {
                result.started = Some((Instant::now(), current.id));
                result.counts.calls = 1;
                result.counts.outside_scene_calls = u64::from(
                    current.marks[FrameMark::SceneBegin as usize].is_none()
                        || current.marks[FrameMark::SceneEnd as usize].is_some(),
                );
            }
        });
        result
    }

    pub(crate) fn add_completed(&mut self, count: usize) {
        if self.started.is_some() {
            self.counts.completed = self
                .counts
                .completed
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        }
    }

    pub(crate) fn add_cpu_clone(&mut self, bytes: usize) {
        if self.started.is_some() {
            self.counts.cpu_clones = self.counts.cpu_clones.saturating_add(1);
            self.counts.cpu_bytes = self
                .counts
                .cpu_bytes
                .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        }
    }

    pub(crate) fn add_upload_attempt(&mut self) {
        if self.started.is_some() {
            self.counts.upload_attempts = self.counts.upload_attempts.saturating_add(1);
        }
    }

    pub(crate) fn add_upload_success(&mut self) {
        if self.started.is_some() {
            self.counts.upload_successes = self.counts.upload_successes.saturating_add(1);
        }
    }

    pub(crate) fn add_deferred(&mut self) {
        if self.started.is_some() {
            self.counts.deferred = self.counts.deferred.saturating_add(1);
        }
    }
}

impl Drop for MaterializedTrace {
    fn drop(&mut self) {
        let Some((started, batch)) = self.started else {
            return;
        };
        self.counts.nanos = started.elapsed().as_nanos();
        FRAME_TRACE.with(|cell| {
            if let Some(trace) = cell.borrow_mut().as_mut()
                && let Some(current) = trace.current.as_mut()
                && current.id == batch
            {
                current.materialized.accumulate(self.counts);
                trace.materialized_totals.accumulate(self.counts);
            }
        });
    }
}

fn trace_dequeued(admission: Instant) {
    if FRAME_TRACE_ENABLED.get() != Some(&true) {
        return;
    }
    FRAME_TRACE.with(|cell| {
        let mut trace = cell.borrow_mut();
        let Some(trace) = trace.as_mut() else { return };
        let Some(current) = trace.current.as_mut() else {
            trace.unframed_dequeues_in_recorded_thread =
                trace.unframed_dequeues_in_recorded_thread.saturating_add(1);
            return;
        };
        current.dequeues = current.dequeues.saturating_add(1);
        let now = Instant::now();
        let pending = trace.pending.get_or_insert(TraceInput {
            admission,
            first_dequeue: now,
            last_dequeue: now,
            first_batch: current.id,
            batches: 0,
            exporting_at_first_dequeue: EXPORTING.load(Ordering::Relaxed),
            crossed_control_boundary: current.control_boundary,
        });
        pending.admission = pending.admission.min(admission);
        pending.last_dequeue = now;
        pending.batches = pending.batches.saturating_add(1);
    });
}

impl Drop for FrameBatch {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        FRAME_TRACE.with(|cell| {
            let mut trace = cell.borrow_mut();
            let Some(trace) = trace.as_mut() else { return };
            let Some(mut sample) = trace.current.take() else {
                return;
            };
            sample.end = Instant::now();
            sample.input = trace.pending;
            let present = sample.marks[FrameMark::PresentEnd as usize];
            if present.is_some() {
                trace.presented = trace.presented.saturating_add(1);
                if sample.input.is_some_and(|input| input.batches > 1) {
                    trace.coalesced_presentations = trace.coalesced_presentations.saturating_add(1);
                }
            } else if sample.marks[FrameMark::Render as usize].is_some() {
                trace.skipped = trace.skipped.saturating_add(1);
            } else {
                trace.control_only = trace.control_only.saturating_add(1);
            }
            let input_tail = sample.input.is_some_and(|input| {
                present
                    .unwrap_or(sample.end)
                    .saturating_duration_since(input.admission)
                    .as_micros()
                    >= FRAME_TRACE_TAIL_US
            });
            let tail = sample.end.duration_since(sample.wake).as_micros() >= FRAME_TRACE_TAIL_US;
            let unpresented = sample.dequeues > 0 && present.is_none();
            if tail || input_tail || unpresented {
                if trace.stored < FRAME_TRACE_CAPACITY {
                    trace.samples[trace.stored] = Some(sample);
                    trace.stored += 1;
                } else {
                    trace.omitted = trace.omitted.saturating_add(1);
                }
            } else {
                trace.below_threshold = trace.below_threshold.saturating_add(1);
            }
            if present.is_some() {
                trace.pending = None;
            }
        });
    }
}

impl FrameSample {
    fn print(self, owner: &str) {
        // Signed offsets preserve admission/dequeue carried from an older batch.
        // -1 is not a sentinel: absent boundaries are printed as `None`.
        let offset = |at: Instant| -> i128 {
            if at >= self.wake {
                i128::try_from(at.duration_since(self.wake).as_micros()).unwrap_or(i128::MAX)
            } else {
                -i128::try_from(self.wake.duration_since(at).as_micros()).unwrap_or(i128::MAX)
            }
        };
        let marks: [Option<i128>; 7] = std::array::from_fn(|index| self.marks[index].map(offset));
        let viewport_begin = self.marks[FrameMark::ViewportBegin as usize].map(offset);
        let viewport_end = self.marks[FrameMark::ViewportEnd as usize].map(offset);
        let materialized = self.materialized;
        let status = if self.marks[FrameMark::PresentEnd as usize].is_some() {
            "present-request-returned"
        } else if self.marks[FrameMark::Render as usize].is_some() {
            "render-without-present"
        } else {
            "control-only"
        };
        let input = self.input;
        let admission_to_present = input.and_then(|input| {
            self.marks[FrameMark::PresentEnd as usize]
                .map(|end| end.saturating_duration_since(input.admission).as_micros())
        });
        println!(
            "performance-frame owner={owner} batch={} status={status} control_boundary={} export_at_wake={} offsets_us_render_scene_begin_scene_end_acquire_begin_acquire_end_present_begin_present_end={marks:?} end_us={} dequeues={} input_first_batch={:?} input_batches={:?} admission_offset_us={:?} first_dequeue_offset_us={:?} last_dequeue_offset_us={:?} export_at_first_dequeue={:?} crossed_control_boundary={:?} admission_to_present_us={admission_to_present:?} viewport_calls={} viewport_last_begin_us={viewport_begin:?} viewport_last_end_us={viewport_end:?} materialized_us={} materialized_calls={} materialized_completed={} materialized_cpu_clones={} materialized_cpu_bytes={} materialized_upload_attempts={} materialized_upload_successes={} materialized_deferred={} materialized_outside_scene_calls={} claim=cpu-api-correlation-not-artwork-proof",
            self.id,
            self.control_boundary,
            self.exporting_at_wake,
            offset(self.end),
            self.dequeues,
            input.map(|i| i.first_batch),
            input.map(|i| i.batches),
            input.map(|i| offset(i.admission)),
            input.map(|i| offset(i.first_dequeue)),
            input.map(|i| offset(i.last_dequeue)),
            input.map(|i| i.exporting_at_first_dequeue),
            input.map(|i| i.crossed_control_boundary),
            self.viewport_calls,
            materialized.nanos / 1000,
            materialized.calls,
            materialized.completed,
            materialized.cpu_clones,
            materialized.cpu_bytes,
            materialized.upload_attempts,
            materialized.upload_successes,
            materialized.deferred,
            materialized.outside_scene_calls,
        );
    }
}

fn flush_frame_trace(owner: &str) {
    let trace = FRAME_TRACE.with(|cell| {
        let mut trace = cell.borrow_mut();
        // If flush is requested while a batch is active, defer trace output
        // until a later flush after FrameBatch::drop accounts for that batch.
        // Histogram flushing remains independent.
        if trace.as_ref().is_some_and(|trace| trace.current.is_some()) {
            return None;
        }
        trace.take()
    });
    let Some(trace) = trace else { return };
    for sample in trace.samples.iter().flatten() {
        sample.print(owner);
    }
    let materialized = trace.materialized_totals;
    println!(
        "performance-frames owner={owner} batches={} recorded={} omitted={} below_threshold={} pending_unpresented_batches={} unframed_dequeues_in_recorded_thread={} presented={} skipped={} control_only={} coalesced_presentations={} materialized_us={} materialized_calls={} materialized_completed={} materialized_cpu_clones={} materialized_cpu_bytes={} materialized_upload_attempts={} materialized_upload_successes={} materialized_deferred={} materialized_outside_scene_calls={} capacity={FRAME_TRACE_CAPACITY} threshold_us={FRAME_TRACE_TAIL_US} selection=first-qualifying wake=mailbox-observed scope=actor-thread-lifetime helper_threads=unobserved",
        trace.sequence,
        trace.stored,
        trace.omitted,
        trace.below_threshold,
        trace.pending.map_or(0, |input| input.batches),
        trace.unframed_dequeues_in_recorded_thread,
        trace.presented,
        trace.skipped,
        trace.control_only,
        trace.coalesced_presentations,
        materialized.nanos / 1000,
        materialized.calls,
        materialized.completed,
        materialized.cpu_clones,
        materialized.cpu_bytes,
        materialized.upload_attempts,
        materialized.upload_successes,
        materialized.deferred,
        materialized.outside_scene_calls,
    );
}

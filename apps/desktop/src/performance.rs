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
const STAGES: [&str; 17] = [
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
}

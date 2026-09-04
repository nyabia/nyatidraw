//! Release-only synthetic overload measurement for the bounded input handoff.
//!
//! This is deliberately a queue benchmark, not a physical-pen or present-latency
//! benchmark. It uses virtual 240 Hz timestamps, 8K document coordinates, and a
//! 256px brush scenario so that coalescing behaviour can be compared over time.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_input_queue::{InputQueue, InputQueueStats};

const PRODUCER_HZ: u32 = 240;
const CONSUMER_HZ: u32 = 60;
const DURATION_SECONDS: u64 = 1_200;
const CAPACITY: usize = 512;
const CANVAS_WIDTH_PX: u32 = 7_680;
const CANVAS_HEIGHT_PX: u32 = 4_320;
const BRUSH_DIAMETER_PX: u32 = 256;

fn main() {
    let destination = env::args_os().nth(1).map_or_else(
        || PathBuf::from("target/input-queue-stress.json"),
        PathBuf::from,
    );
    let move_count = u64::from(PRODUCER_HZ) * DURATION_SECONDS;
    let mut queue = InputQueue::with_capacity(CAPACITY);
    let mut all_push_costs = Vec::new();
    let mut coalesced_push_costs = Vec::new();
    let mut begin_processed = false;

    measure_push(
        &mut queue,
        sample(PointerPhase::Begin, 0),
        &mut all_push_costs,
        &mut coalesced_push_costs,
    );
    for sequence in 1..=move_count {
        measure_push(
            &mut queue,
            sample(PointerPhase::Move, sequence),
            &mut all_push_costs,
            &mut coalesced_push_costs,
        );
        if sequence % u64::from(PRODUCER_HZ / CONSUMER_HZ) == 0 {
            begin_processed |= queue
                .pop()
                .is_some_and(|entry| entry.sample.phase == PointerPhase::Begin);
        }
    }
    measure_push(
        &mut queue,
        sample(PointerPhase::End, move_count + 1),
        &mut all_push_costs,
        &mut coalesced_push_costs,
    );

    let drained: Vec<_> = std::iter::from_fn(|| queue.pop()).collect();
    let stats = queue.stats();
    let last = drained.last().map(|entry| entry.sample);
    let expected_last = sample(PointerPhase::End, move_count + 1);
    let transitions_preserved =
        begin_processed && last.is_some_and(|entry| entry.phase == PointerPhase::End);
    let latest_endpoint_preserved = last.is_some_and(|entry| {
        entry.sequence == expected_last.sequence
            && entry.position_document == expected_last.position_document
    });
    let passed = stats.max_len <= CAPACITY
        && stats.coalesced_moves > 0
        && stats.rejected_transitions == 0
        && transitions_preserved
        && latest_endpoint_preserved;

    let output = report_json(
        &destination,
        stats,
        &all_push_costs,
        &coalesced_push_costs,
        transitions_preserved,
        latest_endpoint_preserved,
        passed,
    );
    if let Some(parent) = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create JSON artifact directory");
    }
    fs::write(&destination, output).expect("write JSON artifact");
    println!("input queue stress artifact: {}", destination.display());
    assert!(
        passed,
        "bounded overload gate failed; inspect the JSON artifact"
    );
}

fn measure_push(
    queue: &mut InputQueue,
    sample: StylusSample,
    all_push_costs: &mut Vec<Duration>,
    coalesced_push_costs: &mut Vec<Duration>,
) {
    let before = queue.stats().coalesced_moves;
    let started = Instant::now();
    queue
        .push(sample)
        .expect("moves must be coalesced; transitions must fit");
    let elapsed = started.elapsed();
    all_push_costs.push(elapsed);
    if queue.stats().coalesced_moves != before {
        coalesced_push_costs.push(elapsed);
    }
}

fn sample(phase: PointerPhase, sequence: u64) -> StylusSample {
    let sequence_32 = u32::try_from(sequence).expect("fixture sequence fits u32");
    let t = f64::from(sequence_32) / f64::from(PRODUCER_HZ);
    let pressure_step = u16::try_from(sequence_32 % PRODUCER_HZ).expect("step fits u16");
    let pressure_divisor = u16::try_from(PRODUCER_HZ - 1).expect("rate fits u16");
    let x = ((t * 1_037.0).sin().mul_add(0.5, 0.5) * f64::from(CANVAS_WIDTH_PX - 1)).round();
    let y = ((t * 811.0).sin().mul_add(0.5, 0.5) * f64::from(CANVAS_HEIGHT_PX - 1)).round();
    StylusSample {
        sequence,
        timestamp_ns: sequence * (1_000_000_000 / u64::from(PRODUCER_HZ)),
        device_id: 1,
        phase,
        position_document: Point { x, y },
        pressure: f32::from(pressure_step) / f32::from(pressure_divisor),
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 8,
    }
}

fn report_json(
    destination: &Path,
    stats: InputQueueStats,
    all_push_costs: &[Duration],
    coalesced_push_costs: &[Duration],
    transitions_preserved: bool,
    latest_endpoint_preserved: bool,
    passed: bool,
) -> String {
    let cpu = env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "not-collected".into());
    let os_version = Command::new("cmd")
        .args(["/C", "ver"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(
            || env::consts::OS.to_owned(),
            |value| value.trim().to_owned(),
        );
    format!(
        concat!(
            "{{\n",
            "  \"schema_version\": 1,\n",
            "  \"fixture\": \"queue-overload-8k-large-round-brush\",\n",
            "  \"artifact_path\": \"{}\",\n",
            "  \"profile\": \"release\",\n",
            "  \"hardware\": {{\"cpu\": \"{}\", \"os\": \"{}\", \"backend\": \"not-applicable-queue-only\"}},\n",
            "  \"scenario\": {{\"canvas_px\": [{} , {}], \"brush_diameter_px\": {}, \"producer_hz\": {}, \"consumer_hz\": {}, \"virtual_duration_seconds\": {}, \"capacity\": {}}},\n",
            "  \"queue\": {{\"enqueued\": {}, \"coalesced_moves\": {}, \"rejected_transitions\": {}, \"max_len\": {}}},\n",
            "  \"push_cost_ns\": {},\n",
            "  \"coalesced_push_cost_ns\": {},\n",
            "  \"checks\": {{\"bounded_capacity\": {}, \"begin_end_preserved\": {}, \"latest_endpoint_preserved\": {}}},\n",
            "  \"pass\": {}\n",
            "}}\n"
        ),
        escape_json(&destination.display().to_string()),
        escape_json(&cpu),
        escape_json(&os_version),
        CANVAS_WIDTH_PX,
        CANVAS_HEIGHT_PX,
        BRUSH_DIAMETER_PX,
        PRODUCER_HZ,
        CONSUMER_HZ,
        DURATION_SECONDS,
        CAPACITY,
        stats.enqueued,
        stats.coalesced_moves,
        stats.rejected_transitions,
        stats.max_len,
        quantiles_json(all_push_costs),
        quantiles_json(coalesced_push_costs),
        stats.max_len <= CAPACITY,
        transitions_preserved,
        latest_endpoint_preserved,
        passed,
    )
}

fn quantiles_json(values: &[Duration]) -> String {
    let mut nanos: Vec<_> = values.iter().map(Duration::as_nanos).collect();
    nanos.sort_unstable();
    let point = |percent: usize| nanos[nanos.len().saturating_sub(1) * percent / 100];
    format!(
        "{{\"samples\": {}, \"p50\": {}, \"p95\": {}, \"p99\": {}, \"max\": {}}}",
        nanos.len(),
        point(50),
        point(95),
        point(99),
        nanos.last().copied().unwrap_or(0)
    )
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

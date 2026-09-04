//! Release measurement of branch history's root-pointer transition cost.
//!
//! The 4K pixel fixtures remain outside `History`; undo/redo changes only the
//! immutable `ContentRootId` cursor and does not clone either pixel buffer.

use std::{
    collections::BTreeMap,
    env, fs,
    hint::black_box,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

use nyatidraw_api::{ContentRootId, HistoryNodeId};
use nyatidraw_history::{History, HistoryNode, OperationRecord};

const WIDTH: u32 = 3_840;
const HEIGHT: u32 = 2_160;
const CHANNELS: usize = 4;
const ITERATIONS: usize = 20_000;
const P95_BUDGET_NS: u128 = 16_000_000;

fn main() {
    let destination = env::args_os().nth(1).map_or_else(
        || PathBuf::from("target/history-root-transition-4k.json"),
        PathBuf::from,
    );
    let byte_count = usize::try_from(WIDTH)
        .expect("4K width fits usize")
        .checked_mul(usize::try_from(HEIGHT).expect("4K height fits usize"))
        .and_then(|pixels| pixels.checked_mul(CHANNELS))
        .expect("4K RGBA8 fixture fits usize");
    let root_one = ContentRootId(0x10);
    let root_two = ContentRootId(0x20);
    let buffers: BTreeMap<_, Arc<[u8]>> = [
        (root_one, vec![0x11; byte_count].into()),
        (root_two, vec![0x22; byte_count].into()),
    ]
    .into_iter()
    .collect();
    let pointers_before: BTreeMap<_, _> = buffers
        .iter()
        .map(|(root, pixels)| (*root, Arc::as_ptr(pixels)))
        .collect();

    let mut history = History::new(ContentRootId(0));
    history
        .append(node(1, None, ContentRootId(0), root_one))
        .expect("first fixture root");
    history
        .append(node(2, Some(1), root_one, root_two))
        .expect("second fixture root");

    let mut undo_costs = Vec::with_capacity(ITERATIONS);
    let mut redo_costs = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let root = history.undo().expect("undo to first 4K root");
        undo_costs.push(started.elapsed());
        black_box(buffers.get(&root).expect("first pixels").as_ptr());

        let started = Instant::now();
        let root = history
            .redo_to(HistoryNodeId(2))
            .expect("redo to second 4K root");
        redo_costs.push(started.elapsed());
        black_box(buffers.get(&root).expect("second pixels").as_ptr());
    }

    let pointers_after: BTreeMap<_, _> = buffers
        .iter()
        .map(|(root, pixels)| (*root, Arc::as_ptr(pixels)))
        .collect();
    let pointer_identity_preserved = pointers_before == pointers_after;
    let undo = quantiles(&undo_costs);
    let redo = quantiles(&redo_costs);
    let release_profile = !cfg!(debug_assertions);
    let passed = release_profile
        && pointer_identity_preserved
        && history.current_root() == root_two
        && undo.p95 <= P95_BUDGET_NS
        && redo.p95 <= P95_BUDGET_NS;
    let report = report_json(
        &destination,
        byte_count,
        undo,
        redo,
        pointer_identity_preserved,
        release_profile,
        passed,
    );
    if let Some(parent) = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).expect("create artifact directory");
    }
    fs::write(&destination, report).expect("write history artifact");
    println!(
        "history root transition artifact: {}",
        destination.display()
    );
    assert!(passed, "history root transition gate failed");
}

fn node(
    id: u128,
    parent: Option<u128>,
    before_root: ContentRootId,
    after_root: ContentRootId,
) -> HistoryNode {
    HistoryNode {
        id: HistoryNodeId(id),
        parent: parent.map(HistoryNodeId),
        timestamp_ns: u64::try_from(id).unwrap_or(u64::MAX),
        operation: OperationRecord::StructuralChange,
        before_root,
        after_root,
    }
}

#[derive(Clone, Copy)]
struct Quantiles {
    samples: usize,
    p50: u128,
    p95: u128,
    p99: u128,
    max: u128,
}

fn quantiles(values: &[Duration]) -> Quantiles {
    let mut nanos: Vec<_> = values.iter().map(Duration::as_nanos).collect();
    nanos.sort_unstable();
    let point = |percent: usize| nanos[nanos.len().saturating_sub(1) * percent / 100];
    Quantiles {
        samples: nanos.len(),
        p50: point(50),
        p95: point(95),
        p99: point(99),
        max: nanos.last().copied().unwrap_or(0),
    }
}

#[allow(clippy::too_many_arguments)]
fn report_json(
    destination: &Path,
    byte_count: usize,
    undo: Quantiles,
    redo: Quantiles,
    pointer_identity_preserved: bool,
    release_profile: bool,
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
            "  \"fixture\": \"history-root-pointer-transition-4k\",\n",
            "  \"artifact_path\": \"{}\",\n",
            "  \"profile\": \"{}\",\n",
            "  \"hardware\": {{\"cpu\": \"{}\", \"os\": \"{}\", \"backend\": \"history-root-pointer-only\"}},\n",
            "  \"scenario\": {{\"canvas_px\": [{}, {}], \"rgba8_bytes_per_root\": {}, \"resident_roots\": 2, \"iterations\": {}}},\n",
            "  \"undo_cost_ns\": {},\n",
            "  \"redo_cost_ns\": {},\n",
            "  \"checks\": {{\"pointer_identity_preserved\": {}, \"pixel_bytes_copied_per_transition\": 0, \"current_root_restored\": true, \"p95_budget_ns\": {}}},\n",
            "  \"pass\": {}\n",
            "}}\n"
        ),
        escape_json(&destination.display().to_string()),
        if release_profile { "release" } else { "debug" },
        escape_json(&cpu),
        escape_json(&os_version),
        WIDTH,
        HEIGHT,
        byte_count,
        ITERATIONS,
        quantiles_json(undo),
        quantiles_json(redo),
        pointer_identity_preserved,
        P95_BUDGET_NS,
        passed,
    )
}

fn quantiles_json(value: Quantiles) -> String {
    format!(
        "{{\"samples\": {}, \"p50\": {}, \"p95\": {}, \"p99\": {}, \"max\": {}}}",
        value.samples, value.p50, value.p95, value.p99, value.max
    )
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#![forbid(unsafe_code)]

//! Scratch-only subprocess recovery evidence for the redb transaction boundary.
//!
//! Run with:
//! `cargo run -p nyatidraw-project-redb --features diagnostic --example crash_recovery_probe`

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use nyatidraw_api::{HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_brush::{
    BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, ROUND_BRUSH_ENGINE_VERSION,
    RoundBrushEvaluator, begin_round_stroke,
};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_history::HistoryNode;
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_project::ProjectCommitBatch;
use nyatidraw_project_redb::{DiagnosticCommitBoundary, DiagnosticCommitPause, ProjectDb};
use nyatidraw_stroke::StrokeColor;
use nyatidraw_tiles::TileSnapshot;

const STAGE_TIMEOUT: Duration = Duration::from_secs(10);

fn main() {
    if let Err(error) = run() {
        eprintln!("crash_recovery_probe: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let _program = args.next();
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--child")) {
        return run_child(args);
    }
    if args.next().is_some() {
        return Err(
            "usage: crash_recovery_probe [--child <project> <stage> <boundary>]".to_owned(),
        );
    }

    let scratch = scratch_directory()?;
    let result: Result<(), String> = (|| {
        let project = scratch.join("recovery.redb");
        let first = prepared_batch(
            SnapshotId(1),
            TileSnapshot::empty(),
            SnapshotId(2),
            HistoryNodeId(3),
            42,
            10,
        )?;
        let second = prepared_child_batch(&first, SnapshotId(3), HistoryNodeId(4), 84, 20)?;
        let database = ProjectDb::open(&project).map_err(debug_error("initialize scratch"))?;
        database
            .commit(&first)
            .map_err(debug_error("commit baseline"))?;
        drop(database);

        run_kill_case(
            &project,
            &scratch.join("before-commit.stage"),
            DiagnosticCommitBoundary::BeforeCommit,
            &first,
        )?;
        run_kill_case(
            &project,
            &scratch.join("after-durable-commit.stage"),
            DiagnosticCommitBoundary::AfterDurableCommit,
            &second,
        )?;
        println!(
            concat!(
                "{{\"event\":\"crash-recovery-probe\",\"status\":\"ok\",",
                "\"before_commit_snapshot\":{},\"after_durable_commit_snapshot\":{},",
                "\"fixture_root\":\"{:?}\"}}"
            ),
            first.snapshot_id.0,
            second.snapshot_id.0,
            second.materialized.after.root().hash,
        );
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(&scratch).map_err(|error| {
        format!(
            "remove owned scratch directory {}: {error}",
            scratch.display()
        )
    });
    result?;
    cleanup
}

fn run_child(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<(), String> {
    let project = required_path(&mut args, "project")?;
    let stage = required_path(&mut args, "stage")?;
    let boundary = match required_string(&mut args, "boundary")?.as_str() {
        "before-commit" => DiagnosticCommitBoundary::BeforeCommit,
        "after-durable-commit" => DiagnosticCommitBoundary::AfterDurableCommit,
        value => return Err(format!("unknown diagnostic boundary {value:?}")),
    };
    if args.next().is_some() {
        return Err("unexpected child arguments".to_owned());
    }
    let first = prepared_batch(
        SnapshotId(1),
        TileSnapshot::empty(),
        SnapshotId(2),
        HistoryNodeId(3),
        42,
        10,
    )?;
    let second = prepared_child_batch(&first, SnapshotId(3), HistoryNodeId(4), 84, 20)?;
    let database = ProjectDb::open(&project).map_err(debug_error("child open"))?;
    database
        .commit_pausing_for_diagnostics(
            &second,
            DiagnosticCommitPause {
                boundary,
                stage_path: stage,
            },
        )
        .map_err(debug_error("child diagnostic commit"))
}

fn run_kill_case(
    project: &Path,
    stage: &Path,
    boundary: DiagnosticCommitBoundary,
    expected: &ProjectCommitBatch,
) -> Result<(), String> {
    let mut child = Command::new(
        env::current_exe().map_err(|error| format!("current probe executable: {error}"))?,
    )
    .arg("--child")
    .arg(project)
    .arg(stage)
    .arg(boundary_name(boundary))
    .spawn()
    .map_err(|error| format!("spawn {boundary:?} child: {error}"))?;
    let pid = child.id();
    wait_for_stage(stage, pid, boundary, &mut child)?;
    child
        .kill()
        .map_err(|error| format!("kill staged child PID {pid}: {error}"))?;
    let status = child
        .wait()
        .map_err(|error| format!("wait for killed child PID {pid}: {error}"))?;
    if status.success() {
        return Err(format!(
            "staged child PID {pid} unexpectedly exited successfully"
        ));
    }
    let reopened = ProjectDb::open(project).map_err(debug_error("reopen after child kill"))?;
    let current = reopened
        .load_current()
        .map_err(debug_error("load after child kill"))?
        .ok_or_else(|| "reopen lost every durable snapshot".to_owned())?;
    if current.snapshot_id != expected.snapshot_id
        || current.materialized.after.root() != expected.materialized.after.root()
    {
        return Err(format!(
            "{boundary:?} kill reopened snapshot {} instead of durable snapshot {}",
            current.snapshot_id.0, expected.snapshot_id.0
        ));
    }
    let flattened = current
        .materialized
        .after
        .flatten_base_layer_rgba8(LayerId(4))
        .map_err(|error| format!("flatten reopened snapshot: {error:?}"))?;
    let expected_flattened = expected
        .materialized
        .after
        .flatten_base_layer_rgba8(LayerId(4))
        .map_err(|error| format!("flatten expected snapshot: {error:?}"))?;
    if flattened.hash() != expected_flattened.hash() {
        return Err(format!(
            "{boundary:?} kill changed flattened pixel semantics"
        ));
    }
    Ok(())
}

fn wait_for_stage(
    stage: &Path,
    child_pid: u32,
    boundary: DiagnosticCommitBoundary,
    child: &mut Child,
) -> Result<(), String> {
    let deadline = Instant::now() + STAGE_TIMEOUT;
    loop {
        if let Ok(contents) = fs::read_to_string(stage) {
            let expected = format!("pid={child_pid} boundary={}", boundary_name(boundary));
            if contents.trim() == expected {
                return Ok(());
            }
            return Err(format!(
                "stage file did not identify the spawned child exactly: {contents:?}"
            ));
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("poll child PID {child_pid}: {error}"))?
        {
            return Err(format!(
                "child PID {child_pid} exited before stage synchronization: {status}"
            ));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "timed out waiting for {boundary:?} stage from child PID {child_pid}"
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn prepared_batch(
    parent_snapshot: SnapshotId,
    before: TileSnapshot,
    snapshot_id: SnapshotId,
    history_id: HistoryNodeId,
    seed: u64,
    sequence: u64,
) -> Result<ProjectCommitBatch, String> {
    let preset = BrushPreset {
        id: BrushPresetId(17),
        schema_version: 1,
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        size_px: 18.0,
        opacity: 0.9,
        flow: 0.6,
        spacing_ratio: 0.2,
    };
    let samples = vec![
        sample(sequence, PointerPhase::Begin, -8.0, 5.0),
        sample(sequence + 1, PointerPhase::Move, 32.0, 18.0),
        sample(sequence + 2, PointerPhase::End, 150.0, 31.0),
    ];
    let mut evaluator = RoundBrushEvaluator::new(seed);
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
    evaluator.push(&mut token, &samples[1..], &mut dabs);
    let recorded = evaluator.end(token, &mut dabs);
    HeadlessStrokeSession::new(parent_snapshot, before)
        .prepare_round_stroke(
            snapshot_id,
            history_id,
            12_000,
            LayerId(4),
            BrushSnapshot { preset },
            recorded,
            StrokeColor::new([18, 9, 3, 24])
                .map_err(|error| format!("fixture colour: {error:?}"))?,
            samples,
        )
        .map_err(|error| format!("prepare closed stroke fixture: {error:?}"))
}

fn prepared_child_batch(
    parent: &ProjectCommitBatch,
    snapshot_id: SnapshotId,
    history_id: HistoryNodeId,
    seed: u64,
    sequence: u64,
) -> Result<ProjectCommitBatch, String> {
    let prepared = prepared_batch(
        parent.snapshot_id,
        parent.materialized.after.clone(),
        snapshot_id,
        history_id,
        seed,
        sequence,
    )?;
    let history_node = HistoryNode {
        parent: Some(parent.history_node.id),
        ..prepared.history_node.clone()
    };
    ProjectCommitBatch::new(
        prepared.snapshot_id,
        prepared.before.clone(),
        prepared.stroke.clone(),
        prepared.materialized.clone(),
        history_node,
    )
    .map_err(|error| format!("prepare child history fixture: {error:?}"))
}

fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence * 1_000,
        device_id: 9,
        phase,
        position_document: Point { x, y },
        pressure: 0.75,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 4,
    }
}

fn scratch_directory() -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read system time: {error}"))?
        .as_nanos();
    let path = env::temp_dir().join(format!(
        "nyatidraw-crash-probe-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path)
        .map_err(|error| format!("create scratch directory {}: {error}", path.display()))?;
    Ok(path)
}

fn boundary_name(boundary: DiagnosticCommitBoundary) -> &'static str {
    match boundary {
        DiagnosticCommitBoundary::BeforeCommit => "before-commit",
        DiagnosticCommitBoundary::AfterDurableCommit => "after-durable-commit",
    }
}

fn required_path(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}"))
}

fn required_string(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing {name}"))?
        .into_string()
        .map_err(|_| format!("{name} must be Unicode"))
}

fn debug_error(
    context: &'static str,
) -> impl FnOnce(nyatidraw_project::ProjectOpenError) -> String {
    move |error| format!("{context}: {error:?}")
}

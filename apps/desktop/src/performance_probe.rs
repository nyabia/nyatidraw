//! Explicitly marked scratch-only paced input. Never an OS/physical-input probe.
use crate::{
    live_ink::{ExportStatus, LiveInkBridge},
    performance_workload as workload,
};
use nyatidraw_api::{
    DrawingTool, EditorCommand, ProjectCommand, SnapshotId, ToolCommand, UiProjection,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

static USED: AtomicBool = AtomicBool::new(false);

pub(crate) struct PerformanceProbe {
    stop: Arc<AtomicBool>,
}

impl Drop for PerformanceProbe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl PerformanceProbe {
    pub(crate) fn start(ink: &LiveInkBridge, project: &Path) -> Option<Self> {
        let mode = std::env::var("NAYATI_PERFORMANCE_SCENARIO").ok()?;
        if !matches!(mode.as_str(), "baseline" | "export")
            || std::env::var("NAYATI_PERFORMANCE").as_deref() != Ok("1")
            || project.file_name()?.to_str()? != "performance-scratch.ntdr"
            || !project
                .parent()?
                .join(".nyatidraw-performance-scratch")
                .is_file()
            || ink.protocol_snapshot().0.canvas != workload::CANVAS
            || USED.swap(true, Ordering::AcqRel)
        {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let driver_stop = stop.clone();
        let ink = ink.clone();
        let start_marker = project.with_extension("performance-start");
        let launched = thread::Builder::new().name("nyatidraw-perf-input".into()).spawn(move || {
            let result = await_start(&ink, &driver_stop, &start_marker)
                .and_then(|()| run(&ink, &driver_stop, mode == "export"));
            match result {
                Ok(()) => println!("performance-probe event=complete mode={mode} strokes={} samples_per_stroke={} input=synthetic-paced physical_pen=false", workload::STROKES, workload::LAST_SAMPLE + 1),
                Err(error) => eprintln!("performance-probe event=stopped mode={mode} reason={error}"),
            }
        });
        if let Err(error) = launched {
            eprintln!("performance-probe event=start-failed error={error}");
            return None;
        }
        Some(Self { stop })
    }
}

fn await_start(ink: &LiveInkBridge, stop: &AtomicBool, marker: &Path) -> Result<(), String> {
    println!(
        "performance-probe event=awaiting-start marker={}",
        marker.display()
    );
    let started = Instant::now();
    loop {
        healthy(ink, stop)?;
        if marker.is_file() {
            return Ok(());
        }
        if started.elapsed() > Duration::from_mins(2) {
            return Err("start-marker-timeout".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn healthy(ink: &LiveInkBridge, stop: &AtomicBool) -> Result<(), String> {
    if stop.load(Ordering::Acquire) || ink.is_closing() || ink.workspace_failed() {
        Err("cancelled-or-workspace-failed".into())
    } else {
        Ok(())
    }
}

fn wait(ink: &LiveInkBridge, stop: &AtomicBool, ready: impl Fn() -> bool) -> Result<(), String> {
    let started = Instant::now();
    loop {
        healthy(ink, stop)?;
        if ready() {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(20) {
            return Err("condition-timeout".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn control(
    ink: &LiveInkBridge,
    stop: &AtomicBool,
    command: ToolCommand,
    ready: impl Fn(&UiProjection) -> bool,
) -> Result<(), String> {
    if ready(&ink.protocol_snapshot().0) {
        return Ok(());
    }
    ink.push_ui_editor_command(EditorCommand::Tool(command))
        .map_err(|e| format!("tool-admission:{e:?}"))?;
    wait(ink, stop, || ready(&ink.protocol_snapshot().0))
}

fn save(ink: &LiveInkBridge, stop: &AtomicBool, wait_for_start: bool) -> Result<(), String> {
    let old_generation = ink.export_status_snapshot().generation();
    ink.push_ui_editor_command(EditorCommand::Project(ProjectCommand::Save))
        .map_err(|e| format!("save-admission:{e:?}"))?;
    wait(ink, stop, || {
        let status = ink.export_status_snapshot();
        status.generation() > old_generation
            && if wait_for_start {
                matches!(status, ExportStatus::Running { .. })
            } else {
                matches!(status, ExportStatus::Current { .. })
            }
    })
}

fn run(ink: &LiveInkBridge, stop: &AtomicBool, exporting: bool) -> Result<(), String> {
    // Let the first frame's Fit/projection finish before configuring the fixed brush.
    thread::sleep(Duration::from_millis(250));
    healthy(ink, stop)?;
    let initial = ink.protocol_snapshot().0;
    if initial.history.entries.len() != 2 || !initial.history.redo_branches.is_empty() {
        return Err("requires-fresh-initial-fixture".into());
    }
    control(ink, stop, ToolCommand::Select(DrawingTool::Brush), |p| {
        p.drawing_tool == DrawingTool::Brush
    })?;
    control(ink, stop, ToolCommand::SetSizeTenths(640), |p| {
        p.brush_size_tenths == 640
    })?;
    control(ink, stop, ToolCommand::SetOpacityU16(u16::MAX), |p| {
        p.brush_opacity_u16 == u16::MAX
    })?;
    control(ink, stop, ToolCommand::SetColor(workload::COLOR), |p| {
        p.brush_color == workload::COLOR
    })?;
    println!(
        "performance-probe event=started canvas=3840x2160 brush={} rate_hz=240 strokes={} export={exporting} input=synthetic-paced",
        workload::BRUSH.size_px,
        workload::STROKES
    );
    for stroke in 0..workload::STROKES {
        healthy(ink, stop)?;
        if exporting {
            save(ink, stop, true)?;
        }
        let started = Instant::now();
        let mut max_lateness = 0_u128;
        for index in 0..=workload::LAST_SAMPLE {
            let due = started + Duration::from_nanos(u64::from(index) * workload::INTERVAL_NS);
            if let Some(delay) = due.checked_duration_since(Instant::now()) {
                thread::sleep(delay);
            }
            healthy(ink, stop)?;
            max_lateness =
                max_lateness.max(Instant::now().saturating_duration_since(due).as_micros());
            ink.push(workload::sample(stroke, index))
                .map_err(|e| format!("input-admission:{e:?}"))?;
        }
        let snapshot = SnapshotId(u128::from(stroke) + 2);
        wait(ink, stop, || {
            ink.protocol_snapshot().0.history.entries.len() == stroke as usize + 3
        })?;
        println!(
            "performance-probe event=stroke-complete stroke={stroke} max_schedule_lateness_us={max_lateness} expected_snapshot={}",
            snapshot.0
        );
    }
    save(ink, stop, false)?;
    Ok(())
}

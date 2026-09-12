//! Actual adapter/actor acceptance, excluded from ordinary headless test runs.
use super::*;
use nyatidraw_api::{AffineTransform, HistoryCommand, TransformCommand};
use std::time::Duration;

const VIEW: [u32; 2] = [640, 480];

fn settle(canvas: &mut ActiveCanvas) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        canvas.render(VIEW[0], VIEW[1], 1.0, 0);
        assert!(
            !canvas.stroke.live_ink.workspace_failed(),
            "actor entered fail-stop"
        );
        if canvas.artwork_job.is_none()
            && !canvas.stroke.transform_starting
            && canvas.stroke.pending_transform.is_none()
            && !canvas.stroke.pending_transform_cancel
            && canvas.stroke.pending_tool.is_none()
        {
            assert!(
                canvas.projection.current().edit.error.is_none(),
                "{:?}",
                canvas.projection.current().edit.error
            );
            return;
        }
        assert!(Instant::now() < deadline, "transform actor did not settle");
        thread::sleep(Duration::from_millis(2));
    }
}

fn dispatch(canvas: &mut ActiveCanvas, command: TransformCommand) {
    canvas.enqueue_free_transform(command).unwrap();
    settle(canvas);
}

fn cpu(snapshot: &TileSnapshot) -> BTreeMap<TileKey, Vec<u8>> {
    snapshot
        .iter()
        .map(|(key, tile)| (key, tile.pixels().to_vec()))
        .collect()
}

fn compare_gpu(canvas: &ActiveCanvas, expected: &TileSnapshot, checked: &BTreeSet<TileKey>) {
    for &key in checked {
        let actual = canvas.scene.readback_resident_raster_tile(key).unwrap();
        let expected = expected.get(key).map_or(
            &TRANSPARENT_HISTORY_TILE[..],
            nyatidraw_tiles::TileObject::pixels,
        );
        if let Some(actual) = actual {
            assert_eq!([actual.width, actual.height], [128; 2]);
            assert_eq!(
                actual.pixels, expected,
                "GPU differs from disposable snapshot at {key:?}"
            );
        } else {
            assert!(
                expected.iter().all(|&value| value == 0),
                "nonempty tile missing from GPU: {key:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires a real WGPU adapter; run explicitly with --ignored --exact --nocapture"]
#[allow(clippy::too_many_lines)]
fn actor_preview_cancel_commit_matches_gpu_and_durable_artwork() {
    // Product risk: an actor preview overwrites durable CPU pixels, leaves
    // ghost GPU tiles on Cancel, or allows unrelated edits into the draft.
    const REOPEN: &str = "NYATIDRAW_UX_TRANSFORM_REOPEN";
    const EXPECTED_ROOT: &str = "NYATIDRAW_UX_TRANSFORM_ROOT";
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = std::path::Path::new(&path);
        let db = ProjectDb::open(path).unwrap();
        let reopened = db.load_reopened().unwrap().unwrap();
        let tree = db.load_layer_tree().unwrap().unwrap();
        let canvas = db.load_canvas_spec().unwrap();
        assert_eq!(
            format!("{:?}", reopened.current_tiles().root()),
            std::env::var(EXPECTED_ROOT).unwrap()
        );
        assert_eq!(tree, default_layer_tree());
        assert_eq!(
            reopened.history().node_count(),
            3,
            "two tool transitions create exactly two history operations"
        );
        let page =
            nyatidraw_paint_cpu::flatten_layer_tree_rgba8(reopened.current_tiles(), &tree, canvas)
                .unwrap();
        let expected_png = nyatidraw_png_io::decode_png_bytes(
            &nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
            LIVE_LAYER,
            u64::from(canvas.width_px) * u64::from(canvas.height_px),
        )
        .unwrap();
        let saved_png =
            nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
        assert_eq!(
            saved_png.tiles, expected_png.tiles,
            "saved PNG survives process restart with identical artwork"
        );
        return;
    }
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("explicit acceptance requires an available GPU adapter");
    let info = adapter.get_info();
    println!(
        "transform-actor-gpu adapter={} backend={:?} driver={} profile={} physical-pen-proof=false present-proof=false",
        info.name,
        info.backend,
        info.driver,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nyatidraw-transform-actor-acceptance"),
        ..Default::default()
    }))
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-transform-gpu-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let location = ProjectLocation::Explicit {
        path: path.clone(),
        bootstrap_png: None,
    };
    let tree = default_layer_tree();
    let canvas_spec = CanvasSpec {
        width_px: 256,
        height_px: 256,
        ..CanvasSpec::default()
    };
    let mut tiles = BTreeMap::new();
    for (x, y, color) in [
        (-3_i32, -2_i32, [128, 0, 0, 128]),
        (-2, -2, [255, 0, 0, 255]),
        (9, 9, [0, 255, 0, 255]),
    ] {
        let key = TileKey {
            layer: LIVE_LAYER,
            mip: 0,
            x: x.div_euclid(128),
            y: y.div_euclid(128),
        };
        let tile = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
        let index = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
        tile[index..index + 4].copy_from_slice(&color);
    }
    let before = TileSnapshot::from_tiles(tiles).unwrap();
    {
        let (mut session, _, _, _, next_id, sink) =
            open_materialization_session(&location).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        db.persist_canvas_spec(canvas_spec).unwrap();
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
    }
    let ink = LiveInkBridge::with_capacity(64, LIVE_LAYER);
    let mut canvas = ActiveCanvas::new(&device, &queue, &ink, &location).unwrap();
    settle(&mut canvas);
    canvas
        .enqueue_edit(EditCommand::SelectLasso {
            vertices: vec![[-8, -8], [8, -8], [8, 8], [-8, 8]],
        })
        .unwrap();
    settle(&mut canvas);
    let selection = canvas.selection.clone().unwrap();
    let before_cpu = cpu(&before);
    assert_eq!(canvas.cpu_tiles, before_cpu);
    let mut checked: BTreeSet<_> = before.iter().map(|(key, _)| key).collect();
    let before_history = canvas.projection.current().history.clone();
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::Select(DrawingTool::Move)),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    for tool in [
        DrawingTool::MoveSelection,
        DrawingTool::Wand,
        DrawingTool::Lasso,
        DrawingTool::RectangleSelection,
        DrawingTool::Move,
    ] {
        canvas
            .apply_editor_command(
                EditorCommand::Tool(ToolCommand::CycleSelectionFamily),
                VIEW[0],
                VIEW[1],
                1.0,
            )
            .unwrap();
        assert_eq!(canvas.drawing.tool, tool);
        assert!(
            !canvas.transform_active(),
            "selecting a tool must not capture artwork"
        );
    }
    assert_eq!(canvas.cpu_tiles, before_cpu);
    assert_eq!(canvas.projection.current().history, before_history);
    // A switch out of an unchanged explicit draft is a discard with no new
    // history node. A cancelled asynchronous Begin cannot resurrect the draft.
    dispatch(&mut canvas, TransformCommand::Begin);
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::Select(DrawingTool::Brush)),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(&mut canvas);
    assert_eq!(canvas.drawing.tool, DrawingTool::Brush);
    assert_eq!(canvas.projection.current().history, before_history);
    canvas
        .enqueue_free_transform(TransformCommand::Begin)
        .unwrap();
    assert_eq!(canvas.drawing.tool, DrawingTool::MoveSelection);
    assert!(canvas.projection.current().edit.can_cancel);
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::CancelGesture),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    assert!(!canvas.projection.current().edit.can_cancel);
    settle(&mut canvas);
    assert!(!canvas.transform_active());
    assert_eq!(canvas.selection.as_deref(), Some(selection.as_ref()));
    assert_eq!(canvas.cpu_tiles, before_cpu);
    // One more independent Escape clears only the completed selection.
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::CancelGesture),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    assert!(
        !canvas.projection.current().edit.can_cancel,
        "selection worker is not cancellable"
    );
    settle(&mut canvas);
    assert!(canvas.selection.is_none());
    assert_eq!(canvas.cpu_tiles, before_cpu);
    canvas
        .enqueue_edit(EditCommand::SelectLasso {
            vertices: vec![[-8, -8], [8, -8], [8, 8], [-8, 8]],
        })
        .unwrap();
    settle(&mut canvas);
    dispatch(&mut canvas, TransformCommand::Begin);
    for command in [
        EditorCommand::Project(ProjectCommand::Save),
        EditorCommand::History(HistoryCommand::Undo),
    ] {
        assert!(
            canvas
                .apply_editor_command(command, VIEW[0], VIEW[1], 1.0)
                .is_err(),
            "active draft admitted an unrelated mutation"
        );
    }
    assert!(
        ink.queue_save_as(path.with_file_name("must-not-create.ntdr"))
            .is_err()
    );
    dispatch(&mut canvas, TransformCommand::Cancel);
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::Select(DrawingTool::MoveSelection)),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    assert!(!canvas.transform_active());
    // Feed native document samples, not a semantic UI command. Drag release
    // leaves the transform provisional; it must not create history/artwork.
    let revision = ink.canvas_viewport_snapshot().revision;
    for (sequence, phase, position) in [
        (100, PointerPhase::Begin, [0.0, 0.0]),
        (101, PointerPhase::Move, [2.0, 1.0]),
        (102, PointerPhase::End, [3.0, 2.0]),
    ] {
        let mut sample = input_probe_sample(sequence, phase);
        sample.position_document = Point {
            x: position[0],
            y: position[1],
        };
        sample.viewport_revision = revision;
        ink.push(sample).unwrap();
        if phase == PointerPhase::Begin {
            canvas.render(VIEW[0], VIEW[1], 1.0, 0);
            assert!(canvas.stroke.transform_starting);
            assert!(canvas.stroke.transform_gesture.is_some());
            assert!(canvas.projection.current().edit.can_cancel);
        }
    }
    settle(&mut canvas);
    assert_eq!(
        canvas
            .stroke
            .transform_projection
            .as_ref()
            .unwrap()
            .transform
            .offset_milli,
        [3000, 2000]
    );
    assert_eq!(canvas.cpu_tiles, before_cpu);
    // A preview already executing must be discarded before its cancellation
    // response, while preserving the source selection and durable artwork.
    canvas
        .enqueue_free_transform(TransformCommand::Preview(AffineTransform {
            offset_milli: [9000, 8000],
            ..Default::default()
        }))
        .unwrap();
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::CancelGesture),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(&mut canvas);
    assert!(!canvas.transform_active());
    assert_eq!(canvas.selection.as_deref(), Some(selection.as_ref()));
    assert_eq!(canvas.cpu_tiles, before_cpu);
    dispatch(&mut canvas, TransformCommand::Begin);
    let chosen = AffineTransform {
        offset_milli: [132_000, 7000],
        scale_ppm: [1_500_000, 750_000],
        rotation_millidegrees: 27_000,
        ..Default::default()
    };
    dispatch(&mut canvas, TransformCommand::Preview(chosen));
    let preview = canvas.transform_display.as_ref().unwrap().clone();
    checked.extend(preview.iter().map(|(key, _)| key));
    assert_ne!(preview, before);
    assert_eq!(
        canvas.cpu_tiles, before_cpu,
        "preview overwrote durable CPU cache"
    );
    compare_gpu(&canvas, &preview, &checked);
    dispatch(&mut canvas, TransformCommand::Cancel);
    assert_eq!(canvas.cpu_tiles, before_cpu);
    assert_eq!(canvas.selection.as_deref(), Some(selection.as_ref()));
    assert!(canvas.transform_display.is_none());
    compare_gpu(&canvas, &before, &checked);
    dispatch(&mut canvas, TransformCommand::Begin);
    canvas
        .enqueue_free_transform(TransformCommand::Preview(AffineTransform {
            offset_milli: [4000, 6000],
            ..Default::default()
        }))
        .unwrap();
    canvas.stroke.pending_transform = Some(chosen);
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::CycleBrushFamily),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    assert_eq!(
        canvas.drawing.tool,
        DrawingTool::MoveSelection,
        "tool switched before artwork adoption"
    );
    // Repeated family intent uses one latest target, without queuing commits.
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::CycleBrushFamily),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    assert_eq!(canvas.stroke.pending_tool, Some(DrawingTool::Pen));
    settle(&mut canvas);
    assert_eq!(canvas.drawing.tool, DrawingTool::Pen);
    assert_eq!(
        canvas.projection.current().history.entries.len(),
        before_history.entries.len() + 1
    );
    assert_eq!(canvas.cpu_tiles, cpu(&preview));
    assert!(canvas.stroke.transform_projection.is_none());
    assert!(
        canvas
            .selection
            .as_ref()
            .is_some_and(|mask| mask.selected_pixels() > 0)
    );
    compare_gpu(&canvas, &preview, &checked);
    // Revalidate the last valid display after a failed candidate before a tool
    // transition. The failure must never commit a stale invalid generation.
    dispatch(&mut canvas, TransformCommand::Begin);
    dispatch(
        &mut canvas,
        TransformCommand::Preview(AffineTransform {
            offset_milli: [1000, 0],
            ..Default::default()
        }),
    );
    let valid = canvas.transform_display.as_ref().unwrap().clone();
    canvas
        .enqueue_free_transform(TransformCommand::Preview(AffineTransform {
            scale_ppm: [0, 1_000_000],
            ..Default::default()
        }))
        .unwrap();
    canvas
        .apply_editor_command(
            EditorCommand::Tool(ToolCommand::Select(DrawingTool::Brush)),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(&mut canvas);
    assert_eq!(canvas.cpu_tiles, cpu(&valid));
    assert_eq!(canvas.drawing.tool, DrawingTool::Brush);
    canvas
        .apply_editor_command(
            EditorCommand::History(HistoryCommand::Undo),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(&mut canvas);
    assert_eq!(canvas.cpu_tiles, cpu(&preview));
    canvas
        .apply_editor_command(
            EditorCommand::Project(ProjectCommand::Save),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    // Real worker shutdown drains the explicitly requested project PNG save.
    drop(canvas);
    assert!(!ink.workspace_failed());
    assert!(matches!(
        ink.export_status_snapshot(),
        ExportStatus::Current { .. }
    ));
    let (_, reopened, reopened_tree, reopened_canvas, _, sink) =
        open_materialization_session(&location).unwrap();
    assert_eq!(reopened, preview);
    assert_eq!(reopened_tree, tree);
    assert_eq!(reopened_canvas, canvas_spec);
    drop(sink);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "native_canvas::transform_gpu_acceptance::actor_preview_cancel_commit_matches_gpu_and_durable_artwork", "--nocapture"])
        .env(REOPEN, &path)
        .env(EXPECTED_ROOT, format!("{:?}", preview.root()))
        .status().unwrap();
    assert!(status.success());
    for file in [&path, &path.with_extension("png")] {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
}

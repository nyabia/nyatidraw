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
            && canvas.stroke.pending_transform.is_none()
            && !canvas.stroke.pending_transform_cancel
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
    dispatch(&mut canvas, TransformCommand::Begin);
    for command in [
        EditorCommand::Project(ProjectCommand::Save),
        EditorCommand::History(HistoryCommand::Undo),
        EditorCommand::Tool(ToolCommand::Select(DrawingTool::Brush)),
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
    dispatch(&mut canvas, TransformCommand::Preview(chosen));
    let generation = canvas
        .stroke
        .transform_projection
        .as_ref()
        .unwrap()
        .generation;
    dispatch(&mut canvas, TransformCommand::Commit { generation });
    assert_eq!(canvas.cpu_tiles, cpu(&preview));
    assert!(canvas.stroke.transform_projection.is_none());
    assert!(
        canvas
            .selection
            .as_ref()
            .is_some_and(|mask| mask.selected_pixels() > 0)
    );
    compare_gpu(&canvas, &preview, &checked);
    drop(canvas); // Real worker shutdown drains storage and automatic PNG export.
    assert!(!ink.workspace_failed());
    let (_, reopened, reopened_tree, reopened_canvas, _, sink) =
        open_materialization_session(&location).unwrap();
    assert_eq!(reopened, preview);
    assert_eq!(reopened_tree, tree);
    assert_eq!(reopened_canvas, canvas_spec);
    drop(sink);
    for file in [&path, &path.with_extension("png")] {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
}

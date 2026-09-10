//! Explicit actor/GPU artwork evidence; no HWND, physical-pen or latency claim.
use super::*;
use nyatidraw_api::{HistoryCommand, LayerBlendMode, LayerTreeNodeId, ViewportCommand};
use std::time::Duration;

const SHADE: LayerId = LayerId(90);
const VIEW: [u32; 2] = [640, 480];
const PAGE: CanvasSpec = CanvasSpec {
    width_px: 128,
    height_px: 128,
    pixels_per_inch: 96,
};
const REOPEN: &str = "NYATIDRAW_SHADING_ACTOR_REOPEN";
const ROOT_HASH: &str = "NYATIDRAW_SHADING_ACTOR_ROOT";

fn fixture() -> (LayerTree, TileSnapshot) {
    let mut tree = default_layer_tree();
    let mut shade = tree.raster(LIVE_LAYER).unwrap().clone();
    shade.id = SHADE;
    shade.name = "Shading".into();
    tree.insert(tree.root_id(), 1, LayerTreeNode::Raster(shade))
        .unwrap();
    let mut tiles = BTreeMap::new();
    for layer in [LIVE_LAYER, SHADE] {
        for y in 4_i32..48 {
            for x in -20_i32..60 {
                if layer == LIVE_LAYER && (!(-12..40).contains(&x) || !(8..44).contains(&y)) {
                    continue;
                }
                let alpha = if x < 0 { 128 } else { 255 };
                let color = if layer == LIVE_LAYER {
                    [alpha, alpha / 2, alpha / 4, alpha]
                } else {
                    [0, 80, 0, 128]
                };
                let key = TileKey {
                    layer,
                    mip: 0,
                    x: x.div_euclid(128),
                    y: y.div_euclid(128),
                };
                let tile = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
                let offset =
                    usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
                tile[offset..offset + 4].copy_from_slice(&color);
            }
        }
    }
    (tree, TileSnapshot::from_tiles(tiles).unwrap())
}

fn settle(canvas: &mut ActiveCanvas) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut quiet = 0;
    loop {
        canvas.render(VIEW[0], VIEW[1], 1.0, 0);
        assert!(
            !canvas.stroke.live_ink.workspace_failed(),
            "shading actor fail-stop"
        );
        if canvas.artwork_job.is_none()
            && canvas.pending_save.is_none()
            && canvas.stroke.active.is_none()
            && canvas.stroke.materializer.pending.load(Ordering::Acquire) == 0
            && !matches!(
                canvas.stroke.live_ink.export_status_snapshot(),
                ExportStatus::Queued { .. } | ExportStatus::Running { .. }
            )
        {
            quiet += 1;
            if quiet == 3 {
                return;
            }
        } else {
            quiet = 0;
        }
        assert!(Instant::now() < deadline, "shading actor did not settle");
        thread::sleep(Duration::from_millis(2));
    }
}

fn command(canvas: &mut ActiveCanvas, command: EditorCommand) {
    canvas
        .apply_editor_command(command, VIEW[0], VIEW[1], 1.0)
        .unwrap();
    settle(canvas);
}

fn snapshot(canvas: &ActiveCanvas) -> TileSnapshot {
    TileSnapshot::from_tiles(canvas.cpu_tiles.clone()).unwrap()
}

fn assert_alpha_and_other_layer(before: &TileSnapshot, after: &TileSnapshot) {
    let mut changed = false;
    for (key, old) in before.iter() {
        let new = after.get(key).unwrap();
        if key.layer == SHADE {
            assert_eq!(
                old.pixels(),
                new.pixels(),
                "recoloring changed separate shading artwork"
            );
        } else {
            for (a, b) in old
                .pixels()
                .chunks_exact(4)
                .zip(new.pixels().chunks_exact(4))
            {
                assert_eq!(a[3], b[3], "native alpha-locked stroke changed silhouette");
                assert!(b[..3].iter().all(|channel| *channel <= b[3]));
                changed |= a != b;
            }
        }
    }
    assert!(changed, "native stroke must actually recolor base artwork");
    assert_eq!(
        before.len(),
        after.len(),
        "recoloring created off-artwork tiles"
    );
}

fn compare_gpu(canvas: &ActiveCanvas, expected: &TileSnapshot) {
    for (key, tile) in expected.iter() {
        let actual = canvas
            .scene
            .readback_resident_raster_tile(key)
            .unwrap()
            .unwrap();
        assert_eq!(
            actual.pixels,
            tile.pixels(),
            "closed CPU/GPU raster adoption mismatch at {key:?}"
        );
    }
    for point @ [x, y] in [[-6_i32, 20_i32], [12, 20], [30, 35], [52, 20]] {
        let expected =
            nyatidraw_paint_cpu::sample_display_pixel(expected, canvas.scene.tree(), point, None)
                .unwrap();
        let actual = canvas
            .scene
            .readback_resident_group_tile(
                canvas.scene.tree().root_id(),
                nyatidraw_api::TileCoordinate {
                    mip: 0,
                    x: x.div_euclid(128),
                    y: y.div_euclid(128),
                },
            )
            .unwrap()
            .unwrap();
        let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
        assert_eq!(
            &actual.pixels[offset..offset + 4],
            &expected,
            "raw clipping/Multiply group cache mismatch at {point:?}"
        );
    }
}

fn sample(sequence: u64, phase: PointerPhase, x: f64, eraser: bool, revision: u64) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence * 1_000_000,
        device_id: 71,
        phase,
        position_document: Point { x, y: 20.0 },
        pressure: 1.0,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: nyatidraw_input::PenButtons(0),
        eraser,
        viewport_revision: revision,
    }
}

fn check_png(path: &std::path::Path, pixels: &TileSnapshot, tree: &LayerTree) {
    let flat = flatten_layer_tree_rgba8(pixels, tree, PAGE).unwrap();
    let saved = nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
    // File export is sRGB16 and must preserve the linear RGBA8 artwork.
    // encode_png_bytes is an sRGB8 disposable preview, not an exact oracle.
    let decoded = flatten_layer_tree_rgba8(&saved.tiles, &default_layer_tree(), PAGE).unwrap();
    assert_eq!(saved.canvas, PAGE);
    if let Some((index, (actual, wanted))) = decoded
        .pixels
        .chunks_exact(4)
        .zip(flat.pixels.chunks_exact(4))
        .enumerate()
        .find(|(_, (a, b))| a != b)
    {
        panic!("PNG mismatch pixel={index} actual={actual:?} expected={wanted:?}");
    }
    assert!(saved.tiles.iter().all(|(key, _)| key.x == 0 && key.y == 0));
}

#[test]
#[ignore = "requires a real WGPU adapter; run explicitly with --ignored --exact --nocapture"]
#[allow(clippy::too_many_lines)]
fn actor_shading_alpha_stroke_history_and_fresh_reopen_match() {
    // Product risk: metadata controls are decorative, alpha lock is lost between
    // native Begin and writer, or clipping/Multiply diverges after Undo/reopen.
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .unwrap();
    let info = adapter.get_info();
    println!(
        "shading-actor adapter={} backend={:?} driver={} profile={} physical-pen-proof=false present-proof=false",
        info.name,
        info.backend,
        info.driver,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let (tree, before) = fixture();
    let mut expected_tree = tree.clone();
    expected_tree.set_alpha_locked(LIVE_LAYER, true).unwrap();
    expected_tree
        .set_clip_to_below(LayerTreeNodeId::Raster(SHADE), true)
        .unwrap();
    expected_tree
        .set_blend_mode(LayerTreeNodeId::Raster(SHADE), LayerBlendMode::Multiply)
        .unwrap();
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = PathBuf::from(path);
        let location = ProjectLocation::from_positional_path(path.with_extension("png"))
            .unwrap()
            .unwrap();
        assert!(
            matches!(
                &location,
                ProjectLocation::Explicit { path: resolved, bootstrap_png: None }
                    if std::fs::canonicalize(resolved).unwrap() == std::fs::canonicalize(&path).unwrap()
            ),
            "paired PNG activation must reopen its layered project, not bootstrap a flat image"
        );
        {
            let (_, pixels, tree, canvas, _, sink) =
                open_materialization_session(&location).unwrap();
            assert_eq!(
                format!("{:?}", pixels.root()),
                std::env::var(ROOT_HASH).unwrap()
            );
            assert_eq!(tree, expected_tree);
            assert_eq!(canvas, PAGE);
            assert_alpha_and_other_layer(&before, &pixels);
            check_png(&path, &pixels, &tree);
            let db = match &sink {
                ProjectSink::UntitledRecovery { db, .. }
                | ProjectSink::ExplicitProject { db, .. } => db,
            };
            assert!(
                db.load_current().unwrap().unwrap().stroke.alpha_locked(),
                "Begin flag missing from durable semantic stroke"
            );
        }
        let ink = LiveInkBridge::with_capacity(64, LIVE_LAYER);
        let mut canvas = ActiveCanvas::new(&device, &queue, &ink, &location).unwrap();
        settle(&mut canvas);
        command(
            &mut canvas,
            EditorCommand::Viewport(ViewportCommand::ActualPixels),
        );
        let pixels = snapshot(&canvas);
        assert_eq!(
            format!("{:?}", pixels.root()),
            std::env::var(ROOT_HASH).unwrap()
        );
        assert_eq!(canvas.scene.tree(), &expected_tree);
        compare_gpu(&canvas, &pixels);
        // A paired-image activation must leave the restored layered document
        // editable, not only render a flattened preview successfully.
        for tool in [
            ToolCommand::Select(DrawingTool::Pen),
            ToolCommand::SetSizeTenths(50),
            ToolCommand::SetOpacityU16(u16::MAX),
            ToolCommand::SetSizePressure(false),
            ToolCommand::SetOpacityPressure(false),
            ToolCommand::SetHardnessU16(u16::MAX),
            ToolCommand::SetSmoothing(0),
            ToolCommand::SetColor([30, 60, 240, 255]),
        ] {
            command(&mut canvas, EditorCommand::Tool(tool));
        }
        let revision = ink.canvas_viewport_snapshot().revision;
        ink.push(sample(1_000, PointerPhase::Begin, -6.0, false, revision))
            .unwrap();
        canvas.render(VIEW[0], VIEW[1], 1.0, 0);
        assert!(canvas.stroke.active.as_ref().unwrap().alpha_locked);
        ink.push(sample(1_001, PointerPhase::Move, 12.0, false, revision))
            .unwrap();
        ink.push(sample(1_002, PointerPhase::End, 18.0, false, revision))
            .unwrap();
        settle(&mut canvas);
        assert!(canvas.projection.current().edit.error.is_none());
        let reedited = snapshot(&canvas);
        assert_ne!(reedited.root(), pixels.root());
        assert_alpha_and_other_layer(&pixels, &reedited);
        compare_gpu(&canvas, &reedited);
        command(&mut canvas, EditorCommand::History(HistoryCommand::Undo));
        assert_eq!(snapshot(&canvas), pixels);
        command(&mut canvas, EditorCommand::History(HistoryCommand::Redo));
        assert_eq!(snapshot(&canvas), reedited);
        command(&mut canvas, EditorCommand::Project(ProjectCommand::Save));
        drop(canvas);
        assert!(!ink.workspace_failed());
        let (_, persisted, restored_tree, _, _, _sink) =
            open_materialization_session(&location).unwrap();
        assert_eq!(persisted, reedited);
        assert_eq!(restored_tree, expected_tree);
        check_png(&path, &persisted, &restored_tree);
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-shading-gpu-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let location = ProjectLocation::Explicit {
        path: path.clone(),
        bootstrap_png: None,
    };
    {
        let (mut session, _, _, _, next_id, sink) =
            open_materialization_session(&location).unwrap();
        let db = match &sink {
            ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => {
                db
            }
        };
        db.persist_canvas_spec(PAGE).unwrap();
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
    command(
        &mut canvas,
        EditorCommand::Viewport(ViewportCommand::ActualPixels),
    );
    command(
        &mut canvas,
        EditorCommand::Layer(LayerCommand::SetAlphaLocked {
            layer: LIVE_LAYER,
            alpha_locked: true,
        }),
    );
    command(
        &mut canvas,
        EditorCommand::Layer(LayerCommand::SetClipToBelow {
            node: LayerTreeNodeId::Raster(SHADE),
            clip_to_below: true,
        }),
    );
    assert_eq!(
        nyatidraw_paint_cpu::sample_display_pixel(&before, canvas.scene.tree(), [52, 20], None)
            .unwrap(),
        [0; 4]
    );
    let normal =
        nyatidraw_paint_cpu::sample_display_pixel(&before, canvas.scene.tree(), [30, 35], None)
            .unwrap();
    compare_gpu(&canvas, &before);
    command(
        &mut canvas,
        EditorCommand::Layer(LayerCommand::SetBlendMode {
            node: LayerTreeNodeId::Raster(SHADE),
            blend_mode: LayerBlendMode::Multiply,
        }),
    );
    let multiply =
        nyatidraw_paint_cpu::sample_display_pixel(&before, canvas.scene.tree(), [30, 35], None)
            .unwrap();
    assert_ne!(
        normal, multiply,
        "blend mode command must change composition"
    );
    assert_eq!(canvas.scene.tree(), &expected_tree);
    compare_gpu(&canvas, &before);
    for tool in [
        ToolCommand::Select(DrawingTool::Eraser),
        ToolCommand::SetSizeTenths(120),
        ToolCommand::Select(DrawingTool::Pen),
        ToolCommand::SetSizeTenths(70),
        ToolCommand::SetSizeTenths(50),
        ToolCommand::SetOpacityU16(u16::MAX),
        ToolCommand::SetSizePressure(false),
        ToolCommand::SetOpacityPressure(false),
        ToolCommand::SetHardnessU16(u16::MAX),
        ToolCommand::SetSmoothing(0),
        ToolCommand::SetColor([240, 40, 60, 255]),
    ] {
        command(&mut canvas, EditorCommand::Tool(tool));
    }
    assert!(
        canvas.projection.current().recent_brush_sizes.is_empty(),
        "settings and tool changes must not masquerade as drawing usage"
    );
    let history_before_begin = canvas.projection.current().history.clone();
    let revision = ink.canvas_viewport_snapshot().revision;
    ink.push(sample(100, PointerPhase::Begin, -6.0, false, revision))
        .unwrap();
    canvas.render(VIEW[0], VIEW[1], 1.0, 0);
    assert_eq!(canvas.projection.current().recent_brush_sizes, [50]);
    assert_eq!(
        canvas.projection.current().history,
        history_before_begin,
        "recent size publication must not add artwork history"
    );
    assert!(
        canvas.stroke.active.as_ref().unwrap().alpha_locked,
        "alpha lock must be captured at native Begin"
    );
    assert!(
        canvas
            .apply_editor_command(
                EditorCommand::Layer(LayerCommand::SetAlphaLocked {
                    layer: LIVE_LAYER,
                    alpha_locked: false
                }),
                VIEW[0],
                VIEW[1],
                1.0
            )
            .is_err(),
        "layer mutation must not reinterpret an active stroke"
    );
    ink.push(sample(101, PointerPhase::Move, 12.0, false, revision))
        .unwrap();
    ink.push(sample(102, PointerPhase::End, 18.0, false, revision))
        .unwrap();
    settle(&mut canvas);
    assert!(canvas.projection.current().edit.error.is_none());
    let after = snapshot(&canvas);
    assert_alpha_and_other_layer(&before, &after);
    compare_gpu(&canvas, &after);
    for (operation, expected) in [
        (HistoryCommand::Undo, &before),
        (HistoryCommand::Redo, &after),
    ] {
        command(&mut canvas, EditorCommand::History(operation));
        assert_eq!(snapshot(&canvas), *expected);
        assert_eq!(canvas.scene.tree(), &expected_tree);
        assert_eq!(
            canvas.projection.current().recent_brush_sizes,
            [50],
            "artwork Undo/Redo must not rewind session usage"
        );
        compare_gpu(&canvas, expected);
    }
    let picked =
        nyatidraw_paint_cpu::sample_display_pixel(&after, &expected_tree, [30, 35], None).unwrap();
    let mut expected_color = nyatidraw_tiles::color::linear_premultiplied_to_srgb8(picked).unwrap();
    expected_color[3] = 255;
    command(
        &mut canvas,
        EditorCommand::Edit(EditCommand::PickDisplayColor {
            point: [30, 35],
            solo: None,
            input_sequence: (71, 300),
        }),
    );
    assert_eq!(
        canvas.drawing.color, expected_color,
        "actor picker must sample Multiply result"
    );
    let history = canvas.projection.current().history.clone();
    ink.push(sample(500, PointerPhase::Begin, 12.0, true, revision))
        .unwrap();
    ink.push(sample(501, PointerPhase::End, 18.0, true, revision))
        .unwrap();
    settle(&mut canvas);
    assert!(
        canvas.projection.current().edit.error.is_some(),
        "alpha-locked eraser must report explicit rejection"
    );
    assert_eq!(snapshot(&canvas), after);
    assert_eq!(canvas.projection.current().history, history);
    assert_eq!(
        canvas.projection.current().recent_brush_sizes,
        [50],
        "picker and rejected hardware eraser must not record usage"
    );
    for (index, (size, expected)) in [
        (70, vec![70, 50]),
        (120, vec![120, 70, 50]),
        (30, vec![30, 120, 70, 50]),
        (80, vec![80, 30, 120, 70]),
        (120, vec![120, 80, 30, 70]),
    ]
    .into_iter()
    .enumerate()
    {
        command(
            &mut canvas,
            EditorCommand::Tool(ToolCommand::SetSizeTenths(size)),
        );
        let sequence = 600 + u64::try_from(index).unwrap() * 2;
        ink.push(sample(sequence, PointerPhase::Begin, 12.0, false, revision))
            .unwrap();
        canvas.render(VIEW[0], VIEW[1], 1.0, 0);
        ink.push(sample(
            sequence + 1,
            PointerPhase::Cancel,
            12.0,
            false,
            revision,
        ))
        .unwrap();
        settle(&mut canvas);
        assert_eq!(canvas.projection.current().recent_brush_sizes, expected);
        assert_eq!(canvas.projection.current().history, history);
        assert_eq!(
            snapshot(&canvas),
            after,
            "usage survives Cancel but artwork must roll back"
        );
    }
    compare_gpu(&canvas, &after);
    command(&mut canvas, EditorCommand::Project(ProjectCommand::Save));
    drop(canvas);
    assert!(!ink.workspace_failed());
    check_png(&path, &after, &expected_tree);
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_canvas::shading_gpu_acceptance::actor_shading_alpha_stroke_history_and_fresh_reopen_match", "--ignored", "--nocapture"])
        .env(REOPEN, &path).env(ROOT_HASH, format!("{:?}", after.root())).status().unwrap().success());
    for path in [&path, &path.with_extension("png")] {
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
}

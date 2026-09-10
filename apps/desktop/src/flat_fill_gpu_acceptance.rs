//! Real worker/actor/GPU adoption evidence, not HWND or physical-pen evidence.
use super::*;
use nyatidraw_api::{EditSource, FillSettings, HistoryCommand, ViewportCommand};
use std::time::Duration;

const VIEW: [u32; 2] = [640, 480];
const LINE: LayerId = LayerId(91);
const COLOR: [u8; 4] = [255, 0, 0, 255];
const PAGE: CanvasSpec = CanvasSpec {
    width_px: 32,
    height_px: 32,
    pixels_per_inch: 96,
};

fn settle(canvas: &mut ActiveCanvas) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        canvas.render(VIEW[0], VIEW[1], 1.0, 0);
        assert!(
            !canvas.stroke.live_ink.workspace_failed(),
            "fill actor entered fail-stop"
        );
        if canvas.artwork_job.is_none()
            && canvas.pending_save.is_none()
            && canvas.stroke.materializer.pending.load(Ordering::Acquire) == 0
            && !matches!(
                canvas.stroke.live_ink.export_status_snapshot(),
                ExportStatus::Queued { .. } | ExportStatus::Running { .. }
            )
        {
            assert!(
                canvas.projection.current().edit.error.is_none(),
                "{:?}",
                canvas.projection.current().edit.error
            );
            return;
        }
        assert!(Instant::now() < deadline, "fill actor did not settle");
        thread::sleep(Duration::from_millis(2));
    }
}

fn fixture() -> (LayerTree, TileSnapshot) {
    let mut tree = default_layer_tree();
    let mut line = tree.raster(LIVE_LAYER).unwrap().clone();
    line.id = LINE;
    line.name = "Reference line".into();
    line.reference = true;
    tree.insert(tree.root_id(), 1, LayerTreeNode::Raster(line))
        .unwrap();
    let mut tiles = BTreeMap::new();
    let mut put = |x: i32, y: i32, color: [u8; 4]| {
        let key = TileKey {
            layer: LINE,
            mip: 0,
            x: x.div_euclid(128),
            y: y.div_euclid(128),
        };
        let tile = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
        let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
        tile[offset..offset + 4].copy_from_slice(&color);
    };
    for y in 3..=20 {
        for x in 3..=20 {
            let outer = x == 3 || x == 20 || y == 3 || y == 20;
            let inner = x == 4 || x == 19 || y == 4 || y == 19;
            if (outer || inner) && !(y == 10 && x >= 19) {
                // Cross the 32px page edge inside the same 128px tile: both
                // page and sparse group textures own coordinate (0, 0).
                put(x + 21, y, [0, 0, 0, if outer { 255 } else { 128 }]);
            }
        }
    }
    put(-3, 5, [0, 128, 0, 128]);
    (tree, TileSnapshot::from_tiles(tiles).unwrap())
}

fn pixels(snapshot: &TileSnapshot) -> BTreeMap<TileKey, Vec<u8>> {
    snapshot
        .iter()
        .map(|(key, tile)| (key, tile.pixels().to_vec()))
        .collect()
}

fn pixel(snapshot: &TileSnapshot, layer: LayerId, [x, y]: [i32; 2]) -> [u8; 4] {
    let key = TileKey {
        layer,
        mip: 0,
        x: x.div_euclid(128),
        y: y.div_euclid(128),
    };
    let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
    snapshot.get(key).map_or([0; 4], |tile| {
        tile.pixels()[offset..offset + 4].try_into().unwrap()
    })
}

fn compare_gpu(canvas: &ActiveCanvas, snapshot: &TileSnapshot, checked: &BTreeSet<TileKey>) {
    for &key in checked {
        let expected = snapshot.get(key).map_or(
            &TRANSPARENT_HISTORY_TILE[..],
            nyatidraw_tiles::TileObject::pixels,
        );
        if let Some(actual) = canvas.scene.readback_resident_raster_tile(key).unwrap() {
            assert_eq!([actual.width, actual.height], [128; 2]);
            assert_eq!(
                actual.pixels, expected,
                "GPU fill/history adoption drift at {key:?}"
            );
        } else {
            assert!(
                expected.iter().all(|value| *value == 0),
                "nonempty GPU tile missing at {key:?}"
            );
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite nonnegative display coordinates are range-checked before integer indexing"
)]
fn display_pixel(canvas: &ActiveCanvas, [x, y]: [i32; 2]) -> [u8; 4] {
    let display = canvas.scene.readback_display().unwrap().unwrap();
    let point = canvas
        .view
        .mapping()
        .document_to_window(Point {
            x: f64::from(x) + 0.5,
            y: f64::from(y) + 0.5,
        })
        .unwrap();
    assert!(point.x >= 0.0 && point.y >= 0.0);
    assert!(point.x < f64::from(display.width) && point.y < f64::from(display.height));
    let offset = usize::try_from((point.y.floor() as u32) * display.width + point.x.floor() as u32)
        .unwrap()
        * 4;
    display.pixels[offset..offset + 4].try_into().unwrap()
}

fn actual_pixels(canvas: &mut ActiveCanvas) {
    canvas
        .apply_editor_command(
            EditorCommand::Viewport(ViewportCommand::ActualPixels),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(canvas);
}

#[test]
#[ignore = "requires a real WGPU adapter; run explicitly with --ignored --exact --nocapture"]
#[allow(clippy::too_many_lines)]
fn actor_reference_fill_undo_redo_matches_gpu_and_reopened_artwork() {
    // Product risk: the worker fills the wrong layer, GPU adoption drops the
    // reference/AA result, Undo leaves painted ghosts, or reopened tiles differ.
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("explicit fill acceptance requires an available GPU adapter");
    let info = adapter.get_info();
    println!(
        "flat-fill-actor-gpu adapter={} backend={:?} driver={} profile={} physical-pen-proof=false present-proof=false",
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
        label: Some("nyatidraw-flat-fill-actor-acceptance"),
        ..Default::default()
    }))
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-flat-fill-gpu-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let location = ProjectLocation::Explicit {
        path: path.clone(),
        bootstrap_png: None,
    };
    let (tree, before) = fixture();
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
    actual_pixels(&mut canvas);
    assert_eq!(canvas.cpu_tiles, pixels(&before));
    assert_eq!(canvas.active_layer, LIVE_LAYER);
    // Five document pixels beyond the page, away from its two-pixel border.
    // Raw raster equality alone cannot detect stale group composite pixels.
    let empty_workspace = display_pixel(&canvas, [35, 12]);
    assert_ne!(empty_workspace, COLOR);
    assert_eq!(empty_workspace[3], 255);
    canvas
        .enqueue_edit(EditCommand::FloodFillAdvanced {
            seed: [30, 10],
            tolerance: 0,
            source: EditSource::ReferenceLayers,
            color: COLOR,
            settings: FillSettings {
                gap_close_px: 1,
                expand_px: 1,
                antialias: true,
            },
        })
        .unwrap();
    settle(&mut canvas);
    let after = TileSnapshot::from_tiles(canvas.cpu_tiles.clone()).unwrap();
    for position in [[30, 10], [25, 12], [35, 12]] {
        assert_eq!(
            pixel(&after, LIVE_LAYER, position),
            COLOR,
            "empty target/interior AA must receive fill"
        );
    }
    for position in [[1, 1], [45, 10], [-3, 5]] {
        assert_eq!(
            pixel(&after, LIVE_LAYER, position),
            [0; 4],
            "gap-closing fill escaped into another region"
        );
    }
    for (key, tile) in before.iter() {
        assert_eq!(
            after.get(key).unwrap().pixels(),
            tile.pixels(),
            "reference artwork changed"
        );
    }
    let checked: BTreeSet<_> = before
        .iter()
        .chain(after.iter())
        .map(|(key, _)| key)
        .collect();
    compare_gpu(&canvas, &after, &checked);
    assert_eq!(
        display_pixel(&canvas, [30, 12]),
        COLOR,
        "page composite did not adopt fill"
    );
    assert_eq!(
        display_pixel(&canvas, [35, 12]),
        COLOR,
        "sparse composite reused page cache validity"
    );
    for (command, expected) in [
        (HistoryCommand::Undo, &before),
        (HistoryCommand::Redo, &after),
    ] {
        canvas
            .apply_editor_command(EditorCommand::History(command), VIEW[0], VIEW[1], 1.0)
            .unwrap();
        settle(&mut canvas);
        assert_eq!(canvas.cpu_tiles, pixels(expected));
        compare_gpu(&canvas, expected, &checked);
        assert_eq!(
            display_pixel(&canvas, [35, 12]),
            if command == HistoryCommand::Undo {
                empty_workspace
            } else {
                COLOR
            },
            "Undo/Redo left a stale sparse group composite"
        );
    }
    canvas
        .apply_editor_command(
            EditorCommand::Project(ProjectCommand::Save),
            VIEW[0],
            VIEW[1],
            1.0,
        )
        .unwrap();
    settle(&mut canvas);
    drop(canvas); // Drain the explicit Save, then release the actual worker.
    assert!(!ink.workspace_failed());
    let (_, reopened, reopened_tree, reopened_page, _, sink) =
        open_materialization_session(&location).unwrap();
    assert_eq!(reopened, after);
    assert_eq!(reopened_tree, tree);
    assert_eq!(reopened_page, PAGE);
    let flat = flatten_layer_tree_rgba8(&reopened, &tree, PAGE).unwrap();
    let png = nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
    let expected_png = nyatidraw_png_io::decode_png_bytes(
        &nyatidraw_png_io::encode_png_bytes(&flat).unwrap(),
        LIVE_LAYER,
        1024,
    )
    .unwrap();
    assert_eq!(png.tiles, expected_png.tiles);
    drop(sink);
    let reopened_ink = LiveInkBridge::with_capacity(64, LIVE_LAYER);
    let mut reopened_canvas = ActiveCanvas::new(&device, &queue, &reopened_ink, &location).unwrap();
    settle(&mut reopened_canvas);
    actual_pixels(&mut reopened_canvas);
    assert_eq!(reopened_canvas.cpu_tiles, pixels(&after));
    compare_gpu(&reopened_canvas, &after, &checked);
    assert_eq!(display_pixel(&reopened_canvas, [35, 12]), COLOR);
    drop(reopened_canvas);
    assert!(!reopened_ink.workspace_failed());
    for file in [&path, &path.with_extension("png")] {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
}

use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode, LayerId, LayerTreeNodeId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree};
use nyatidraw_project::{ProjectToolState, encode_editor_tool_state};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileObject};
use nyatidraw_web_core::{StrokePoint, WebTool};

use super::*;

#[test]
fn file_first_tool_fallback_and_worker_save_preserve_brush_state_without_changing_artwork() {
    let legacy = MemoryProjectDb::open(&[], MAX_NTDR_BYTES)
        .unwrap()
        .into_bytes()
        .unwrap();
    let mut without_settings = WebProject::decode_ntdr(&legacy).unwrap();
    let saved_defaults = without_settings.encode_ntdr().unwrap();
    assert!(
        WebProject::decode_ntdr(&saved_defaults)
            .unwrap()
            .restored_tool()
            .is_some()
    );
    let (original, tiles, _, _) = fixture();
    let mut recent = WebProject::from_document(WebDocument::default());
    recent.select_tool(WebTool::Pencil2B).unwrap();
    recent.set_project_tool(DrawingTool::Pencil);
    let mut settings = recent.brush_settings();
    settings.size_px = 17.0;
    settings.opacity = 0.63;
    recent.set_brush_settings(settings).unwrap();
    recent.set_foreground([12, 34, 56, 255]);
    recent.select_tool(WebTool::Pen).unwrap();
    recent.set_project_tool(DrawingTool::Pen);
    let fallback = recent.encode_tool_preferences().unwrap();
    let mut existing = WebProject::decode_ntdr(&original).unwrap();
    let file_settings = existing.encode_tool_preferences().unwrap();
    existing
        .restore_fallback_tool_preferences(&fallback)
        .unwrap();
    assert_eq!(existing.encode_tool_preferences().unwrap(), file_settings);
    assert_eq!(existing.snapshot(), &tiles);

    let mut blank = WebProject::from_document(WebDocument::default());
    blank.restore_fallback_tool_preferences(&fallback).unwrap();
    assert_eq!(blank.encode_tool_preferences().unwrap(), fallback);
    assert!(blank.snapshot().is_empty());
    let request = blank.prepare_recovery(1, 1, None).unwrap();
    let bytes = RecoveryWriter::default().stage(&request.bytes).unwrap();
    let reopened = WebProject::decode_ntdr(&bytes).unwrap();
    assert_eq!(reopened.encode_tool_preferences().unwrap(), fallback);
    assert_eq!(reopened.pencil_template(), PencilTemplate::Graphite2B);
    let native = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
    assert_eq!(
        native.db().load_editor_tool_state().unwrap().unwrap(),
        fallback
    );
    assert!(reopened.snapshot().is_empty());
}

#[test]
#[allow(clippy::too_many_lines)]
fn browser_edits_and_group_metadata_survive_native_process_restart() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, LayerCommand, TransformCommand as T};
    const FILE: &str = "NYATIDRAW_TEST_WEB_REOPEN_FILE";
    const ROOT: &str = "NYATIDRAW_TEST_WEB_REOPEN_ROOT";
    const ACTIVE: &str = "NYATIDRAW_TEST_WEB_REOPEN_ACTIVE";
    if let Some(path) = std::env::var_os(FILE) {
        let bytes = std::fs::read(&path).unwrap();
        let browser = WebProject::decode_ntdr(&bytes).unwrap();
        let native = ProjectDb::open(std::path::Path::new(&path)).unwrap();
        let reopened = native.load_reopened().unwrap().unwrap().into_parts();
        let expected_active = LayerId(std::env::var(ACTIVE).unwrap().parse().unwrap());
        assert_eq!(browser.active_layer(), expected_active);
        assert_eq!(
            browser
                .layers()
                .raster(expected_active)
                .unwrap()
                .color_effect,
            Some(nyatidraw_api::LayerColorEffect {
                enabled: false,
                dark_rgb: [24, 80, 190],
                light_rgb: [240, 250, 255],
            })
        );
        assert_eq!(native.load_active_layer().unwrap(), Some(expected_active));
        assert_eq!(reopened.current_tiles, *browser.snapshot());
        assert_eq!(
            format!("{:?}", reopened.current_tiles.root()),
            std::env::var(ROOT).unwrap()
        );
        assert_eq!(
            native
                .load_cursor_layer_tree(reopened.current_cursor)
                .unwrap()
                .as_ref(),
            Some(browser.layers())
        );
        return;
    }
    let mut browser = WebProject::from_document(WebDocument::default());
    browser
        .apply_edit(E::SelectLasso {
            vertices: vec![[2, 2], [20, 2], [20, 20], [2, 20]],
        })
        .unwrap();
    browser
        .apply_edit(E::FillSelection {
            color: [120, 20, 10, 255],
        })
        .unwrap();
    browser.apply_edit(E::FreeTransform(T::Begin)).unwrap();
    browser
        .apply_edit(E::FreeTransform(T::Preview(AffineTransform {
            offset_milli: [7000, -3000],
            ..Default::default()
        })))
        .unwrap();
    let generation = browser.transform_projection().unwrap().generation;
    browser
        .apply_edit(E::FreeTransform(T::Commit { generation }))
        .unwrap();
    browser.apply_layer_command(LayerCommand::AddGroup).unwrap();
    let active = browser.active_layer();
    browser
        .apply_layer_command(LayerCommand::DuplicateRaster(active))
        .unwrap();
    let raw = browser.snapshot().clone();
    let active = browser.active_layer();
    let effect = nyatidraw_api::LayerColorEffect {
        enabled: true,
        dark_rgb: [24, 80, 190],
        light_rgb: [240, 250, 255],
    };
    browser
        .apply_layer_command(LayerCommand::SetColorEffect {
            layer: active,
            effect: Some(effect),
        })
        .unwrap();
    browser
        .apply_layer_command(LayerCommand::SetColorEffect {
            layer: active,
            effect: Some(nyatidraw_api::LayerColorEffect {
                enabled: false,
                ..effect
            }),
        })
        .unwrap();
    browser.undo().unwrap();
    assert_eq!(
        browser.layers().raster(active).unwrap().color_effect,
        Some(effect)
    );
    browser.redo().unwrap();
    assert_eq!(
        browser.snapshot(),
        &raw,
        "effect edits and their history preserve original raster bytes"
    );
    let bytes = browser.encode_ntdr().unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-web-reopen-{}-{nonce}.ntdr",
        std::process::id()
    ));
    std::fs::write(&path, bytes).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::browser_edits_and_group_metadata_survive_native_process_restart",
            "--nocapture",
        ])
        .env(FILE, &path)
        .env(ROOT, format!("{:?}", browser.snapshot().root()))
        .env(ACTIVE, browser.active_layer().0.to_string())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn imported_image_and_group_copy_preserve_sources_through_undo_and_fresh_reopen() {
    use nyatidraw_api::{EditCommand as E, LayerCommand as L, RasterImport, TransformCommand as T};
    const FILE: &str = "NYATIDRAW_IMPORT_GROUP_REOPEN_FILE";
    const ROOT: &str = "NYATIDRAW_IMPORT_GROUP_REOPEN_ROOT";
    if let Some(path) = std::env::var_os(FILE) {
        let bytes = std::fs::read(&path).unwrap();
        let web = WebProject::decode_ntdr(&bytes).unwrap();
        let native = ProjectDb::open(std::path::Path::new(&path)).unwrap();
        let reopened = native.load_reopened().unwrap().unwrap();
        assert_eq!(web.snapshot(), reopened.current_tiles());
        assert_eq!(
            format!("{:?}", web.snapshot().root()),
            std::env::var(ROOT).unwrap()
        );
        assert_eq!(
            native.load_layer_tree().unwrap().as_ref(),
            Some(web.layers())
        );
        assert_eq!(web.layers().root().children.len(), 2);
        assert_eq!(
            web.layers().raster(web.active_layer()).unwrap().name,
            "가져온 PNG"
        );
        assert_eq!(web.canvas().width_px, 64);
        return;
    }
    let (original, _, _, _) = fixture();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    web.apply_layer_command(L::AddGroup).unwrap();
    let group = match web.layers().root().children.last().unwrap() {
        LayerTreeNode::Group(group) => group.id,
        LayerTreeNode::Raster(_) => panic!("new group missing"),
    };
    for (index, layer) in [LayerId(19), LayerId(23)].into_iter().enumerate() {
        web.apply_layer_command(L::Reorder {
            node: LayerTreeNodeId::Raster(layer),
            new_parent: group,
            index,
        })
        .unwrap();
    }
    web.set_active_layer(LayerId(19)).unwrap();
    web.apply_layer_command(L::AddGroup).unwrap();
    let inner = match &web.layers().root().children[0] {
        LayerTreeNode::Group(outer) => match &outer.children[1] {
            LayerTreeNode::Group(inner) => inner.id,
            LayerTreeNode::Raster(_) => panic!("nested group missing"),
        },
        LayerTreeNode::Raster(_) => panic!("outer group missing"),
    };
    web.apply_layer_command(L::Reorder {
        node: LayerTreeNodeId::Raster(LayerId(23)),
        new_parent: inner,
        index: 0,
    })
    .unwrap();
    web.apply_layer_command(L::SetOpacity {
        node: LayerTreeNodeId::Group(group),
        opacity_u16: 32_000,
    })
    .unwrap();
    let before_tree = web.layers().clone();
    let before_tiles = web.snapshot().clone();
    let history = web.undo_len();
    web.apply_layer_command(L::DuplicateGroup(group)).unwrap();
    let copied_layer = web.active_layer();
    assert_ne!(copied_layer, LayerId(19));
    let mut expected = before_tree.clone();
    let copied_group = match &web.layers().root().children[1] {
        LayerTreeNode::Group(group) => group.id,
        LayerTreeNode::Raster(_) => panic!("copy missing"),
    };
    let mapping = expected.duplicate_group(group, copied_group.0).unwrap();
    assert_eq!(web.layers(), &expected);
    assert_eq!(web.undo_len(), history + 1);
    for (source, destination) in mapping {
        let mut expected_layer = before_tree.raster(source).unwrap().clone();
        expected_layer.id = destination;
        assert_eq!(
            web.layers().raster(destination),
            Some(&expected_layer),
            "raw source, disabled color effect, clip, lock and blend metadata must survive"
        );
        for (key, tile) in before_tiles.iter().filter(|(key, _)| key.layer == source) {
            assert_eq!(
                web.snapshot().get(TileKey {
                    layer: destination,
                    ..key
                }),
                Some(tile)
            );
        }
    }
    let duplicated = web.snapshot().clone();
    web.undo().unwrap();
    assert_eq!(web.snapshot(), &before_tiles);
    assert_eq!(web.layers(), &before_tree);
    web.redo().unwrap();
    assert_eq!(web.snapshot(), &duplicated);
    web.set_active_layer(copied_layer).unwrap();
    web.apply_layer_command(L::SetAlphaLocked {
        layer: copied_layer,
        alpha_locked: false,
    })
    .unwrap();
    web.apply_edit(E::ClearActiveLayer).unwrap();
    for (key, tile) in before_tiles.iter() {
        assert_eq!(
            web.snapshot().get(key),
            Some(tile),
            "editing copy must not touch original"
        );
    }
    web.undo().unwrap();
    web.undo().unwrap();
    let mut invalid = before_tree.clone();
    assert!(invalid.duplicate_group(group, u128::MAX).is_err());
    assert_eq!(invalid, before_tree);
    assert!(invalid.duplicate_group(group, group.0).is_err());
    assert_eq!(invalid, before_tree);
    web.apply_edit(E::FreeTransform(T::ImportRaster(RasterImport {
        project_epoch: 1,
        name: "가져온 PNG".into(),
        size: [2, 1],
        pixels: std::sync::Arc::from([0, 90, 0, 128, 0, 0, 0, 0]),
    })))
    .unwrap();
    let generation = web.transform_projection().unwrap().generation;
    web.apply_edit(E::FreeTransform(T::Commit { generation }))
        .unwrap();
    // The imported raster is inside the copied group, preserving the page and hierarchy.
    assert_eq!(web.layers().root().children.len(), 2);
    let path = scratch_path("import-group-fresh");
    std::fs::write(&path, web.encode_ntdr().unwrap()).unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::imported_image_and_group_copy_preserve_sources_through_undo_and_fresh_reopen",
            "--nocapture",
        ])
        .env(FILE, &path)
        .env(ROOT, format!("{:?}", web.snapshot().root()))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn recovered_color_target_survives_selection_only_save_without_artwork_history() {
    let (original, tiles, _, _) = fixture();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    let initial_memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
    let before = initial_memory
        .db()
        .load_reopened()
        .unwrap()
        .unwrap()
        .into_parts();
    assert_eq!(web.active_layer(), LayerId(23));
    web.set_active_layer(LayerId(19)).unwrap();
    let tools = web.encode_tool_preferences().unwrap();
    let request = web.prepare_recovery(1, 1, None).unwrap();
    let mut writer = RecoveryWriter::default();
    let bytes = writer.stage(&request.bytes).unwrap();
    writer.accept().unwrap();
    let restored = WebProject::decode_ntdr(&bytes).unwrap();
    assert_eq!(restored.active_layer(), LayerId(19));
    assert_eq!(restored.encode_tool_preferences().unwrap(), tools);
    assert_eq!(restored.snapshot(), &tiles);
    let memory = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
    let after = memory.db().load_reopened().unwrap().unwrap().into_parts();
    assert_eq!(after.current_cursor, before.current_cursor);
    assert_eq!(after.history.node_count(), before.history.node_count());
    for stale in [LayerId(999), LayerId(77)] {
        let memory = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
        memory.db().persist_active_layer(stale).unwrap();
        let bytes = memory.into_bytes().unwrap();
        let restored = WebProject::decode_ntdr(&bytes).unwrap();
        assert_eq!(restored.active_layer(), LayerId(23));
        assert_eq!(restored.snapshot(), &tiles);
    }
}

fn fixture() -> (Vec<u8>, TileSnapshot, LayerTree, ProjectToolState) {
    let memory = MemoryProjectDb::open(&[], MAX_NTDR_BYTES).unwrap();
    let db = memory.db();
    let canvas = CanvasSpec {
        width_px: 64,
        height_px: 64,
        pixels_per_inch: 300,
    };
    let layers = LayerTree::new(GroupNode {
        id: GroupId(77),
        name: "Native root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: [LayerId(19), LayerId(23)]
            .into_iter()
            .map(|id| {
                LayerTreeNode::Raster(LayerNode {
                    mask: None,
                    color_effect: if id.0 == 19 {
                        Some(nyatidraw_api::LayerColorEffect {
                            enabled: false,
                            dark_rgb: [20, 90, 200],
                            light_rgb: [240, 245, 255],
                        })
                    } else {
                        None
                    },
                    id,
                    name: format!("Layer {}", id.0),
                    visible: true,
                    locked: false,
                    alpha_locked: id.0 == 19,
                    clip_to_below: id.0 == 23,
                    reference: id.0 == 19,
                    opacity_u16: 40_000,
                    blend_mode: LayerBlendMode::Multiply,
                    content_root: ContentRootId(id.0 + 10),
                })
            })
            .collect(),
    })
    .unwrap();
    db.persist_canvas_spec(canvas).unwrap();
    db.persist_layer_tree(&layers).unwrap();
    let state = preferences::to_native(WebDocument::default().preferences(), None, None).unwrap();
    db.persist_editor_tool_state(&encode_editor_tool_state(&state))
        .unwrap();
    let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
    let mut tiles = TileSnapshot::empty();
    for (id, x, rgba) in [(1, -1, [12, 34, 45, 70]), (2, 0, [24, 30, 40, 80])] {
        let mut pixels = vec![0; TILE_BYTE_LEN];
        pixels[..4].copy_from_slice(&rgba);
        tiles = tiles
            .with_replacements([(
                TileKey {
                    layer: LayerId(19),
                    mip: 0,
                    x,
                    y: 0,
                },
                pixels,
            )])
            .unwrap();
        let batch = session
            .prepare_structural_change(SnapshotId(id), HistoryNodeId(id), 0, tiles.clone())
            .unwrap();
        db.commit_structural_with_metadata(&batch, &layers, canvas)
            .unwrap();
        session.accept_structural_change(&batch).unwrap();
    }
    (memory.into_bytes().unwrap(), tiles, layers, state)
}

#[test]
fn native_web_native_roundtrip_preserves_artwork_metadata_preferences_and_history() {
    let (original, tiles, layers, mut state) = fixture();
    state.tool = DrawingTool::Pencil;
    state.last_painting_slot = 0;
    state.remembered[0].size_tenths = 1;
    state.remembered[0].settings.size_minimum_u16 = 42_000;
    state.remembered[4].settings.opacity_minimum_u16 = 15_000;
    let memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
    memory
        .db()
        .persist_editor_tool_state(&encode_editor_tool_state(&state))
        .unwrap();
    let original = memory.into_bytes().unwrap();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    assert_eq!(web.snapshot(), &tiles);
    assert_eq!(web.layers(), &layers);
    assert_eq!(web.brush_settings().size_px.to_bits(), 0.1_f32.to_bits());
    assert_eq!(
        web.brush_preset().size_min_ratio.to_bits(),
        (42_000.0_f32 / 65_535.0).to_bits()
    );
    let output = web.encode_ntdr().unwrap();
    let path = scratch_path("roundtrip");
    std::fs::write(&path, output).unwrap();
    let native = ProjectDb::open(&path).unwrap();
    let reopened = native.load_reopened().unwrap().unwrap().into_parts();
    assert_eq!(reopened.current_tiles, tiles);
    assert_eq!(reopened.history.node_count(), 2);
    assert_eq!(native.load_layer_tree().unwrap(), Some(layers));
    assert_eq!(native.load_canvas_spec().unwrap().pixels_per_inch, 300);
    assert_eq!(
        native.load_editor_tool_state().unwrap(),
        Some(encode_editor_tool_state(&state))
    );
    drop(native);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn editing_an_undone_native_project_preserves_redo_branch_and_exact_new_pixels() {
    let (original, _, _, _) = fixture();
    let memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
    let reopened = memory.db().load_reopened().unwrap().unwrap();
    let session = HeadlessStrokeSession::from_reopened(reopened);
    let undo = session.prepare_undo_cursor().unwrap();
    memory.db().persist_history_cursor(undo.target()).unwrap();
    let original = memory.into_bytes().unwrap();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    web.set_active_layer(LayerId(23)).unwrap();
    web.select_tool(WebTool::Pen).unwrap();
    web.set_foreground([30, 60, 90, 255]);
    let point = StrokePoint {
        x: 20.0,
        y: 20.0,
        pressure: 1.0,
        time_ms: 1.0,
    };
    web.begin_stroke(point).unwrap();
    web.end_stroke(StrokePoint {
        x: 25.0,
        time_ms: 2.0,
        ..point
    })
    .unwrap();
    let expected = web.snapshot().clone();
    let output = web.encode_ntdr().unwrap();
    let memory = MemoryProjectDb::open(&output, MAX_NTDR_BYTES).unwrap();
    let parts = memory.db().load_reopened().unwrap().unwrap().into_parts();
    assert_eq!(parts.current_tiles, expected);
    assert_eq!(parts.history.node_count(), 3);
    assert_eq!(
        parts.history.node(HistoryNodeId(2)).unwrap().parent,
        Some(HistoryNodeId(1))
    );
    assert_eq!(
        parts.history.node(HistoryNodeId(3)).unwrap().parent,
        Some(HistoryNodeId(1))
    );
    let cursor = parts.cursors[&Some(HistoryNodeId(1))];
    assert_eq!(memory.db().load_cursor_tiles(cursor).unwrap().len(), 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn blend_modes_survive_native_web_native_process_restart_without_repainting() {
    const REOPEN: &str = "NYATIDRAW_BLEND_MODES_REOPEN";
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = std::path::PathBuf::from(path);
        let bytes = std::fs::read(&path).unwrap();
        let web = WebProject::decode_ntdr(&bytes).unwrap();
        let native = ProjectDb::open(&path).unwrap();
        assert_eq!(
            native.load_layer_tree().unwrap().as_ref(),
            Some(web.layers())
        );
        assert_eq!(
            native.load_reopened().unwrap().unwrap().current_tiles(),
            web.snapshot()
        );
        assert_eq!(
            web.snapshot().root().hash.0.as_slice(),
            std::fs::read(path.with_extension("root")).unwrap()
        );
        assert_eq!(
            web.export_rgba8().unwrap(),
            std::fs::read(path.with_extension("rgba")).unwrap()
        );
        assert_eq!(
            nyatidraw_project::encode_layer_tree(web.layers()).unwrap(),
            std::fs::read(path.with_extension("layers")).unwrap()
        );
        return;
    }
    for mode in [
        LayerBlendMode::Screen,
        LayerBlendMode::Add,
        LayerBlendMode::Overlay,
    ] {
        let (original, tiles, mut layers, _) = fixture();
        for id in [LayerId(19), LayerId(23)] {
            layers
                .set_blend_mode(LayerTreeNodeId::Raster(id), mode)
                .unwrap();
        }
        let root = layers.root().clone();
        let layers = LayerTree::new(GroupNode {
            children: vec![LayerTreeNode::Group(GroupNode {
                id: GroupId(88),
                name: "Light group".into(),
                visible: true,
                opacity_u16: 50_000,
                clip_to_below: false,
                blend_mode: mode,
                children: root.children.clone(),
            })],
            ..root
        })
        .unwrap();
        let mut light = vec![0; TILE_BYTE_LEN];
        light[..4].copy_from_slice(&[64, 16, 32, 128]);
        let tiles = tiles
            .with_replacements([(
                TileKey {
                    layer: LayerId(23),
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                light,
            )])
            .unwrap();
        let memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
        let session =
            HeadlessStrokeSession::from_reopened(memory.db().load_reopened().unwrap().unwrap());
        let batch = session
            .prepare_structural_change(SnapshotId(3), HistoryNodeId(3), 0, tiles.clone())
            .unwrap();
        memory
            .db()
            .commit_structural_with_layer_tree(&batch, &layers)
            .unwrap();
        let native_bytes = memory.into_bytes().unwrap();
        let mut web = WebProject::decode_ntdr(&native_bytes).unwrap();
        assert_eq!(web.layers(), &layers);
        assert_eq!(web.snapshot(), &tiles);
        let expected = web.export_rgba8().unwrap();
        let output = web.encode_ntdr().unwrap();
        assert_eq!(
            web.snapshot(),
            &tiles,
            "saving blend modes must not bake raw artwork"
        );
        let path = scratch_path("blend-process-reopen");
        std::fs::write(&path, output).unwrap();
        std::fs::write(path.with_extension("rgba"), expected).unwrap();
        std::fs::write(path.with_extension("root"), tiles.root().hash.0).unwrap();
        std::fs::write(
            path.with_extension("layers"),
            nyatidraw_project::encode_layer_tree(&layers).unwrap(),
        )
        .unwrap();
        assert!(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::blend_modes_survive_native_web_native_process_restart_without_repainting"])
            .env(REOPEN, &path)
            .status().unwrap().success());
        for file in [
            path.with_extension("rgba"),
            path.with_extension("layers"),
            path.with_extension("root"),
            path,
        ] {
            std::fs::remove_file(file).unwrap();
        }
    }
}

#[test]
fn groups_roundtrip_and_invalid_data_never_replace_source_artwork() {
    let (original, tiles, layers, _) = fixture();
    let memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
    let mut root = layers.root().clone();
    root.children.push(LayerTreeNode::Group(GroupNode {
        id: GroupId(99),
        name: "Nested".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: Vec::new(),
    }));
    let tree = LayerTree::new(root).unwrap();
    let session =
        HeadlessStrokeSession::from_reopened(memory.db().load_reopened().unwrap().unwrap());
    let batch = session
        .prepare_structural_change(SnapshotId(3), HistoryNodeId(3), 0, tiles)
        .unwrap();
    memory
        .db()
        .commit_structural_with_metadata(
            &batch,
            &tree,
            CanvasSpec {
                width_px: 64,
                height_px: 64,
                pixels_per_inch: 300,
            },
        )
        .unwrap();
    let grouped = memory.into_bytes().unwrap();
    let preserved = grouped.clone();
    let mut web = WebProject::decode_ntdr(&grouped).unwrap();
    assert_eq!(web.layers(), &tree);
    web.apply_layer_command(nyatidraw_api::LayerCommand::Reorder {
        node: nyatidraw_api::LayerTreeNodeId::Raster(LayerId(19)),
        new_parent: GroupId(99),
        index: 0,
    })
    .unwrap();
    let nested_tree = web.layers().clone();
    let bytes = web.encode_ntdr().unwrap();
    let reopened = WebProject::decode_ntdr(&bytes).unwrap();
    assert_eq!(reopened.layers(), &nested_tree);
    assert_eq!(reopened.snapshot(), web.snapshot());
    assert_eq!(grouped, preserved);
    assert!(WebProject::decode_ntdr(b"invalid nonempty file").is_err());
    assert_eq!(
        WebProject::decode_ntdr(&original).unwrap().snapshot().len(),
        2
    );
    let mut pixels = vec![0; TILE_BYTE_LEN];
    pixels[..4].copy_from_slice(&[255, 0, 0, 1]);
    let invalid = TileSnapshot::from_objects([(
        TileKey {
            layer: LayerId(19),
            mip: 0,
            x: 0,
            y: 0,
        },
        TileObject::new(pixels).unwrap(),
    )])
    .unwrap();
    assert!(
        WebDocument::from_snapshot(
            CanvasSpec {
                width_px: 64,
                height_px: 64,
                pixels_per_inch: 300
            },
            invalid,
            layers,
            LayerId(19)
        )
        .is_err()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn group_transform_commit_survives_native_process_restart_with_exact_raw_tiles() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, TransformCommand as T};
    const REOPEN: &str = "NYATIDRAW_GROUP_TRANSFORM_REOPEN";
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = std::path::PathBuf::from(path);
        let saved_bytes = std::fs::read(&path).unwrap();
        let native = ProjectDb::open(&path).unwrap();
        let reopened = native.load_reopened().unwrap().unwrap();
        assert_eq!(
            reopened.current_tiles().root().hash.0.as_slice(),
            std::fs::read(path.with_extension("root")).unwrap()
        );
        let tree = native.load_layer_tree().unwrap().unwrap();
        assert_eq!(
            nyatidraw_project::encode_layer_tree(&tree).unwrap(),
            std::fs::read(path.with_extension("layers")).unwrap()
        );
        let web = WebProject::decode_ntdr(&saved_bytes).unwrap();
        assert_eq!(
            web.export_rgba8().unwrap(),
            std::fs::read(path.with_extension("rgba")).unwrap()
        );
        let session = HeadlessStrokeSession::from_reopened(reopened);
        let undo = session.prepare_undo_cursor().unwrap();
        let before = native.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(
            before.root().hash.0.as_slice(),
            std::fs::read(path.with_extension("before")).unwrap()
        );
        return;
    }
    let (original, _, mut layers, _) = fixture();
    layers.set_alpha_locked(LayerId(19), false).unwrap();
    let root = layers.root().clone();
    let layers = LayerTree::new(GroupNode {
        children: vec![LayerTreeNode::Group(GroupNode {
            id: GroupId(88),
            name: "transform subject".into(),
            visible: true,
            opacity_u16: 40000,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Screen,
            children: root.children.clone(),
        })],
        ..root
    })
    .unwrap();
    let memory = MemoryProjectDb::open(&original, MAX_NTDR_BYTES).unwrap();
    let session =
        HeadlessStrokeSession::from_reopened(memory.db().load_reopened().unwrap().unwrap());
    let group_batch = session
        .prepare_structural_change(SnapshotId(3), HistoryNodeId(3), 0, session.tiles().clone())
        .unwrap();
    memory
        .db()
        .commit_structural_with_layer_tree(&group_batch, &layers)
        .unwrap();
    let mut web = WebProject::decode_ntdr(&memory.into_bytes().unwrap()).unwrap();
    let before = web.snapshot().clone();
    web.apply_edit(E::FreeTransform(T::BeginGroup(GroupId(88))))
        .unwrap();
    web.apply_edit(E::FreeTransform(T::Preview(AffineTransform {
        offset_milli: [2000, 3000],
        ..AffineTransform::default()
    })))
    .unwrap();
    assert_eq!(web.snapshot(), &before);
    let generation = web.transform_projection().unwrap().generation;
    web.apply_edit(E::FreeTransform(T::Commit { generation }))
        .unwrap();
    let after = web.snapshot().clone();
    assert_ne!(after, before);
    assert_eq!(web.layers(), &layers);
    let path = scratch_path("group-transform-process-reopen");
    std::fs::write(path.with_extension("root"), after.root().hash.0).unwrap();
    std::fs::write(path.with_extension("before"), before.root().hash.0).unwrap();
    std::fs::write(
        path.with_extension("layers"),
        nyatidraw_project::encode_layer_tree(&layers).unwrap(),
    )
    .unwrap();
    std::fs::write(path.with_extension("rgba"), web.export_rgba8().unwrap()).unwrap();
    std::fs::write(&path, web.encode_ntdr().unwrap()).unwrap();
    assert!(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::group_transform_commit_survives_native_process_restart_with_exact_raw_tiles"
            ])
            .env(REOPEN, &path)
            .status()
            .unwrap()
            .success()
    );
    for file in [
        path.with_extension("root"),
        path.with_extension("before"),
        path.with_extension("layers"),
        path.with_extension("rgba"),
        path,
    ] {
        std::fs::remove_file(file).unwrap();
    }
}

#[test]
fn legacy_browser_import_saves_native_format_and_unknown_tool_records_are_not_overwritten() {
    let legacy = WebDocument::import_rgba8(8, 8, &[127; 8 * 8 * 4]).unwrap();
    let pixels = legacy.snapshot().clone();
    let mut imported = WebProject::decode_ntdr(&legacy.encode_portable().unwrap()).unwrap();
    let bytes = imported.encode_ntdr().unwrap();
    assert!(!bytes.starts_with(b"NYWEB001"));
    assert_eq!(WebProject::decode_ntdr(&bytes).unwrap().snapshot(), &pixels);
    let memory = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
    memory
        .db()
        .persist_editor_tool_state(b"future prefs")
        .unwrap();
    let bytes = memory.into_bytes().unwrap();
    let mut imported = WebProject::decode_ntdr(&bytes).unwrap();
    let no_change = imported.encode_ntdr().unwrap();
    assert_eq!(
        MemoryProjectDb::open(&no_change, MAX_NTDR_BYTES)
            .unwrap()
            .db()
            .load_editor_tool_state()
            .unwrap(),
        Some(b"future prefs".to_vec())
    );
    imported.set_foreground([200, 100, 50, 255]);
    imported.set_project_tool(DrawingTool::Pen);
    let point = StrokePoint {
        x: 4.0,
        y: 4.0,
        pressure: 1.0,
        time_ms: 1.0,
    };
    imported.begin_stroke(point).unwrap();
    imported.end_stroke(point).unwrap();
    let expected = imported.snapshot().clone();
    assert_ne!(expected, pixels);
    let request = imported.prepare_recovery(1, 1, None).unwrap();
    let mut writer = RecoveryWriter::default();
    let recovered = writer.stage(&request.bytes).unwrap();
    writer.accept().unwrap();
    for bytes in [imported.encode_ntdr().unwrap(), recovered] {
        let memory = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
        assert_eq!(
            memory.db().load_editor_tool_state().unwrap(),
            Some(b"future prefs".to_vec())
        );
        assert_eq!(
            memory
                .db()
                .load_reopened()
                .unwrap()
                .unwrap()
                .current_tiles(),
            &expected
        );
        let reopened = WebProject::decode_ntdr(&bytes).unwrap();
        assert_eq!(reopened.snapshot(), &expected);
        assert!(reopened.import_notice().is_some());
    }
}

fn scratch_path(label: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "nyatidraw-web-bridge-{label}-{}-{nonce}.ntdr",
        std::process::id()
    ))
}

#[test]
fn batched_recovery_preserves_every_stroke_pixel_and_session_undo() {
    for count in [1, 8] {
        let mut web = WebProject::new(CanvasSpec {
            width_px: 64,
            height_px: 64,
            pixels_per_inch: 96,
        })
        .unwrap();
        web.select_tool(WebTool::Pen).unwrap();
        let mut before_last = web.snapshot().clone();
        for index in 0..count {
            before_last = web.snapshot().clone();
            let point = StrokePoint {
                x: 8.0,
                y: 8.0 + f64::from(index) * 6.0,
                pressure: 1.0,
                time_ms: f64::from(index) * 10.0,
            };
            web.begin_stroke(point).unwrap();
            web.end_stroke(StrokePoint {
                x: 12.0,
                time_ms: point.time_ms + 1.0,
                ..point
            })
            .unwrap();
        }
        let expected = web.snapshot().clone();
        let path = scratch_path("batched-recovery");
        std::fs::write(&path, web.encode_ntdr().unwrap()).unwrap();
        let native = ProjectDb::open(&path).unwrap();
        assert_eq!(
            native
                .load_reopened()
                .unwrap()
                .unwrap()
                .into_parts()
                .current_tiles,
            expected
        );
        drop(native);
        std::fs::remove_file(path).unwrap();
        assert_eq!(web.undo_len(), usize::try_from(count).unwrap());
        web.undo().unwrap();
        assert_eq!(web.snapshot(), &before_last);
        web.redo().unwrap();
        assert_eq!(web.snapshot(), &expected);
        assert_eq!(
            WebProject::decode_ntdr(&web.encode_ntdr().unwrap())
                .unwrap()
                .snapshot(),
            &expected
        );
    }
}

#[test]
fn repeated_browser_saves_keep_native_retention_and_reopen_after_128_edits() {
    let mut web = WebProject::new(CanvasSpec {
        width_px: 16,
        height_px: 16,
        pixels_per_inch: 96,
    })
    .unwrap();
    for index in 0..130 {
        let active = web.active_layer();
        web.rename_layer(active, &format!("Edit {index}")).unwrap();
        let bytes = web.encode_ntdr().unwrap();
        web = WebProject::decode_ntdr(&bytes).unwrap();
    }
    let memory = MemoryProjectDb::open(&web.backing, MAX_NTDR_BYTES).unwrap();
    assert_eq!(
        memory
            .db()
            .load_reopened()
            .unwrap()
            .unwrap()
            .into_parts()
            .history
            .node_count(),
        128
    );
    assert_eq!(
        web.layers().raster(web.active_layer()).unwrap().name,
        "Edit 129"
    );
}

#[test]
fn worker_deltas_keep_cumulative_pixels_deletions_and_native_history() {
    let (original, tiles, layers, _) = fixture();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    let mut writer = RecoveryWriter::default();
    let initial = web.prepare_recovery(1, 1, None).unwrap();
    writer.stage(&initial.bytes).unwrap();
    assert!(writer.file().is_err());
    writer.accept().unwrap();
    let mut checkpoint = initial.checkpoint;
    web.set_active_layer(LayerId(23)).unwrap();
    web.select_tool(WebTool::Pen).unwrap();
    for (revision, x) in [(2, 20.0), (3, 280.0)] {
        let point = StrokePoint {
            x,
            y: 20.0,
            pressure: 1.0,
            time_ms: 1.0,
        };
        web.begin_stroke(point).unwrap();
        web.end_stroke(StrokePoint {
            x: x + 8.0,
            time_ms: 2.0,
            ..point
        })
        .unwrap();
        if revision == 2 {
            continue;
        }
        let request = web
            .prepare_recovery(1, revision, Some(&checkpoint))
            .unwrap();
        let bytes = writer.stage(&request.bytes).unwrap();
        assert_eq!(
            WebProject::decode_ntdr(&bytes).unwrap().snapshot(),
            web.snapshot()
        );
        writer.accept().unwrap();
        checkpoint = request.checkpoint;
    }
    web.undo().unwrap();
    web.undo().unwrap();
    assert_eq!(web.snapshot(), &tiles);
    let removal = web.prepare_recovery(1, 4, Some(&checkpoint)).unwrap();
    assert!(
        removal.bytes.len() < 1024,
        "unchanged pixels must not cross the worker boundary again"
    );
    let bytes = writer.stage(&removal.bytes).unwrap();
    writer.accept().unwrap();
    let memory = MemoryProjectDb::open(&bytes, MAX_NTDR_BYTES).unwrap();
    let reopened = memory.db().load_reopened().unwrap().unwrap().into_parts();
    assert_eq!(reopened.current_tiles, tiles);
    assert_eq!(reopened.history.node_count(), 4);
    assert_eq!(memory.db().load_layer_tree().unwrap(), Some(layers));
}

#[test]
fn worker_failed_stale_and_corrupt_requests_never_replace_accepted_artwork() {
    let (original, _, _, _) = fixture();
    let mut web = WebProject::decode_ntdr(&original).unwrap();
    let mut writer = RecoveryWriter::default();
    let initial = web.prepare_recovery(1, 1, None).unwrap();
    writer.stage(&initial.bytes).unwrap();
    writer.accept().unwrap();
    let preserved = writer.file().unwrap().to_vec();
    web.set_foreground([10, 20, 30, 255]);
    let change = web
        .prepare_recovery(1, 2, Some(&initial.checkpoint))
        .unwrap();
    writer.stage(&change.bytes).unwrap();
    assert!(writer.stage(&change.bytes).is_err());
    writer.abort();
    assert_eq!(writer.file().unwrap(), preserved);
    assert!(writer.accept().is_err());
    for packet in [
        &change.bytes[..3],
        &change.bytes[..change.bytes.len() - 1],
        &initial.bytes,
    ] {
        assert!(writer.stage(packet).is_err());
        assert_eq!(writer.file().unwrap(), preserved);
    }
    writer.stage(&change.bytes).unwrap();
    writer.accept().unwrap();
    let accepted = writer.file().unwrap().to_vec();
    assert!(writer.stage(&change.bytes).is_err());
    assert_eq!(writer.file().unwrap(), accepted);
    let replacement = WebProject::new(CanvasSpec {
        width_px: 32,
        height_px: 32,
        pixels_per_inch: 96,
    })
    .unwrap();
    let request = replacement
        .prepare_recovery(2, 3, Some(&change.checkpoint))
        .unwrap();
    let bytes = writer.stage(&request.bytes).unwrap();
    writer.accept().unwrap();
    assert!(
        WebProject::decode_ntdr(&bytes)
            .unwrap()
            .snapshot()
            .is_empty()
    );
    assert!(writer.stage(&change.bytes).is_err());
    assert_eq!(writer.file().unwrap(), bytes);
}

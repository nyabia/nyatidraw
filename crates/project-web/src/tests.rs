use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode, LayerId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree};
use nyatidraw_project::{ProjectToolState, encode_editor_tool_state};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileObject};
use nyatidraw_web_core::{StrokePoint, WebTool};

use super::*;

#[test]
fn browser_edits_and_group_metadata_survive_native_process_restart() {
    use nyatidraw_api::{AffineTransform, EditCommand as E, LayerCommand, TransformCommand as T};
    const FILE: &str = "NYATIDRAW_TEST_WEB_REOPEN_FILE";
    const ROOT: &str = "NYATIDRAW_TEST_WEB_REOPEN_ROOT";
    if let Some(path) = std::env::var_os(FILE) {
        let bytes = std::fs::read(&path).unwrap();
        let browser = WebProject::decode_ntdr(&bytes).unwrap();
        let native = ProjectDb::open(std::path::Path::new(&path)).unwrap();
        let reopened = native.load_reopened().unwrap().unwrap().into_parts();
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
    assert!(imported.encode_ntdr().is_err());
    assert_eq!(imported.backing, no_change);
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

use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode, LayerId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree};
use nyatidraw_project::{ProjectToolState, encode_editor_tool_state};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileObject};
use nyatidraw_web_core::{StrokePoint, WebTool};

use super::*;

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
fn unsupported_groups_corruption_and_invalid_pixels_never_replace_source_artwork() {
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
    assert!(
        WebProject::decode_ntdr(&grouped)
            .err()
            .expect("groups must be rejected")
            .contains("groups")
    );
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

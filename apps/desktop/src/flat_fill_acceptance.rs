use super::*;
use nyatidraw_api::{DrawingTool, EditCommand, EditSettings, EditSource, FillSettings};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask, selection_all};

const LINE: LayerId = LayerId(91);
const SENTINEL: LayerId = LayerId(92);
const COLOR: [u8; 4] = [255, 0, 0, 255];
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 32,
    height_px: 32,
    pixels_per_inch: 96,
};

fn pixel(snapshot: &TileSnapshot, layer: LayerId, x: i32, y: i32) -> [u8; 4] {
    let key = TileKey {
        layer,
        mip: 0,
        x: x.div_euclid(128),
        y: y.div_euclid(128),
    };
    let index = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
    snapshot.get(key).map_or([0; 4], |tile| {
        tile.pixels()[index..index + 4].try_into().unwrap()
    })
}

fn fixture() -> (LayerTree, TileSnapshot) {
    let mut tree = default_layer_tree();
    for (id, reference, name) in [(LINE, true, "Linework"), (SENTINEL, false, "Other artwork")] {
        let mut layer = tree.raster(LIVE_LAYER).unwrap().clone();
        layer.id = id;
        layer.name = name.into();
        layer.reference = reference;
        tree.insert(tree.root_id(), 1, LayerTreeNode::Raster(layer))
            .unwrap();
    }
    let mut tiles = std::collections::BTreeMap::new();
    let mut put = |layer, x: i32, y: i32, color: [u8; 4]| {
        let key = TileKey {
            layer,
            mip: 0,
            x: x.div_euclid(128),
            y: y.div_euclid(128),
        };
        let tile = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
        let index = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
        tile[index..index + 4].copy_from_slice(&color);
    };
    for y in 8..=23 {
        for x in 8..=23 {
            let edge = x == 8 || x == 23 || y == 8 || y == 23;
            let aa = x == 9 || x == 22 || y == 9 || y == 22;
            if (edge || aa) && !(y == 15 && x >= 22) {
                put(LINE, x, y, [0, 0, 0, if edge { 255 } else { 128 }]);
            }
        }
    }
    put(SENTINEL, -5, 3, [0, 255, 0, 255]);
    put(SENTINEL, 29, 29, [0, 0, 255, 255]);
    (tree, TileSnapshot::from_tiles(tiles).unwrap())
}

fn fill_gesture(has_selection: bool) -> EditCommand {
    let sample = |phase| StylusSample {
        sequence: 1,
        timestamp_ns: 1,
        device_id: 1,
        phase,
        position_document: Point { x: 12.5, y: 16.5 },
        pressure: 1.0,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons(0),
        eraser: false,
        viewport_revision: 1,
    };
    let settings = EditSettings {
        source: EditSource::ReferenceLayers,
        fill: FillSettings {
            gap_close_px: 1,
            expand_px: 1,
            antialias: true,
        },
        ..Default::default()
    };
    let command = crate::edit_gesture::EditGesture::begin(
        DrawingTool::Fill,
        settings,
        COLOR,
        has_selection,
        sample(PointerPhase::Begin),
    )
    .finish(sample(PointerPhase::End))
    .unwrap();
    assert!(
        matches!(command, EditCommand::FloodFillAdvanced { seed: [12, 16], source: EditSource::ReferenceLayers, settings: actual, .. } if actual == settings.fill),
        "a selection must clip connected fill, not turn it into fill-all-selected"
    );
    command
}

fn execute(
    command: EditCommand,
    tree: &LayerTree,
    selection: &mut Option<Arc<SelectionMask>>,
    session: &mut nyatidraw_editor::HeadlessStrokeSession,
    next_id: &mut u128,
    db: &nyatidraw_project_redb::ProjectDb,
) -> crate::edit_worker::EditOutcome {
    crate::edit_worker::execute(
        command, LIVE_LAYER, CANVAS, tree, selection, session, next_id, db,
    )
    .unwrap_or_else(|error| match error {
        crate::edit_worker::EditFailure::Rejected(message)
        | crate::edit_worker::EditFailure::Fatal(message) => panic!("fill acceptance: {message}"),
    })
}

fn assert_artwork(before: &TileSnapshot, after: &TileSnapshot) {
    assert_eq!(
        pixel(after, LIVE_LAYER, 12, 16),
        COLOR,
        "empty target must fill using separate reference linework"
    );
    assert_eq!(
        pixel(after, LIVE_LAYER, 9, 16),
        COLOR,
        "expansion must underpaint the semitransparent line edge"
    );
    for (x, y) in [(2, 2), (26, 15), (29, 29)] {
        assert_eq!(
            pixel(after, LIVE_LAYER, x, y),
            [0; 4],
            "one-pixel gap closure must prevent outside leakage"
        );
    }
    for (key, tile) in before.iter() {
        assert_eq!(
            after.get(key).unwrap().pixels(),
            tile.pixels(),
            "fill must not modify reference or unrelated signed artwork"
        );
    }
    assert_eq!(
        pixel(after, LINE, 23, 15),
        [0; 4],
        "gap closure must not repair the actual line layer"
    );
    assert_eq!(pixel(after, SENTINEL, -5, 3), [0, 255, 0, 255]);
}

#[test]
#[allow(clippy::too_many_lines)]
fn reference_fill_selection_undo_and_fresh_reopen_preserve_artwork() {
    // Product risk: an empty target ignores reference linework, a selection fills
    // disconnected pixels, or fill/AA/history/reopen corrupts unrelated artwork.
    const REOPEN: &str = "NYATIDRAW_TEST_FLAT_FILL_REOPEN";
    const EXPECTED_ROOT: &str = "NYATIDRAW_TEST_FLAT_FILL_ROOT";
    let (tree, before) = fixture();
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = PathBuf::from(path);
        let (_, actual, reopened_tree, reopened_canvas, _, _) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        assert_eq!(
            format!("{:?}", actual.root()),
            std::env::var(EXPECTED_ROOT).unwrap(),
            "fresh process must reopen the exact committed content root"
        );
        assert_eq!(reopened_tree, tree);
        assert_eq!(reopened_canvas, CANVAS);
        assert_artwork(&before, &actual);
        let page = flatten_layer_tree_rgba8(&actual, &tree, CANVAS).unwrap();
        let saved_png =
            nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
        let rebuilt_png = nyatidraw_png_io::decode_png_bytes(
            &nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
            LIVE_LAYER,
            1024,
        )
        .unwrap();
        assert_eq!(
            saved_png.tiles, rebuilt_png.tiles,
            "saved PNG and reopened artwork must have equal decoded pixels"
        );
        assert_eq!(saved_png.canvas, CANVAS);
        assert!(
            saved_png
                .tiles
                .iter()
                .all(|(key, _)| key.x == 0 && key.y == 0),
            "signed off-page sentinel must not leak into PNG"
        );
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-flat-fill-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let (mut session, _, _, _, mut next_id, sink) =
        open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
    let db = match &sink {
        ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => db,
    };
    db.persist_canvas_spec(CANVAS).unwrap();
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
    next_id += 1;
    let mut selection = Some(Arc::new(
        selection_all([-4, -4], [20, 36], EditLimits::default()).unwrap(),
    ));
    let selected = execute(
        fill_gesture(true),
        &tree,
        &mut selection,
        &mut session,
        &mut next_id,
        db,
    );
    assert!(selected.tiles.is_some());
    assert_eq!(pixel(session.tiles(), LIVE_LAYER, 12, 16), COLOR);
    assert_eq!(
        pixel(session.tiles(), LIVE_LAYER, 17, 16),
        [0; 4],
        "fill expansion cannot cross the selection boundary"
    );
    assert_eq!(
        pixel(session.tiles(), LIVE_LAYER, 5, 5),
        [0; 4],
        "selected but disconnected pixels must remain unchanged"
    );
    assert!(selection.as_ref().unwrap().contains_signed(5, 5));
    let undo = session.prepare_undo_cursor().unwrap();
    let restored = db.load_cursor_tiles(undo.target()).unwrap();
    assert_eq!(restored, before);
    db.persist_history_cursor(undo.target()).unwrap();
    session.accept_history_cursor_move(undo, restored).unwrap();
    selection = None;
    execute(
        fill_gesture(false),
        &tree,
        &mut selection,
        &mut session,
        &mut next_id,
        db,
    );
    let after = session.tiles().clone();
    assert_artwork(&before, &after);
    let head = session.current_snapshot();
    let history = session.history_projection();
    let next_before_selection = next_id;
    let mut previous_count = 0;
    for command in [
        EditCommand::SelectLayerAlpha,
        EditCommand::GrowSelection { radius: 1 },
        EditCommand::ShrinkSelection { radius: 1 },
    ] {
        let grew = matches!(command, EditCommand::GrowSelection { .. });
        let shrank = matches!(command, EditCommand::ShrinkSelection { .. });
        let result = execute(
            command,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        );
        assert!(result.tiles.is_none());
        assert_eq!(
            result.history, history,
            "selection-only changes must not consume artwork history"
        );
        assert_eq!(session.current_snapshot(), head);
        assert_eq!(session.tiles(), &after);
        assert_eq!(next_id, next_before_selection);
        assert!(selection.as_ref().unwrap().contains_signed(12, 16));
        assert!(
            !selection.as_ref().unwrap().contains_signed(-5, 3),
            "alpha selection must use the active layer only"
        );
        let count = selection.as_ref().unwrap().selected_pixels();
        if grew {
            assert!(
                count > previous_count,
                "grow command must enlarge the selection"
            );
        }
        if shrank {
            assert!(
                count < previous_count,
                "shrink command must reduce the selection"
            );
        }
        previous_count = count;
    }
    let undo = session.prepare_undo_cursor().unwrap();
    let restored = db.load_cursor_tiles(undo.target()).unwrap();
    assert_eq!(
        restored, before,
        "one Undo must restore the empty paint layer"
    );
    db.persist_history_cursor(undo.target()).unwrap();
    session.accept_history_cursor_move(undo, restored).unwrap();
    let redo = session
        .prepare_redo_to_cursor(HistoryNodeId(next_before_selection - 1))
        .unwrap();
    let redone = db.load_cursor_tiles(redo.target()).unwrap();
    assert_eq!(redone, after);
    db.persist_history_cursor(redo.target()).unwrap();
    session.accept_history_cursor_move(redo, redone).unwrap();
    let page = flatten_layer_tree_rgba8(&after, &tree, CANVAS).unwrap();
    std::fs::write(
        path.with_extension("png"),
        nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
    )
    .unwrap();
    drop(session);
    drop(sink);
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_canvas::flat_fill_acceptance::reference_fill_selection_undo_and_fresh_reopen_preserve_artwork"])
        .env(REOPEN, &path).env(EXPECTED_ROOT, format!("{:?}", after.root()))
        .status().unwrap().success());
    std::fs::remove_file(path.with_extension("png")).unwrap();
    std::fs::remove_file(path).unwrap();
}

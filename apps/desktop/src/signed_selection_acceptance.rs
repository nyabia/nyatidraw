use super::*;

#[test]
#[allow(clippy::too_many_lines)]
fn signed_selection_edits_preserve_unselected_pixels_and_reopen_exactly() {
    // Product risk: off-page selection algebra/deletion can target the wrong
    // tile or export outside pixels, even when an isolated mask test passes.
    const REOPEN: &str = "NYATIDRAW_TEST_SIGNED_SELECTION_REOPEN";
    let canvas = CanvasSpec {
        width_px: 2,
        height_px: 1,
        pixels_per_inch: 96,
    };
    let tree = default_layer_tree();
    let key = |x| TileKey {
        layer: LIVE_LAYER,
        mip: 0,
        x,
        y: 0,
    };
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let mut negative = vec![0; TILE_BYTE_LEN];
    negative[126 * 4..127 * 4].copy_from_slice(&green);
    negative[127 * 4..128 * 4].copy_from_slice(&red);
    let mut positive = vec![0; TILE_BYTE_LEN];
    positive[..4].copy_from_slice(&red);
    positive[4..8].copy_from_slice(&green);
    positive[8..12].copy_from_slice(&green);
    let expected = TileSnapshot::from_tiles([(key(-1), negative), (key(0), positive)]).unwrap();
    let expected_png = nyatidraw_png_io::encode_png_bytes(
        &flatten_layer_tree_rgba8(&expected, &tree, canvas).unwrap(),
    )
    .unwrap();
    if let Some(path) = std::env::var_os(REOPEN) {
        let (_, actual, reopened_tree, reopened_canvas, _, _) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.into())).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(reopened_tree, tree);
        assert_eq!(reopened_canvas, canvas);
        assert_eq!(
            nyatidraw_png_io::encode_png_bytes(
                &flatten_layer_tree_rgba8(&actual, &tree, canvas).unwrap(),
            )
            .unwrap(),
            expected_png
        );
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-signed-selection-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let (mut session, _, _, _, mut next_id, sink) =
        open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
    let db = match &sink {
        ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => db,
    };
    db.persist_canvas_spec(canvas).unwrap();
    let mut selection = None;
    let rectangle = |left, right| vec![[left, 0], [right, 0], [right, 1], [left, 1]];
    for (command, selected) in [
        (
            EditCommand::SelectLasso {
                vertices: rectangle(-2, 2),
            },
            4,
        ),
        (EditCommand::FillSelection { color: red }, 4),
        (
            EditCommand::CombineLasso {
                vertices: rectangle(-1, 1),
                mode: nyatidraw_api::SelectionMode::Subtract,
            },
            2,
        ),
        (EditCommand::DeleteSelectedPixels, 2),
        (
            EditCommand::CombineLasso {
                vertices: rectangle(2, 3),
                mode: nyatidraw_api::SelectionMode::Add,
            },
            3,
        ),
        (EditCommand::FillSelection { color: green }, 3),
    ] {
        let result = crate::edit_worker::execute(
            command,
            LIVE_LAYER,
            canvas,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        );
        assert!(result.is_ok(), "signed edit rejected");
        assert_eq!(selection.as_ref().unwrap().selected_pixels(), selected);
    }
    assert_eq!(session.tiles(), &expected);
    let durable_id = next_id;
    for command in [
        EditCommand::SelectAll,
        EditCommand::InvertSelection,
        EditCommand::ClearSelection,
    ] {
        assert!(
            crate::edit_worker::execute(
                command,
                LIVE_LAYER,
                canvas,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db
            )
            .is_ok()
        );
        assert_eq!(session.tiles(), &expected);
        assert_eq!(
            next_id, durable_id,
            "selection-only commands must not create artwork history"
        );
    }
    assert!(selection.is_none());
    let undo = session.prepare_undo_cursor().unwrap();
    let previous = db.load_cursor_tiles(undo.target()).unwrap();
    assert_ne!(previous, expected);
    drop(session);
    drop(sink);
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_canvas::signed_selection_acceptance::signed_selection_edits_preserve_unselected_pixels_and_reopen_exactly"])
        .env(REOPEN, &path).status().unwrap().success());
    std::fs::remove_file(path).unwrap();
}

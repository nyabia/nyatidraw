use super::*;
use nyatidraw_api::AffineTransform;
use nyatidraw_paint_cpu::{
    AffineDraft, AffineDraftError, EditLimits, SelectionMask, transform_selection_affine,
};

const OTHER: LayerId = LayerId(91);

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

fn fixture() -> (LayerTree, TileSnapshot, SelectionMask) {
    let mut tree = default_layer_tree();
    let mut other = tree.raster(LIVE_LAYER).unwrap().clone();
    other.id = OTHER;
    other.name = "Unselected reference".into();
    tree.insert(tree.root_id(), 0, LayerTreeNode::Raster(other))
        .unwrap();
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
    for y in -3..1 {
        for x in -4..0 {
            let alpha = if x == -4 || x == -1 || y == -3 || y == 0 {
                64
            } else {
                128
            };
            put(LIVE_LAYER, x, y, [alpha, 0, 0, alpha]);
        }
    }
    put(LIVE_LAYER, 9, 9, [0, 255, 0, 255]);
    put(LIVE_LAYER, -12, 8, [0, 255, 0, 255]);
    put(OTHER, -3, -2, [0, 0, 255, 255]);
    put(OTHER, 10, 10, [0, 0, 128, 128]);
    let selection = SelectionMask::from_packed_bits_at([-4, -3], [4, 4], &[255, 255]).unwrap();
    (tree, TileSnapshot::from_tiles(tiles).unwrap(), selection)
}

fn assert_artwork_contract(before: &TileSnapshot, after: &TileSnapshot, retained: &SelectionMask) {
    assert_eq!(pixel(before, LIVE_LAYER, -3, -2), [128, 0, 0, 128]);
    assert_eq!(
        pixel(after, LIVE_LAYER, -3, -2),
        [0; 4],
        "cut source must clear"
    );
    for (layer, x, y, expected) in [
        (LIVE_LAYER, 9, 9, [0, 255, 0, 255]),
        (LIVE_LAYER, -12, 8, [0, 255, 0, 255]),
        (OTHER, -3, -2, [0, 0, 255, 255]),
        (OTHER, 10, 10, [0, 0, 128, 128]),
    ] {
        assert_eq!(
            pixel(after, layer, x, y),
            expected,
            "unselected artwork changed"
        );
    }
    for (key, tile) in before.iter().filter(|(key, _)| key.layer == OTHER) {
        assert_eq!(after.get(key).unwrap().pixels(), tile.pixels());
    }
    let mut outside = false;
    let mut inside = false;
    let mut antialiased = false;
    for y in 2..11 {
        for x in -7..5 {
            let rgba = pixel(after, LIVE_LAYER, x, y);
            if rgba[3] != 0 {
                assert_eq!(
                    rgba[0], rgba[3],
                    "premultiplied red must not gain a dark fringe"
                );
                assert_eq!([rgba[1], rgba[2]], [0, 0]);
                assert!(
                    rgba[3] <= 128,
                    "interpolation must not increase source opacity"
                );
                assert!(
                    retained.contains_signed(x, y),
                    "resampled edge must remain selected"
                );
                outside |= x < 0;
                inside |= x >= 0;
                antialiased |= ![64, 128].contains(&rgba[3]);
            }
        }
    }
    assert!(
        outside && inside && antialiased,
        "fixture must exercise signed destination and filtered alpha"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn affine_preview_commit_undo_and_fresh_reopen_preserve_artwork() {
    // Product risk: repeated previews accumulate filtering, Cancel commits,
    // signed cut loses unrelated pixels, or durable reopen/export drops AA.
    const REOPEN: &str = "NYATIDRAW_TEST_AFFINE_REOPEN";
    let canvas = CanvasSpec {
        width_px: 12,
        height_px: 12,
        pixels_per_inch: 96,
    };
    let (tree, before, selection) = fixture();
    let chosen = AffineTransform {
        offset_milli: [1_000, 7_000],
        scale_ppm: [1_500_000, 750_000],
        rotation_millidegrees: 27_000,
        ..Default::default()
    };
    let expected = transform_selection_affine(
        &before,
        &tree,
        LIVE_LAYER,
        &selection,
        &chosen,
        EditLimits::default(),
    )
    .unwrap();
    assert_artwork_contract(&before, &expected.paint.after, &expected.selection);
    let expected_page = flatten_layer_tree_rgba8(&expected.paint.after, &tree, canvas).unwrap();
    if let Some(path) = std::env::var_os(REOPEN) {
        let path = PathBuf::from(path);
        let (_, actual, reopened_tree, reopened_canvas, _, _) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        assert_eq!(
            actual, expected.paint.after,
            "fresh process must preserve every signed tile"
        );
        assert_eq!(reopened_tree, tree);
        assert_eq!(reopened_canvas, canvas);
        assert_artwork_contract(&before, &actual, &expected.selection);
        let reopened_page = flatten_layer_tree_rgba8(&actual, &tree, canvas).unwrap();
        assert_eq!(reopened_page, expected_page);
        let saved_png =
            nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
        let reopened_png = nyatidraw_png_io::decode_png_bytes(
            &nyatidraw_png_io::encode_png_bytes(&reopened_page).unwrap(),
            LIVE_LAYER,
            144,
        )
        .unwrap();
        assert_eq!(saved_png.tiles, reopened_png.tiles);
        assert_eq!(saved_png.canvas, canvas);
        assert_eq!(
            flatten_layer_tree_rgba8(&saved_png.tiles, &default_layer_tree(), canvas).unwrap(),
            expected_page
        );
        assert!(
            saved_png
                .tiles
                .iter()
                .all(|(key, _)| key.x == 0 && key.y == 0)
        );
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-affine-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let (mut session, _, _, _, mut next_id, sink) =
        open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
    let db = match &sink {
        ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => db,
    };
    db.persist_canvas_spec(canvas).unwrap();
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
    let before_head = session.current_snapshot();
    let mut canceled =
        AffineDraft::new(before_head, session.tiles(), &tree, LIVE_LAYER, &selection).unwrap();
    canceled.preview(1, &chosen, EditLimits::default()).unwrap();
    drop(canceled);
    assert_eq!(session.tiles(), &before);
    assert_eq!(session.current_snapshot(), before_head);
    let mut draft =
        AffineDraft::new(before_head, session.tiles(), &tree, LIVE_LAYER, &selection).unwrap();
    for (generation, angle) in (1_u64..).zip([27_000, -41_000, 13_000, 27_000]) {
        let preview = draft
            .preview(
                generation,
                &AffineTransform {
                    rotation_millidegrees: angle,
                    ..chosen
                },
                EditLimits::default(),
            )
            .unwrap();
        if angle == chosen.rotation_millidegrees {
            assert_eq!(
                preview.paint.after, expected.paint.after,
                "preview must use immutable source"
            );
            assert_eq!(preview.selection, expected.selection);
        }
        assert_eq!(session.tiles(), &before);
        assert_eq!(session.current_snapshot(), before_head);
    }
    assert!(matches!(
        draft.prepare_commit(
            3,
            before_head,
            session.tiles(),
            &tree,
            LIVE_LAYER,
            &selection
        ),
        Err(AffineDraftError::StaleGeneration)
    ));
    assert!(matches!(
        draft.prepare_commit(
            4,
            before_head,
            &expected.paint.after,
            &tree,
            LIVE_LAYER,
            &selection
        ),
        Err(AffineDraftError::SourceChanged)
    ));
    assert!(matches!(
        draft.prepare_commit(4, before_head, session.tiles(), &tree, OTHER, &selection),
        Err(AffineDraftError::SourceChanged)
    ));
    assert!(
        draft
            .preview(
                5,
                &AffineTransform {
                    scale_ppm: [0, 1_000_000],
                    ..chosen
                },
                EditLimits::default()
            )
            .is_err()
    );
    for generation in [4, 5] {
        assert!(matches!(
            draft.prepare_commit(
                generation,
                before_head,
                session.tiles(),
                &tree,
                LIVE_LAYER,
                &selection
            ),
            Err(AffineDraftError::StaleGeneration)
        ));
    }
    draft.preview(6, &chosen, EditLimits::default()).unwrap();
    let ready = draft
        .prepare_commit(
            6,
            before_head,
            session.tiles(),
            &tree,
            LIVE_LAYER,
            &selection,
        )
        .unwrap()
        .clone();
    assert_eq!(ready.paint.after, expected.paint.after);
    let committed = session
        .prepare_structural_change(
            SnapshotId(next_id),
            HistoryNodeId(next_id),
            2,
            ready.paint.after,
        )
        .unwrap();
    db.commit_structural_with_layer_tree(&committed, &tree)
        .unwrap();
    session.accept_structural_change(&committed).unwrap();
    let retained = ready.selection;
    drop(draft);
    assert!(retained.selected_pixels() > 0);
    let reselected = transform_selection_affine(
        session.tiles(),
        &tree,
        LIVE_LAYER,
        &retained,
        &AffineTransform {
            offset_milli: [2_000, 0],
            ..Default::default()
        },
        EditLimits::default(),
    )
    .unwrap();
    assert_ne!(
        reselected.paint.after,
        *session.tiles(),
        "retained selection must support another move"
    );
    drop(reselected);
    let undo = session.prepare_undo_cursor().unwrap();
    let restored = db.load_cursor_tiles(undo.target()).unwrap();
    assert_eq!(restored, before, "one Undo must undo all preview changes");
    db.persist_history_cursor(undo.target()).unwrap();
    session.accept_history_cursor_move(undo, restored).unwrap();
    let redo = session.prepare_redo_cursor().unwrap();
    let redone = db.load_cursor_tiles(redo.target()).unwrap();
    assert_eq!(redone, expected.paint.after);
    db.persist_history_cursor(redo.target()).unwrap();
    session.accept_history_cursor_move(redo, redone).unwrap();
    std::fs::write(
        path.with_extension("png"),
        nyatidraw_png_io::encode_png_bytes(&expected_page).unwrap(),
    )
    .unwrap();
    drop(session);
    drop(sink);
    assert!(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "native_canvas::affine_acceptance::affine_preview_commit_undo_and_fresh_reopen_preserve_artwork"])
        .env(REOPEN, &path).status().unwrap().success());
    std::fs::remove_file(path.with_extension("png")).unwrap();
    std::fs::remove_file(path).unwrap();
}

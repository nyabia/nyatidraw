//! Durable alpha-lock editing policy through the actual project worker.
use super::*;
use nyatidraw_api::{EditSource, FillSettings, RasterTransform};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask, selection_all};

const PAGE: CanvasSpec = CanvasSpec {
    width_px: 8,
    height_px: 8,
    pixels_per_inch: 96,
};

fn fixture() -> TileSnapshot {
    let mut pixels = vec![0; TILE_BYTE_LEN];
    for (index, alpha) in [0_u8, 1, 64, 128, 254, 255].into_iter().enumerate() {
        let offset = (128 + index) * 4;
        pixels[offset..offset + 4].copy_from_slice(&[alpha, 0, 0, alpha]);
    }
    TileSnapshot::from_tiles(BTreeMap::from([(
        TileKey {
            layer: LIVE_LAYER,
            mip: 0,
            x: 0,
            y: 0,
        },
        pixels,
    )]))
    .unwrap()
}

fn verify_alpha(before: &TileSnapshot, after: &TileSnapshot) {
    for (key, original) in before.iter() {
        let actual = after.get(key).unwrap();
        for (old, new) in original
            .pixels()
            .chunks_exact(4)
            .zip(actual.pixels().chunks_exact(4))
        {
            assert_eq!(old[3], new[3], "alpha lock changed existing coverage");
            assert!(new[..3].iter().all(|channel| *channel <= new[3]));
        }
    }
    assert_eq!(before.len(), after.len());
}

#[test]
#[allow(clippy::too_many_lines)]
fn locked_fill_gradient_and_rejected_destructive_edits_survive_process_restart() {
    // Product risk: an alternate edit path bypasses alpha lock, or saving and
    // replaying lock metadata changes the painted silhouette.
    const CHILD: &str = "NYATIDRAW_ALPHA_EDIT_REOPEN";
    const ROOT: &str = "NYATIDRAW_ALPHA_EDIT_ROOT";
    let original = fixture();
    if let Some(path) = std::env::var_os(CHILD) {
        let path = PathBuf::from(path);
        let (_, actual, tree, canvas, _, _) =
            open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
        assert_eq!(format!("{:?}", actual.root()), std::env::var(ROOT).unwrap());
        assert!(tree.raster(LIVE_LAYER).unwrap().alpha_locked);
        assert_eq!(canvas, PAGE);
        verify_alpha(&original, &actual);
        let flat = flatten_layer_tree_rgba8(&actual, &tree, PAGE).unwrap();
        let expected = nyatidraw_png_io::decode_png_bytes(
            &nyatidraw_png_io::encode_png_bytes(&flat).unwrap(),
            LIVE_LAYER,
            64,
        )
        .unwrap();
        let png = nyatidraw_png_io::decode_png(&path.with_extension("png"), LIVE_LAYER).unwrap();
        assert_eq!(png.tiles, expected.tiles);
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-alpha-edits-{}-{}.ntdr",
        std::process::id(),
        system_timestamp_ns()
    ));
    let (mut session, _, mut tree, _, mut next_id, sink) =
        open_materialization_session(&ProjectLocation::UntitledRecovery(path.clone())).unwrap();
    let db = match &sink {
        ProjectSink::UntitledRecovery { db, .. } | ProjectSink::ExplicitProject { db, .. } => db,
    };
    tree.set_alpha_locked(LIVE_LAYER, true).unwrap();
    db.persist_canvas_spec(PAGE).unwrap();
    let seed = session
        .prepare_structural_change(
            SnapshotId(next_id),
            HistoryNodeId(next_id),
            1,
            original.clone(),
        )
        .unwrap();
    db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
    session.accept_structural_change(&seed).unwrap();
    next_id += 1;
    let mut selection: Option<Arc<SelectionMask>> = Some(Arc::new(
        selection_all([0, 0], [8, 8], EditLimits::default()).unwrap(),
    ));
    for command in [
        EditCommand::ClearActiveLayer,
        EditCommand::DeleteSelectedPixels,
        EditCommand::CutSelection,
        EditCommand::Transform(RasterTransform::default()),
    ] {
        let head = session.current_snapshot();
        assert!(matches!(
            crate::edit_worker::execute(
                command,
                LIVE_LAYER,
                PAGE,
                &tree,
                &mut selection,
                &mut session,
                &mut next_id,
                db
            ),
            Err(crate::edit_worker::EditFailure::Rejected(_))
        ));
        assert_eq!(session.current_snapshot(), head);
        assert_eq!(session.tiles(), &original);
    }
    let mut transform = crate::transform_worker::TransformWorker::default();
    assert!(matches!(
        transform.execute(
            nyatidraw_api::TransformCommand::Begin,
            LIVE_LAYER,
            PAGE,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db
        ),
        Err(crate::edit_worker::EditFailure::Rejected(_))
    ));
    for command in [
        EditCommand::FillSelection {
            color: [0, 255, 0, 255],
        },
        EditCommand::GradientSelection {
            start: [0, 0],
            end: [8, 0],
            start_color: [0, 0, 255, 255],
            end_color: [0, 0, 0, 0],
        },
        EditCommand::FloodFillAdvanced {
            seed: [3, 1],
            tolerance: 255,
            source: EditSource::ActiveLayer,
            color: [128, 0, 128, 128],
            settings: FillSettings::default(),
        },
    ] {
        let before = session.tiles().clone();
        if let Err(error) = crate::edit_worker::execute(
            command,
            LIVE_LAYER,
            PAGE,
            &tree,
            &mut selection,
            &mut session,
            &mut next_id,
            db,
        ) {
            match error {
                crate::edit_worker::EditFailure::Rejected(message)
                | crate::edit_worker::EditFailure::Fatal(message) => {
                    panic!("alpha edit rejected: {message}")
                }
            }
        }
        verify_alpha(&original, session.tiles());
        assert_ne!(
            &before,
            session.tiles(),
            "allowed recolor was silently ignored"
        );
        let after = session.tiles().clone();
        let undo = session.prepare_undo_cursor().unwrap();
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, before);
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        let redo = session.prepare_redo_cursor().unwrap();
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, after);
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
    }
    let final_root = format!("{:?}", session.tiles().root());
    let flat = flatten_layer_tree_rgba8(session.tiles(), &tree, PAGE).unwrap();
    std::fs::write(
        path.with_extension("png"),
        nyatidraw_png_io::encode_png_bytes(&flat).unwrap(),
    )
    .unwrap();
    drop(session);
    drop(sink);
    assert!(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "native_canvas::alpha_lock_acceptance::locked_fill_gradient_and_rejected_destructive_edits_survive_process_restart"])
        .env(CHILD, &path).env(ROOT, final_root).status().unwrap().success());
    std::fs::remove_file(path.with_extension("png")).unwrap();
    std::fs::remove_file(path).unwrap();
}

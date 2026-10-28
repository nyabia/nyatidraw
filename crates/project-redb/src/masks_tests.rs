use super::*;
use nyatidraw_api::{CelId, FrameId, GroupId, LayerBlendMode, LayerId, MaskId};
use nyatidraw_document::{
    AnimationCel, AnimationFrame, AnimationTimeline, GroupNode, LayerMask, LayerNode, LayerTreeNode,
};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey, TILE_BYTE_LEN, TileKey};

fn tree() -> LayerTree {
    let group = |id, layer| GroupNode {
        id: GroupId(id),
        name: "Cel".into(),
        visible: true,
        opacity_u16: u16::MAX,
        blend_mode: LayerBlendMode::Normal,
        clip_to_below: false,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LayerId(layer),
            name: "Image".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            color_effect: None,
            content_root: ContentRootId(0),
            mask: Some(LayerMask {
                id: MaskId(layer),
                enabled: true,
                default_coverage: 255,
            }),
        })],
    };
    LayerTree::new(GroupNode {
        id: GroupId(1),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        blend_mode: LayerBlendMode::Normal,
        clip_to_below: false,
        children: vec![
            LayerTreeNode::Group(group(2, 4)),
            LayerTreeNode::Group(group(3, 5)),
        ],
    })
    .unwrap()
}

fn timeline() -> AnimationTimeline {
    AnimationTimeline::new(
        vec![
            AnimationFrame {
                id: FrameId(1),
                cel: CelId(1),
                duration_ms: 100,
            },
            AnimationFrame {
                id: FrameId(2),
                cel: CelId(2),
                duration_ms: 200,
            },
        ],
        vec![
            AnimationCel {
                id: CelId(1),
                root_group: GroupId(2),
            },
            AnimationCel {
                id: CelId(2),
                root_group: GroupId(3),
            },
        ],
        &tree(),
    )
    .unwrap()
}

fn tiles(value: u8) -> TileSnapshot {
    TileSnapshot::from_tiles([4, 5].map(|id| {
        (
            TileKey::from_pixel(LayerId(id), 128, -1, -1),
            vec![127; TILE_BYTE_LEN],
        )
    }))
    .unwrap()
    .with_coverage_replacements([4, 5].map(|id| {
        (
            CoverageTileKey::from_pixel(MaskId(id), 128, -1, -1),
            vec![if id == 4 { value } else { 77 }; COVERAGE_TILE_BYTE_LEN],
        )
    }))
    .unwrap()
}

#[test]
fn mask_planes_and_inactive_cels_survive_retention_migration_and_fresh_process_reopen() {
    const CHILD: &str = "NYATIDRAW_MASK_REOPEN";
    if let Some(path) = std::env::var_os(CHILD) {
        let db = ProjectDb::open(std::path::Path::new(&path)).unwrap();
        assert_eq!(db.load_layer_tree().unwrap(), Some(tree()));
        assert_eq!(db.load_animation_timeline().unwrap(), Some(timeline()));
        let mut session =
            HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        assert_eq!(session.tiles(), &tiles(130));
        let mut count = 0;
        while let Ok(movement) = session.prepare_undo_cursor() {
            db.persist_history_cursor(movement.target()).unwrap();
            let restored = db.load_cursor_tiles(movement.target()).unwrap();
            count += 1;
            assert_eq!(restored, tiles(130 - count));
            assert_eq!(db.load_layer_tree().unwrap(), Some(tree()));
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
        }
        assert_eq!(usize::from(count), nyatidraw_history::HISTORY_LIMIT);
        while let Ok(movement) = session.prepare_redo_cursor() {
            db.persist_history_cursor(movement.target()).unwrap();
            let restored = db.load_cursor_tiles(movement.target()).unwrap();
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
        }
        assert_eq!(session.tiles(), &tiles(130));
        return;
    }
    let directory = std::env::temp_dir().join(format!(
        "nyatidraw-masks-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("mask.ntdr");
    {
        let db = ProjectDb::open(&path).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        for value in 1..=130_u8 {
            let id = u128::from(value);
            let batch = session
                .prepare_structural_change(SnapshotId(id), HistoryNodeId(id), 0, tiles(value))
                .unwrap();
            db.commit_structural_with_document_metadata(
                &batch,
                &tree(),
                CanvasSpec::DEFAULT,
                Some(&timeline()),
            )
            .unwrap();
            session.accept_structural_change(&batch).unwrap();
        }
    }
    #[cfg(feature = "legacy-migration")]
    {
        let copied = directory.join("copy.ntdr");
        let original = std::fs::read(&path).unwrap();
        migrate_legacy_copy(&path, &copied).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let copy = ProjectDb::open_read_only(&copied).unwrap();
        assert_eq!(
            copy.load_reopened().unwrap().unwrap().current_tiles(),
            &tiles(130)
        );
        drop(copy);
        std::fs::remove_file(copied).unwrap();
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact","masks_tests::mask_planes_and_inactive_cels_survive_retention_migration_and_fresh_process_reopen"])
        .env(CHILD,&path).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn mask_ownership_and_capability_corruption_cannot_publish_or_reopen_partial_artwork() {
    for corruption in 0..3 {
        let memory = MemoryProjectDb::open(&[], 32 * 1024 * 1024).unwrap();
        let db = memory.db();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 0, tiles(0))
            .unwrap();
        db.commit_structural_with_document_metadata(
            &batch,
            &tree(),
            CanvasSpec::DEFAULT,
            Some(&timeline()),
        )
        .unwrap();
        session.accept_structural_change(&batch).unwrap();
        let mut missing_owner = tree();
        missing_owner.set_mask(LayerId(5), None).unwrap();
        let candidate = session
            .prepare_structural_change(SnapshotId(2), HistoryNodeId(2), 0, tiles(8))
            .unwrap();
        assert!(
            db.commit_structural_with_document_metadata(
                &candidate,
                &missing_owner,
                CanvasSpec::DEFAULT,
                Some(&timeline())
            )
            .is_err()
        );
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles(),
            &tiles(0)
        );
        let transaction = db.db.begin_write().unwrap();
        if corruption == 0 {
            let mut meta = transaction.open_table(META).unwrap();
            let marker = meta.get("schema_version").unwrap().unwrap().value();
            meta.insert(
                "schema_version",
                marker & !nyatidraw_project::MASK_SCHEMA_FLAG,
            )
            .unwrap();
        } else {
            let hash = tiles(0).coverage_iter().next().unwrap().1.hash();
            let mut objects = transaction.open_table(OBJECTS).unwrap();
            if corruption == 1 {
                objects.remove(hash.0.as_slice()).unwrap();
            } else {
                let bytes = Envelope::new(RecordKind::Tile, vec![0; TILE_BYTE_LEN]).encode();
                objects.insert(hash.0.as_slice(), bytes.as_slice()).unwrap();
            }
        }
        transaction.commit().unwrap();
        assert!(db.validate_existing().is_err());
    }
}

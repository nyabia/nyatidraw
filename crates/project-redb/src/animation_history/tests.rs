use super::*;
use nyatidraw_api::{ContentRootId, GroupId, HistoryNodeId, LayerBlendMode, LayerId};
use nyatidraw_document::{
    AnimationCel, AnimationFrame, CelId, FrameId, GroupNode, LayerNode, LayerTreeNode,
};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::path::Path;

fn tree() -> LayerTree {
    let group = |id, layer| GroupNode {
        id: GroupId(id),
        name: format!("Cel {id}"),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: vec![LayerTreeNode::Raster(LayerNode {
            mask: None,
            id: LayerId(layer),
            name: "Paint".into(),
            visible: true,
            locked: false,
            opacity_u16: u16::MAX,
            reference: false,
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            color_effect: None,
            content_root: ContentRootId(0),
        })],
    };
    LayerTree::new(GroupNode {
        id: GroupId(1),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: vec![
            LayerTreeNode::Group(group(2, 4)),
            LayerTreeNode::Group(group(3, 5)),
        ],
    })
    .unwrap()
}

fn timeline(duration_ms: u32) -> AnimationTimeline {
    AnimationTimeline::new(
        vec![
            AnimationFrame {
                id: FrameId(1),
                cel: CelId(1),
                duration_ms,
            },
            AnimationFrame {
                id: FrameId(2),
                cel: CelId(2),
                duration_ms: 100,
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

fn tiles() -> TileSnapshot {
    TileSnapshot::from_tiles([4, 5].map(|layer| {
        let mut pixels = vec![0; TILE_BYTE_LEN];
        pixels[..4].copy_from_slice(&[100, 0, 0, 255]);
        (
            TileKey {
                layer: LayerId(layer),
                mip: 0,
                x: 0,
                y: 0,
            },
            pixels,
        )
    }))
    .unwrap()
}

fn commit(db: &ProjectDb, session: &mut HeadlessStrokeSession, id: u128, duration: u32) {
    let batch = session
        .prepare_structural_change(SnapshotId(id), HistoryNodeId(id), 0, tiles())
        .unwrap();
    db.commit_structural_with_document_metadata(
        &batch,
        &tree(),
        nyatidraw_api::CanvasSpec::DEFAULT,
        Some(&timeline(duration)),
    )
    .unwrap();
    session.accept_structural_change(&batch).unwrap();
}

#[cfg(feature = "legacy-migration")]
#[test]
fn migration_preserves_animated_redo_and_reader_guard_after_undo_to_static() {
    let directory = std::env::temp_dir().join(format!(
        "nyatidraw-animation-migration-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).unwrap();
    let source = directory.join("source.ntdr");
    let destination = directory.join("migrated.ntdr");
    {
        let db = ProjectDb::open(&source).unwrap();
        db.persist_layer_tree(&tree()).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        commit(&db, &mut session, 1, 100);
        commit(&db, &mut session, 2, 200);
        while let Ok(movement) = session.prepare_undo_cursor() {
            db.persist_history_cursor(movement.target()).unwrap();
            let restored = db.load_cursor_tiles(movement.target()).unwrap();
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
        }
        assert_eq!(db.load_animation_timeline().unwrap(), None);
    }
    let original = std::fs::read(&source).unwrap();
    crate::migration::migrate_legacy_copy(&source, &destination).unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), original);
    {
        let db = ProjectDb::open(&destination).unwrap();
        assert_eq!(db.load_animation_timeline().unwrap(), None);
        assert_eq!(db.load_layer_tree().unwrap(), Some(tree()));
        let marker = {
            let transaction = db.db.begin_read().unwrap();
            transaction
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value()
        };
        assert_ne!(marker & ANIMATION_SCHEMA_FLAG, 0);
        let previous_reader_flags = super::super::SCHEMA_CAPABILITY_FLAGS & !ANIMATION_SCHEMA_FLAG;
        assert!(!(1..=4).contains(&(marker & !previous_reader_flags)));
        let mut session =
            HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        assert_eq!(session.tiles(), &TileSnapshot::empty());
        for duration in [100, 200] {
            let movement = session.prepare_redo_cursor().unwrap();
            db.persist_history_cursor(movement.target()).unwrap();
            let restored = db.load_cursor_tiles(movement.target()).unwrap();
            assert_eq!(restored, tiles());
            assert_eq!(
                db.load_animation_timeline().unwrap(),
                Some(timeline(duration))
            );
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
        }
        assert!(session.prepare_redo_cursor().is_err());
    }
    let db = ProjectDb::open_read_only(&destination).unwrap();
    assert_eq!(db.load_animation_timeline().unwrap(), Some(timeline(200)));
    assert_eq!(
        db.load_reopened().unwrap().unwrap().current_tiles(),
        &tiles()
    );
    drop(db);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(destination).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn animation_metadata_undo_retention_and_fresh_process_reopen_preserve_all_cels() {
    const CHILD: &str = "NYATIDRAW_ANIMATION_REOPEN_TEST";
    if let Some(path) = std::env::var_os(CHILD) {
        let db = ProjectDb::open(Path::new(&path)).unwrap();
        assert_eq!(db.load_animation_timeline().unwrap(), Some(timeline(140)));
        let mut session =
            HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        let mut moves = 0;
        while let Ok(movement) = session.prepare_undo_cursor() {
            db.persist_history_cursor(movement.target()).unwrap();
            let restored = db.load_cursor_tiles(movement.target()).unwrap();
            assert_eq!(restored, tiles());
            let restored_timeline = db.load_animation_timeline().unwrap().unwrap();
            assert_eq!(restored_timeline.cels(), timeline(100).cels());
            session
                .accept_history_cursor_move(movement, restored)
                .unwrap();
            moves += 1;
        }
        assert_eq!(moves, nyatidraw_history::HISTORY_LIMIT);
        while let Ok(movement) = session.prepare_redo_cursor() {
            db.persist_history_cursor(movement.target()).unwrap();
            session
                .accept_history_cursor_move(movement, tiles())
                .unwrap();
        }
        assert_eq!(db.load_animation_timeline().unwrap(), Some(timeline(140)));
        return;
    }
    let path = std::env::temp_dir().join(format!(
        "nyatidraw-animation-{}-{}.ntdr",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let db = ProjectDb::open(&path).unwrap();
        db.persist_layer_tree(&tree()).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        for id in 1..=140 {
            commit(&db, &mut session, id, u32::try_from(id).unwrap());
        }
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles(),
            &tiles()
        );
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "animation_history::tests::animation_metadata_undo_retention_and_fresh_process_reopen_preserve_all_cels", "--nocapture"])
        .env(CHILD, &path).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn animation_schema_and_invalid_metadata_fail_closed_without_rewriting_artwork() {
    for corruption in 0..4 {
        let memory = super::super::MemoryProjectDb::open(&[], 16 * 1024 * 1024).unwrap();
        let db = memory.db();
        db.persist_layer_tree(&tree()).unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        commit(db, &mut session, 1, 100);
        let first = db
            .load_reopened()
            .unwrap()
            .unwrap()
            .into_parts()
            .current_cursor;
        commit(db, &mut session, 2, 200);
        assert_eq!(session.tiles(), &tiles());
        let undo = session.prepare_undo_cursor().unwrap();
        db.persist_history_cursor(undo.target()).unwrap();
        assert_eq!(db.load_animation_timeline().unwrap(), Some(timeline(100)));
        db.persist_history_cursor(first).unwrap();
        let marker = {
            let transaction = db.db.begin_read().unwrap();
            transaction
                .open_table(META)
                .unwrap()
                .get("schema_version")
                .unwrap()
                .unwrap()
                .value()
        };
        let previous_reader_flags = super::super::SCHEMA_CAPABILITY_FLAGS & !ANIMATION_SCHEMA_FLAG;
        assert!(!(1..=4).contains(&(marker & !previous_reader_flags)));
        let before = db.load_reopened().unwrap().unwrap().current_tiles().root();
        let mut wrong_tree = tree().root().clone();
        wrong_tree.children.remove(1);
        let wrong_tree = LayerTree::new(wrong_tree).unwrap();
        let reopened = HeadlessStrokeSession::from_reopened(db.load_reopened().unwrap().unwrap());
        let invalid = reopened
            .prepare_structural_change(SnapshotId(3), HistoryNodeId(3), 0, tiles())
            .unwrap();
        assert!(
            db.commit_structural_with_layer_tree(&invalid, &wrong_tree)
                .is_err()
        );
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles().root(),
            before
        );
        assert_eq!(db.load_layer_tree().unwrap(), Some(tree()));
        let transaction = db.db.begin_write().unwrap();
        match corruption {
            0 => {
                transaction
                    .open_table(STATE)
                    .unwrap()
                    .remove(CURRENT_ANIMATION)
                    .unwrap();
            }
            1 => {
                transaction
                    .open_table(SNAPSHOT_ANIMATION)
                    .unwrap()
                    .remove(first.snapshot_id.0.to_le_bytes().as_slice())
                    .unwrap();
            }
            2 => {
                transaction
                    .open_table(META)
                    .unwrap()
                    .insert("schema_version", marker & !ANIMATION_SCHEMA_FLAG)
                    .unwrap();
            }
            _ => {
                let mut payload = encode_animation_timeline(Some(&timeline(100))).unwrap();
                payload[39..43].copy_from_slice(&0_u32.to_le_bytes());
                let bytes = Envelope::new(RecordKind::AnimationTimeline, payload).encode();
                transaction
                    .open_table(SNAPSHOT_ANIMATION)
                    .unwrap()
                    .insert(
                        first.snapshot_id.0.to_le_bytes().as_slice(),
                        bytes.as_slice(),
                    )
                    .unwrap();
            }
        }
        transaction.commit().unwrap();
        let bytes = memory.into_bytes().unwrap();
        let preserved = bytes.clone();
        assert!(super::super::MemoryProjectDb::open(&bytes, 16 * 1024 * 1024).is_err());
        assert_eq!(bytes, preserved);
    }
}

#[test]
fn painting_an_animated_cel_inherits_timeline_and_preserves_inactive_cel_after_reopen() {
    use nyatidraw_brush::{
        BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, RoundBrushEvaluator,
        begin_round_stroke,
    };
    use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
    let memory = super::super::MemoryProjectDb::open(&[], 16 * 1024 * 1024).unwrap();
    let db = memory.db();
    db.persist_layer_tree(&tree()).unwrap();
    let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
    commit(db, &mut session, 1, 100);
    let preset = BrushPreset {
        id: BrushPresetId(1),
        schema_version: 1,
        engine_version: 1,
        size_px: 4.0,
        opacity: 1.0,
        flow: 1.0,
        spacing_ratio: 0.2,
        size_pressure: false,
        opacity_pressure: false,
        size_min_ratio: 0.0,
        opacity_min_ratio: 0.0,
        hardness: 1.0,
    };
    let samples: Vec<_> = [PointerPhase::Begin, PointerPhase::End]
        .into_iter()
        .enumerate()
        .map(|(index, phase)| StylusSample {
            sequence: index as u64 + 1,
            timestamp_ns: index as u64 + 1,
            device_id: 1,
            phase,
            position_document: Point { x: 8.0, y: 8.0 },
            pressure: 1.0,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        })
        .collect();
    let mut evaluator = RoundBrushEvaluator::new(1);
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
    evaluator.push(&mut token, &samples[1..], &mut dabs);
    let recorded = evaluator.end(token, &mut dabs);
    let batch = session
        .prepare_round_stroke(
            SnapshotId(2),
            HistoryNodeId(2),
            10,
            LayerId(4),
            BrushSnapshot { preset },
            recorded,
            nyatidraw_stroke::StrokeColor::new([0, 255, 0, 255]).unwrap(),
            samples,
        )
        .unwrap();
    db.commit(&batch).unwrap();
    session.accept_committed(&batch).unwrap();
    let painted = session.tiles().clone();
    assert_ne!(painted, tiles());
    let inactive = TileKey {
        layer: LayerId(5),
        mip: 0,
        x: 0,
        y: 0,
    };
    assert_eq!(painted.get(inactive), tiles().get(inactive));
    let reopened =
        super::super::MemoryProjectDb::open(&memory.into_bytes().unwrap(), 16 * 1024 * 1024)
            .unwrap();
    assert_eq!(
        reopened.db().load_animation_timeline().unwrap(),
        Some(timeline(100))
    );
    assert_eq!(
        reopened
            .db()
            .load_reopened()
            .unwrap()
            .unwrap()
            .current_tiles(),
        &painted
    );
    let source = tree();
    let display = timeline(100)
        .resolve_frame_tree(&source, FrameId(2))
        .unwrap();
    assert_eq!(display.root().children.len(), 1);
    assert_eq!(source, tree());
}

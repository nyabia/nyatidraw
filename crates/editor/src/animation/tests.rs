use super::*;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

#[test]
#[allow(clippy::too_many_lines)]
fn frame_edits_preserve_inactive_artwork_and_metadata_or_fail_without_mutation() {
    let canvas = CanvasSpec {
        width_px: 64,
        height_px: 64,
        pixels_per_inch: 96,
    };
    let original_tree = LayerTree::new(cel_group(
        GroupId(0),
        vec![
            LayerTreeNode::Raster(empty_raster(LayerId(1), "선화".into())),
            LayerTreeNode::Raster(empty_raster(LayerId(2), "밑색".into())),
        ],
    ))
    .unwrap();
    let key = TileKey {
        layer: LayerId(1),
        mip: 0,
        x: -1,
        y: 0,
    };
    let original_tiles =
        TileSnapshot::from_tiles([(key, [20, 40, 60, 128].repeat(TILE_BYTE_LEN / 4))]).unwrap();
    let enabled = prepare_animation_edit(
        AnimationCommand::Enable,
        &original_tree,
        &original_tiles,
        None,
        None,
        LayerId(1),
        canvas,
        3,
    )
    .unwrap();
    let first = enabled.active_frame.unwrap();
    assert_eq!(enabled.tiles.root(), original_tiles.root());
    assert_eq!(
        enabled.tiles.get(key).unwrap().pixels(),
        original_tiles.get(key).unwrap().pixels()
    );
    assert_eq!(
        active_edit_tree(&enabled.tree, enabled.timeline.as_ref(), Some(first))
            .unwrap()
            .root()
            .children,
        original_tree.root().children
    );

    let copied = prepare_animation_edit(
        AnimationCommand::Duplicate(first),
        &enabled.tree,
        &enabled.tiles,
        enabled.timeline.as_ref(),
        Some(first),
        enabled.active_layer,
        canvas,
        1,
    )
    .unwrap();
    let second = copied.active_frame.unwrap();
    let second_tree =
        active_edit_tree(&copied.tree, copied.timeline.as_ref(), Some(second)).unwrap();
    let copy_layers = raster_layer_ids(&second_tree);
    let copied_key = copied
        .tiles
        .iter()
        .find(|(key, _)| copy_layers.contains(&key.layer))
        .unwrap()
        .0;
    assert_eq!(copied_key.x, -1);
    assert_eq!(
        copied.tiles.get(copied_key).unwrap().pixels(),
        original_tiles.get(key).unwrap().pixels()
    );
    let changed = copied
        .tiles
        .with_replacements([(copied_key, [0, 0, 200, 255].repeat(TILE_BYTE_LEN / 4))])
        .unwrap();
    assert_eq!(changed.get(key), original_tiles.get(key));
    assert_ne!(changed.get(copied_key), copied.tiles.get(copied_key));

    let mut edited = second_tree.clone();
    edited
        .rename(LayerTreeNodeId::Raster(copy_layers[0]), "복사본")
        .unwrap();
    let merged = merge_active_edit_tree(
        &copied.tree,
        copied.timeline.as_ref(),
        Some(second),
        &edited,
    )
    .unwrap();
    assert_eq!(
        active_edit_tree(&merged, copied.timeline.as_ref(), Some(first)).unwrap(),
        active_edit_tree(&copied.tree, copied.timeline.as_ref(), Some(first)).unwrap()
    );
    assert!(
        merge_active_edit_tree(&copied.tree, copied.timeline.as_ref(), Some(first), &edited)
            .is_err()
    );

    for command in [
        AnimationCommand::Select(first),
        AnimationCommand::SetDuration {
            frame: first,
            duration_ms: 250,
        },
        AnimationCommand::Move {
            frame: second,
            index: 0,
        },
    ] {
        let result = prepare_animation_edit(
            command,
            &copied.tree,
            &copied.tiles,
            copied.timeline.as_ref(),
            Some(second),
            copied.active_layer,
            canvas,
            1,
        )
        .unwrap();
        assert_eq!(result.tiles, copied.tiles);
        assert_eq!(result.tree, copied.tree);
        assert_eq!(
            result.artwork_changed,
            !matches!(command, AnimationCommand::Select(_))
        );
    }
    for command in [
        AnimationCommand::SetDuration {
            frame: first,
            duration_ms: 0,
        },
        AnimationCommand::SetDuration {
            frame: first,
            duration_ms: 60_001,
        },
        AnimationCommand::Select(FrameId(u128::MAX)),
        AnimationCommand::Move {
            frame: second,
            index: 2,
        },
    ] {
        assert!(
            prepare_animation_edit(
                command,
                &copied.tree,
                &copied.tiles,
                copied.timeline.as_ref(),
                Some(second),
                copied.active_layer,
                canvas,
                1
            )
            .is_err()
        );
    }
    let deleted = prepare_animation_edit(
        AnimationCommand::Delete(second),
        &copied.tree,
        &copied.tiles,
        copied.timeline.as_ref(),
        Some(second),
        copied.active_layer,
        canvas,
        1,
    )
    .unwrap();
    assert_eq!(deleted.tiles, original_tiles);
    assert_eq!(deleted.active_frame, Some(first));
    assert!(
        prepare_animation_edit(
            AnimationCommand::Delete(first),
            &deleted.tree,
            &deleted.tiles,
            deleted.timeline.as_ref(),
            deleted.active_frame,
            deleted.active_layer,
            canvas,
            1
        )
        .is_err()
    );
}

#[test]
fn playback_skips_late_ticks_by_duration_without_mutating_artwork() {
    let mut root = cel_group(GroupId(0), Vec::new());
    for id in 1..=3 {
        root.children
            .push(LayerTreeNode::Group(cel_group(GroupId(id), Vec::new())));
    }
    let tree = LayerTree::new(root).unwrap();
    let timeline = AnimationTimeline::new(
        vec![
            AnimationFrame {
                id: FrameId(1),
                cel: CelId(1),
                duration_ms: 100,
            },
            AnimationFrame {
                id: FrameId(2),
                cel: CelId(2),
                duration_ms: 250,
            },
            AnimationFrame {
                id: FrameId(3),
                cel: CelId(3),
                duration_ms: 50,
            },
        ],
        (1..=3)
            .map(|id| AnimationCel {
                id: CelId(id),
                root_group: GroupId(id),
            })
            .collect(),
        &tree,
    )
    .unwrap();
    for (elapsed, looping, expected, finished) in [
        (0, true, 2, false),
        (249, true, 2, false),
        (250, true, 3, false),
        (300, true, 1, false),
        (400, true, 2, false),
        (1_000_001, true, 2, false),
        (300, false, 3, true),
    ] {
        assert_eq!(
            playback_frame(&timeline, FrameId(2), elapsed, looping).unwrap(),
            (FrameId(expected), finished)
        );
    }
    assert!(playback_frame(&timeline, FrameId(9), 0, true).is_err());
}

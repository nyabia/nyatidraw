use nyatidraw_api::{CompositeTileKey, ContentRootId, GroupId, LayerId, LayerTreeNodeId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeError, LayerTreeNode};
use nyatidraw_tiles::{CompositeCache, TILE_EDGE, TileKey};

fn raster(id: u128) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: LayerId(id),
        name: format!("Layer {id}"),
        visible: true,
        locked: false,
        reference: false,
        opacity_u16: u16::MAX,
        content_root: ContentRootId(id),
    })
}

fn fixture() -> LayerTree {
    let lower = GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: GroupId(10),
        name: "Lower ten".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: (1..=10).map(raster).collect(),
    };
    let upper = GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: GroupId(20),
        name: "Upper ten".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: (11..=20).map(raster).collect(),
    };
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: GroupId(1),
        name: "Document root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Group(lower), LayerTreeNode::Group(upper)],
    })
    .expect("20-layer fixture is valid")
}

#[test]
fn one_4k_tile_edit_invalidates_only_ancestor_composites_at_that_coordinate() {
    let mut tree = fixture();
    let changed_layer = LayerId(5);
    let changed_tile = TileKey::from_pixel(
        changed_layer,
        i32::try_from(TILE_EDGE).expect("tile edge fits i32"),
        3_839,
        2_159,
    )
    .coordinate();
    let other_tile = TileKey::from_pixel(changed_layer, 128, 0, 0).coordinate();
    let key = |group, tile| CompositeTileKey { group, tile };

    let mut cache = CompositeCache::default();
    for group in [GroupId(1), GroupId(10), GroupId(20)] {
        cache.insert(key(group, changed_tile), "changed-coordinate");
        cache.insert(key(group, other_tile), "other-coordinate");
    }

    let pixel_edit = tree
        .invalidate_raster_tile(changed_layer, changed_tile)
        .expect("changed layer exists");
    assert_eq!(
        cache.apply_invalidation(&pixel_edit),
        2,
        "only the changed layer's parent and root composites are stale"
    );
    assert!(cache.get(key(GroupId(1), changed_tile)).is_none());
    assert!(cache.get(key(GroupId(10), changed_tile)).is_none());
    assert!(cache.get(key(GroupId(20), changed_tile)).is_some());
    assert!(cache.get(key(GroupId(1), other_tile)).is_some());
    assert!(cache.get(key(GroupId(10), other_tile)).is_some());

    let visibility_edit = tree
        .set_visibility(LayerTreeNodeId::Raster(changed_layer), false)
        .expect("visibility edit succeeds");
    assert_eq!(
        cache.apply_invalidation(&visibility_edit),
        2,
        "a structural property change clears every cached coordinate only above the node"
    );
    assert!(cache.get(key(GroupId(20), changed_tile)).is_some());
    assert!(cache.get(key(GroupId(20), other_tile)).is_some());

    let before_rejected_move = tree.clone();
    assert_eq!(
        tree.reorder(LayerTreeNodeId::Group(GroupId(10)), GroupId(10), 0),
        Err(LayerTreeError::MoveIntoDescendant)
    );
    assert_eq!(
        tree, before_rejected_move,
        "a rejected group cycle must not partially detach artwork"
    );

    let move_edit = tree
        .reorder(LayerTreeNodeId::Raster(changed_layer), GroupId(20), 0)
        .expect("cross-group reorder succeeds");
    assert_eq!(
        tree.ancestors(LayerTreeNodeId::Raster(changed_layer)),
        Some(vec![GroupId(20), GroupId(1)])
    );
    cache.insert(key(GroupId(1), changed_tile), "root");
    cache.insert(key(GroupId(10), changed_tile), "old-parent");
    assert_eq!(
        cache.apply_invalidation(&move_edit),
        4,
        "reorder clears cached coordinates above both old and new parents"
    );
    assert!(cache.is_empty());
}

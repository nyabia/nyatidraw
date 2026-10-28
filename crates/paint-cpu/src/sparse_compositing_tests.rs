use super::*;
use nyatidraw_api::{ContentRootId, GroupId, LayerColorEffect};
use nyatidraw_document::{LayerNode, LayerTree};

fn raster(id: u128, clipped: bool, mode: LayerBlendMode) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        mask: None,
        id: LayerId(id),
        name: id.to_string(),
        visible: true,
        locked: false,
        alpha_locked: false,
        clip_to_below: clipped,
        reference: false,
        opacity_u16: 40_000,
        blend_mode: mode,
        content_root: ContentRootId(0),
        color_effect: (id == 1).then(LayerColorEffect::default),
    })
}

fn group(id: u128, children: Vec<LayerTreeNode>) -> GroupNode {
    GroupNode {
        id: GroupId(id),
        name: id.to_string(),
        visible: true,
        clip_to_below: false,
        opacity_u16: u16::MAX,
        blend_mode: LayerBlendMode::Normal,
        children,
    }
}

#[test]
fn skipping_absent_tiles_never_reveals_clipped_pixels_or_changes_export_colors() {
    let snapshot = TileSnapshot::from_tiles([(1, 0), (2, 0), (2, 1), (3, 1), (4, 2), (4, -1)].map(
        |(layer, x)| {
            (
                TileKey {
                    layer: LayerId(layer),
                    mip: 0,
                    x,
                    y: 0,
                },
                [40, 20, 80, 128].repeat(TILE_BYTE_LEN / 4),
            )
        },
    ))
    .unwrap();
    let original = snapshot.clone();
    let canvas = CanvasSpec {
        width_px: 384,
        height_px: 2,
        pixels_per_inch: 96,
    };
    for mode in [
        LayerBlendMode::Normal,
        LayerBlendMode::Multiply,
        LayerBlendMode::Screen,
        LayerBlendMode::Add,
        LayerBlendMode::Overlay,
    ] {
        let mut nested = group(101, vec![raster(3, false, mode), raster(4, true, mode)]);
        nested.opacity_u16 = 32_000;
        let mut tree = LayerTree::new(group(
            100,
            vec![
                raster(1, false, mode),
                raster(2, true, mode),
                LayerTreeNode::Group(nested),
                LayerTreeNode::Group(group(
                    102,
                    vec![raster(5, false, mode), raster(6, true, mode)],
                )),
            ],
        ))
        .unwrap();
        for hidden in [false, true] {
            tree.set_visibility(LayerTreeNodeId::Raster(LayerId(1)), !hidden)
                .unwrap();
            let actual = crate::flatten_layer_tree_rgba8(&snapshot, &tree, canvas).unwrap();
            for y in 0..canvas.height_px {
                for x in 0..canvas.width_px {
                    let index = (y * canvas.width_px + x) as usize * 4;
                    assert_eq!(
                        actual.pixels[index..index + 4],
                        group_pixel(
                            &snapshot,
                            tree.root(),
                            [x.cast_signed(), y.cast_signed()],
                            None
                        ),
                        "sparse export differs from point composition at ({x}, {y}), {mode:?}, hidden={hidden}",
                    );
                }
            }
            assert!(
                actual.pixels[256 * 4..384 * 4]
                    .iter()
                    .all(|&byte| byte == 0)
            );
        }
    }
    assert_eq!(snapshot, original);
}

use super::*;
use crate::{EditLimits, PremultipliedRgba8, SelectionPaint, SelectionSource};
use nyatidraw_api::{ContentRootId, FillSettings, GroupId};
use nyatidraw_document::{LayerNode, LayerTree};

fn raster(id: u128, clip: bool, opacity: u16, mode: LayerBlendMode) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        id: LayerId(id),
        name: id.to_string(),
        visible: true,
        locked: false,
        alpha_locked: false,
        clip_to_below: clip,
        reference: id == 4,
        blend_mode: mode,
        opacity_u16: opacity,
        content_root: ContentRootId(0),
    })
}

fn group(id: u128, children: Vec<LayerTreeNode>) -> GroupNode {
    GroupNode {
        id: GroupId(id),
        name: id.to_string(),
        visible: true,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        opacity_u16: u16::MAX,
        children,
    }
}

#[test]
fn byte_blends_preserve_transparent_backdrops_and_atop_alpha() {
    // Product risk: a shortcut Multiply blend drops transparent-backdrop ink;
    // rounding or double clipping weakens the silhouette's antialiased alpha.
    let source = [64, 32, 16, 128];
    for (opacity, mode, atop, expected) in [
        (65535, LayerBlendMode::Normal, false, [74, 72, 36, 192]),
        (65535, LayerBlendMode::Multiply, false, [47, 66, 30, 192]),
        (65535, LayerBlendMode::Normal, true, [42, 56, 28, 128]),
        (65535, LayerBlendMode::Multiply, true, [15, 50, 22, 128]),
        (32768, LayerBlendMode::Normal, false, [47, 76, 38, 160]),
        (32768, LayerBlendMode::Multiply, false, [33, 73, 35, 160]),
        (32768, LayerBlendMode::Normal, true, [31, 68, 34, 128]),
        (32768, LayerBlendMode::Multiply, true, [17, 65, 31, 128]),
    ] {
        let mut destination = [20, 80, 40, 128];
        blend_pixel(&mut destination, &source, opacity, mode, atop);
        assert_eq!(destination, expected, "{opacity} {mode:?} atop={atop}");
    }
    for mode in [LayerBlendMode::Normal, LayerBlendMode::Multiply] {
        let mut empty = [0; 4];
        blend_pixel(&mut empty, &source, u16::MAX, mode, false);
        assert_eq!(empty, source);
        let mut destination = [20, 80, 40, 128];
        blend_pixel(&mut destination, &[0; 4], u16::MAX, mode, true);
        assert_eq!(destination, [20, 80, 40, 128]);
        let mut empty = [0; 4];
        blend_pixel(&mut empty, &source, u16::MAX, mode, true);
        assert_eq!(empty, [0; 4]);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn stack_composite_preview_picker_and_reference_preserve_one_base_alpha() {
    // Product risk: consecutive clips must share one raw base alpha; groups,
    // Solo/reference dependency filtering and export must not invent new bases.
    let snapshot = TileSnapshot::from_tiles([(-1, 0), (0, 0)].into_iter().flat_map(|(x, y)| {
        [
            (1, [100, 100, 100, 255]),
            (2, [128, 0, 0, 128]),
            (3, [0, 128, 0, 128]),
            (4, [0, 0, 128, 128]),
            (5, [255; 4]),
        ]
        .map(move |(id, pixel)| {
            (
                TileKey {
                    layer: LayerId(id),
                    mip: 0,
                    x,
                    y,
                },
                pixel.repeat(TILE_BYTE_LEN / 4),
            )
        })
    }))
    .unwrap();
    let canvas = CanvasSpec {
        width_px: 2,
        height_px: 1,
        pixels_per_inch: 96,
    };
    for grouped in [false, true] {
        let (base, clip, base_id) = if grouped {
            let mut base = group(20, vec![raster(2, false, u16::MAX, LayerBlendMode::Normal)]);
            base.opacity_u16 = 32768;
            base.blend_mode = LayerBlendMode::Multiply;
            let mut clip = group(30, vec![raster(3, false, u16::MAX, LayerBlendMode::Normal)]);
            clip.clip_to_below = true;
            (
                LayerTreeNode::Group(base),
                LayerTreeNode::Group(clip),
                LayerTreeNodeId::Group(GroupId(20)),
            )
        } else {
            (
                raster(2, false, 32768, LayerBlendMode::Multiply),
                raster(3, true, u16::MAX, LayerBlendMode::Normal),
                LayerTreeNodeId::Raster(LayerId(2)),
            )
        };
        let mut tree = LayerTree::new(group(
            100,
            vec![
                raster(5, true, u16::MAX, LayerBlendMode::Normal), // Orphan cannot leak white.
                raster(1, false, u16::MAX, LayerBlendMode::Normal),
                base,
                clip,
                raster(4, true, u16::MAX, LayerBlendMode::Normal),
            ],
        ))
        .unwrap();
        let flat = crate::flatten_layer_tree_rgba8(&snapshot, &tree, canvas).unwrap();
        assert_eq!(flat.pixels, [81, 81, 87, 255].repeat(2));
        assert_eq!(
            crate::render_page_preview_rgba8(&snapshot, &tree, canvas, 2, 1).unwrap(),
            flat
        );
        for point in [[0, 0], [-128, 0]] {
            assert_eq!(
                crate::sample_artwork_pixel(
                    &snapshot,
                    &tree,
                    LayerId(2),
                    point,
                    SelectionSource::AllVisible
                )
                .unwrap(),
                [81, 81, 87, 255]
            );
            assert_eq!(
                crate::sample_display_pixel(
                    &snapshot,
                    &tree,
                    point,
                    Some(LayerTreeNodeId::Raster(LayerId(4)))
                )
                .unwrap(),
                [32, 0, 32, 64]
            );
            assert_eq!(
                crate::sample_artwork_pixel(
                    &snapshot,
                    &tree,
                    LayerId(2),
                    point,
                    SelectionSource::ReferenceLayers
                )
                .unwrap(),
                [32, 0, 32, 64]
            );
        }
        let raw =
            crate::render_raster_page_preview_rgba8(&snapshot, LayerId(2), canvas, 2, 1).unwrap();
        assert_eq!(raw.pixels, [128, 0, 0, 128].repeat(2));
        let mut reference = tree.root().clone();
        assert_eq!(
            crate::selection::restrict_references(&mut reference, true),
            1
        );
        assert_eq!(
            crate::flatten_layer_tree_rgba8(&snapshot, &LayerTree::new(reference).unwrap(), canvas)
                .unwrap()
                .pixels,
            [32, 0, 32, 64].repeat(2)
        );
        tree.set_visibility(base_id, false).unwrap();
        assert_eq!(
            crate::flatten_layer_tree_rgba8(&snapshot, &tree, canvas)
                .unwrap()
                .pixels,
            [100, 100, 100, 255].repeat(2)
        );
        assert_eq!(
            crate::sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(2),
                [0, 0],
                SelectionSource::ReferenceLayers
            )
            .unwrap(),
            [0; 4]
        );
        assert_eq!(
            crate::sample_display_pixel(
                &snapshot,
                &tree,
                [0, 0],
                Some(LayerTreeNodeId::Raster(LayerId(4)))
            )
            .unwrap(),
            [32, 0, 32, 64]
        );
    }
}

#[test]
fn alpha_locked_fill_gradient_and_delete_preserve_stored_silhouettes() {
    // Product risk: non-brush edit routes must not bypass alpha protection or
    // recolor unselected pixels, even when the selected content is off page.
    let mut tree = LayerTree::new(group(
        100,
        vec![raster(1, false, u16::MAX, LayerBlendMode::Normal)],
    ))
    .unwrap();
    tree.set_alpha_locked(LayerId(1), true).unwrap();
    let mut pixels = vec![0; TILE_BYTE_LEN];
    pixels[..12].copy_from_slice(&[1, 0, 0, 1, 128, 0, 0, 128, 255, 0, 0, 255]);
    let key = TileKey {
        layer: LayerId(1),
        mip: 0,
        x: -1,
        y: -1,
    };
    let snapshot = TileSnapshot::from_tiles([(key, pixels)]).unwrap();
    let selection = crate::selection_all([-128, -128], [3, 1], EditLimits::default()).unwrap();
    for paint in [
        SelectionPaint::Solid(PremultipliedRgba8([0, 0, 255, 255])),
        SelectionPaint::LinearGradient {
            start: [-128, -128],
            end: [-125, -128],
            start_color: PremultipliedRgba8([0, 0, 255, 255]),
            end_color: PremultipliedRgba8([0, 255, 0, 255]),
        },
    ] {
        let after = crate::paint_selection(
            &snapshot,
            &tree,
            LayerId(1),
            &selection,
            paint,
            EditLimits::default(),
        )
        .unwrap()
        .after;
        for (before, after) in snapshot
            .get(key)
            .unwrap()
            .pixels()
            .chunks_exact(4)
            .zip(after.get(key).unwrap().pixels().chunks_exact(4))
        {
            assert_eq!(before[3], after[3]);
            assert!(after[..3].iter().all(|value| *value <= after[3]));
        }
    }
    let filled = crate::flood_fill(
        &snapshot,
        &tree,
        crate::FloodFillRequest {
            canvas: CanvasSpec {
                width_px: 3,
                height_px: 1,
                pixels_per_inch: 96,
            },
            active: LayerId(1),
            source: SelectionSource::ActiveLayer,
            seed: [-128, -128],
            tolerance: 255,
            settings: FillSettings::default(),
        },
        Some(&selection),
        PremultipliedRgba8([0, 0, 255, 255]),
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(
        &filled.after.get(key).unwrap().pixels()[..12],
        &[0, 0, 1, 1, 0, 0, 128, 128, 0, 0, 255, 255]
    );
    assert!(matches!(
        crate::clear_raster(&snapshot, &tree, LayerId(1)),
        Err(crate::EditError::AlphaLockedLayer)
    ));
    assert!(matches!(
        crate::clear_selection(
            &snapshot,
            &tree,
            LayerId(1),
            &selection,
            EditLimits::default()
        ),
        Err(crate::EditError::AlphaLockedLayer)
    ));
    assert_eq!(&snapshot.get(key).unwrap().pixels()[..4], &[1, 0, 0, 1]);
}

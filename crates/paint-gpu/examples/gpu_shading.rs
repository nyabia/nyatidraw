//! Explicit real-device E3 composition probe; not physical-pen/present evidence.
use nyatidraw_api::{
    ContentRootId, GroupId, LayerBlendMode, LayerId, LayerTreeNodeId, TileCoordinate,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_cpu::sample_display_pixel;
use nyatidraw_paint_gpu::GpuCompositeScene;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::collections::{BTreeMap, BTreeSet};

const ROOT: GroupId = GroupId(100);
const NORMAL: LayerBlendMode = LayerBlendMode::Normal;
const MULTIPLY: LayerBlendMode = LayerBlendMode::Multiply;

fn raster(id: u128, clip: bool, blend_mode: LayerBlendMode, opacity_u16: u16) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        mask: None,
        id: LayerId(id),
        name: format!("Raster{id}"),
        visible: true,
        locked: false,
        reference: false,
        alpha_locked: false,
        clip_to_below: clip,
        blend_mode,
        opacity_u16,
        content_root: ContentRootId(0),
        color_effect: None,
    })
}

fn group(
    id: u128,
    clip: bool,
    blend_mode: LayerBlendMode,
    opacity_u16: u16,
    children: Vec<LayerTreeNode>,
) -> LayerTreeNode {
    LayerTreeNode::Group(GroupNode {
        id: GroupId(id),
        name: format!("Group{id}"),
        visible: true,
        clip_to_below: clip,
        blend_mode,
        opacity_u16,
        children,
    })
}

fn tree(children: Vec<LayerTreeNode>) -> LayerTree {
    match group(ROOT.0, false, NORMAL, u16::MAX, children) {
        LayerTreeNode::Group(root) => LayerTree::new(root).unwrap(),
        LayerTreeNode::Raster(_) => unreachable!(),
    }
}

fn rgba(layer: LayerId) -> [u8; 4] {
    match layer.0 {
        1 => [128, 0, 0, 128],
        2 => [0, 0, 128, 128],
        3 => [0, 128, 0, 128],
        4 => [64, 64, 0, 64],
        5 => [40, 100, 70, 128],
        6 => [120, 30, 80, 160],
        _ => [64; 4],
    }
}

fn snapshot(tree: &LayerTree, points: &[[i32; 2]]) -> TileSnapshot {
    fn layers(group: &GroupNode, ids: &mut BTreeSet<LayerId>) {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => {
                    ids.insert(layer.id);
                }
                LayerTreeNode::Group(group) => layers(group, ids),
            }
        }
    }
    let mut ids = BTreeSet::new();
    layers(tree.root(), &mut ids);
    let mut tiles = BTreeMap::new();
    for layer in ids {
        for &[x, y] in points {
            let key = TileKey::from_pixel(layer, 128, x, y);
            let bytes = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
            let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
            bytes[offset..offset + 4].copy_from_slice(&rgba(layer));
        }
    }
    let images = TileSnapshot::from_tiles(tiles).unwrap();
    let coverage = tree.mask_ids().into_iter().flat_map(|mask| {
        points
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 1)
            .map(move |(index, &[x, y])| {
                let key = nyatidraw_tiles::CoverageTileKey::from_pixel(mask, 128, x, y);
                let value = if index == 0 { 0 } else { 96 };
                (key, vec![value; nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN])
            })
    });
    images.with_coverage_replacements(coverage).unwrap()
}

fn view() -> ViewportTransform {
    ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [320, 128],
        dpi_scale: 1.0,
        pan: Point { x: 32.0, y: 32.0 },
        zoom: 1.0,
        rotation_radians: 0.0,
        mirrored_horizontal: false,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn expected_display(raw: [u8; 4], [x, y]: [i32; 2], page: u32) -> [u8; 4] {
    // Mirrors presentation chrome only; raw composite comparison below is exact.
    let backdrop =
        if x >= 0 && y >= 0 && i64::from(x) < i64::from(page) && i64::from(y) < i64::from(page) {
            if (x.div_euclid(16) + y.div_euclid(16)) & 1 == 0 {
                0.72
            } else {
                0.58
            }
        } else {
            0.035
        };
    let mut result = [0, 0, 0, 255];
    for channel in 0..3 {
        result[channel] =
            (f64::from(raw[channel]) + backdrop * (255.0 - f64::from(raw[3]))).round() as u8;
    }
    result
}

fn verify(
    scene: &mut GpuCompositeScene,
    tiles: &TileSnapshot,
    points: &[[i32; 2]],
    page: u32,
    solo: Option<LayerTreeNodeId>,
    oracle: Option<[u8; 4]>,
    label: &str,
) -> u8 {
    scene.set_solo(solo).unwrap();
    scene.render_viewport(view()).unwrap();
    let display = scene.readback_display().unwrap().unwrap();
    let mut maximum = 0;
    for &[x, y] in points {
        let expected = sample_display_pixel(tiles, scene.tree(), [x, y], solo).unwrap();
        if let Some(oracle) = oracle {
            assert_eq!(expected, oracle, "independent byte oracle: {label}");
        }
        let coordinate = TileCoordinate {
            mip: 0,
            x: x.div_euclid(128),
            y: y.div_euclid(128),
        };
        let actual = scene
            .readback_resident_group_tile(ROOT, coordinate)
            .unwrap()
            .unwrap();
        let offset = usize::try_from(y.rem_euclid(128) * 128 + x.rem_euclid(128)).unwrap() * 4;
        assert_eq!(
            &actual.pixels[offset..offset + 4],
            &expected,
            "raw GPU/CPU mismatch {label} page={page} at{x},{y}"
        );
        let offset = usize::try_from((y + 32) * 320 + x + 32).unwrap() * 4;
        let shown = expected_display(expected, [x, y], page);
        for (actual, expected) in display.pixels[offset..offset + 4].iter().zip(shown) {
            maximum = maximum.max(actual.abs_diff(expected));
            assert!(
                actual.abs_diff(expected) <= 1,
                "display mismatch {label} page={page} at{x},{y}: {:?} vs{shown:?}",
                &display.pixels[offset..offset + 4]
            );
        }
    }
    maximum
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn verify_cached_fit(device: &wgpu::Device, queue: &wgpu::Queue) -> usize {
    let mut tree = tree(vec![group(
        10,
        false,
        NORMAL,
        65535,
        vec![raster(1, false, NORMAL, 65535)],
    )]);
    let mask = nyatidraw_document::LayerMask {
        id: nyatidraw_api::MaskId(1001),
        enabled: true,
        default_coverage: 255,
    };
    tree.set_mask(LayerId(1), Some(mask)).unwrap();
    let key = TileKey::from_pixel(LayerId(1), 128, 0, 0);
    let mut rgba = vec![0; TILE_BYTE_LEN];
    for y in 0..32 {
        for x in 0..32 {
            let at = (y * 128 + x) * 4;
            rgba[at..at + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let mut values = vec![255; nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN];
    for y in 6..11 {
        for x in 6..11 {
            values[y * 128 + x] = 0;
        }
    }
    let tiles = TileSnapshot::from_tiles([(key, rgba)])
        .unwrap()
        .with_coverage_replacements([(
            nyatidraw_tiles::CoverageTileKey {
                mask: mask.id,
                mip: 0,
                x: 0,
                y: 0,
            },
            values,
        )])
        .unwrap();
    let mut scene = GpuCompositeScene::new(device, queue, [32; 2], tree).unwrap();
    scene.replace_mask_tiles(&tiles).unwrap();
    scene
        .upload_closed_tile(key, tiles.get(key).unwrap().pixels())
        .unwrap();
    for (size, zoom) in [
        ([1238, 953], 26.8),
        ([1238, 857], 24.1),
        ([1238, 857], 24.1),
        ([1240, 859], 24.1),
    ] {
        let viewport = ViewportTransform {
            physical_size: size,
            pan: Point {
                x: (f64::from(size[0]) - 32.0 * zoom) / 2.0,
                y: (f64::from(size[1]) - 32.0 * zoom) / 2.0,
            },
            zoom,
            ..view()
        };
        for _ in 0..16 {
            scene.render_viewport(viewport).unwrap();
        }
        let raw = scene
            .readback_resident_group_tile(ROOT, TileCoordinate { mip: 0, x: 0, y: 0 })
            .unwrap()
            .unwrap();
        assert_eq!(
            &raw.pixels[(8 * 128 + 8) * 4..(8 * 128 + 8) * 4 + 4],
            &[0; 4],
            "cached high-zoom root lost mask"
        );
        let display = scene.readback_display().unwrap().unwrap();
        let x = (viewport.pan.x + 8.5 * zoom).floor() as usize;
        let y = (viewport.pan.y + 8.5 * zoom).floor() as usize;
        let at = (y * size[0] as usize + x) * 4;
        for (actual, expected) in
            display.pixels[at..at + 4]
                .iter()
                .zip(expected_display([0; 4], [8, 8], 32))
        {
            assert!(
                actual.abs_diff(expected) <= 1,
                "cached high-zoom display lost mask"
            );
        }
    }
    4
}

#[allow(clippy::too_many_lines)]
fn main() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .unwrap();
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nyatidraw-shading-acceptance"),
        ..Default::default()
    }))
    .unwrap();
    let mut cases = vec![
        (
            "normal",
            tree(vec![raster(1, false, NORMAL, 65535)]),
            Some([128, 0, 0, 128]),
        ),
        (
            "multiply-transparent-backdrop",
            tree(vec![raster(1, false, MULTIPLY, 65535)]),
            Some([128, 0, 0, 128]),
        ),
        (
            "one-clip-base-opacity-once",
            tree(vec![
                raster(1, false, NORMAL, 32768),
                raster(2, true, NORMAL, 65535),
            ]),
            Some([32, 0, 32, 64]),
        ),
        (
            "two-clips-same-base",
            tree(vec![
                raster(1, false, NORMAL, 32768),
                raster(2, true, NORMAL, 65535),
                raster(3, true, NORMAL, 65535),
            ]),
            Some([16, 32, 16, 64]),
        ),
        (
            "multiply-clip",
            tree(vec![
                raster(1, false, NORMAL, 65535),
                raster(2, true, MULTIPLY, 65535),
            ]),
            Some([64, 0, 0, 128]),
        ),
        (
            "multiply-stack-parent",
            tree(vec![
                raster(4, false, NORMAL, 65535),
                raster(1, false, MULTIPLY, 32768),
                raster(2, true, NORMAL, 40000),
            ]),
            None,
        ),
        (
            "group-base-clipped-group",
            tree(vec![
                group(
                    10,
                    false,
                    MULTIPLY,
                    32768,
                    vec![raster(1, false, NORMAL, 65535)],
                ),
                group(
                    11,
                    true,
                    NORMAL,
                    65535,
                    vec![raster(2, false, NORMAL, 65535)],
                ),
                raster(3, true, MULTIPLY, 32000),
            ]),
            None,
        ),
        (
            "nested-clipped-group",
            tree(vec![
                raster(4, false, NORMAL, 65535),
                group(
                    10,
                    false,
                    MULTIPLY,
                    48000,
                    vec![
                        raster(1, false, NORMAL, 40000),
                        group(
                            11,
                            true,
                            MULTIPLY,
                            65535,
                            vec![
                                raster(2, false, NORMAL, 65535),
                                raster(3, true, NORMAL, 30000),
                            ],
                        ),
                    ],
                ),
            ]),
            None,
        ),
        (
            "orphan-before-base",
            tree(vec![
                raster(2, true, NORMAL, 65535),
                raster(1, false, NORMAL, 65535),
            ]),
            Some([128, 0, 0, 128]),
        ),
        (
            "only-orphan",
            tree(vec![raster(2, true, NORMAL, 65535)]),
            Some([0; 4]),
        ),
        (
            "zero-base-opacity",
            tree(vec![
                raster(1, false, NORMAL, 0),
                raster(2, true, NORMAL, 65535),
            ]),
            Some([0; 4]),
        ),
    ];
    for mode in [
        LayerBlendMode::Screen,
        LayerBlendMode::Add,
        LayerBlendMode::Overlay,
    ] {
        for opacity in [0, 32768, 65535] {
            for clipped in [false, true] {
                cases.push((
                    "extended-raster",
                    tree(vec![
                        raster(5, false, NORMAL, 50000),
                        raster(6, clipped, mode, opacity),
                    ]),
                    None,
                ));
                cases.push((
                    "extended-group",
                    tree(vec![
                        raster(5, false, NORMAL, 50000),
                        group(
                            10,
                            clipped,
                            mode,
                            opacity,
                            vec![raster(6, false, NORMAL, 65535)],
                        ),
                    ]),
                    None,
                ));
            }
        }
    }
    for default_coverage in [0, 255] {
        for enabled in [false, true] {
            for mode in [
                NORMAL,
                MULTIPLY,
                LayerBlendMode::Screen,
                LayerBlendMode::Add,
                LayerBlendMode::Overlay,
            ] {
                let mut masked = tree(vec![
                    raster(1, false, NORMAL, 65535),
                    group(
                        10,
                        false,
                        mode,
                        40000,
                        vec![
                            raster(5, false, NORMAL, 55000),
                            raster(6, true, mode, 50000),
                        ],
                    ),
                ]);
                for layer in [5, 6] {
                    masked
                        .set_mask(
                            LayerId(layer),
                            Some(nyatidraw_document::LayerMask {
                                id: nyatidraw_api::MaskId(layer + 1000),
                                enabled,
                                default_coverage,
                            }),
                        )
                        .unwrap();
                }
                cases.push(("mask-stack", masked, None));
            }
        }
    }
    let mut hidden = tree(vec![
        raster(1, false, NORMAL, 65535),
        raster(2, true, NORMAL, 65535),
    ]);
    hidden
        .set_visibility(LayerTreeNodeId::Raster(LayerId(1)), false)
        .unwrap();
    cases.push(("hidden-base", hidden, Some([0; 4])));
    let mut maximum = 0;
    let mut checks = verify_cached_fit(&device, &queue);
    for page in [32, 128] {
        let points = [[8, 8], [i32::try_from(page).unwrap() + 8, 8], [-8, 8]];
        for (label, tree, oracle) in &cases {
            let tiles = snapshot(tree, &points);
            let mut scene =
                GpuCompositeScene::new(&device, &queue, [page; 2], tree.clone()).unwrap();
            for (key, tile) in tiles.iter() {
                scene.upload_closed_tile(key, tile.pixels()).unwrap();
            }
            scene.replace_mask_tiles(&tiles).unwrap();
            maximum = maximum.max(verify(
                &mut scene, &tiles, &points, page, None, *oracle, label,
            ));
            checks += 1;
            if *label == "mask-stack" {
                for _ in 0..3 {
                    maximum = maximum.max(verify(
                        &mut scene,
                        &tiles,
                        &points,
                        page,
                        None,
                        None,
                        "mask-unchanged-redraw",
                    ));
                    checks += 1;
                }
                let changed = tiles
                    .with_coverage_replacements(tiles.coverage_iter().map(|(key, tile)| {
                        (key, tile.pixels().iter().map(|value| 255 - value).collect())
                    }))
                    .unwrap();
                for (key, tile) in changed.coverage_iter() {
                    scene.upload_mask_tile(key, tile.pixels()).unwrap();
                }
                maximum = maximum.max(verify(
                    &mut scene,
                    &changed,
                    &points,
                    page,
                    Some(LayerTreeNodeId::Raster(LayerId(6))),
                    None,
                    "mask-dirty-solo",
                ));
                scene.replace_mask_tiles(&tiles).unwrap();
                let mut toggled = tree.clone();
                for layer in [5, 6] {
                    let mut mask = toggled.raster(LayerId(layer)).unwrap().mask.unwrap();
                    mask.enabled = !mask.enabled;
                    toggled.set_mask(LayerId(layer), Some(mask)).unwrap();
                }
                scene.replace_tree(toggled).unwrap();
                maximum = maximum.max(verify(
                    &mut scene,
                    &tiles,
                    &points,
                    page,
                    None,
                    None,
                    "mask-toggle-cached",
                ));
                let mut unmasked = tree.clone();
                for layer in [5, 6] {
                    unmasked.set_mask(LayerId(layer), None).unwrap();
                }
                scene.replace_tree(unmasked).unwrap();
                maximum = maximum.max(verify(
                    &mut scene,
                    &tiles,
                    &points,
                    page,
                    None,
                    None,
                    "mask-remove-originals",
                ));
                checks += 3;
            }
            if label.starts_with("extended-") {
                maximum = maximum.max(verify(
                    &mut scene,
                    &tiles,
                    &points,
                    page,
                    Some(LayerTreeNodeId::Raster(LayerId(6))),
                    None,
                    label,
                ));
                checks += 1;
            }
            if *label == "group-base-clipped-group" {
                maximum = maximum.max(verify(
                    &mut scene,
                    &tiles,
                    &points,
                    page,
                    Some(LayerTreeNodeId::Group(GroupId(11))),
                    Some([32, 0, 32, 64]),
                    "solo-clipped-group-base-context",
                ));
                checks += 1;
            }
        }
        let tree = tree(vec![
            raster(1, false, NORMAL, 32768),
            raster(2, true, NORMAL, 65535),
            raster(3, true, NORMAL, 65535),
        ]);
        let tiles = snapshot(&tree, &points);
        let mut scene = GpuCompositeScene::new(&device, &queue, [page; 2], tree).unwrap();
        for (key, tile) in tiles.iter() {
            scene.upload_closed_tile(key, tile.pixels()).unwrap();
        }
        let changed = tiles
            .with_replacements(tiles.iter().filter(|(key, _)| key.layer == LayerId(1)).map(
                |(key, tile)| {
                    let mut pixels = tile.pixels().to_vec();
                    for pixel in pixels.chunks_exact_mut(4) {
                        if pixel[3] != 0 {
                            pixel.copy_from_slice(&[64, 0, 0, 64]);
                        }
                    }
                    (key, pixels)
                },
            ))
            .unwrap();
        for (key, tile) in changed.iter() {
            scene.upload_closed_tile(key, tile.pixels()).unwrap();
        }
        maximum = maximum.max(verify(
            &mut scene,
            &changed,
            &points,
            page,
            None,
            Some([8, 16, 8, 32]),
            "dirty-base-alpha-updates-stack",
        ));
        for (key, tile) in tiles.iter() {
            scene.upload_closed_tile(key, tile.pixels()).unwrap();
        }
        scene
            .set_opacity(LayerTreeNodeId::Raster(LayerId(1)), 16384)
            .unwrap();
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            None,
            Some([8, 16, 8, 32]),
            "base-opacity-updates-whole-stack-once",
        ));
        scene
            .set_opacity(LayerTreeNodeId::Raster(LayerId(1)), 32768)
            .unwrap();
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            Some(LayerTreeNodeId::Raster(LayerId(2))),
            Some([32, 0, 32, 64]),
            "solo-clip-base-context",
        ));
        scene
            .set_visibility(LayerTreeNodeId::Raster(LayerId(1)), false)
            .unwrap();
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            None,
            Some([0; 4]),
            "hide-cached-base",
        ));
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            Some(LayerTreeNodeId::Raster(LayerId(2))),
            Some([32, 0, 32, 64]),
            "solo-reveals-hidden-dependency",
        ));
        scene
            .set_visibility(LayerTreeNodeId::Raster(LayerId(1)), true)
            .unwrap();
        scene
            .reorder(LayerTreeNodeId::Raster(LayerId(1)), ROOT, 2)
            .unwrap();
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            None,
            Some([64, 0, 0, 64]),
            "reorder-orphans-no-rebind",
        ));
        let mut deleted = scene.tree().clone();
        deleted.remove(LayerTreeNodeId::Raster(LayerId(1))).unwrap();
        scene.replace_tree(deleted).unwrap();
        maximum = maximum.max(verify(
            &mut scene,
            &tiles,
            &points,
            page,
            None,
            Some([0; 4]),
            "delete-base-orphans",
        ));
        checks += 7;
    }
    println!(
        "gpu-shading adapter={} backend={:?} profile={} cases={checks} raw_max_delta=0 display_max_delta={maximum} physical_pen=false hwnd_present=false",
        info.name,
        info.backend,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
}

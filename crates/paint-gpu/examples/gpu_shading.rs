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
    TileSnapshot::from_tiles(tiles).unwrap()
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
    let mut hidden = tree(vec![
        raster(1, false, NORMAL, 65535),
        raster(2, true, NORMAL, 65535),
    ]);
    hidden
        .set_visibility(LayerTreeNodeId::Raster(LayerId(1)), false)
        .unwrap();
    cases.push(("hidden-base", hidden, Some([0; 4])));
    let mut maximum = 0;
    let mut checks = 0;
    for page in [32, 128] {
        let points = [[8, 8], [i32::try_from(page).unwrap() + 8, 8], [-8, 8]];
        for (label, tree, oracle) in &cases {
            let tiles = snapshot(tree, &points);
            let mut scene =
                GpuCompositeScene::new(&device, &queue, [page; 2], tree.clone()).unwrap();
            for (key, tile) in tiles.iter() {
                scene.upload_closed_tile(key, tile.pixels()).unwrap();
            }
            maximum = maximum.max(verify(
                &mut scene, &tiles, &points, page, None, *oracle, label,
            ));
            checks += 1;
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

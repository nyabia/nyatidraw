//! Exact device compositing acceptance; no physical-pen or presentation claim.
use nyatidraw_api::{
    ContentRootId, GroupId, LayerBlendMode, LayerColorEffect, LayerId, LayerTreeNodeId,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_cpu::{SelectionSource, sample_artwork_pixel, sample_display_pixel};
use nyatidraw_paint_gpu::GpuCompositeScene;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::collections::BTreeMap;

const ROOT: GroupId = GroupId(100);

fn raster(id: u128, clip: bool, mode: LayerBlendMode, opacity: u16) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        mask: None,
        id: LayerId(id),
        name: format!("Layer{id}"),
        visible: true,
        locked: false,
        reference: id == 4,
        alpha_locked: false,
        clip_to_below: clip,
        blend_mode: mode,
        opacity_u16: opacity,
        content_root: ContentRootId(0),
        color_effect: None,
    })
}

fn group(id: u128, opacity: u16, children: Vec<LayerTreeNode>) -> GroupNode {
    GroupNode {
        id: GroupId(id),
        name: format!("Group{id}"),
        visible: true,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        opacity_u16: opacity,
        children,
    }
}

fn tree(opacity: u16, mode: LayerBlendMode, clips: bool, nested: bool) -> LayerTree {
    let mut foreground = vec![raster(2, false, mode, opacity)];
    if clips {
        foreground.push(raster(4, true, mode, 40000));
    }
    let mut children = vec![raster(1, false, LayerBlendMode::Normal, 65535)];
    if nested {
        children.push(LayerTreeNode::Group(group(10, 45000, foreground)));
    } else {
        children.extend(foreground);
    }
    LayerTree::new(group(ROOT.0, 65535, children)).unwrap()
}

fn snapshot(clips: bool) -> TileSnapshot {
    let palette = [
        [0; 4],
        [0, 0, 0, 255],
        [255; 4],
        [128, 0, 0, 128],
        [1; 4],
        [12, 24, 60, 64],
        [0, 96, 48, 128],
        [30, 40, 50, 60],
    ];
    let mut tiles = BTreeMap::new();
    for layer in [LayerId(1), LayerId(2), LayerId(4)] {
        if layer == LayerId(4) && !clips {
            continue;
        }
        for x in -1..=1 {
            let mut bytes = vec![0; TILE_BYTE_LEN];
            for (index, pixel) in bytes.chunks_exact_mut(4).enumerate() {
                let color = if layer == LayerId(1) {
                    [20, 40, 60, 255]
                } else {
                    palette[(index + usize::try_from(layer.0).unwrap()) % palette.len()]
                };
                pixel.copy_from_slice(&color);
            }
            tiles.insert(
                TileKey {
                    layer,
                    mip: 0,
                    x,
                    y: 0,
                },
                bytes,
            );
        }
    }
    TileSnapshot::from_tiles(tiles).unwrap()
}

fn view() -> ViewportTransform {
    ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [320, 160],
        dpi_scale: 1.0,
        pan: Point { x: 32.0, y: 0.0 },
        zoom: 1.0,
        rotation_radians: 0.0,
        mirrored_horizontal: false,
    }
}

fn verify(
    scene: &mut GpuCompositeScene,
    snapshot: &TileSnapshot,
    page: u32,
    solo: Option<LayerTreeNodeId>,
    label: &str,
) -> usize {
    scene.set_solo(solo).unwrap();
    scene.render_viewport(view()).unwrap();
    let mut checks = 0;
    for x in [
        -8,
        8,
        16,
        24,
        i32::try_from(page).unwrap() - 1,
        i32::try_from(page).unwrap() + 8,
    ] {
        let coordinate = TileKey::from_pixel(LayerId(2), 128, x, 8).coordinate();
        let composite = scene
            .readback_resident_group_tile(ROOT, coordinate)
            .unwrap()
            .unwrap();
        // Check every pixel of each visited resident product, including alpha-zero,
        // low-alpha and tile padding. The page and signed cache products coexist.
        for y in 0..composite.height {
            for local_x in 0..composite.width {
                let point = [
                    coordinate.x * 128 + i32::try_from(local_x).unwrap(),
                    coordinate.y * 128 + i32::try_from(y).unwrap(),
                ];
                let expected = sample_display_pixel(snapshot, scene.tree(), point, solo).unwrap();
                let offset = usize::try_from((y * composite.width + local_x) * 4).unwrap();
                assert_eq!(
                    &composite.pixels[offset..offset + 4],
                    &expected,
                    "CPU/GPU {label} page={page} solo={solo:?} point={point:?}"
                );
                checks += 1;
            }
        }
    }
    for (key, raw) in snapshot.iter() {
        let gpu = scene.readback_resident_raster_tile(key).unwrap().unwrap();
        assert_eq!(
            gpu.pixels,
            raw.pixels(),
            "effect mutated durable raster {key:?}"
        );
    }
    checks
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
        label: Some("nyatidraw-layer-color-acceptance"),
        ..Default::default()
    }))
    .unwrap();
    let mut checks = 0;
    let effect = LayerColorEffect::default();
    let custom = LayerColorEffect {
        dark_rgb: [240, 80, 48],
        light_rgb: [20, 40, 60],
        ..effect
    };
    for page in [32, 128] {
        for (opacity, mode, clips, nested) in [
            (65535, LayerBlendMode::Normal, false, false),
            (32768, LayerBlendMode::Normal, true, false),
            (40000, LayerBlendMode::Multiply, true, true),
        ] {
            let raw_tree = tree(opacity, mode, clips, nested);
            let snapshot = snapshot(clips);
            let mut scene =
                GpuCompositeScene::new(&device, &queue, [page; 2], raw_tree.clone()).unwrap();
            for (key, tile) in snapshot.iter() {
                scene.upload_closed_tile(key, tile.pixels()).unwrap();
            }
            for (label, value) in [
                ("raw", None),
                ("default", Some(effect)),
                ("custom", Some(custom)),
                (
                    "disabled",
                    Some(LayerColorEffect {
                        enabled: false,
                        ..custom
                    }),
                ),
                ("removed", None),
            ] {
                scene.set_color_effect(LayerId(2), value).unwrap();
                if clips {
                    scene.set_color_effect(LayerId(4), value).unwrap();
                }
                checks += verify(&mut scene, &snapshot, page, None, label);
                if clips {
                    checks += verify(
                        &mut scene,
                        &snapshot,
                        page,
                        Some(LayerTreeNodeId::Raster(LayerId(4))),
                        label,
                    );
                }
                for point in [[8, 8], [-8, 8]] {
                    assert_eq!(
                        sample_artwork_pixel(
                            &snapshot,
                            scene.tree(),
                            LayerId(2),
                            point,
                            SelectionSource::ActiveLayer
                        )
                        .unwrap(),
                        sample_artwork_pixel(
                            &snapshot,
                            &raw_tree,
                            LayerId(2),
                            point,
                            SelectionSource::ActiveLayer
                        )
                        .unwrap()
                    );
                    if value.is_none_or(|effect| !effect.enabled) {
                        assert_eq!(
                            sample_display_pixel(&snapshot, scene.tree(), point, None).unwrap(),
                            sample_display_pixel(&snapshot, &raw_tree, point, None).unwrap()
                        );
                    }
                }
            }
        }
    }
    println!(
        "gpu-layer-color adapter={} backend={:?} profile={} pixel_checks={checks} raw_max_delta=0 stored_pixels_unchanged=true physical_pen=false hwnd_present=false",
        info.name,
        info.backend,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
}

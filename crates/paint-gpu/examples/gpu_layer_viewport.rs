//! Real-device probe for group composition, coordinate invalidation, and the
//! affine viewport projection.
//!
//! Run with `cargo run -p nyatidraw-paint-gpu --example gpu_layer_viewport --release`.

use nyatidraw_api::{ContentRootId, GroupId, LayerId, LayerTreeNodeId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_gpu::{CompositeRenderError, GpuCompositeScene, MAX_PERSISTENT_SCENE_BYTES};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

const SIZE: u32 = 256;
const ROOT: GroupId = GroupId(100);
const OVERLAY_GROUP: GroupId = GroupId(10);
const BOTTOM: LayerId = LayerId(1);
const TOP: LayerId = LayerId(2);
const TOLERANCE: u8 = 2;

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let adapter_info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nayati-layer-viewport-probe"),
        ..Default::default()
    }))?;

    let allocation_guard = match GpuCompositeScene::new(
        &device,
        &queue,
        [8_192, 8_192],
        over_budget_tree().map_err(debug_error)?,
    ) {
        Err(CompositeRenderError::AllocationBudgetExceeded {
            required_bytes,
            max_bytes,
            surfaces,
        }) if max_bytes == MAX_PERSISTENT_SCENE_BYTES => (required_bytes, max_bytes, surfaces),
        Err(error) => return Err(format!("unexpected allocation guard error: {error:?}").into()),
        Ok(_) => return Err("8K 20-layer fixture bypassed the scene allocation budget".into()),
    };

    let tree = fixture_tree().map_err(debug_error)?;
    let mut scene =
        GpuCompositeScene::new(&device, &queue, [SIZE, SIZE], tree).map_err(debug_error)?;
    // Transparent document pixels are displayed over the viewport-only
    // checkerboard; this probe reads the disposable display texture, not any
    // persisted raster surface.
    scene
        .render_viewport(viewport(Point::default(), 1.0, 0.0))
        .map_err(debug_error)?;
    let transparent_pixels = readback(&scene, &device, &queue)?;
    require_pixel(
        &transparent_pixels,
        0,
        0,
        [184, 184, 184, 255],
        "transparent checkerboard light cell",
    )?;
    require_pixel(
        &transparent_pixels,
        20,
        0,
        [148, 148, 148, 255],
        "transparent checkerboard dark cell",
    )?;
    // Real shader readback: the same document cell must keep its color through
    // pan, fractional zoom, rotation and HiDPI. This is not a latency probe.
    for (pan, zoom, rotation, scale, mirrored) in [
        (Point { x: 37.0, y: 29.0 }, 1.0, 0.0, 1.0, false),
        (Point { x: 17.0, y: 11.0 }, 2.5, 0.0, 1.0, false),
        (
            Point { x: 100.0, y: 30.0 },
            1.5,
            std::f64::consts::FRAC_PI_2,
            1.0,
            false,
        ),
        (Point { x: 8.0, y: 5.0 }, 1.0, 0.0, 1.5, false),
        (Point { x: 100.0, y: 80.0 }, 1.5, 0.41, 1.5, true),
    ] {
        let mut view = viewport(pan, zoom, rotation);
        view.dpi_scale = scale;
        view.mirrored_horizontal = mirrored;
        scene.render_viewport(view).map_err(debug_error)?;
        let pixels = readback(&scene, &device, &queue)?;
        for (x, expected) in [(8.0, [184, 184, 184, 255]), (24.0, [148, 148, 148, 255])] {
            let physical = view
                .document_to_window(Point { x, y: 8.0 })
                .ok_or("invalid checker transform")?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            require_pixel(
                &pixels,
                physical.x.floor() as u32,
                physical.y.floor() as u32,
                expected,
                "document-aligned checker under affine/HiDPI",
            )?;
        }
    }
    scene
        .upload_layer_rgba8(BOTTOM, &solid([255, 0, 0, 255]))
        .map_err(debug_error)?;
    scene
        .upload_layer_rgba8(TOP, &solid([0, 0, 255, 255]))
        .map_err(debug_error)?;

    let identity = viewport(Point::default(), 1.0, 0.0);
    let initial = scene.render_viewport(identity).map_err(debug_error)?;
    let initial_pixels = readback(&scene, &device, &queue)?;
    require_pixel(&initial_pixels, 64, 64, [127, 0, 128, 255], "group opacity")?;
    if initial.group_tiles_rebuilt != 8 {
        return Err(format!(
            "initial render rebuilt {} group tiles instead of 8",
            initial.group_tiles_rebuilt
        )
        .into());
    }

    scene
        .upload_closed_tile(
            TileKey {
                layer: TOP,
                mip: 0,
                x: 0,
                y: 0,
            },
            &vec![[0_u8, 255, 0, 255]; TILE_BYTE_LEN / 4]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        )
        .map_err(debug_error)?;
    let incremental = scene.render_viewport(identity).map_err(debug_error)?;
    let incremental_pixels = readback(&scene, &device, &queue)?;
    require_pixel(
        &incremental_pixels,
        64,
        64,
        [127, 128, 0, 255],
        "single-tile update",
    )?;
    require_pixel(
        &incremental_pixels,
        192,
        192,
        [127, 0, 128, 255],
        "unrelated tile retained",
    )?;
    if incremental.group_tiles_rebuilt != 2 {
        return Err(format!(
            "one raster tile rebuilt {} group tiles instead of its group and root",
            incremental.group_tiles_rebuilt
        )
        .into());
    }

    scene
        .set_visibility(LayerTreeNodeId::Group(OVERLAY_GROUP), false)
        .map_err(debug_error)?;
    let hidden = scene.render_viewport(identity).map_err(debug_error)?;
    let hidden_pixels = readback(&scene, &device, &queue)?;
    require_pixel(&hidden_pixels, 64, 64, [255, 0, 0, 255], "group visibility")?;
    if hidden.group_tiles_rebuilt != 4 {
        return Err(format!(
            "group visibility rebuilt {} tiles instead of root's 4 resident tiles",
            hidden.group_tiles_rebuilt
        )
        .into());
    }

    scene
        .set_visibility(LayerTreeNodeId::Group(OVERLAY_GROUP), true)
        .map_err(debug_error)?;
    scene
        .reorder(LayerTreeNodeId::Group(OVERLAY_GROUP), ROOT, 0)
        .map_err(debug_error)?;
    let reordered = scene.render_viewport(identity).map_err(debug_error)?;
    let reordered_pixels = readback(&scene, &device, &queue)?;
    require_pixel(&reordered_pixels, 64, 64, [255, 0, 0, 255], "group order")?;

    scene
        .set_visibility(LayerTreeNodeId::Group(OVERLAY_GROUP), false)
        .map_err(debug_error)?;
    scene
        .upload_layer_rgba8(BOTTOM, &quadrants())
        .map_err(debug_error)?;
    let affine = viewport(
        Point {
            x: 384.0,
            y: -128.0,
        },
        2.0,
        std::f64::consts::FRAC_PI_2,
    );
    let transformed = scene.render_viewport(affine).map_err(debug_error)?;
    let transformed_pixels = readback(&scene, &device, &queue)?;
    require_pixel(
        &transformed_pixels,
        32,
        32,
        [0, 0, 255, 255],
        "affine top-left",
    )?;
    require_pixel(
        &transformed_pixels,
        224,
        32,
        [255, 0, 0, 255],
        "affine top-right",
    )?;
    require_pixel(
        &transformed_pixels,
        32,
        224,
        [255, 255, 0, 255],
        "affine bottom-left",
    )?;
    require_pixel(
        &transformed_pixels,
        224,
        224,
        [0, 255, 0, 255],
        "affine bottom-right",
    )?;

    // Literal quadrant checks independently verify the shared inverse affine:
    // document-X reflection then 90-degree rotation is an anti-diagonal swap.
    let mirrored = ViewportTransform {
        mirrored_horizontal: true,
        ..viewport(
            Point { x: 256.0, y: 256.0 },
            1.0,
            std::f64::consts::FRAC_PI_2,
        )
    };
    scene.render_viewport(mirrored).map_err(debug_error)?;
    let mirrored_pixels = readback(&scene, &device, &queue)?;
    for (x, y, expected) in [
        (32, 32, [255, 255, 0, 255]),
        (224, 32, [0, 255, 0, 255]),
        (32, 224, [0, 0, 255, 255]),
        (224, 224, [255, 0, 0, 255]),
    ] {
        require_pixel(&mirrored_pixels, x, y, expected, "mirror plus rotation")?;
    }
    scene.render_viewport(identity).map_err(debug_error)?;
    require_pixel(
        &readback(&scene, &device, &queue)?,
        32,
        32,
        [255, 0, 0, 255],
        "view mirror never edits raster",
    )?;

    verify_tree_replacement(&mut scene, &device, &queue)?;
    verify_nested_group_solo(&device, &queue)?;

    println!(
        concat!(
            "{{\"event\":\"gpu_layer_viewport\",\"adapter_name\":{:?},",
            "\"backend\":\"{:?}\",\"device_type\":\"{:?}\",",
            "\"release_profile\":{},\"dimensions\":{},",
            "\"initial_group_tiles\":{},\"single_tile_group_tiles\":{},",
            "\"visibility_group_tiles\":{},\"reorder_group_tiles\":{},",
            "\"affine_group_tiles\":{},\"allocation_guard\":{{",
            "\"required_bytes\":{},\"max_bytes\":{},\"surfaces\":{}}},",
            "\"tree_delete_restore\":true,\"view_mirror_rotation\":true,\"nested_group_solo\":true,\"tolerance\":{},\"pass\":true}}"
        ),
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        !cfg!(debug_assertions),
        SIZE,
        initial.group_tiles_rebuilt,
        incremental.group_tiles_rebuilt,
        hidden.group_tiles_rebuilt,
        reordered.group_tiles_rebuilt,
        transformed.group_tiles_rebuilt,
        allocation_guard.0,
        allocation_guard.1,
        allocation_guard.2,
        TOLERANCE,
    );
    Ok(())
}

fn verify_nested_group_solo(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(), Box<dyn std::error::Error>> {
    // Product risk: a picker must not see a different image from the viewport
    // when Solo targets a group with another group nested inside it.
    let outside = LayerId(30);
    let nested = GroupId(11);
    let tree = LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![
            LayerTreeNode::Group(GroupNode {
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: OVERLAY_GROUP,
                name: "Solo group".into(),
                visible: true,
                opacity_u16: u16::MAX,
                children: vec![
                    raster(BOTTOM, "Direct child"),
                    LayerTreeNode::Group(GroupNode {
                        clip_to_below: false,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        id: nested,
                        name: "Nested group".into(),
                        visible: true,
                        opacity_u16: u16::MAX,
                        children: vec![raster(TOP, "Nested child")],
                    }),
                ],
            }),
            raster(outside, "Excluded by Solo"),
        ],
    })
    .map_err(debug_error)?;
    let mut scene =
        GpuCompositeScene::new(device, queue, [SIZE, SIZE], tree).map_err(debug_error)?;
    for (layer, color) in [
        (BOTTOM, [255, 0, 0, 255]),
        (TOP, [0, 0, 255, 255]),
        (outside, [0, 255, 0, 255]),
    ] {
        scene
            .upload_layer_rgba8(layer, &solid(color))
            .map_err(debug_error)?;
        scene
            .upload_closed_tile(
                TileKey {
                    layer,
                    mip: 0,
                    x: -1,
                    y: 0,
                },
                &color.repeat(TILE_BYTE_LEN / 4),
            )
            .map_err(debug_error)?;
    }
    for (target, expected) in [
        (None, [0, 255, 0, 255]),
        (
            Some(LayerTreeNodeId::Group(OVERLAY_GROUP)),
            [0, 0, 255, 255],
        ),
        (Some(LayerTreeNodeId::Raster(TOP)), [0, 0, 255, 255]),
    ] {
        scene.set_solo(target).map_err(debug_error)?;
        // Both finite-page composition and signed workspace composition must agree.
        scene
            .render_viewport(viewport(Point { x: 128.0, y: 0.0 }, 1.0, 0.0))
            .map_err(debug_error)?;
        let pixels = readback(&scene, device, queue)?;
        require_pixel(&pixels, 192, 64, expected, "nested Solo page")?;
        require_pixel(&pixels, 32, 64, expected, "nested Solo signed workspace")?;
    }
    scene
        .set_solo(Some(LayerTreeNodeId::Group(OVERLAY_GROUP)))
        .map_err(debug_error)?;
    scene
        .set_visibility(LayerTreeNodeId::Group(nested), false)
        .map_err(debug_error)?;
    scene
        .render_viewport(viewport(Point::default(), 1.0, 0.0))
        .map_err(debug_error)?;
    require_pixel(
        &readback(&scene, device, queue)?,
        64,
        64,
        [255, 0, 0, 255],
        "group Solo still respects hidden descendants",
    )?;
    Ok(())
}

fn verify_tree_replacement(
    scene: &mut GpuCompositeScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(), Box<dyn std::error::Error>> {
    let tree = fixture_tree().map_err(debug_error)?;
    scene.replace_tree(tree.clone()).map_err(debug_error)?;
    scene
        .upload_layer_rgba8(BOTTOM, &solid([255, 0, 0, 255]))
        .map_err(debug_error)?;
    scene
        .upload_layer_rgba8(TOP, &solid([0, 0, 255, 255]))
        .map_err(debug_error)?;
    let mut deleted = tree.clone();
    deleted
        .remove(LayerTreeNodeId::Group(OVERLAY_GROUP))
        .map_err(debug_error)?;
    scene.replace_tree(deleted).map_err(debug_error)?;
    let identity = viewport(Point::default(), 1.0, 0.0);
    scene.render_viewport(identity).map_err(debug_error)?;
    require_pixel(
        &readback(scene, device, queue)?,
        64,
        64,
        [255, 0, 0, 255],
        "deleted group retains underlying raster",
    )?;
    scene.replace_tree(tree).map_err(debug_error)?;
    scene
        .upload_closed_tile(
            TileKey {
                layer: TOP,
                mip: 0,
                x: 0,
                y: 0,
            },
            &[0, 255, 0, 255].repeat(TILE_BYTE_LEN / 4),
        )
        .map_err(debug_error)?;
    scene.render_viewport(identity).map_err(debug_error)?;
    let restored = readback(scene, device, queue)?;
    require_pixel(
        &restored,
        64,
        64,
        [127, 128, 0, 255],
        "restored group and CPU tile",
    )?;
    require_pixel(
        &restored,
        192,
        192,
        [255, 0, 0, 255],
        "deleted surface pixels do not leak into restoration",
    )?;
    Ok(())
}

fn over_budget_tree() -> Result<LayerTree, nyatidraw_document::LayerTreeError> {
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: GroupId(900),
        name: "8K guard root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: (0..20_u128)
            .map(|index| raster(LayerId(1_000 + index), "8K raster"))
            .collect(),
    })
}

fn fixture_tree() -> Result<LayerTree, nyatidraw_document::LayerTreeError> {
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![
            raster(BOTTOM, "Bottom"),
            LayerTreeNode::Group(GroupNode {
                clip_to_below: false,
                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                id: OVERLAY_GROUP,
                name: "Overlay group".into(),
                visible: true,
                opacity_u16: 32_768,
                children: vec![raster(TOP, "Top")],
            }),
        ],
    })
}

fn raster(id: LayerId, name: &str) -> LayerTreeNode {
    LayerTreeNode::Raster(LayerNode {
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id,
        name: name.into(),
        visible: true,
        locked: false,
        reference: false,
        opacity_u16: u16::MAX,
        content_root: ContentRootId(0),
    })
}

fn viewport(pan: Point, zoom: f64, rotation_radians: f64) -> ViewportTransform {
    ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [SIZE, SIZE],
        dpi_scale: 1.0,
        pan,
        zoom,
        rotation_radians,
        mirrored_horizontal: false,
    }
}

fn solid(color: [u8; 4]) -> Vec<u8> {
    vec![color; usize::try_from(SIZE * SIZE).expect("fixture pixel count")]
        .into_iter()
        .flatten()
        .collect()
}

fn quadrants() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(usize::try_from(SIZE * SIZE * 4).expect("fixture bytes"));
    for y in 0..SIZE {
        for x in 0..SIZE {
            let color = match (x < SIZE / 2, y < SIZE / 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 0, 255],
            };
            pixels.extend_from_slice(&color);
        }
    }
    pixels
}

fn readback(
    scene: &GpuCompositeScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let display = scene.display().ok_or("display texture is missing")?;
    Ok(display
        .readback_rgba8(device, queue)
        .map_err(debug_error)?
        .pixels)
}

fn require_pixel(
    pixels: &[u8],
    x: u32,
    y: u32,
    expected: [u8; 4],
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let index = usize::try_from((y * SIZE + x) * 4).expect("fixture pixel index");
    let actual: [u8; 4] = pixels[index..index + 4].try_into()?;
    if actual
        .into_iter()
        .zip(expected)
        .any(|(actual, expected)| actual.abs_diff(expected) > TOLERANCE)
    {
        return Err(format!("{label}: expected {expected:?}, got {actual:?}").into());
    }
    Ok(())
}

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

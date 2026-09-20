//! Real-device probe for signed sparse tile reopen, display, live paint, and cancel.

use nyatidraw_api::{ContentRootId, GroupId, LayerId, TileCoordinate};
use nyatidraw_brush::BrushDab;
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_gpu::{GpuCompositeScene, GpuRoundDabPainter, LiveStrokeDisposition};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

const SIZE: u32 = 256;
const ROOT: GroupId = GroupId(1);
const LAYER: LayerId = LayerId(2);

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let tree = LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: LAYER,
            name: "Ink".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })],
    })
    .map_err(debug_error)?;
    let mut scene =
        GpuCompositeScene::new(&device, &queue, [SIZE, SIZE], tree).map_err(debug_error)?;
    let negative = TileKey {
        layer: LAYER,
        mip: 0,
        x: -1,
        y: 0,
    };
    let cyan = solid([0, 180, 210, 255]);
    scene
        .upload_closed_tile(negative, &cyan)
        .map_err(debug_error)?;
    let viewport = ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [SIZE, SIZE],
        dpi_scale: 1.0,
        pan: Point { x: 128.0, y: 0.0 },
        zoom: 1.0,
        rotation_radians: 0.0,
        mirrored_horizontal: false,
    };
    scene.render_viewport(viewport).map_err(debug_error)?;
    require_pixel(
        &readback(&scene, &device, &queue)?,
        32,
        64,
        [0, 180, 210, 255],
    )?;

    let mut painter = GpuRoundDabPainter::new(&device, &queue, [1.0, 0.0, 0.0, 1.0]);
    let token = scene.begin_live_stroke(LAYER).map_err(debug_error)?;
    let submission = scene
        .apply_live_dabs(
            &mut painter,
            token,
            &[BrushDab {
                center: Point { x: -64.0, y: 64.0 },
                radius_px: 18.0,
                opacity: 1.0,
                flow: 1.0,
                hardness: 1.0,
                grain: None,
            }],
        )
        .map_err(debug_error)?;
    if submission
        .document_dirty
        .is_none_or(|dirty| dirty.min_x >= 0)
    {
        return Err("live signed dirty bounds did not retain the negative tile".into());
    }
    scene.render_viewport(viewport).map_err(debug_error)?;
    require_pixel(
        &readback(&scene, &device, &queue)?,
        64,
        64,
        [255, 0, 0, 255],
    )?;
    scene
        .finish_live_stroke(token, LiveStrokeDisposition::Cancel)
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    require_pixel(
        &readback(&scene, &device, &queue)?,
        64,
        64,
        [0, 180, 210, 255],
    )?;

    let first = scene.begin_live_stroke(LAYER).map_err(debug_error)?;
    scene
        .apply_live_dabs(
            &mut painter,
            first,
            &[BrushDab {
                center: Point { x: -64.0, y: 64.0 },
                radius_px: 18.0,
                opacity: 1.0,
                flow: 1.0,
                hardness: 1.0,
                grain: None,
            }],
        )
        .map_err(debug_error)?;
    scene
        .finish_live_stroke(first, LiveStrokeDisposition::Commit)
        .map_err(debug_error)?;
    let mut replacement = GpuRoundDabPainter::new(&device, &queue, [0.0, 1.0, 0.0, 1.0]);
    let second = scene.begin_live_stroke(LAYER).map_err(debug_error)?;
    scene
        .apply_live_dabs(
            &mut replacement,
            second,
            &[BrushDab {
                center: Point { x: -64.0, y: 64.0 },
                radius_px: 18.0,
                opacity: 1.0,
                flow: 1.0,
                hardness: 1.0,
                grain: None,
            }],
        )
        .map_err(debug_error)?;
    scene
        .finish_live_stroke(second, LiveStrokeDisposition::Cancel)
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    require_pixel(
        &readback(&scene, &device, &queue)?,
        64,
        64,
        [255, 0, 0, 255],
    )?;

    let mirrored = viewport
        .with_view_at(Point { x: 128.0, y: 128.0 }, 1.0, 0.37, true)
        .ok_or("mirrored signed viewport")?;
    scene.render_viewport(mirrored).map_err(debug_error)?;
    let physical = mirrored
        .document_to_window(Point { x: -64.0, y: 64.0 })
        .ok_or("signed point")?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    require_pixel(
        &readback(&scene, &device, &queue)?,
        physical.x.floor() as u32,
        physical.y.floor() as u32,
        [255, 0, 0, 255],
    )?;

    check_workspace_background(&mut scene, &device, &queue, viewport)?;
    check_zoomed_out_workspace(&device, &queue, scene.tree().clone())?;

    println!(
        "{{\"event\":\"gpu_signed_workspace\",\"adapter\":{:?},\"backend\":{:?},\"negative_reopen_upload\":true,\"live_cancel_restored\":true,\"pending_preview_cancel_restored\":true,\"mirrored_rotated_signed_view\":true,\"workspace_background_isolated\":true,\"zoomed_out_atlas_batches\":true,\"pass\":true}}",
        info.name, info.backend,
    );
    Ok(())
}

fn check_workspace_background(
    scene: &mut GpuCompositeScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    viewport: ViewportTransform,
) -> Result<(), Box<dyn std::error::Error>> {
    let viewport = ViewportTransform {
        pan: Point { x: 128.0, y: 128.0 },
        ..viewport
    };
    scene.set_workspace_background([[188; 3], [172; 3]]);
    scene.render_viewport(viewport).map_err(debug_error)?;
    let checker = readback(scene, device, queue)?;
    require_pixel(
        &checker,
        8,
        8,
        nyatidraw_tiles::color::srgb8_to_linear_premultiplied([188, 188, 188, 255]),
    )?;
    require_pixel(
        &checker,
        24,
        8,
        nyatidraw_tiles::color::srgb8_to_linear_premultiplied([172, 172, 172, 255]),
    )?;
    require_pixel(&checker, 127, 160, [0, 0, 0, 255])?;
    scene.set_workspace_background([[32, 64, 96]; 2]);
    scene.render_viewport(viewport).map_err(debug_error)?;
    let solid = readback(scene, device, queue)?;
    require_pixel(
        &solid,
        8,
        8,
        nyatidraw_tiles::color::srgb8_to_linear_premultiplied([32, 64, 96, 255]),
    )?;
    let inside = usize::try_from((136 * SIZE + 136) * 4)?;
    if checker[inside..inside + 4] != solid[inside..inside + 4] {
        return Err("workspace backdrop changed the page checkerboard".into());
    }
    let root = scene
        .readback_resident_group_tile(ROOT, TileCoordinate { mip: 0, x: 0, y: 0 })
        .map_err(debug_error)?
        .ok_or("page group missing")?;
    if root.pixels.iter().any(|&value| value != 0) {
        return Err("workspace backdrop contaminated transparent artwork".into());
    }
    Ok(())
}

fn check_zoomed_out_workspace(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tree: LayerTree,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut scene =
        GpuCompositeScene::new(device, queue, [SIZE, SIZE], tree).map_err(debug_error)?;
    let columns = 32;
    let rows =
        i32::try_from((device.limits().max_texture_array_layers.min(2048) / 2 + 32).div_ceil(32))?;
    let mut expected = Vec::new();
    for row in 1..=rows {
        for column in 1..=columns {
            let key = TileKey {
                layer: LAYER,
                mip: 0,
                x: -column,
                y: -row,
            };
            let color = [u8::try_from(column * 5)?, u8::try_from(row * 5)?, 180, 255];
            scene
                .upload_closed_tile(key, &solid(color))
                .map_err(debug_error)?;
            expected.push((key, color));
        }
    }
    let viewport = ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [1024, 768],
        dpi_scale: 1.0,
        pan: Point { x: 800.0, y: 650.0 },
        zoom: 0.14,
        rotation_radians: 0.0,
        mirrored_horizontal: false,
    };
    for _ in 0..2 {
        let stats = scene.render_viewport(viewport).map_err(debug_error)?;
        if stats.display_passes < 3 {
            return Err("workspace regression did not exercise multiple atlas batches".into());
        }
        let pixels = scene
            .readback_display()
            .map_err(debug_error)?
            .ok_or("display missing")?;
        for &(key, expected) in &expected {
            let point = viewport
                .document_to_window(Point {
                    x: f64::from(key.x) * 128.0 + 64.0,
                    y: f64::from(key.y) * 128.0 + 64.0,
                })
                .ok_or("workspace tile projection failed")?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let index =
                usize::try_from(((point.y.floor() as u32) * 1024 + point.x.floor() as u32) * 4)?;
            if pixels.pixels[index..index + 4] != expected {
                return Err(format!("zoomed-out workspace lost tile {key:?}").into());
            }
        }
    }
    Ok(())
}

fn solid(color: [u8; 4]) -> Vec<u8> {
    vec![color; TILE_BYTE_LEN / 4]
        .into_iter()
        .flatten()
        .collect()
}

fn readback(
    scene: &GpuCompositeScene,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    Ok(scene
        .display()
        .ok_or("display is missing")?
        .readback_rgba8(device, queue)
        .map_err(debug_error)?
        .pixels)
}

fn require_pixel(
    pixels: &[u8],
    x: u32,
    y: u32,
    expected: [u8; 4],
) -> Result<(), Box<dyn std::error::Error>> {
    let index = usize::try_from((y * SIZE + x) * 4)?;
    let actual: [u8; 4] = pixels[index..index + 4].try_into()?;
    if actual != expected {
        return Err(format!("pixel ({x}, {y}): expected {expected:?}, got {actual:?}").into());
    }
    Ok(())
}

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

//! Real-device probe for signed sparse tile reopen, display, live paint, and cancel.

use nyatidraw_api::{ContentRootId, GroupId, LayerId};
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
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let tree = LayerTree::new(GroupNode {
        id: ROOT,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
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

    println!(
        "{{\"event\":\"gpu_signed_workspace\",\"adapter\":{:?},\"backend\":{:?},\"negative_reopen_upload\":true,\"live_cancel_restored\":true,\"pending_preview_cancel_restored\":true,\"pass\":true}}",
        info.name, info.backend,
    );
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

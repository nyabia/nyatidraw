//! Real-device source-atop/selection/cancel probe, not physical-pen evidence.
//! Run `cargo run -p nyatidraw-paint-gpu --example gpu_alpha_lock --release --locked`.
use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode, LayerId};
use nyatidraw_brush::BrushDab;
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::Point;
use nyatidraw_paint_cpu::{
    CpuCanvas, GPU_UNORM_CLOSED_STROKE_TOLERANCE, PremultipliedRgba8, SelectionMask,
    compare_rgba8_premultiplied,
};
use nyatidraw_paint_gpu::{GpuCompositeScene, GpuRoundDabPainter, LiveStrokeDisposition};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

const LAYER: LayerId = LayerId(2);
const COLOR: [u8; 4] = [0, 128, 64, 128];

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nyatidraw-alpha-lock-probe"),
        ..Default::default()
    }))?;
    let tree = LayerTree::new(GroupNode {
        id: GroupId(1),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LAYER,
            name: "Alpha locked".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
            alpha_locked: true,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
        })],
    })
    .map_err(debug_error)?;
    let mut scene =
        GpuCompositeScene::new(&device, &queue, [128, 128], tree).map_err(debug_error)?;
    let mut painter =
        GpuRoundDabPainter::new_alpha_locked(&device, &queue, [0.0, 1.0, 0.5, 128.0 / 255.0]);
    let before = (0..TILE_BYTE_LEN / 4)
        .flat_map(|index| {
            let alpha = [0, 1, 64, 128, 255][index % 5];
            [alpha / 2, alpha / 3, 0, alpha]
        })
        .collect::<Vec<_>>();
    let keys = [-1, 0].map(|x| TileKey {
        layer: LAYER,
        mip: 0,
        x,
        y: 0,
    });
    let mask = selection()?;
    let gpu_mask = painter
        .create_selection_mask_at(mask.origin(), mask.dimensions(), &mask.packed_bits())
        .map_err(debug_error)?;
    let mut max_delta = 0;
    let mut cases = 0;
    for hardness in [1.0, 0.25] {
        for selected in [false, true] {
            for key in keys {
                scene
                    .upload_closed_tile(key, &before)
                    .map_err(debug_error)?;
            }
            painter.set_selection_mask(selected.then_some(&gpu_mask));
            let dabs = (0..12)
                .map(|index| BrushDab {
                    center: Point {
                        x: -25.0 + f64::from(index) * 5.0,
                        y: 30.0 + f64::from(index % 3) * 3.0,
                    },
                    radius_px: 11.5,
                    opacity: 0.65,
                    flow: 0.55,
                    hardness,
                    grain: None,
                })
                .collect::<Vec<_>>();
            let token = scene.begin_live_stroke(LAYER).map_err(debug_error)?;
            for chunk in dabs.chunks(3) {
                scene
                    .apply_live_dabs(&mut painter, token, chunk)
                    .map_err(debug_error)?;
            }
            let mut previews = Vec::new();
            let mut changed = 0;
            for key in keys {
                let expected = expected(&before, key, &dabs, selected.then_some(&mask))?;
                let actual = scene
                    .readback_resident_raster_tile(key)
                    .map_err(debug_error)?
                    .ok_or("resident alpha-lock tile missing")?;
                let diff = compare_rgba8_premultiplied(
                    &expected,
                    actual.width,
                    actual.height,
                    &actual.pixels,
                    GPU_UNORM_CLOSED_STROKE_TOLERANCE,
                )
                .map_err(debug_error)?;
                if !diff.is_within() {
                    return Err(format!("alpha-lock GPU/CPU difference: {diff:?}").into());
                }
                max_delta = max_delta.max(diff.max_channel_delta);
                let (origin_x, origin_y) = key.pixel_origin();
                for (index, (old, new)) in before
                    .chunks_exact(4)
                    .zip(actual.pixels.chunks_exact(4))
                    .enumerate()
                {
                    if old[3] != new[3] {
                        return Err("GPU source-atop changed an alpha byte".into());
                    }
                    if new[..3].iter().any(|channel| *channel > new[3]) {
                        return Err("GPU source-atop broke premultiplied color".into());
                    }
                    let x = i32::try_from(origin_x + i64::try_from(index % 128)?)?;
                    let y = i32::try_from(origin_y + i64::try_from(index / 128)?)?;
                    if selected && !mask.contains_signed(x, y) && old != new {
                        return Err("GPU source-atop escaped signed selection or its holes".into());
                    }
                    changed += usize::from(old != new);
                }
                previews.push(actual.pixels);
            }
            if changed == 0 {
                return Err("alpha-lock fixture did not recolor any existing artwork".into());
            }
            scene
                .finish_live_stroke(token, LiveStrokeDisposition::Cancel)
                .map_err(debug_error)?;
            for key in keys {
                let canceled = scene
                    .readback_resident_raster_tile(key)
                    .map_err(debug_error)?
                    .ok_or("canceled alpha-lock tile missing")?;
                if canceled.pixels != before {
                    return Err("alpha-lock Cancel failed exact restoration".into());
                }
            }
            let token = scene.begin_live_stroke(LAYER).map_err(debug_error)?;
            for chunk in dabs.chunks(3) {
                scene
                    .apply_live_dabs(&mut painter, token, chunk)
                    .map_err(debug_error)?;
            }
            scene
                .finish_live_stroke(token, LiveStrokeDisposition::Commit)
                .map_err(debug_error)?;
            for (key, preview) in keys.into_iter().zip(previews) {
                let committed = scene
                    .readback_resident_raster_tile(key)
                    .map_err(debug_error)?
                    .ok_or("committed alpha-lock tile missing")?;
                if committed.pixels != preview {
                    return Err("alpha-lock Commit differed from identical preview".into());
                }
            }
            cases += 1;
        }
    }
    println!(
        "{{\"event\":\"gpu_alpha_lock\",\"adapter\":{:?},\"backend\":{:?},\"driver\":{:?},\"os\":{:?},\"release\":{},\"cases\":{},\"max_rgb_delta\":{},\"exact_alpha\":true,\"signed_selection\":true,\"cancel_exact\":true,\"commit_exact\":true,\"physical_pen_proof\":false,\"pass\":true}}",
        info.name,
        info.backend,
        info.driver,
        std::env::consts::OS,
        !cfg!(debug_assertions),
        cases,
        max_delta
    );
    Ok(())
}

fn selection() -> Result<SelectionMask, Box<dyn std::error::Error>> {
    let mut packed = vec![0; 64 * 56 / 8];
    for y in 0..56_usize {
        for x in 0..64_usize {
            if x % 11 != 0 {
                let index = y * 64 + x;
                packed[index / 8] |= 1 << (index % 8);
            }
        }
    }
    Ok(SelectionMask::from_packed_bits_at([-24, 12], [64, 56], &packed).map_err(debug_error)?)
}

fn expected(
    before: &[u8],
    key: TileKey,
    dabs: &[BrushDab],
    mask: Option<&SelectionMask>,
) -> Result<CpuCanvas, Box<dyn std::error::Error>> {
    let (origin_x, origin_y) = key.pixel_origin();
    let origin_x = i32::try_from(origin_x)?;
    let origin_y = i32::try_from(origin_y)?;
    let mut cpu =
        CpuCanvas::from_rgba8_premultiplied(128, 128, before.to_vec()).map_err(debug_error)?;
    for dab in dabs {
        cpu.apply_dab_alpha_locked(
            BrushDab {
                center: Point {
                    x: dab.center.x - f64::from(origin_x),
                    y: dab.center.y - f64::from(origin_y),
                },
                ..*dab
            },
            PremultipliedRgba8(COLOR),
        );
    }
    let mut pixels = cpu.pixels_rgba8_premultiplied().to_vec();
    if let Some(mask) = mask {
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let x = origin_x + i32::try_from(index % 128)?;
            let y = origin_y + i32::try_from(index / 128)?;
            if !mask.contains_signed(x, y) {
                pixel.copy_from_slice(&before[index * 4..index * 4 + 4]);
            }
        }
    }
    CpuCanvas::from_rgba8_premultiplied(128, 128, pixels).map_err(|error| debug_error(error).into())
}

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

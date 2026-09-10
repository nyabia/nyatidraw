//! Real-device probe for transactional live raster strokes.
//!
//! Run with
//! `cargo run --release -p nyatidraw-paint-gpu --example gpu_live_stroke_transaction --locked`.

use nyatidraw_api::{ContentRootId, GroupId, LayerId};
use nyatidraw_brush::BrushDab;
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_input::{Point, ViewportTransform};
use nyatidraw_paint_gpu::{
    GpuCompositeScene, GpuRoundDabPainter, LayerUploadError, LiveStrokeDisposition, LiveStrokeError,
};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

const SIZE: u32 = 256;
const ROOT: GroupId = GroupId(700);
const BOTTOM: LayerId = LayerId(701);
const INK: LayerId = LayerId(702);

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
        label: Some("nayati-live-stroke-transaction-probe"),
        ..Default::default()
    }))?;

    let mut scene =
        GpuCompositeScene::new(&device, &queue, [SIZE, SIZE], tree().map_err(debug_error)?)
            .map_err(debug_error)?;
    scene
        .upload_layer_rgba8(BOTTOM, &solid([12, 34, 56, 255]))
        .map_err(debug_error)?;
    let viewport = identity_viewport();
    scene.render_viewport(viewport).map_err(debug_error)?;
    let base = readback(&scene, &device, &queue)?;
    let mut painter = GpuRoundDabPainter::new(&device, &queue, [1.0, 0.1, 0.05, 1.0]);

    let cancel_token = scene.begin_live_stroke(INK).map_err(debug_error)?;
    match scene.begin_live_stroke(BOTTOM) {
        Err(LiveStrokeError::AlreadyActive(active)) if active == cancel_token => {}
        result => return Err(format!("nested Begin was not rejected: {result:?}").into()),
    }
    match scene.upload_closed_tile(
        TileKey {
            layer: INK,
            mip: 0,
            x: 0,
            y: 0,
        },
        &vec![0; TILE_BYTE_LEN],
    ) {
        Err(LayerUploadError::LiveStrokeActive(active)) if active == cancel_token => {}
        result => return Err(format!("active-layer mutation was not rejected: {result:?}").into()),
    }

    scene
        .apply_live_dabs(&mut painter, cancel_token, &[dab(64.0, 64.0)])
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    let first_live_frame = readback(&scene, &device, &queue)?;
    if first_live_frame == base {
        return Err("first live frame did not change".into());
    }
    scene
        .apply_live_dabs(&mut painter, cancel_token, &[dab(192.0, 192.0)])
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    let second_live_frame = readback(&scene, &device, &queue)?;
    if second_live_frame == first_live_frame {
        return Err("second live frame did not change".into());
    }
    let cancelled = scene
        .finish_live_stroke(cancel_token, LiveStrokeDisposition::Cancel)
        .map_err(debug_error)?;
    if cancelled.dirty_tiles.len() != 2 {
        return Err(format!(
            "cancelled stroke backed {} exact tiles instead of 2",
            cancelled.dirty_tiles.len()
        )
        .into());
    }
    scene.render_viewport(viewport).map_err(debug_error)?;
    let restored = readback(&scene, &device, &queue)?;
    if restored != base {
        return Err("Cancel did not restore the exact base pixels".into());
    }

    let commit_token = scene.begin_live_stroke(INK).map_err(debug_error)?;
    scene
        .apply_live_dabs(&mut painter, commit_token, &[dab(64.0, 192.0)])
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    let commit_preview = readback(&scene, &device, &queue)?;
    scene
        .finish_live_stroke(commit_token, LiveStrokeDisposition::Commit)
        .map_err(debug_error)?;
    scene.render_viewport(viewport).map_err(debug_error)?;
    let committed = readback(&scene, &device, &queue)?;
    if committed != commit_preview || committed == base {
        return Err("Commit did not retain the exact live GPU result".into());
    }

    let replace_source = scene.begin_live_stroke(INK).map_err(debug_error)?;
    scene
        .apply_live_dabs(&mut painter, replace_source, &[dab(200.0, 40.0)])
        .map_err(debug_error)?;
    let replacement = scene.replace_live_stroke(BOTTOM).map_err(debug_error)?;
    if replacement
        .cancelled
        .as_ref()
        .is_none_or(|outcome| outcome.token != replace_source)
        || replacement.active.layer() != BOTTOM
        || replacement.active.ordinal() <= replace_source.ordinal()
    {
        return Err("replacement did not cancel and advance the pinned stroke".into());
    }
    scene.render_viewport(viewport).map_err(debug_error)?;
    if readback(&scene, &device, &queue)? != committed {
        return Err("replacement Begin did not restore the replaced stroke".into());
    }
    match scene.apply_live_dabs(&mut painter, replace_source, &[dab(20.0, 20.0)]) {
        Err(LiveStrokeError::StaleToken { expected, actual })
            if expected == replacement.active && actual == replace_source => {}
        result => return Err(format!("stale stroke token was not rejected: {result:?}").into()),
    }
    scene
        .finish_live_stroke(replacement.active, LiveStrokeDisposition::Cancel)
        .map_err(debug_error)?;

    println!(
        concat!(
            "{{\"event\":\"gpu_live_stroke_transaction\",",
            "\"adapter_name\":{:?},\"backend\":{:?},\"device_type\":{:?},",
            "\"release_profile\":{},\"cancel_tiles\":{},",
            "\"cancel_exact\":true,\"commit_exact\":true,",
            "\"replacement_cancelled\":true,\"stale_rejected\":true,",
            "\"full_document_readbacks_in_live_path\":0,\"pass\":true}}"
        ),
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        !cfg!(debug_assertions),
        cancelled.dirty_tiles.len(),
    );
    Ok(())
}

fn tree() -> Result<LayerTree, nyatidraw_document::LayerTreeError> {
    LayerTree::new(GroupNode {
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: ROOT,
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![raster(BOTTOM, "Background"), raster(INK, "Ink")],
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

fn identity_viewport() -> ViewportTransform {
    ViewportTransform {
        revision: 1,
        window_origin_physical: Point::default(),
        physical_size: [SIZE, SIZE],
        dpi_scale: 1.0,
        pan: Point::default(),
        zoom: 1.0,
        rotation_radians: 0.0,
        mirrored_horizontal: false,
    }
}

fn dab(x: f64, y: f64) -> BrushDab {
    BrushDab {
        center: Point { x, y },
        radius_px: 18.0,
        opacity: 0.9,
        flow: 0.85,
        hardness: 1.0,
    }
}

fn solid(color: [u8; 4]) -> Vec<u8> {
    vec![color; usize::try_from(SIZE * SIZE).expect("fixture pixel count")]
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
        .ok_or("display texture is missing")?
        .readback_rgba8(device, queue)
        .map_err(debug_error)?
        .pixels)
}

fn debug_error(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

//! Explicit hardware probe for restoring closed CPU snapshot bytes after a GPU device recreation.
//!
//! Run with `cargo run -p nyatidraw-paint-gpu --example gpu_device_restore --release`.
//! This deliberately creates two real devices and performs one readback from
//! each restored texture; it is not a per-frame recovery implementation.

use nyatidraw_brush::BrushDab;
use nyatidraw_input::Point;
use nyatidraw_paint_cpu::{
    CpuCanvas, ImageDiffStats, ImageDiffTolerance, PremultipliedRgba8, compare_rgba8_premultiplied,
};
use nyatidraw_paint_gpu::GpuRoundDabPainter;

const WIDTH: u32 = 37;
const HEIGHT: u32 = 29;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let adapter_info = adapter.get_info();
    let snapshot = closed_snapshot();
    let first = restore_once(&adapter, &snapshot)?;
    let second = restore_once(&adapter, &snapshot)?;
    let passed = first.is_within() && second.is_within();

    println!(
        concat!(
            "{{\"event\":\"gpu_device_restore\",",
            "\"adapter_name\":{:?},\"backend\":\"{:?}\",",
            "\"device_type\":\"{:?}\",",
            "\"dimensions\":{{\"width\":{},\"height\":{}}},",
            "\"generation_1\":{},\"generation_2\":{},\"pass\":{}}}"
        ),
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        WIDTH,
        HEIGHT,
        diff_json(first),
        diff_json(second),
        passed,
    );
    if !passed {
        return Err(
            std::io::Error::other("restored GPU bytes differ from closed CPU snapshot").into(),
        );
    }
    Ok(())
}

fn closed_snapshot() -> CpuCanvas {
    let mut snapshot = CpuCanvas::new(WIDTH, HEIGHT);
    snapshot.apply_dabs(
        &[
            dab(3.25, 4.75, 4.5, 0.8, 0.9),
            dab(11.875, 8.5, 4.375, 0.65, 0.7),
            dab(18.25, 16.125, 6.25, 0.9, 0.8),
            dab(34.375, 27.625, 5.0, 0.55, 0.95),
        ],
        PremultipliedRgba8([64, 128, 192, 255]),
    );
    snapshot
}

fn restore_once(
    adapter: &wgpu::Adapter,
    snapshot: &CpuCanvas,
) -> Result<ImageDiffStats, Box<dyn std::error::Error>> {
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nayati-gpu-device-restore-probe"),
        ..Default::default()
    }))?;
    let painter = GpuRoundDabPainter::new(&device, &queue, [0.0; 4]);
    let tile = painter
        .restore_closed_rgba8(
            snapshot.width(),
            snapshot.height(),
            snapshot.pixels_rgba8_premultiplied(),
        )
        .map_err(|error| std::io::Error::other(format!("GPU restore failed: {error:?}")))?;
    let readback = tile
        .readback_rgba8(&device, &queue)
        .map_err(|error| std::io::Error::other(format!("GPU readback failed: {error:?}")))?;
    compare_rgba8_premultiplied(
        snapshot,
        readback.width,
        readback.height,
        &readback.pixels,
        ImageDiffTolerance::exact(),
    )
    .map_err(|error| std::io::Error::other(format!("restore diff failed: {error:?}")).into())
}

fn diff_json(stats: ImageDiffStats) -> String {
    format!(
        concat!(
            "{{\"compared_pixels\":{},\"differing_pixels\":{},",
            "\"max_channel_delta\":{},\"out_of_tolerance_pixels\":{}}}"
        ),
        stats.compared_pixels,
        stats.differing_pixels,
        stats.max_channel_delta,
        stats.out_of_tolerance_pixels,
    )
}

fn dab(x: f64, y: f64, radius_px: f32, opacity: f32, flow: f32) -> BrushDab {
    BrushDab {
        center: Point { x, y },
        radius_px,
        opacity,
        flow,
        hardness: 1.0,
    }
}

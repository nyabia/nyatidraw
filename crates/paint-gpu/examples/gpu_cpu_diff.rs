//! Explicit hardware probe for one closed GPU dab batch versus the CPU reference.
//!
//! Run with `cargo run -p nyatidraw-paint-gpu --example gpu_cpu_diff --release`.
//! This is deliberately an example, rather than an automated test: it creates a
//! real adapter/device and performs one synchronous readback after the batch.

use nyatidraw_brush::BrushDab;
use nyatidraw_input::Point;
use nyatidraw_paint_cpu::{
    CpuCanvas, GPU_UNORM_CLOSED_STROKE_TOLERANCE, ImageDiffStats, ImageDiffTolerance,
    PremultipliedRgba8, compare_rgba8_premultiplied,
};
use nyatidraw_paint_gpu::GpuRoundDabPainter;

const WIDTH: u32 = 37;
const HEIGHT: u32 = 29;

struct ProbeFixture {
    name: &'static str,
    brush_rgba8: [u8; 4],
    dabs: Vec<BrushDab>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))?;
    let adapter_info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("nayati-gpu-cpu-diff-probe"),
        ..Default::default()
    }))?;

    let fixtures = probe_fixtures();
    let mut passed = true;
    let mut fixture_output = Vec::with_capacity(fixtures.len());

    for fixture in fixtures {
        let (submission_count, strict, accepted) = run_fixture(&device, &queue, &fixture)?;
        passed &= accepted.is_within();
        fixture_output.push(format!(
            concat!(
                "{{\"name\":\"{}\",\"dabs_submitted\":{},\"dabs_applied\":{},",
                "\"strict_diff\":{},\"accepted_diff\":{}}}"
            ),
            fixture.name,
            fixture.dabs.len(),
            submission_count,
            diff_json(strict),
            diff_json(accepted),
        ));
    }
    let (eraser_submission_count, eraser_strict, eraser_accepted) =
        run_eraser_fixture(&device, &queue)?;
    passed &= eraser_accepted.is_within();

    println!(
        concat!(
            "{{\"event\":\"gpu_cpu_diff\",",
            "\"adapter_name\":{:?},\"backend\":\"{:?}\",",
            "\"device_type\":\"{:?}\",",
            "\"dimensions\":{{\"width\":{},\"height\":{}}},",
            "\"tolerance\":{{\"max_channel_delta\":{},",
            "\"max_out_of_tolerance_pixels\":{}}},",
            "\"fixtures\":[{}],\"eraser\":{{\"dabs_applied\":{},",
            "\"strict_diff\":{},\"accepted_diff\":{}}},\"pass\":{}}}"
        ),
        adapter_info.name,
        adapter_info.backend,
        adapter_info.device_type,
        WIDTH,
        HEIGHT,
        GPU_UNORM_CLOSED_STROKE_TOLERANCE.max_channel_delta,
        GPU_UNORM_CLOSED_STROKE_TOLERANCE.max_out_of_tolerance_pixels,
        fixture_output.join(","),
        eraser_submission_count,
        diff_json(eraser_strict),
        diff_json(eraser_accepted),
        passed,
    );

    if !passed {
        return Err(std::io::Error::other(
            "GPU/CPU diff exceeds the closed-stroke UNORM tolerance",
        )
        .into());
    }

    Ok(())
}

fn run_eraser_fixture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Result<(u32, ImageDiffStats, ImageDiffStats), Box<dyn std::error::Error>> {
    let initial_pixel = [128, 64, 32, 128];
    let pixel_count = usize::try_from(WIDTH)
        .expect("probe width fits usize")
        .checked_mul(usize::try_from(HEIGHT).expect("probe height fits usize"))
        .expect("probe dimensions do not overflow");
    let initial = initial_pixel.repeat(pixel_count);
    let dabs: Vec<_> = overlapping_dabs(7, 18.25, 14.0, 5.25, 0.38, 0.52)
        .into_iter()
        .enumerate()
        .map(|(index, dab)| BrushDab {
            hardness: if index % 2 == 0 { 0.2 } else { 1.0 },
            ..dab
        })
        .collect();
    let mut eraser = GpuRoundDabPainter::new_eraser(device, queue);
    let tile = eraser
        .restore_closed_rgba8(WIDTH, HEIGHT, &initial)
        .map_err(|error| std::io::Error::other(format!("GPU restore failed: {error:?}")))?;
    let submission = eraser.apply(&tile, &dabs);
    let readback = tile
        .readback_rgba8(device, queue)
        .map_err(|error| std::io::Error::other(format!("GPU readback failed: {error:?}")))?;

    let mut reference = CpuCanvas::from_rgba8_premultiplied(WIDTH, HEIGHT, initial)
        .map_err(|error| std::io::Error::other(format!("CPU restore failed: {error:?}")))?;
    reference.erase_dabs(&dabs);
    let strict = compare_rgba8_premultiplied(
        &reference,
        readback.width,
        readback.height,
        &readback.pixels,
        ImageDiffTolerance::exact(),
    )
    .map_err(|error| std::io::Error::other(format!("CPU/GPU diff failed: {error:?}")))?;
    let accepted = compare_rgba8_premultiplied(
        &reference,
        readback.width,
        readback.height,
        &readback.pixels,
        GPU_UNORM_CLOSED_STROKE_TOLERANCE,
    )
    .map_err(|error| std::io::Error::other(format!("CPU/GPU diff failed: {error:?}")))?;
    Ok((submission.dab_count, strict, accepted))
}

fn run_fixture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fixture: &ProbeFixture,
) -> Result<(u32, ImageDiffStats, ImageDiffStats), Box<dyn std::error::Error>> {
    let mut painter = GpuRoundDabPainter::new(device, queue, rgba8_to_float(fixture.brush_rgba8));
    let tile = painter.create_working_tile(WIDTH, HEIGHT);
    let submission = painter.apply(&tile, &fixture.dabs);
    let readback = tile
        .readback_rgba8(device, queue)
        .map_err(|error| std::io::Error::other(format!("GPU readback failed: {error:?}")))?;

    let mut reference = CpuCanvas::new(WIDTH, HEIGHT);
    reference.apply_dabs(&fixture.dabs, PremultipliedRgba8(fixture.brush_rgba8));
    let strict = compare_rgba8_premultiplied(
        &reference,
        readback.width,
        readback.height,
        &readback.pixels,
        ImageDiffTolerance::exact(),
    )
    .map_err(|error| std::io::Error::other(format!("CPU/GPU diff failed: {error:?}")))?;
    let accepted = compare_rgba8_premultiplied(
        &reference,
        readback.width,
        readback.height,
        &readback.pixels,
        GPU_UNORM_CLOSED_STROKE_TOLERANCE,
    )
    .map_err(|error| std::io::Error::other(format!("CPU/GPU diff failed: {error:?}")))?;
    Ok((submission.dab_count, strict, accepted))
}

fn rgba8_to_float(rgba8: [u8; 4]) -> [f32; 4] {
    rgba8.map(|channel| f32::from(channel) / 255.0)
}

fn diff_json(stats: ImageDiffStats) -> String {
    format!(
        concat!(
            "{{\"compared_pixels\":{},\"differing_pixels\":{},",
            "\"out_of_tolerance_pixels\":{},\"max_channel_delta\":{},",
            "\"total_absolute_error\":{}}}"
        ),
        stats.compared_pixels,
        stats.differing_pixels,
        stats.out_of_tolerance_pixels,
        stats.max_channel_delta,
        stats.total_absolute_error,
    )
}

fn probe_fixtures() -> Vec<ProbeFixture> {
    vec![
        ProbeFixture {
            name: "soft_tips_and_clipped_falloff",
            brush_rgba8: [64, 128, 192, 255],
            dabs: vec![
                BrushDab {
                    hardness: 0.0,
                    ..dab(8.25, 7.75, 7.5, 0.9, 0.9)
                },
                BrushDab {
                    hardness: 0.35,
                    ..dab(18.25, 16.125, 6.25, 0.9, 0.8)
                },
                BrushDab {
                    hardness: 0.8,
                    ..dab(36.5, 28.5, 6.0, 0.41, 0.76)
                },
                BrushDab {
                    hardness: 0.0,
                    ..dab(-1.0, 22.0, 7.0, 0.7, 0.65)
                },
            ],
        },
        ProbeFixture {
            name: "soft_dense_overlap",
            brush_rgba8: [232, 24, 192, 255],
            dabs: overlapping_dabs(12, 23.0, 13.0, 7.75, 0.38, 0.52)
                .into_iter()
                .map(|dab| BrushDab {
                    hardness: 0.2,
                    ..dab
                })
                .collect(),
        },
        ProbeFixture {
            name: "overlap_blue",
            brush_rgba8: [64, 128, 192, 255],
            dabs: vec![
                dab(8.25, 7.75, 5.5, 0.78, 0.9),
                dab(11.875, 8.5, 4.375, 0.65, 0.7),
                dab(18.25, 16.125, 6.25, 0.9, 0.8),
                dab(32.375, 24.625, 5.0, 0.55, 0.95),
            ],
        },
        ProbeFixture {
            name: "clipped_red_edges",
            brush_rgba8: [255, 32, 16, 255],
            dabs: vec![
                dab(0.25, 0.5, 5.75, 0.81, 0.63),
                dab(36.75, 0.125, 4.625, 0.53, 0.92),
                dab(0.0, 28.875, 3.875, 0.97, 0.48),
                dab(36.5, 28.5, 6.0, 0.41, 0.76),
            ],
        },
        ProbeFixture {
            name: "low_flow_green",
            brush_rgba8: [8, 240, 72, 255],
            dabs: vec![
                dab(6.125, 20.75, 2.375, 0.08, 0.12),
                dab(7.5, 19.875, 2.625, 0.16, 0.09),
                dab(9.0, 19.0, 3.125, 0.11, 0.17),
                dab(10.375, 18.125, 2.875, 0.22, 0.14),
            ],
        },
        ProbeFixture {
            name: "dense_yellow_overlap",
            brush_rgba8: [252, 216, 12, 255],
            dabs: vec![
                dab(20.125, 5.375, 4.125, 0.95, 0.98),
                dab(20.75, 5.875, 4.125, 0.92, 0.94),
                dab(21.375, 6.375, 4.125, 0.89, 0.91),
                dab(22.0, 6.875, 4.125, 0.86, 0.88),
                dab(22.625, 7.375, 4.125, 0.83, 0.85),
            ],
        },
        ProbeFixture {
            name: "eight_low_alpha_cyan_overlap",
            brush_rgba8: [16, 224, 224, 255],
            dabs: overlapping_dabs(8, 18.25, 18.0, 4.5, 0.12, 0.14),
        },
        ProbeFixture {
            name: "twelve_mid_alpha_magenta_overlap",
            brush_rgba8: [232, 24, 192, 255],
            dabs: overlapping_dabs(12, 23.0, 13.0, 3.75, 0.38, 0.52),
        },
    ]
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

fn overlapping_dabs(
    count: usize,
    start_x: f64,
    start_y: f64,
    radius_px: f32,
    opacity: f32,
    flow: f32,
) -> Vec<BrushDab> {
    (0..count)
        .map(|index| {
            let offset = f64::from(u32::try_from(index).expect("fixture count fits u32")) * 0.375;
            dab(
                start_x + offset,
                start_y + offset * 0.75,
                radius_px,
                opacity,
                flow,
            )
        })
        .collect()
}

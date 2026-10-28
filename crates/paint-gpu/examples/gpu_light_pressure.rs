use nyatidraw_brush::{
    BrushDab, BrushEvaluator, BrushPreset, BrushPresetId, PencilKind, ROUND_BRUSH_ENGINE_VERSION,
    ROUND_BRUSH_PRESET_SCHEMA_VERSION, RoundBrushEvaluator, begin_round_stroke, pencil_preset,
};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_paint_cpu::{CpuCanvas, GPU_UNORM_CLOSED_STROKE_TOLERANCE, PremultipliedRgba8};
use nyatidraw_paint_gpu::GpuRoundDabPainter;

fn error(value: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{value:?}"))
}

fn stroke(preset: &BrushPreset, pressure: f32, offset: f64) -> Vec<BrushDab> {
    let first = StylusSample {
        sequence: 1,
        timestamp_ns: 1,
        device_id: 1,
        phase: PointerPhase::Begin,
        position_document: Point {
            x: -58.0,
            y: 16.0 + offset,
        },
        pressure,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    };
    let last = StylusSample {
        sequence: 2,
        timestamp_ns: 2,
        phase: PointerPhase::End,
        position_document: Point {
            x: -4.0,
            ..first.position_document
        },
        ..first
    };
    let mut evaluator = RoundBrushEvaluator::default();
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, preset, first, &mut dabs);
    evaluator.push(&mut token, &[last], &mut dabs);
    evaluator.end(token, &mut dabs);
    dabs.into_iter().map(|dab| dab.to_local(-64, 0)).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))?;
    let info = adapter.get_info();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
    let mut painter = GpuRoundDabPainter::new(&device, &queue, [0.0, 0.0, 0.0, 1.0]);
    let round = BrushPreset {
        id: BrushPresetId(1),
        schema_version: ROUND_BRUSH_PRESET_SCHEMA_VERSION,
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        size_px: 20.0,
        opacity: 1.0,
        flow: 1.0,
        spacing_ratio: 0.12,
        size_pressure: true,
        opacity_pressure: false,
        size_min_ratio: 0.0,
        opacity_min_ratio: 0.0,
        hardness: 1.0,
    };
    let mut presets = vec![
        round,
        BrushPreset {
            hardness: 0.0,
            ..round
        },
    ];
    for engine in [3, 5, 7] {
        for kind in [PencilKind::Mechanical2H, PencilKind::Graphite2B] {
            presets.push(BrushPreset {
                engine_version: engine,
                size_px: 5.0,
                size_min_ratio: 0.0,
                opacity_min_ratio: 0.0,
                ..pencil_preset(kind)
            });
        }
    }
    let mut max_delta = 0;
    let mut cases = 0;
    for preset in presets {
        for pressure in [0.002, 0.02, 0.05, 0.1, 1.0] {
            for offset in [0.0, 0.25, 0.5, 0.75] {
                let dabs = stroke(&preset, pressure, offset);
                let mut cpu = CpuCanvas::new(64, 32);
                cpu.apply_dabs(&dabs, PremultipliedRgba8([0, 0, 0, 255]));
                let tile = painter
                    .restore_closed_rgba8(64, 32, &vec![0; 64 * 32 * 4])
                    .map_err(error)?;
                painter.apply(&tile, &dabs);
                let gpu = tile.readback_rgba8(&device, &queue).map_err(error)?;
                let delta = cpu
                    .pixels_rgba8_premultiplied()
                    .iter()
                    .zip(&gpu.pixels)
                    .map(|(left, right)| left.abs_diff(*right))
                    .max()
                    .unwrap_or(0);
                if delta > GPU_UNORM_CLOSED_STROKE_TOLERANCE.max_channel_delta {
                    return Err(format!(
                        "engine={} pressure={pressure} offset={offset} delta={delta}",
                        preset.engine_version
                    )
                    .into());
                }
                max_delta = max_delta.max(delta);
                cases += 1;
            }
        }
    }
    println!(
        "adapter={:?} backend={:?} release={} cases={cases} max_delta={max_delta} physical_pen_proof=false PASS",
        info.name,
        info.backend,
        !cfg!(debug_assertions)
    );
    Ok(())
}

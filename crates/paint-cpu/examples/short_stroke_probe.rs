use nyatidraw_brush::{
    BrushDab, BrushEvaluator, BrushPreset, BrushPresetId, PencilKind, ROUND_BRUSH_ENGINE_VERSION,
    ROUND_BRUSH_PRESET_SCHEMA_VERSION, RoundBrushEvaluator, begin_round_stroke, pencil_preset,
};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_paint_cpu::{CpuCanvas, PremultipliedRgba8};

fn sample(sequence: u64, x: f64, y: f64, pressure: f32) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence * 1_000_000,
        device_id: 1,
        phase: if sequence == 1 {
            PointerPhase::Begin
        } else {
            PointerPhase::Move
        },
        position_document: Point { x, y },
        pressure,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    }
}

fn render(preset: &BrushPreset, samples: &[StylusSample]) -> (usize, usize, u8) {
    let mut evaluator = RoundBrushEvaluator::default();
    let mut dabs: Vec<BrushDab> = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, preset, samples[0], &mut dabs);
    let mut remaining = samples[1..].to_vec();
    if let Some(last) = remaining.last_mut() {
        last.phase = PointerPhase::End;
    }
    evaluator.push(&mut token, &remaining, &mut dabs);
    evaluator.end(token, &mut dabs);
    let mut canvas = CpuCanvas::new(64, 64);
    canvas.apply_dabs(&dabs, PremultipliedRgba8::new(0, 0, 0, 255));
    let alphas: Vec<_> = canvas
        .pixels_rgba8_premultiplied()
        .chunks_exact(4)
        .map(|rgba| rgba[3])
        .collect();
    (
        dabs.len(),
        alphas.iter().filter(|&&alpha| alpha > 0).count(),
        alphas.into_iter().max().unwrap_or(0),
    )
}

fn main() {
    let pen = BrushPreset {
        id: BrushPresetId(1),
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        schema_version: ROUND_BRUSH_PRESET_SCHEMA_VERSION,
        size_px: 20.0,
        opacity: 1.0,
        flow: 1.0,
        spacing_ratio: 0.08,
        size_pressure: true,
        opacity_pressure: false,
        size_min_ratio: 0.0,
        opacity_min_ratio: 0.0,
        hardness: 1.0,
    };
    for distance in [0.0, 1.0, 2.0] {
        let samples = [
            sample(1, 20.0, 20.0, 0.0),
            sample(2, 20.0 + distance, 20.0, 1.0),
            sample(3, 20.0 + distance, 20.0, 0.0),
        ];
        println!(
            "pen distance={distance}: dabs/nonzero_pixels/max_alpha={:?}",
            render(&pen, &samples)
        );
    }
    let pencil = pencil_preset(PencilKind::Mechanical2H);
    for pressure in [0.0, 0.1, 0.5, 1.0] {
        let mut empty = 0;
        let mut first_empty = None;
        for y in 8..24 {
            for x in 8..24 {
                let samples = [
                    sample(1, f64::from(x), f64::from(y), pressure),
                    sample(2, f64::from(x), f64::from(y), 0.0),
                ];
                if render(&pencil, &samples).1 == 0 {
                    empty += 1;
                    first_empty.get_or_insert((x, y));
                }
            }
        }
        println!("2H pressure={pressure}: transparent_taps={empty}/256 first={first_empty:?}");
    }
}

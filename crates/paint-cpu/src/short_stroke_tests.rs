use super::{CpuCanvas, PremultipliedRgba8};
use nyatidraw_brush::{
    BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, PencilKind,
    ROUND_BRUSH_ENGINE_VERSION, ROUND_BRUSH_PRESET_SCHEMA_VERSION, RoundBrushEvaluator,
    begin_round_stroke, pencil_preset, replay_round_stroke,
};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};

fn sample(sequence: u64, point: Point, pressure: f32, phase: PointerPhase) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence * 1_000,
        device_id: 1,
        phase,
        position_document: point,
        pressure,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    }
}

fn render(preset: BrushPreset, samples: &[StylusSample]) -> Vec<u8> {
    let mut evaluator = RoundBrushEvaluator::default();
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
    for sample in &samples[1..] {
        evaluator.push(&mut token, &[*sample], &mut dabs);
    }
    let record = evaluator.end(token, &mut dabs);
    let mut replay = Vec::new();
    replay_round_stroke(&BrushSnapshot { preset }, &record, samples, &mut replay).unwrap();
    assert_eq!(
        dabs, replay,
        "saved and differently batched input must agree"
    );
    let mut canvas = CpuCanvas::new(32, 32);
    canvas.apply_dabs(&dabs, PremultipliedRgba8::new(0, 0, 0, 255));
    canvas.pixels_rgba8_premultiplied().to_vec()
}

fn has_ink(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).any(|pixel| pixel[3] != 0)
}

#[test]
fn pressure_arriving_before_spacing_must_not_erase_short_strokes_or_change_legacy_replay() {
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
    for distance in [0.0, 0.01, 1.0] {
        for pressure in [0.1, 0.5, 1.0] {
            let start = Point { x: 16.0, y: 16.0 };
            let end = Point {
                x: 16.0 + distance,
                ..start
            };
            let samples = [
                sample(1, start, 0.0, PointerPhase::Begin),
                sample(2, end, pressure, PointerPhase::Move),
                sample(3, end, 0.0, PointerPhase::End),
            ];
            assert!(has_ink(&render(pen, &samples)));
            assert!(!has_ink(&render(
                BrushPreset {
                    engine_version: 2,
                    ..pen
                },
                &samples
            )));
            for invisible in [
                BrushPreset {
                    opacity: 0.0,
                    ..pen
                },
                BrushPreset { flow: 0.0, ..pen },
            ] {
                assert!(!has_ink(&render(invisible, &samples)));
            }
        }
    }
}

#[test]
fn tiny_pencil_taps_survive_grain_and_unorm_without_repainting_legacy_artwork() {
    let pencil = BrushPreset {
        engine_version: 5,
        ..pencil_preset(PencilKind::Mechanical2H)
    };
    let mut legacy_empty = [0; 4];
    for (index, pressure) in [0.0, 0.1, 0.5, 1.0].into_iter().enumerate() {
        for offset in [0.0, 0.25, 0.5, 0.75] {
            for y in 8..24 {
                for x in 8..24 {
                    let point = Point {
                        x: f64::from(x) + offset,
                        y: f64::from(y) + offset,
                    };
                    let samples = [
                        sample(1, point, pressure, PointerPhase::Begin),
                        sample(2, point, 0.0, PointerPhase::End),
                    ];
                    assert!(
                        has_ink(&render(pencil, &samples)),
                        "tap {point:?} pressure={pressure}"
                    );
                    if offset == 0.0
                        && !has_ink(&render(
                            BrushPreset {
                                engine_version: 3,
                                ..pencil
                            },
                            &samples,
                        ))
                    {
                        legacy_empty[index] += 1;
                    }
                }
            }
        }
    }
    assert_eq!(legacy_empty, [160, 34, 4, 0]);
    let point = Point { x: 16.0, y: 16.0 };
    let samples = [sample(1, point, 1.0, PointerPhase::Begin)];
    for invisible in [
        BrushPreset {
            opacity: 0.0,
            ..pencil
        },
        BrushPreset {
            flow: 0.0,
            ..pencil
        },
    ] {
        assert!(!has_ink(&render(invisible, &samples)));
    }
    let fixed = BrushPreset {
        size_pressure: false,
        opacity_pressure: false,
        opacity: 0.25,
        ..pencil
    };
    let first = sample(1, point, 0.0, PointerPhase::Begin);
    let later = sample(2, point, 1.0, PointerPhase::Move);
    assert_eq!(
        render(fixed, &[first]),
        render(fixed, &[first, later]),
        "disabled pressure must not add a second stationary dab"
    );
}

#[test]
fn light_pressure_strokes_cannot_break_into_disconnected_stamps_or_missing_pencil_tips() {
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
    let pencils = [PencilKind::Mechanical2H, PencilKind::Graphite2B].map(|kind| BrushPreset {
        size_px: 5.0,
        size_min_ratio: 0.0,
        opacity_min_ratio: 0.0,
        ..pencil_preset(kind)
    });
    for preset in [
        round,
        BrushPreset {
            hardness: 0.0,
            ..round
        },
        pencils[0],
        pencils[1],
    ] {
        let mut legacy_gaps = 0;
        let pressures = if preset.engine_version == ROUND_BRUSH_ENGINE_VERSION {
            [0.002, 0.005, 0.01]
        } else {
            [0.02, 0.05, 0.1]
        };
        for pressure in pressures {
            for offset in [0.0, 0.25, 0.5, 0.75] {
                let samples = [
                    sample(
                        1,
                        Point {
                            x: 4.0,
                            y: 16.0 + offset,
                        },
                        pressure,
                        PointerPhase::Begin,
                    ),
                    sample(
                        2,
                        Point {
                            x: 28.0,
                            y: 16.0 + offset,
                        },
                        pressure,
                        PointerPhase::Move,
                    ),
                ];
                let pixels = render(preset, &samples);
                let empty_columns = |pixels: &[u8]| {
                    (6..26)
                        .filter(|&x| (0..32).all(|y| pixels[(y * 32 + x) * 4 + 3] == 0))
                        .count()
                };
                assert_eq!(
                    empty_columns(&pixels),
                    0,
                    "{preset:?} pressure={pressure}, offset={offset}"
                );
                let old = BrushPreset {
                    engine_version: if preset.engine_version == ROUND_BRUSH_ENGINE_VERSION {
                        4
                    } else {
                        5
                    },
                    ..preset
                };
                legacy_gaps += empty_columns(&render(old, &samples));
            }
        }
        assert!(
            legacy_gaps > 0,
            "fixture must reproduce the reported lost ink for {preset:?}"
        );
    }
    assert_visible_subpixel_contacts(pencils);
}

fn assert_visible_subpixel_contacts(pencils: [BrushPreset; 2]) {
    for pencil in pencils {
        for offset in [0.0, 0.25, 0.5, 0.75] {
            let samples = [sample(
                1,
                Point {
                    x: 16.0 + offset,
                    y: 16.0 + offset,
                },
                0.05,
                PointerPhase::Begin,
            )];
            assert!(
                has_ink(&render(pencil, &samples)),
                "subpixel contact must survive rasterization"
            );
            for invisible in [
                BrushPreset {
                    opacity: 0.0,
                    ..pencil
                },
                BrushPreset {
                    flow: 0.0,
                    ..pencil
                },
            ] {
                assert!(
                    !has_ink(&render(invisible, &samples)),
                    "no artificial alpha floor"
                );
            }
        }
    }
}

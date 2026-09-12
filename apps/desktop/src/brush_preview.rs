//! Disposable, bounded chrome preview using the selected engine and CPU renderer.
//! Never admitted to the editor, project, history, or recent-size list.
use dioxus::prelude::*;
use nyatidraw_api::{BrushSettings, DrawingTool, PencilTemplate, UiProjection};
use nyatidraw_brush::{BrushEvaluator, RoundBrushEvaluator, begin_round_stroke};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_paint_cpu::{CpuCanvas, PremultipliedRgba8};
use nyatidraw_tiles::FlattenedRgba8;

#[component]
pub(crate) fn BrushStrokePreview(
    tool: DrawingTool,
    pencil_template: PencilTemplate,
    size_tenths: u16,
    opacity_u16: u16,
    settings: BrushSettings,
) -> Element {
    let image = use_memo(use_reactive(
        (
            &tool,
            &pencil_template,
            &size_tenths,
            &opacity_u16,
            &settings,
        ),
        move |(tool, pencil_template, size, opacity, settings)| {
            let mut projection = UiProjection::empty();
            projection.drawing_tool = tool;
            projection.pencil_template = pencil_template;
            projection.brush_size_tenths = size;
            projection.brush_opacity_u16 = opacity;
            projection.brush_settings = settings;
            render(&projection).ok()
        },
    ));
    rsx! { div { class: "brush-stroke-preview", title: "현재 브러시의 필압 변화 예시 · 흑백 표시, 큰 크기는 축소 · 보정 감각은 실제 획으로 확인",
        if let Some(source) = image.read().as_ref() {
            img { src: "{source}", draggable: "false", alt: "현재 브러시 획 미리보기" }
        }
    } }
}

fn render(projection: &UiProjection) -> Result<std::sync::Arc<str>, String> {
    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 48;
    let mut preset = crate::native_canvas::preset_for_ui_preview(projection);
    // A display thumbnail, not a 1:1 ruler. Bound allocations and raster work.
    preset.size_px = preset.size_px.min(26.0);
    let sample = |index: u32| {
        let t = f64::from(index) / 48.0;
        #[allow(clippy::cast_possible_truncation)]
        let pressure = (0.12 + 0.88 * (t * std::f64::consts::PI).sin()) as f32;
        StylusSample {
            sequence: u64::from(index),
            timestamp_ns: u64::from(index) * 4_000_000,
            device_id: 0,
            phase: if index == 0 {
                PointerPhase::Begin
            } else if index == 48 {
                PointerPhase::End
            } else {
                PointerPhase::Move
            },
            position_document: Point {
                x: 15.0 + t * 130.0,
                y: 24.0 + (t * std::f64::consts::TAU).sin() * 6.0,
            },
            pressure,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons(0),
            eraser: false,
            viewport_revision: 0,
        }
    };
    let mut evaluator = RoundBrushEvaluator::default();
    let mut dabs = Vec::new();
    let mut stroke = begin_round_stroke(&mut evaluator, &preset, sample(0), &mut dabs);
    let samples: Vec<_> = (1..=48).map(sample).collect();
    evaluator.push(&mut stroke, &samples, &mut dabs);
    evaluator.end(stroke, &mut dabs);
    let mut surface = CpuCanvas::new(WIDTH, HEIGHT);
    if projection.drawing_tool == DrawingTool::Eraser {
        surface =
            CpuCanvas::from_rgba8_premultiplied(WIDTH, HEIGHT, [32, 32, 32, 255].repeat(160 * 48))
                .map_err(|e| format!("brush-preview:{e:?}"))?;
        surface.erase_dabs(&dabs);
    } else {
        surface.apply_dabs(&dabs, PremultipliedRgba8::new(0, 0, 0, 255));
    }
    let pixels = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: WIDTH,
        height: HEIGHT,
        pixels: surface.pixels_rgba8_premultiplied().to_vec(),
    };
    let png =
        nyatidraw_png_io::encode_png_bytes(&pixels).map_err(|e| format!("brush-preview:{e}"))?;
    crate::preview::png_data_uri(&png, 64 * 1024, "brush")
}

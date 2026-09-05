//! Fixed software workload shared by the paced driver and offline replay.
use nyatidraw_api::CanvasSpec;
use nyatidraw_brush::{BrushPreset, BrushPresetId, ROUND_BRUSH_ENGINE_VERSION};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};

pub const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 3840,
    height_px: 2160,
    pixels_per_inch: 96,
};
pub const STROKES: u32 = 32;
pub const INTERVAL_NS: u64 = 4_166_667;
pub const LAST_SAMPLE: u32 = 120;
pub const COLOR: [u8; 4] = [26, 199, 232, 255];
pub const BRUSH: BrushPreset = BrushPreset {
    id: BrushPresetId(1),
    schema_version: 1,
    engine_version: ROUND_BRUSH_ENGINE_VERSION,
    size_px: 64.0,
    opacity: 1.0,
    flow: 0.38,
    spacing_ratio: 0.12,
};

pub fn sample(stroke: u32, index: u32) -> StylusSample {
    let sequence = u64::from(stroke) * u64::from(LAST_SAMPLE + 1) + u64::from(index) + 1;
    StylusSample {
        sequence,
        timestamp_ns: sequence * INTERVAL_NS,
        device_id: u64::MAX - 100,
        phase: if index == 0 {
            PointerPhase::Begin
        } else if index == LAST_SAMPLE {
            PointerPhase::End
        } else {
            PointerPhase::Move
        },
        position_document: Point {
            x: f64::from(240 + 28 * index),
            y: f64::from(128 + 50 * stroke),
        },
        pressure: 1.0,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 0,
    }
}

use nyatidraw_api::{BrushSettings as NativeBrushSettings, DrawingTool, PencilTemplate};
use nyatidraw_project::{ProjectBrushState, ProjectToolState};
use nyatidraw_web_core::{BrushSettings, WebPreferences, WebTool};

const NATIVE_TO_WEB: [WebTool; 5] = [
    WebTool::Pencil2H,
    WebTool::Pen,
    WebTool::SoftBrush,
    WebTool::Eraser,
    WebTool::Pencil2B,
];

pub(super) fn to_web(state: ProjectToolState) -> WebPreferences {
    let mut brushes = WebTool::ALL.map(BrushSettings::for_tool);
    for (index, tool) in NATIVE_TO_WEB.into_iter().enumerate() {
        let brush = state.remembered[index];
        brushes[tool as usize] = BrushSettings {
            size_px: f32::from(brush.size_tenths) / 10.0,
            opacity: f32::from(brush.opacity_u16) / f32::from(u16::MAX),
            hardness: f32::from(brush.settings.hardness_u16) / f32::from(u16::MAX),
            size_pressure: brush.settings.size_pressure,
            opacity_pressure: brush.settings.opacity_pressure,
            size_minimum_u16: brush.settings.size_minimum_u16,
            opacity_minimum_u16: brush.settings.opacity_minimum_u16,
            smoothing: brush.settings.smoothing,
        };
    }
    WebPreferences {
        edit_settings: state.edit_settings,
        tool: NATIVE_TO_WEB[usize::from(state.last_painting_slot)],
        brushes,
        foreground: state.color,
        background: state.background_color,
    }
}

pub(super) fn to_native(
    preferences: WebPreferences,
    selected: Option<DrawingTool>,
    original: Option<ProjectToolState>,
) -> Result<ProjectToolState, String> {
    let mut remembered = [ProjectBrushState {
        size_tenths: 50,
        opacity_u16: u16::MAX,
        settings: NativeBrushSettings::default(),
    }; 5];
    for (index, tool) in NATIVE_TO_WEB.into_iter().enumerate() {
        let brush = preferences.brushes[tool as usize];
        let size = (brush.size_px * 10.0).round();
        if (size / 10.0 - brush.size_px).abs() > 0.00001 || !(1.0..=2000.0).contains(&size) {
            return Err(
                "Native brush size must use 0.1 pixel increments between 0.1 and 200".into(),
            );
        }
        let opacity = unit_u16(brush.opacity);
        if opacity == 0 {
            return Err("Native brush opacity must be greater than zero before saving".into());
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let size_tenths = size as u16;
        remembered[index] = ProjectBrushState {
            size_tenths,
            opacity_u16: opacity,
            settings: NativeBrushSettings {
                size_pressure: brush.size_pressure,
                opacity_pressure: brush.opacity_pressure,
                size_minimum_u16: brush.size_minimum_u16,
                opacity_minimum_u16: brush.opacity_minimum_u16,
                hardness_u16: unit_u16(brush.hardness),
                smoothing: brush.smoothing,
            },
        };
    }
    if preferences.foreground[3] == 0 || preferences.background[3] == 0 {
        return Err("Native foreground/background colors require nonzero alpha".into());
    }
    let (painting_tool, slot) = match preferences.tool {
        WebTool::Pencil2H => (DrawingTool::Pencil, 0),
        WebTool::Pencil2B => (DrawingTool::Pencil, 4),
        WebTool::Pen => (DrawingTool::Pen, 1),
        WebTool::SoftBrush => (DrawingTool::Brush, 2),
        WebTool::Eraser => (DrawingTool::Eraser, 3),
    };
    let tool = match selected {
        Some(DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush | DrawingTool::Eraser)
        | None => painting_tool,
        Some(tool) => tool,
    };
    Ok(ProjectToolState {
        tool,
        pencil_template: match preferences.tool {
            WebTool::Pencil2B => PencilTemplate::Graphite2B,
            WebTool::Pencil2H => PencilTemplate::Mechanical2H,
            _ => original.map_or(PencilTemplate::Mechanical2H, |state| state.pencil_template),
        },
        last_painting_slot: slot,
        color: preferences.foreground,
        background_color: preferences.background,
        edit_settings: preferences.edit_settings,
        remembered,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn unit_u16(value: f32) -> u16 {
    (value * f32::from(u16::MAX)).round() as u16
}

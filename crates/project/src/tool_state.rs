use nyatidraw_api::{
    BrushSettings, DrawingTool, EditSettings, EditSource, FillSettings, PencilTemplate,
    SelectionMode,
};

const MAGIC: &[u8; 8] = b"NYTOOL01";
pub const EDITOR_TOOL_STATE_BYTES: usize = 85;
const TOOLS: [DrawingTool; 12] = [
    DrawingTool::Move,
    DrawingTool::MoveSelection,
    DrawingTool::Pencil,
    DrawingTool::Pen,
    DrawingTool::Brush,
    DrawingTool::Eraser,
    DrawingTool::Wand,
    DrawingTool::Lasso,
    DrawingTool::RectangleSelection,
    DrawingTool::Eyedropper,
    DrawingTool::Fill,
    DrawingTool::Gradient,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectBrushState {
    pub size_tenths: u16,
    pub opacity_u16: u16,
    pub settings: BrushSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectToolState {
    pub tool: DrawingTool,
    pub pencil_template: PencilTemplate,
    pub last_painting_slot: u8,
    pub color: [u8; 4],
    pub background_color: [u8; 4],
    pub edit_settings: EditSettings,
    /// Stable NYTOOL01 order: 2H pencil, pen, brush, eraser, 2B pencil.
    pub remembered: [ProjectBrushState; 5],
}

#[must_use]
pub fn encode_editor_tool_state(state: &ProjectToolState) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(EDITOR_TOOL_STATE_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.push(match state.tool {
        DrawingTool::Move => 0,
        DrawingTool::MoveSelection => 1,
        DrawingTool::Pencil => 2,
        DrawingTool::Pen => 3,
        DrawingTool::Brush => 4,
        DrawingTool::Eraser => 5,
        DrawingTool::Wand => 6,
        DrawingTool::Lasso => 7,
        DrawingTool::RectangleSelection => 8,
        DrawingTool::Eyedropper => 9,
        DrawingTool::Fill => 10,
        DrawingTool::Gradient => 11,
    });
    bytes.push(u8::from(
        state.pencil_template == PencilTemplate::Graphite2B,
    ));
    bytes.push(state.last_painting_slot);
    bytes.extend_from_slice(&state.color);
    bytes.extend_from_slice(&state.background_color);
    bytes.extend_from_slice(&[
        match state.edit_settings.source {
            EditSource::ActiveLayer => 0,
            EditSource::ReferenceLayers => 1,
            EditSource::AllVisible => 2,
        },
        state.edit_settings.tolerance,
        match state.edit_settings.selection_mode {
            SelectionMode::Replace => 0,
            SelectionMode::Add => 1,
            SelectionMode::Subtract => 2,
        },
        state.edit_settings.fill.gap_close_px,
        state.edit_settings.fill.expand_px,
        u8::from(state.edit_settings.fill.antialias),
    ]);
    for brush in state.remembered {
        bytes.extend_from_slice(&brush.size_tenths.to_le_bytes());
        bytes.extend_from_slice(&brush.opacity_u16.to_le_bytes());
        bytes.push(
            u8::from(brush.settings.size_pressure)
                | (u8::from(brush.settings.opacity_pressure) << 1),
        );
        bytes.extend_from_slice(&brush.settings.size_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&brush.settings.opacity_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&brush.settings.hardness_u16.to_le_bytes());
        bytes.push(brush.settings.smoothing);
    }
    debug_assert_eq!(bytes.len(), EDITOR_TOOL_STATE_BYTES);
    bytes
}

/// Unknown or malformed optional settings return `None`; retain their original bytes.
#[must_use]
pub fn decode_editor_tool_state(bytes: &[u8]) -> Option<ProjectToolState> {
    if bytes.len() != EDITOR_TOOL_STATE_BYTES || !bytes.starts_with(MAGIC) {
        return None;
    }
    let tool = *TOOLS.get(usize::from(bytes[8]))?;
    let pencil_template = match bytes[9] {
        0 => PencilTemplate::Mechanical2H,
        1 => PencilTemplate::Graphite2B,
        _ => return None,
    };
    let last_painting_slot = bytes[10];
    let color: [u8; 4] = bytes[11..15].try_into().ok()?;
    let background_color: [u8; 4] = bytes[15..19].try_into().ok()?;
    if color[3] == 0 || background_color[3] == 0 || last_painting_slot >= 5 {
        return None;
    }
    let expected_slot = match tool {
        DrawingTool::Pencil => Some(if pencil_template == PencilTemplate::Graphite2B {
            4
        } else {
            0
        }),
        DrawingTool::Pen => Some(1),
        DrawingTool::Brush => Some(2),
        DrawingTool::Eraser => Some(3),
        _ => None,
    };
    if expected_slot.is_some_and(|slot| slot != last_painting_slot) {
        return None;
    }
    let edit_settings = EditSettings {
        source: match bytes[19] {
            0 => EditSource::ActiveLayer,
            1 => EditSource::ReferenceLayers,
            2 => EditSource::AllVisible,
            _ => return None,
        },
        tolerance: bytes[20],
        selection_mode: match bytes[21] {
            0 => SelectionMode::Replace,
            1 => SelectionMode::Add,
            2 => SelectionMode::Subtract,
            _ => return None,
        },
        fill: FillSettings {
            gap_close_px: bytes[22],
            expand_px: bytes[23],
            antialias: match bytes[24] {
                0 => false,
                1 => true,
                _ => return None,
            },
        },
    };
    if edit_settings.fill.gap_close_px > 8 || edit_settings.fill.expand_px > 64 {
        return None;
    }
    let mut remembered = [ProjectBrushState {
        size_tenths: 50,
        opacity_u16: u16::MAX,
        settings: BrushSettings::for_tool(DrawingTool::Pencil),
    }; 5];
    for (slot, encoded) in remembered.iter_mut().zip(bytes[25..].chunks_exact(12)) {
        let number = |index| u16::from_le_bytes([encoded[index], encoded[index + 1]]);
        *slot = ProjectBrushState {
            size_tenths: number(0),
            opacity_u16: number(2),
            settings: BrushSettings {
                size_pressure: encoded[4] & 1 != 0,
                opacity_pressure: encoded[4] & 2 != 0,
                size_minimum_u16: number(5),
                opacity_minimum_u16: number(7),
                hardness_u16: number(9),
                smoothing: encoded[11],
            },
        };
        if !(1..=2_000).contains(&slot.size_tenths)
            || slot.opacity_u16 == 0
            || encoded[4] > 3
            || slot.settings.smoothing > 100
        {
            return None;
        }
    }
    Some(ProjectToolState {
        tool,
        pencil_template,
        last_painting_slot,
        color,
        background_color,
        edit_settings,
        remembered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_tool_records_reopen_identically_and_unknown_records_are_not_rewritten() {
        let fixture = [
            b'N', b'Y', b'T', b'O', b'O', b'L', b'0', b'1', 6, 1, 4, 21, 42, 63, 255, 65, 43, 21,
            255, 2, 37, 2, 8, 64, 1, 10, 0, 64, 156, 3, 152, 58, 88, 27, 0, 125, 37, 11, 0, 65,
            156, 3, 152, 58, 88, 27, 0, 125, 37, 12, 0, 66, 156, 3, 152, 58, 88, 27, 0, 125, 37,
            13, 0, 67, 156, 3, 152, 58, 88, 27, 0, 125, 37, 14, 0, 68, 156, 3, 152, 58, 88, 27, 0,
            125, 37,
        ];
        let expected = ProjectToolState {
            tool: DrawingTool::Wand,
            pencil_template: PencilTemplate::Graphite2B,
            last_painting_slot: 4,
            color: [21, 42, 63, 255],
            background_color: [65, 43, 21, 255],
            edit_settings: EditSettings {
                source: EditSource::AllVisible,
                tolerance: 37,
                selection_mode: SelectionMode::Subtract,
                fill: FillSettings {
                    gap_close_px: 8,
                    expand_px: 64,
                    antialias: true,
                },
            },
            remembered: std::array::from_fn(|index| ProjectBrushState {
                size_tenths: 10 + u16::try_from(index).unwrap(),
                opacity_u16: 40_000 + u16::try_from(index).unwrap(),
                settings: BrushSettings {
                    size_pressure: true,
                    opacity_pressure: true,
                    size_minimum_u16: 15_000,
                    opacity_minimum_u16: 7_000,
                    hardness_u16: 32_000,
                    smoothing: 37,
                },
            }),
        };
        assert_eq!(fixture.len(), EDITOR_TOOL_STATE_BYTES);
        assert_eq!(decode_editor_tool_state(&fixture), Some(expected));
        assert_eq!(encode_editor_tool_state(&expected), fixture);
        for (index, value) in [
            (0, 0),
            (8, 255),
            (9, 2),
            (10, 5),
            (14, 0),
            (18, 0),
            (19, 3),
            (21, 3),
            (22, 9),
            (23, 65),
            (24, 2),
            (29, 4),
            (36, 101),
        ] {
            let mut malformed = fixture;
            malformed[index] = value;
            assert!(
                decode_editor_tool_state(&malformed).is_none(),
                "invalid field {index}"
            );
        }
        for length in 0..fixture.len() {
            assert!(decode_editor_tool_state(&fixture[..length]).is_none());
        }
        let mut extended = fixture.to_vec();
        extended.push(0);
        assert!(decode_editor_tool_state(&extended).is_none());
        for tool in TOOLS {
            for template in [PencilTemplate::Mechanical2H, PencilTemplate::Graphite2B] {
                for slot in 0..5 {
                    let state = ProjectToolState {
                        tool,
                        pencil_template: template,
                        last_painting_slot: slot,
                        ..expected
                    };
                    let expected_slot = match tool {
                        DrawingTool::Pencil => Some(if template == PencilTemplate::Graphite2B {
                            4
                        } else {
                            0
                        }),
                        DrawingTool::Pen => Some(1),
                        DrawingTool::Brush => Some(2),
                        DrawingTool::Eraser => Some(3),
                        _ => None,
                    };
                    let decoded = decode_editor_tool_state(&encode_editor_tool_state(&state));
                    assert_eq!(
                        decoded,
                        expected_slot
                            .is_none_or(|value| value == slot)
                            .then_some(state)
                    );
                }
            }
        }
        let unknown = b"unknown future settings".to_vec();
        let original = unknown.clone();
        assert!(decode_editor_tool_state(&unknown).is_none());
        assert_eq!(unknown, original);
    }
}

use crate::{BrushSettings, DrawingTool, PencilTemplate, ToolCommand};

pub const BRUSH_PRESETS_MAX: usize = 64;
pub const BRUSH_PRESETS_MAX_BYTES: usize = 9 + BRUSH_PRESETS_MAX * 110;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrushPresetName([u8; 96]);
impl BrushPresetName {
    #[must_use]
    pub fn new(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.len() > 96 || value.chars().any(char::is_control) {
            return None;
        }
        let mut bytes = [0; 96];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Some(Self(bytes))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0)
            .unwrap_or_default()
            .trim_end_matches('\0')
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrushPresetValues {
    pub tool: DrawingTool,
    pub pencil_template: PencilTemplate,
    pub size_tenths: u16,
    pub opacity_u16: u16,
    pub settings: BrushSettings,
}
impl BrushPresetValues {
    #[must_use]
    pub fn from_projection(projection: &crate::UiProjection) -> Self {
        Self {
            tool: projection.drawing_tool,
            pencil_template: projection.pencil_template,
            size_tenths: projection.brush_size_tenths,
            opacity_u16: projection.brush_opacity_u16,
            settings: projection.brush_settings,
        }
    }
    #[must_use]
    pub fn matches(self, mut other: Self) -> bool {
        if self.tool != DrawingTool::Pencil {
            other.pencil_template = self.pencil_template;
        }
        self == other
    }
    #[must_use]
    pub fn valid(self) -> bool {
        matches!(
            self.tool,
            DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush | DrawingTool::Eraser
        ) && (1..=2000).contains(&self.size_tenths)
            && self.opacity_u16 > 0
            && self.settings.smoothing <= 100
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NamedBrushPreset {
    pub name: BrushPresetName,
    pub values: BrushPresetValues,
}

/// Working brush edits never mutate explicitly saved values.
/// # Errors
/// Rejects duplicate names, missing entries, limits and non-painting tools.
pub fn edit_brush_presets(
    presets: &mut Vec<NamedBrushPreset>,
    command: ToolCommand,
    current: BrushPresetValues,
) -> Result<(), &'static str> {
    match command {
        ToolCommand::SaveBrushPreset(name) => {
            if presets.len() >= BRUSH_PRESETS_MAX || presets.iter().any(|p| p.name == name) {
                return Err("이름이 중복되었거나 프리셋 64개 한도에 도달했습니다.");
            }
            if !current.valid() {
                return Err("그리기 도구를 먼저 선택하세요.");
            }
            presets.push(NamedBrushPreset {
                name,
                values: current,
            });
        }
        ToolCommand::UpdateBrushPreset(index) => {
            if !current.valid() {
                return Err("그리기 도구를 먼저 선택하세요.");
            }
            presets
                .get_mut(usize::from(index))
                .ok_or("프리셋이 없습니다.")?
                .values = current;
        }
        ToolCommand::RenameBrushPreset(index, name) => {
            if presets
                .iter()
                .enumerate()
                .any(|(i, p)| i != usize::from(index) && p.name == name)
            {
                return Err("이미 사용 중인 프리셋 이름입니다.");
            }
            presets
                .get_mut(usize::from(index))
                .ok_or("프리셋이 없습니다.")?
                .name = name;
        }
        ToolCommand::DeleteBrushPreset(index) => {
            if usize::from(index) >= presets.len() {
                return Err("프리셋이 없습니다.");
            }
            presets.remove(usize::from(index));
        }
        _ => return Err("지원하지 않는 프리셋 명령입니다."),
    }
    Ok(())
}
#[must_use]
pub fn encode_brush_presets(presets: &[NamedBrushPreset]) -> Vec<u8> {
    let mut bytes = b"NYBRSH01".to_vec();
    bytes.push(u8::try_from(presets.len()).unwrap_or(u8::MAX));
    for preset in presets {
        bytes.extend_from_slice(&preset.name.0);
        bytes.push(match preset.values.tool {
            DrawingTool::Pencil => 0,
            DrawingTool::Pen => 1,
            DrawingTool::Brush => 2,
            DrawingTool::Eraser => 3,
            _ => u8::MAX,
        });
        bytes.push(u8::from(
            preset.values.pencil_template == PencilTemplate::Graphite2B,
        ));
        let value = preset.values;
        bytes.extend_from_slice(&value.size_tenths.to_le_bytes());
        bytes.extend_from_slice(&value.opacity_u16.to_le_bytes());
        bytes.push(
            u8::from(value.settings.size_pressure)
                | (u8::from(value.settings.opacity_pressure) << 1),
        );
        bytes.extend_from_slice(&value.settings.size_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&value.settings.opacity_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&value.settings.hardness_u16.to_le_bytes());
        bytes.push(value.settings.smoothing);
    }
    bytes
}
/// Malformed or future records must be preserved by callers, without overwrite.
#[must_use]
pub fn decode_brush_presets(bytes: &[u8]) -> Option<Vec<NamedBrushPreset>> {
    if bytes.len() < 9 || bytes.len() > BRUSH_PRESETS_MAX_BYTES || &bytes[..8] != b"NYBRSH01" {
        return None;
    }
    let count = usize::from(bytes[8]);
    if count > BRUSH_PRESETS_MAX || bytes.len() != 9 + count * 110 {
        return None;
    }
    let mut presets = Vec::with_capacity(count);
    for record in bytes[9..].chunks_exact(110) {
        let raw_name: [u8; 96] = record[..96].try_into().ok()?;
        let name =
            BrushPresetName::new(std::str::from_utf8(&raw_name).ok()?.trim_end_matches('\0'))?;
        if name.0 != raw_name || presets.iter().any(|p: &NamedBrushPreset| p.name == name) {
            return None;
        }
        let tool = match record[96] {
            0 => DrawingTool::Pencil,
            1 => DrawingTool::Pen,
            2 => DrawingTool::Brush,
            3 => DrawingTool::Eraser,
            _ => return None,
        };
        let pencil_template = match record[97] {
            0 => PencilTemplate::Mechanical2H,
            1 => PencilTemplate::Graphite2B,
            _ => return None,
        };
        let number = |i| u16::from_le_bytes([record[i], record[i + 1]]);
        if record[102] > 3 {
            return None;
        }
        let values = BrushPresetValues {
            tool,
            pencil_template,
            size_tenths: number(98),
            opacity_u16: number(100),
            settings: BrushSettings {
                size_pressure: record[102] & 1 != 0,
                opacity_pressure: record[102] & 2 != 0,
                size_minimum_u16: number(103),
                opacity_minimum_u16: number(105),
                hardness_u16: number(107),
                smoothing: record[109],
            },
        };
        if !values.valid() {
            return None;
        }
        presets.push(NamedBrushPreset { name, values });
    }
    Some(presets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_restoration_preserves_brush_determinism_and_rejects_corrupt_libraries() {
        let mut presets = Vec::new();
        for (index, (tool, pencil_template)) in [
            (DrawingTool::Pencil, PencilTemplate::Mechanical2H),
            (DrawingTool::Pencil, PencilTemplate::Graphite2B),
            (DrawingTool::Pen, PencilTemplate::Graphite2B),
            (DrawingTool::Brush, PencilTemplate::Mechanical2H),
            (DrawingTool::Eraser, PencilTemplate::Mechanical2H),
        ]
        .into_iter()
        .enumerate()
        {
            let values = BrushPresetValues {
                tool,
                pencil_template,
                size_tenths: 173,
                opacity_u16: 42_123,
                settings: BrushSettings {
                    size_pressure: true,
                    opacity_pressure: false,
                    size_minimum_u16: 923,
                    opacity_minimum_u16: 1023,
                    hardness_u16: 4321,
                    smoothing: 79,
                },
            };
            edit_brush_presets(
                &mut presets,
                ToolCommand::SaveBrushPreset(
                    BrushPresetName::new(&format!("연필 {index}")).unwrap(),
                ),
                values,
            )
            .unwrap();
        }
        let encoded = encode_brush_presets(&presets);
        assert_eq!(decode_brush_presets(&encoded), Some(presets.clone()));
        for (offset, value) in [(0, b'X'), (8, 65), (109, 0), (111, 4), (118, 101)] {
            let mut corrupt = encoded.clone();
            corrupt[offset] = value;
            if offset == 109 {
                corrupt[110] = 0;
            }
            assert!(
                decode_brush_presets(&corrupt).is_none(),
                "corrupt field {offset}"
            );
        }
        assert!(decode_brush_presets(&encoded[..encoded.len() - 1]).is_none());
        assert!(decode_brush_presets(&vec![0; BRUSH_PRESETS_MAX_BYTES + 1]).is_none());
        let mut duplicate = presets.clone();
        duplicate.push(presets[0]);
        assert!(decode_brush_presets(&encode_brush_presets(&duplicate)).is_none());
        let original = presets[0];
        let mut working = original.values;
        working.size_tenths = 411;
        assert_eq!(presets[0], original);
        edit_brush_presets(&mut presets, ToolCommand::UpdateBrushPreset(0), working).unwrap();
        assert_eq!(presets[0].values, working);
        assert_eq!(presets[1], duplicate[1]);
    }
}

//! Bounded native gesture capture. Only a completed gesture becomes a command.
use nyatidraw_api::{DrawingTool, EditCommand, EditSettings};
use nyatidraw_input::{PointerPhase, StylusSample};

const MAX_VERTICES: usize = 4096;

pub(crate) struct EditGesture {
    tool: DrawingTool,
    settings: EditSettings,
    color: [u8; 4],
    has_selection: bool,
    viewport_revision: u64,
    start: [i32; 2],
    vertices: Vec<[i32; 2]>,
    error: Option<&'static str>,
}

impl EditGesture {
    pub(crate) fn begin(
        tool: DrawingTool,
        settings: EditSettings,
        color: [u8; 4],
        has_selection: bool,
        sample: StylusSample,
    ) -> Self {
        let point = document_pixel(sample);
        let start = point.unwrap_or([0; 2]);
        Self {
            tool,
            settings,
            color,
            has_selection,
            viewport_revision: sample.viewport_revision,
            start,
            vertices: match tool {
                DrawingTool::Lasso => vec![start],
                DrawingTool::Gradient => vec![start, start],
                _ => Vec::new(),
            },
            error: point
                .is_none()
                .then_some("선택 좌표가 허용 범위를 벗어났습니다."),
        }
    }

    pub(crate) fn push(&mut self, sample: StylusSample) {
        if self.error.is_some() {
            return;
        }
        if sample.viewport_revision != self.viewport_revision {
            self.error = Some("화면 이동으로 선택 제스처가 취소됐습니다.");
            return;
        }
        let Some(point) = document_pixel(sample) else {
            self.error = Some("선택 좌표가 허용 범위를 벗어났습니다.");
            return;
        };
        if self.tool == DrawingTool::Lasso && self.vertices.last() != Some(&point) {
            if self.vertices.len() == MAX_VERTICES {
                self.error = Some("올가미 점이 4096개를 초과해 적용하지 않았습니다.");
            } else {
                self.vertices.push(point);
            }
        }
        if self.tool == DrawingTool::Gradient {
            self.vertices[1] = point;
        }
    }

    pub(crate) fn preview(&self) -> (&[[i32; 2]], bool) {
        if self.error.is_some() {
            return (&[], false);
        }
        (
            &self.vertices,
            self.tool == DrawingTool::Lasso && self.vertices.len() > 2,
        )
    }

    pub(crate) fn finish(mut self, sample: StylusSample) -> Result<EditCommand, String> {
        if sample.phase != PointerPhase::End {
            return Err("선택 제스처가 취소됐습니다.".into());
        }
        self.push(sample);
        if let Some(error) = self.error {
            return Err(error.into());
        }
        let end = document_pixel(sample).ok_or("선택 끝점이 유효하지 않습니다.")?;
        Ok(match self.tool {
            DrawingTool::Wand => EditCommand::SelectWand {
                seed: self.start,
                tolerance: self.settings.tolerance,
                source: self.settings.source,
            },
            DrawingTool::Lasso if self.vertices.len() >= 3 => EditCommand::SelectLasso {
                vertices: self.vertices,
            },
            DrawingTool::Fill if self.has_selection => {
                EditCommand::FillSelection { color: self.color }
            }
            DrawingTool::Fill => EditCommand::FloodFill {
                seed: self.start,
                tolerance: self.settings.tolerance,
                source: self.settings.source,
                color: self.color,
            },
            DrawingTool::Gradient if self.has_selection && self.start != end => {
                EditCommand::GradientSelection {
                    start: self.start,
                    end,
                    start_color: self.color,
                    end_color: [0; 4],
                }
            }
            DrawingTool::Gradient => {
                return Err("선택 영역에서 서로 다른 두 점을 연결하세요.".into());
            }
            _ => return Err("올가미는 서로 다른 점 세 개 이상이 필요합니다.".into()),
        })
    }
}

#[allow(clippy::cast_possible_truncation)]
fn document_pixel(sample: StylusSample) -> Option<[i32; 2]> {
    let point = sample.position_document;
    [point.x, point.y]
        .iter()
        .all(|value| {
            value.is_finite() && *value >= f64::from(i32::MIN) && *value < f64::from(i32::MAX)
        })
        .then(|| [point.x.floor() as i32, point.y.floor() as i32])
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_input::{PenButtons, Point};

    #[test]
    fn invalid_or_cancelled_lasso_never_emits_a_partial_artwork_command() {
        let sample = |x: f64, phase| StylusSample {
            sequence: 1,
            timestamp_ns: 1,
            device_id: 1,
            phase,
            position_document: Point { x, y: x % 2.0 },
            pressure: 1.0,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons(0),
            eraser: false,
            viewport_revision: 1,
        };
        for failure in [
            "cancel",
            "overflow",
            "viewport",
            "nonfinite",
            "short",
            "valid",
        ] {
            let mut gesture = EditGesture::begin(
                DrawingTool::Lasso,
                EditSettings::default(),
                [0; 4],
                false,
                sample(0.0, PointerPhase::Begin),
            );
            let count = if failure == "overflow" {
                4097
            } else if failure == "short" {
                1
            } else {
                3
            };
            for x in 1..count {
                gesture.push(sample(f64::from(x), PointerPhase::Move));
            }
            let mut end = sample(0.0, PointerPhase::End);
            match failure {
                "cancel" => end.phase = PointerPhase::Cancel,
                "viewport" => end.viewport_revision = 2,
                "nonfinite" => end.position_document.x = f64::NAN,
                _ => {}
            }
            assert_eq!(gesture.finish(end).is_ok(), failure == "valid", "{failure}");
        }
    }
}

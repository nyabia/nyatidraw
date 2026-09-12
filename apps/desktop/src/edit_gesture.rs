//! Bounded native gesture capture. Only a completed gesture becomes a command.
use nyatidraw_api::{DrawingTool, EditCommand, EditSettings, RasterTransform};
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
                DrawingTool::RectangleSelection => rectangle_vertices(start, start),
                DrawingTool::Gradient | DrawingTool::MoveSelection => vec![start, start],
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
        if self.tool == DrawingTool::RectangleSelection {
            self.vertices = rectangle_vertices(self.start, point);
        }
        if matches!(
            self.tool,
            DrawingTool::Gradient | DrawingTool::MoveSelection
        ) {
            self.vertices[1] = point;
        }
    }

    pub(crate) fn preview(&self) -> (&[[i32; 2]], bool) {
        if self.error.is_some() {
            return (&[], false);
        }
        (
            &self.vertices,
            matches!(
                self.tool,
                DrawingTool::Lasso | DrawingTool::RectangleSelection
            ) && self.vertices.len() > 2,
        )
    }

    pub(crate) fn picker_point(&self, sample: StylusSample) -> Option<[i32; 2]> {
        (self.tool == DrawingTool::Eyedropper
            && self.error.is_none()
            && sample.viewport_revision == self.viewport_revision)
            .then(|| document_pixel(sample))
            .flatten()
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
            DrawingTool::Eyedropper => EditCommand::PickColor {
                point: end,
                source: self.settings.source,
            },
            DrawingTool::Wand => EditCommand::SelectWand {
                seed: self.start,
                tolerance: self.settings.tolerance,
                source: self.settings.source,
            },
            DrawingTool::Lasso | DrawingTool::RectangleSelection if self.vertices.len() >= 3 => {
                if self.settings.selection_mode == nyatidraw_api::SelectionMode::Replace {
                    EditCommand::SelectLasso {
                        vertices: self.vertices,
                    }
                } else {
                    EditCommand::CombineLasso {
                        vertices: self.vertices,
                        mode: self.settings.selection_mode,
                    }
                }
            }
            DrawingTool::MoveSelection => {
                if !self.has_selection {
                    return Err("먼저 이동할 영역을 선택하세요.".into());
                }
                let offset = [
                    end[0].checked_sub(self.start[0]),
                    end[1].checked_sub(self.start[1]),
                ];
                let [Some(x), Some(y)] = offset else {
                    return Err("선택 이동 거리가 허용 범위를 벗어났습니다.".into());
                };
                if x == 0 && y == 0 {
                    return Err("이동하지 않았습니다. 선택 영역을 유지합니다.".into());
                }
                EditCommand::Transform(RasterTransform {
                    offset: [x, y],
                    ..RasterTransform::default()
                })
            }
            DrawingTool::Fill => EditCommand::FloodFillAdvanced {
                seed: self.start,
                tolerance: self.settings.tolerance,
                source: self.settings.source,
                color: self.color,
                settings: self.settings.fill,
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

// Pixel-inclusive drag endpoints become a half-open polygon of pixel edges.
// document_pixel excludes i32::MAX, so the upper edge cannot overflow.
fn rectangle_vertices(start: [i32; 2], end: [i32; 2]) -> Vec<[i32; 2]> {
    let left = start[0].min(end[0]);
    let top = start[1].min(end[1]);
    let right = start[0].max(end[0]) + 1;
    let bottom = start[1].max(end[1]) + 1;
    vec![[left, top], [right, top], [right, bottom], [left, bottom]]
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
    fn rectangle_edges_and_move_rejection_preserve_exact_selected_pixels() {
        // Product risk: reversed drags or missing selection must not select an
        // extra row, overflow coordinates, or move the entire active layer.
        let sample = |point: [i32; 2], phase| StylusSample {
            sequence: 1,
            timestamp_ns: 1,
            device_id: 1,
            phase,
            position_document: Point {
                x: f64::from(point[0]),
                y: f64::from(point[1]),
            },
            pressure: 1.0,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons(0),
            eraser: false,
            viewport_revision: 1,
        };
        let canvas = nyatidraw_api::CanvasSpec {
            width_px: 8,
            height_px: 8,
            pixels_per_inch: 96,
        };
        for (start, end, bounds) in [
            ([1, 2], [3, 4], [1, 2, 3, 3]),
            ([3, 4], [1, 2], [1, 2, 3, 3]),
            ([1, 4], [3, 2], [1, 2, 3, 3]),
            ([3, 2], [1, 4], [1, 2, 3, 3]),
            ([3, 3], [3, 3], [3, 3, 1, 1]),
            ([-2, -2], [1, 1], [0, 0, 2, 2]),
        ] {
            let mut gesture = EditGesture::begin(
                DrawingTool::RectangleSelection,
                EditSettings::default(),
                [0; 4],
                false,
                sample(start, PointerPhase::Begin),
            );
            gesture.push(sample(end, PointerPhase::Move));
            assert!(gesture.preview().1);
            assert_eq!(gesture.preview().0.len(), 4);
            let EditCommand::SelectLasso { vertices } =
                gesture.finish(sample(end, PointerPhase::End)).unwrap()
            else {
                panic!("rectangle uses the shared polygon selection engine");
            };
            let mask = nyatidraw_paint_cpu::lasso_selection(
                canvas,
                &vertices,
                nyatidraw_paint_cpu::EditLimits::default(),
            )
            .unwrap();
            assert_eq!(mask.bounds(), Some(bounds));
            assert_eq!(mask.selected_pixels(), u64::from(bounds[2] * bounds[3]));
        }
        for (has_selection, phase, revision, end, accepted) in [
            (true, PointerPhase::End, 1, [5, -2], true),
            (false, PointerPhase::End, 1, [5, -2], false),
            (true, PointerPhase::Cancel, 1, [5, -2], false),
            (true, PointerPhase::End, 2, [5, -2], false),
            (true, PointerPhase::End, 1, [1, 1], false),
            (true, PointerPhase::End, 1, [i32::MIN, 1], false),
        ] {
            let gesture = EditGesture::begin(
                DrawingTool::MoveSelection,
                EditSettings::default(),
                [0; 4],
                has_selection,
                sample([1, 1], PointerPhase::Begin),
            );
            let mut end = sample(end, phase);
            end.viewport_revision = revision;
            let result = gesture.finish(end);
            assert_eq!(result.is_ok(), accepted);
            if accepted {
                assert_eq!(
                    result.unwrap(),
                    EditCommand::Transform(RasterTransform {
                        offset: [4, -3],
                        ..RasterTransform::default()
                    })
                );
            }
        }
    }

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

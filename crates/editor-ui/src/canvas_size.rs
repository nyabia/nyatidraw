use nyatidraw_api::CanvasSpec;
use std::collections::VecDeque;

pub const COMMON_PRESETS: [CanvasPreset; 3] = [
    CanvasPreset {
        name: "FHD",
        size: [1920, 1080],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "UXGA",
        size: [1600, 1200],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "64 × 64",
        size: [64, 64],
        ratio: "1:1",
    },
];
pub const MORE_PRESETS: [CanvasPreset; 10] = [
    CanvasPreset {
        name: "HD",
        size: [1280, 720],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "QHD",
        size: [2560, 1440],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "4K UHD",
        size: [3840, 2160],
        ratio: "16:9",
    },
    CanvasPreset {
        name: "XGA",
        size: [1024, 768],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "SVGA",
        size: [800, 600],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "VGA",
        size: [640, 480],
        ratio: "4:3",
    },
    CanvasPreset {
        name: "128 × 128",
        size: [128, 128],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "256 × 256",
        size: [256, 256],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "512 × 512",
        size: [512, 512],
        ratio: "1:1",
    },
    CanvasPreset {
        name: "1024 × 1024",
        size: [1024, 1024],
        ratio: "1:1",
    },
];
const SIZE_UNDO_LIMIT: usize = 32;

#[derive(Clone, Copy, PartialEq)]
pub struct CanvasPreset {
    pub name: &'static str,
    pub size: [u32; 2],
    pub ratio: &'static str,
}

fn within_output_limit([width, height]: [u32; 2]) -> bool {
    width > 0
        && height > 0
        && u64::from(width) * u64::from(height) <= nyatidraw_tiles::MAX_FLATTENED_PIXELS
}

#[derive(Clone, PartialEq)]
pub struct CanvasSizeInput {
    pub width: String,
    pub height: String,
    pub source: Option<(CanvasPreset, u32)>,
}

impl CanvasSizeInput {
    #[must_use]
    pub fn new(canvas: CanvasSpec) -> Self {
        let landscape = [
            canvas.width_px.max(canvas.height_px),
            canvas.width_px.min(canvas.height_px),
        ];
        let source = COMMON_PRESETS
            .iter()
            .chain(&MORE_PRESETS)
            .find(|preset| preset.size == landscape)
            .map(|&preset| (preset, 1));
        Self {
            width: canvas.width_px.to_string(),
            height: canvas.height_px.to_string(),
            source,
        }
    }

    /// # Errors
    /// Rejects nonpositive dimensions and sizes that exceed the output pixel budget.
    pub fn size(&self) -> Result<[u32; 2], &'static str> {
        let (Ok(width), Ok(height)) = (self.width.parse::<u32>(), self.height.parse::<u32>())
        else {
            return Err("가로와 세로를 양의 정수로 입력하세요.");
        };
        if width == 0 || height == 0 {
            return Err("가로와 세로는 1 이상이어야 합니다.");
        }
        within_output_limit([width, height])
            .then_some([width, height])
            .ok_or("현재 출력 한도를 초과합니다. 크기를 줄여주세요.")
    }

    #[must_use]
    pub fn multiplied(&self, factor: u32) -> Option<Self> {
        let [width, height] = self.size().ok()?;
        let size = [width.checked_mul(factor)?, height.checked_mul(factor)?];
        if !within_output_limit(size) {
            return None;
        }
        let source = match self.source {
            Some((preset, multiplier)) => Some((preset, multiplier.checked_mul(factor)?)),
            None => None,
        };
        Some(Self {
            width: size[0].to_string(),
            height: size[1].to_string(),
            source,
        })
    }

    #[must_use]
    pub fn label(&self) -> String {
        match self.source {
            Some((preset, 1)) => preset.name.into(),
            Some((preset, multiplier)) => format!("{} ×{multiplier}", preset.name),
            None => "커스텀".into(),
        }
    }
}

pub struct CanvasDraft {
    pub current: CanvasSizeInput,
    undo: VecDeque<CanvasSizeInput>,
    typing_start: Option<CanvasSizeInput>,
}

impl CanvasDraft {
    #[must_use]
    pub fn new(canvas: CanvasSpec) -> Self {
        Self {
            current: CanvasSizeInput::new(canvas),
            undo: VecDeque::new(),
            typing_start: None,
        }
    }

    fn remember(&mut self, previous: CanvasSizeInput) {
        if previous == self.current {
            return;
        }
        if self.undo.len() == SIZE_UNDO_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(previous);
    }

    pub fn finish_typing(&mut self) {
        if let Some(previous) = self.typing_start.take() {
            self.remember(previous);
        }
    }

    pub fn type_dimension(&mut self, horizontal: bool, text: String) {
        if self.typing_start.is_none() {
            self.typing_start = Some(self.current.clone());
        }
        if horizontal {
            self.current.width = text;
        } else {
            self.current.height = text;
        }
        self.current.source = None;
    }

    fn replace(&mut self, next: CanvasSizeInput) {
        self.finish_typing();
        let previous = std::mem::replace(&mut self.current, next);
        self.remember(previous);
    }

    pub fn choose_preset(&mut self, preset: CanvasPreset) {
        let mut size = preset.size;
        if let Ok([width, height]) = self.current.size()
            && width < height
        {
            size.swap(0, 1);
        }
        self.replace(CanvasSizeInput {
            width: size[0].to_string(),
            height: size[1].to_string(),
            source: Some((preset, 1)),
        });
    }

    pub fn multiply(&mut self, factor: u32) {
        if let Some(next) = self.current.multiplied(factor) {
            self.replace(next);
        }
    }

    pub fn swap_axes(&mut self) {
        let mut next = self.current.clone();
        std::mem::swap(&mut next.width, &mut next.height);
        self.replace(next);
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
            || self
                .typing_start
                .as_ref()
                .is_some_and(|start| start != &self.current)
    }

    pub fn undo(&mut self) {
        self.finish_typing();
        if let Some(previous) = self.undo.pop_back() {
            self.current = previous;
        }
    }
}

/// Scale a raster export from one authoritative axis, preserving its exact value.
#[must_use]
pub fn scaled_export_axis(source: [u32; 2], axis: usize, value: u32) -> Option<[u32; 2]> {
    if axis > 1 || value == 0 || value > source[axis] || source[axis] == 0 {
        return None;
    }
    let other = 1 - axis;
    let mut result = [0; 2];
    result[axis] = value;
    result[other] = u32::try_from(
        ((u64::from(source[other]) * u64::from(value) + u64::from(source[axis]) / 2)
            / u64::from(source[axis]))
        .max(1),
    )
    .ok()?;
    Some(result)
}

#[must_use]
pub fn scaled_export_percent(source: [u32; 2], percent: f64) -> Option<[u32; 2]> {
    if !percent.is_finite() || percent <= 0.0 || percent > 100.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(source.map(|dimension| {
        (f64::from(dimension) * percent / 100.0)
            .round()
            .clamp(1.0, f64::from(u32::MAX)) as u32
    }))
}

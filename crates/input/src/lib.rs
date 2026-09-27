#![forbid(unsafe_code)]

mod smoothing;

pub use smoothing::{SmoothingError, StrokeSmoother};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Backend-neutral canvas viewport. Coordinates entering the model are
/// physical window-client pixels; document coordinates are logical canvas
/// units. `revision` identifies the exact mapping used for a sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportTransform {
    pub revision: u64,
    pub window_origin_physical: Point,
    pub physical_size: [u32; 2],
    pub dpi_scale: f64,
    pub pan: Point,
    pub zoom: f64,
    pub rotation_radians: f64,
    /// Reflect document X before rotating. This is a view transform only.
    pub mirrored_horizontal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewportValidationError {
    NonFinite,
    InvalidDpi,
    InvalidZoom,
    EmptySurface,
}

impl ViewportTransform {
    pub const MIN_DPI: f64 = 0.1;
    pub const MAX_DPI: f64 = 16.0;
    pub const MIN_ZOOM: f64 = 0.01;
    pub const MAX_ZOOM: f64 = 100.0;

    /// Validates dimensions, finite coordinates, DPI, and zoom bounds.
    ///
    /// # Errors
    ///
    /// Returns the first invalid viewport invariant.
    pub fn validate(self) -> Result<Self, ViewportValidationError> {
        if self.physical_size.contains(&0) {
            return Err(ViewportValidationError::EmptySurface);
        }
        if !self.window_origin_physical.x.is_finite()
            || !self.window_origin_physical.y.is_finite()
            || !self.pan.x.is_finite()
            || !self.pan.y.is_finite()
            || !self.rotation_radians.is_finite()
        {
            return Err(ViewportValidationError::NonFinite);
        }
        if !self.dpi_scale.is_finite() || !(Self::MIN_DPI..=Self::MAX_DPI).contains(&self.dpi_scale)
        {
            return Err(ViewportValidationError::InvalidDpi);
        }
        if !self.zoom.is_finite() || !(Self::MIN_ZOOM..=Self::MAX_ZOOM).contains(&self.zoom) {
            return Err(ViewportValidationError::InvalidZoom);
        }
        Ok(self)
    }

    #[must_use]
    pub fn window_to_logical(self, window_physical: Point) -> Option<Point> {
        if self.validate().is_err()
            || !window_physical.x.is_finite()
            || !window_physical.y.is_finite()
        {
            return None;
        }
        Some(Point {
            x: (window_physical.x - self.window_origin_physical.x) / self.dpi_scale,
            y: (window_physical.y - self.window_origin_physical.y) / self.dpi_scale,
        })
    }

    #[must_use]
    pub fn logical_to_window(self, logical: Point) -> Option<Point> {
        if self.validate().is_err() || !logical.x.is_finite() || !logical.y.is_finite() {
            return None;
        }
        Some(Point {
            x: self.window_origin_physical.x + logical.x * self.dpi_scale,
            y: self.window_origin_physical.y + logical.y * self.dpi_scale,
        })
    }

    #[must_use]
    pub fn logical_to_document(self, logical: Point) -> Option<Point> {
        if self.validate().is_err() || !logical.x.is_finite() || !logical.y.is_finite() {
            return None;
        }
        let x = logical.x - self.pan.x;
        let y = logical.y - self.pan.y;
        let (sin, cos) = self.rotation_radians.sin_cos();
        Some(Point {
            x: (cos * x + sin * y) / self.zoom * if self.mirrored_horizontal { -1.0 } else { 1.0 },
            y: (-sin * x + cos * y) / self.zoom,
        })
    }

    #[must_use]
    pub fn document_to_logical(self, document: Point) -> Option<Point> {
        if self.validate().is_err() || !document.x.is_finite() || !document.y.is_finite() {
            return None;
        }
        let (sin, cos) = self.rotation_radians.sin_cos();
        let x = document.x * if self.mirrored_horizontal { -1.0 } else { 1.0 };
        Some(Point {
            x: cos * x * self.zoom - sin * document.y * self.zoom + self.pan.x,
            y: sin * x * self.zoom + cos * document.y * self.zoom + self.pan.y,
        })
    }

    /// Change view orientation/scale without moving the artwork under `focus`.
    /// Pan is adjusted to preserve the document anchor; document pixels are untouched.
    #[must_use]
    pub fn with_view_at(
        self,
        focus: Point,
        zoom: f64,
        rotation_radians: f64,
        mirrored_horizontal: bool,
    ) -> Option<Self> {
        let document = self.logical_to_document(focus)?;
        Self {
            zoom,
            rotation_radians,
            mirrored_horizontal,
            ..self
        }
        .with_document_at(document, focus)
    }

    /// Centering/navigation uses the same forward transform as input and GPU display.
    #[must_use]
    pub fn with_document_at(self, document: Point, logical: Point) -> Option<Self> {
        if !logical.x.is_finite() || !logical.y.is_finite() {
            return None;
        }
        let projected = self.document_to_logical(document)?;
        let candidate = Self {
            pan: Point {
                x: self.pan.x + logical.x - projected.x,
                y: self.pan.y + logical.y - projected.y,
            },
            ..self
        };
        candidate.validate().ok()
    }

    #[must_use]
    pub fn window_to_document(self, window_physical: Point) -> Option<Point> {
        self.window_to_logical(window_physical)
            .and_then(|logical| self.logical_to_document(logical))
    }

    #[must_use]
    pub fn document_to_window(self, document: Point) -> Option<Point> {
        if self.validate().is_err() || !document.x.is_finite() || !document.y.is_finite() {
            return None;
        }
        self.document_to_logical(document)
            .and_then(|logical| self.logical_to_window(logical))
    }
}

/// Deterministic policy for viewport changes while a stroke is active.
/// Moves from a different revision are rejected; end/cancel transitions always
/// pass so an active stroke cannot be left open by a resize or zoom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InStrokeViewportPolicy {
    active_revision: Option<u64>,
}

impl InStrokeViewportPolicy {
    pub fn admit(&mut self, phase: PointerPhase, revision: u64) -> bool {
        match phase {
            PointerPhase::Begin => {
                self.active_revision = Some(revision);
                true
            }
            PointerPhase::Move => self.active_revision == Some(revision),
            PointerPhase::End | PointerPhase::Cancel => {
                let admitted = self.active_revision.is_some();
                self.active_revision = None;
                admitted
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerPhase {
    Begin,
    Move,
    End,
    Cancel,
}

/// App preference applied to live pen samples before brush evaluation and recording.
/// Stored stroke samples already contain effective pressure and must not be mapped again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PressurePreset {
    Linear,
    #[default]
    Soft,
    Softer,
}

impl PressurePreset {
    #[must_use]
    pub const fn storage_value(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Soft => "soft",
            Self::Softer => "softer",
        }
    }

    #[must_use]
    pub fn from_storage(value: &str) -> Option<Self> {
        match value {
            "linear" => Some(Self::Linear),
            "soft" => Some(Self::Soft),
            "softer" => Some(Self::Softer),
            _ => None,
        }
    }

    #[must_use]
    pub fn map(self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        let k = match self {
            Self::Linear => 0.0,
            Self::Soft => 0.35,
            Self::Softer => 0.7,
        };
        p + k * p * (1.0 - p)
    }
}

/// Maps only live pen samples. The caller records the returned effective pressure.
#[derive(Clone, Copy, Debug, Default)]
pub struct LivePressureMapper {
    active: Option<PressurePreset>,
}

impl LivePressureMapper {
    #[must_use]
    pub fn process(
        &mut self,
        phase: PointerPhase,
        pressure: f32,
        is_pen: bool,
        selected: PressurePreset,
    ) -> f32 {
        if phase == PointerPhase::Begin {
            self.active = is_pen.then_some(selected);
        }
        let effective = self.active.map_or(pressure, |preset| preset.map(pressure));
        if matches!(phase, PointerPhase::End | PointerPhase::Cancel) {
            self.active = None;
        }
        effective
    }

    pub fn clear(&mut self) {
        self.active = None;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PenButtons(pub u32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StylusSample {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub device_id: u64,
    pub phase: PointerPhase,
    pub position_document: Point,
    pub pressure: f32,
    pub tilt: Option<[f32; 2]>,
    pub twist_radians: Option<f32>,
    pub tangential_pressure: Option<f32>,
    pub buttons: PenButtons,
    pub eraser: bool,
    pub viewport_revision: u64,
}

impl StylusSample {
    #[must_use]
    pub fn with_normalized_axes(mut self) -> Self {
        self.pressure = self.pressure.clamp(0.0, 1.0);
        self.tangential_pressure = self.tangential_pressure.map(|value| value.clamp(-1.0, 1.0));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn pressure_presets_preserve_contact_endpoints_and_monotonic_order() {
        for preset in [
            PressurePreset::Linear,
            PressurePreset::Soft,
            PressurePreset::Softer,
        ] {
            assert_eq!(preset.map(0.0), 0.0);
            assert_eq!(preset.map(1.0), 1.0);
            let values: Vec<_> = (0_u16..=100)
                .map(|step| preset.map(f32::from(step) / 100.0))
                .collect();
            assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
        }
        assert!(PressurePreset::Soft.map(0.5) > 0.5);
        assert!(PressurePreset::Softer.map(0.5) > PressurePreset::Soft.map(0.5));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn live_pen_mapping_freezes_at_begin_and_leaves_mouse_unchanged() {
        let mut mapper = LivePressureMapper::default();
        let begin = mapper.process(PointerPhase::Begin, 0.0, true, PressurePreset::Soft);
        let move_pressure = mapper.process(PointerPhase::Move, 0.5, true, PressurePreset::Softer);
        let end = mapper.process(PointerPhase::End, 1.0, true, PressurePreset::Softer);
        assert_eq!(begin, 0.0);
        assert_eq!(move_pressure, PressurePreset::Soft.map(0.5));
        assert_eq!(end, 1.0);
        assert_eq!(
            mapper.process(PointerPhase::Begin, 1.0, false, PressurePreset::Softer),
            1.0
        );
        assert_eq!(
            mapper.process(PointerPhase::End, 1.0, false, PressurePreset::Softer),
            1.0
        );
        assert_eq!(
            mapper.process(PointerPhase::Cancel, 0.0, true, PressurePreset::Softer),
            0.0
        );
        assert_eq!(
            mapper.process(PointerPhase::Begin, 0.5, true, PressurePreset::Softer),
            PressurePreset::Softer.map(0.5)
        );
    }

    #[test]
    fn normalizes_pressure_axes() {
        let sample = StylusSample {
            sequence: 0,
            timestamp_ns: 0,
            device_id: 0,
            phase: PointerPhase::Move,
            position_document: Point::default(),
            pressure: 1.4,
            tilt: None,
            twist_radians: None,
            tangential_pressure: Some(-2.0),
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        }
        .with_normalized_axes();

        assert!((sample.pressure - 1.0).abs() < f32::EPSILON);
        let tangential = sample.tangential_pressure.expect("axis remains present");
        assert!((tangential + 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn mirrored_view_anchors_and_round_trips_prevent_drawing_on_the_wrong_pixels() {
        let center = Point { x: 210.0, y: 145.0 };
        let document = Point { x: -32.5, y: 91.75 };
        for (rotation, zoom, scale) in [(0.0, 1.0, 1.0), (0.73, 2.5, 1.5), (-1.7, 0.3, 2.0)] {
            let original = ViewportTransform {
                revision: 7,
                window_origin_physical: Point { x: 17.0, y: 23.0 },
                physical_size: [840, 580],
                dpi_scale: scale,
                pan: Point { x: 57.0, y: -36.0 },
                zoom,
                rotation_radians: rotation,
                mirrored_horizontal: false,
            };
            let anchor = original.logical_to_document(center).expect("anchor");
            let mirrored = original
                .with_view_at(center, zoom, rotation, true)
                .expect("mirror");
            assert_point_near(
                mirrored.logical_to_document(center).expect("same anchor"),
                anchor,
            );
            let twice = mirrored
                .with_view_at(center, zoom, rotation, false)
                .expect("unmirror");
            assert_point_near(twice.pan, original.pan);

            // Reflection reverses document X before rotation, around the held center.
            let original_right = original
                .document_to_logical(Point {
                    x: anchor.x + 12.0,
                    y: anchor.y,
                })
                .expect("right");
            let mirror_right = mirrored
                .document_to_logical(Point {
                    x: anchor.x + 12.0,
                    y: anchor.y,
                })
                .expect("mirrored right");
            assert_point_near(
                Point {
                    x: original_right.x + mirror_right.x,
                    y: original_right.y + mirror_right.y,
                },
                Point {
                    x: center.x * 2.0,
                    y: center.y * 2.0,
                },
            );
            for view in [original, mirrored] {
                let physical = view.document_to_window(document).expect("forward");
                assert_point_near(
                    view.window_to_document(physical).expect("inverse"),
                    document,
                );
                let reset = view
                    .with_view_at(center, zoom, 0.0, view.mirrored_horizontal)
                    .expect("reset angle");
                assert_point_near(
                    reset.logical_to_document(center).expect("reset anchor"),
                    anchor,
                );
                let focus = Point { x: 29.0, y: 79.0 };
                let focus_document = view.logical_to_document(focus).expect("zoom anchor");
                let zoomed = view
                    .with_view_at(focus, zoom * 1.25, rotation, view.mirrored_horizontal)
                    .expect("zoom");
                assert_point_near(
                    zoomed.logical_to_document(focus).expect("held zoom anchor"),
                    focus_document,
                );
                let navigated = view
                    .with_document_at(document, center)
                    .expect("navigator center");
                assert_point_near(
                    navigated
                        .logical_to_document(center)
                        .expect("navigation inverse"),
                    document,
                );
            }
        }
    }

    fn assert_point_near(actual: Point, expected: Point) {
        assert!(
            (actual.x - expected.x).abs() < 1e-9,
            "x: {actual:?} != {expected:?}"
        );
        assert!(
            (actual.y - expected.y).abs() < 1e-9,
            "y: {actual:?} != {expected:?}"
        );
    }

    #[test]
    fn viewport_round_trip_and_stroke_revision_policy_prevent_mixed_coordinates() {
        let viewport = ViewportTransform {
            revision: 7,
            window_origin_physical: Point { x: 11.0, y: 19.0 },
            physical_size: [800, 600],
            dpi_scale: 2.0,
            pan: Point { x: 3.0, y: -4.0 },
            zoom: 1.5,
            rotation_radians: 0.37,
            mirrored_horizontal: false,
        };
        let document = Point { x: 21.0, y: -8.0 };
        let window = viewport
            .document_to_window(document)
            .expect("valid viewport");
        let round_trip = viewport.window_to_document(window).expect("valid viewport");
        assert!((round_trip.x - document.x).abs() < 1e-10);
        assert!((round_trip.y - document.y).abs() < 1e-10);

        let mut policy = InStrokeViewportPolicy::default();
        assert!(policy.admit(PointerPhase::Begin, 7));
        assert!(!policy.admit(PointerPhase::Move, 8));
        assert!(policy.admit(PointerPhase::End, 8));
        assert!(!policy.admit(PointerPhase::Move, 8));
    }
}

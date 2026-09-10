#![forbid(unsafe_code)]

//! Deterministic, platform-neutral round-brush evaluation.
//!
//! `BrushDab::opacity` is the pressure-mapped stroke opacity. `BrushDab::flow`
//! is intentionally separate: a build-up renderer uses their product as the
//! per-dab alpha, while future non-build-up modes can retain opacity without
//! changing the dab contract.

use nyatidraw_input::{Point, StylusSample};

pub const ROUND_BRUSH_ENGINE_VERSION: u32 = 2;
pub const ROUND_BRUSH_PRESET_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BrushPresetId(pub u128);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushPreset {
    pub id: BrushPresetId,
    pub schema_version: u32,
    pub engine_version: u32,
    /// Full diameter at pressure 1.0, in document pixels.
    pub size_px: f32,
    /// Maximum stroke opacity at pressure 1.0.
    pub opacity: f32,
    /// Per-dab build-up amount, independent from `opacity`.
    pub flow: f32,
    /// Dab spacing divided by the preset's full diameter.
    pub spacing_ratio: f32,
    pub size_pressure: bool,
    pub opacity_pressure: bool,
    /// Minimum fraction of the full diameter when size pressure is enabled.
    pub size_min_ratio: f32,
    /// Minimum fraction of opacity when opacity pressure is enabled.
    pub opacity_min_ratio: f32,
    /// Opaque inner radius fraction. One preserves the legacy hard circle.
    pub hardness: f32,
}

/// Immutable brush configuration captured when a stroke is sealed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushSnapshot {
    pub preset: BrushPreset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushDab {
    pub center: Point,
    pub radius_px: f32,
    pub opacity: f32,
    pub flow: f32,
    pub hardness: f32,
}

/// Metadata required to replay the same deterministic brush engine version.
///
/// The owning stroke/history layer records the source `StylusSample` stream
/// separately. This small value captures the evaluator inputs that are not in
/// the samples themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedStroke {
    pub brush_engine_version: u32,
    pub preset_id: BrushPresetId,
    pub preset_schema_version: u32,
    pub random_seed: u64,
    pub sample_count: u64,
    pub first_sample_sequence: u64,
    pub last_sample_sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundBrushReplayError {
    UnsupportedEngineVersion(u32),
    PresetIdentityMismatch,
    EmptySampleStream,
    SampleCountMismatch,
    SampleSequenceMismatch,
    NonFiniteSample,
    InvalidPreset,
}

pub trait BrushEvaluator {
    type StrokeToken;

    fn begin(&mut self, preset: &BrushPreset, first: StylusSample) -> Self::StrokeToken;
    fn push(
        &mut self,
        token: &mut Self::StrokeToken,
        samples: &[StylusSample],
        out: &mut Vec<BrushDab>,
    );
    fn end(&mut self, token: Self::StrokeToken, out: &mut Vec<BrushDab>) -> RecordedStroke;
}

/// A round tip with independently switchable size/opacity pressure mappings.
///
/// Every stroke begins with a dab, so a tap remains visible. Subsequent dabs are
/// emitted on a fixed arc-length spacing calculated from the immutable preset.
#[derive(Clone, Debug)]
pub struct RoundBrushEvaluator {
    session_seed: u64,
    stroke_index: u64,
}

impl RoundBrushEvaluator {
    #[must_use]
    pub const fn new(session_seed: u64) -> Self {
        Self {
            session_seed,
            stroke_index: 0,
        }
    }
}

impl Default for RoundBrushEvaluator {
    fn default() -> Self {
        Self::new(0)
    }
}

#[derive(Clone, Debug)]
pub struct RoundBrushStroke {
    preset: BrushPreset,
    last_input: StylusSample,
    distance_since_dab: f64,
    random_seed: u64,
    sample_count: u64,
    first_sample_sequence: u64,
}

impl BrushEvaluator for RoundBrushEvaluator {
    type StrokeToken = RoundBrushStroke;

    fn begin(&mut self, preset: &BrushPreset, first: StylusSample) -> Self::StrokeToken {
        let random_seed = stroke_seed(self.session_seed, self.stroke_index, *preset, first);
        self.stroke_index = self.stroke_index.wrapping_add(1);

        RoundBrushStroke {
            preset: *preset,
            last_input: first,
            distance_since_dab: 0.0,
            random_seed,
            sample_count: 1,
            first_sample_sequence: first.sequence,
        }
    }

    fn push(
        &mut self,
        token: &mut Self::StrokeToken,
        samples: &[StylusSample],
        out: &mut Vec<BrushDab>,
    ) {
        for &sample in samples {
            resample_segment(token, sample, out);
        }
    }

    fn end(&mut self, token: Self::StrokeToken, out: &mut Vec<BrushDab>) -> RecordedStroke {
        let _ = out;
        RecordedStroke {
            brush_engine_version: token.preset.engine_version,
            preset_id: token.preset.id,
            preset_schema_version: token.preset.schema_version,
            random_seed: token.random_seed,
            sample_count: token.sample_count,
            first_sample_sequence: token.first_sample_sequence,
            last_sample_sequence: token.last_input.sequence,
        }
    }
}

/// Emits the tap dab required at a stroke begin.
pub fn begin_round_stroke(
    evaluator: &mut RoundBrushEvaluator,
    preset: &BrushPreset,
    first: StylusSample,
    out: &mut Vec<BrushDab>,
) -> RoundBrushStroke {
    let token = evaluator.begin(preset, first);
    out.push(dab_for(preset, first));
    token
}

/// Replays a sealed round-brush sample stream using its recorded seed.
///
/// # Errors
///
/// Returns an error when the brush engine is unsupported or the immutable
/// snapshot, record metadata, and sample stream do not describe one stroke.
pub fn replay_round_stroke(
    snapshot: &BrushSnapshot,
    recorded: &RecordedStroke,
    samples: &[StylusSample],
    out: &mut Vec<BrushDab>,
) -> Result<(), RoundBrushReplayError> {
    let preset = snapshot.preset;
    if !matches!(preset.engine_version, 1 | ROUND_BRUSH_ENGINE_VERSION)
        || recorded.brush_engine_version != preset.engine_version
    {
        return Err(RoundBrushReplayError::UnsupportedEngineVersion(
            recorded.brush_engine_version,
        ));
    }
    if !valid_preset(&preset) {
        return Err(RoundBrushReplayError::InvalidPreset);
    }
    if recorded.preset_id != preset.id || recorded.preset_schema_version != preset.schema_version {
        return Err(RoundBrushReplayError::PresetIdentityMismatch);
    }
    let Some((&first, remaining)) = samples.split_first() else {
        return Err(RoundBrushReplayError::EmptySampleStream);
    };
    let sample_count =
        u64::try_from(samples.len()).map_err(|_| RoundBrushReplayError::SampleCountMismatch)?;
    if recorded.sample_count != sample_count {
        return Err(RoundBrushReplayError::SampleCountMismatch);
    }
    let last_sequence = remaining.last().unwrap_or(&first).sequence;
    if recorded.first_sample_sequence != first.sequence
        || recorded.last_sample_sequence != last_sequence
    {
        return Err(RoundBrushReplayError::SampleSequenceMismatch);
    }
    if samples.iter().any(|sample| {
        !sample.position_document.x.is_finite()
            || !sample.position_document.y.is_finite()
            || !sample.pressure.is_finite()
    }) {
        return Err(RoundBrushReplayError::NonFiniteSample);
    }

    let mut token = RoundBrushStroke {
        preset,
        last_input: first,
        distance_since_dab: 0.0,
        random_seed: recorded.random_seed,
        sample_count: 1,
        first_sample_sequence: first.sequence,
    };
    out.push(dab_for(&preset, first));
    let mut evaluator = RoundBrushEvaluator::default();
    evaluator.push(&mut token, remaining, out);
    let replayed = evaluator.end(token, out);
    debug_assert_eq!(&replayed, recorded);
    Ok(())
}

fn resample_segment(token: &mut RoundBrushStroke, next: StylusSample, out: &mut Vec<BrushDab>) {
    let start = token.last_input;
    token.sample_count = token.sample_count.saturating_add(1);

    let dx = next.position_document.x - start.position_document.x;
    let dy = next.position_document.y - start.position_document.y;
    let mut remaining = dx.hypot(dy);
    if !remaining.is_finite() {
        token.last_input = next;
        return;
    }

    let spacing = f64::from(spacing_px(&token.preset));
    let mut segment_progress = 0.0_f64;
    while remaining + token.distance_since_dab >= spacing {
        let advance = spacing - token.distance_since_dab;
        let step = if remaining > 0.0 {
            (advance / remaining).clamp(0.0, 1.0)
        } else {
            1.0
        };
        segment_progress += (1.0 - segment_progress) * step;
        let dab_sample = interpolate_sample(start, next, segment_progress);
        out.push(dab_for(&token.preset, dab_sample));
        remaining -= advance;
        token.distance_since_dab = 0.0;

        if remaining <= f64::EPSILON {
            break;
        }
    }

    token.distance_since_dab += remaining.max(0.0);
    token.last_input = next;
}

#[must_use]
fn spacing_px(preset: &BrushPreset) -> f32 {
    if !preset.size_px.is_finite() || !preset.spacing_ratio.is_finite() {
        return 0.25;
    }
    (preset.size_px.max(0.0) * preset.spacing_ratio.clamp(0.01, 4.0)).max(0.25)
}

#[must_use]
fn dab_for(preset: &BrushPreset, sample: StylusSample) -> BrushDab {
    let pressure = unit(sample.pressure);
    let legacy = preset.engine_version == 1;
    let size = if legacy {
        pressure.sqrt()
    } else {
        pressure_factor(preset.size_pressure, preset.size_min_ratio, pressure.sqrt())
    };
    let opacity = if legacy {
        pressure
    } else {
        pressure_factor(preset.opacity_pressure, preset.opacity_min_ratio, pressure)
    };
    BrushDab {
        center: sample.position_document,
        radius_px: if preset.size_px.is_finite() {
            preset.size_px.max(0.0) * size * 0.5
        } else {
            0.0
        },
        opacity: unit(preset.opacity) * opacity,
        flow: unit(preset.flow),
        hardness: if legacy { 1.0 } else { unit(preset.hardness) },
    }
}

fn unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn pressure_factor(enabled: bool, minimum: f32, pressure: f32) -> f32 {
    if enabled {
        let minimum = unit(minimum);
        minimum + (1.0 - minimum) * pressure
    } else {
        1.0
    }
}

fn valid_preset(preset: &BrushPreset) -> bool {
    [
        preset.size_px,
        preset.opacity,
        preset.flow,
        preset.spacing_ratio,
    ]
    .iter()
    .all(|value| value.is_finite())
        && (preset.engine_version == 1
            || (preset.schema_version == ROUND_BRUSH_PRESET_SCHEMA_VERSION
                && [
                    preset.size_min_ratio,
                    preset.opacity_min_ratio,
                    preset.hardness,
                ]
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))))
}

#[must_use]
fn interpolate_sample(start: StylusSample, end: StylusSample, t: f64) -> StylusSample {
    // `t` is clamped by the segment resampler to [0, 1], while the input axis
    // is already f32. This conversion cannot change the brush's axis domain.
    #[allow(clippy::cast_possible_truncation)]
    let pressure = start.pressure + (end.pressure - start.pressure) * t as f32;
    StylusSample {
        position_document: Point {
            x: start.position_document.x
                + (end.position_document.x - start.position_document.x) * t,
            y: start.position_document.y
                + (end.position_document.y - start.position_document.y) * t,
        },
        pressure,
        ..end
    }
}

#[must_use]
fn stroke_seed(
    session_seed: u64,
    stroke_index: u64,
    preset: BrushPreset,
    first: StylusSample,
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let preset_id_bytes = preset.id.0.to_le_bytes();
    let preset_id_low = u64::from_le_bytes(preset_id_bytes[..8].try_into().expect("exact slice"));
    let preset_id_high = u64::from_le_bytes(preset_id_bytes[8..].try_into().expect("exact slice"));
    for value in [
        session_seed,
        stroke_index,
        preset_id_low,
        preset_id_high,
        u64::from(preset.schema_version),
        u64::from(preset.engine_version),
        first.sequence,
        first.timestamp_ns,
        first.device_id,
    ] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_input::{PenButtons, PointerPhase};

    fn sample(sequence: u64, x: f64, pressure: f32) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 1_000,
            device_id: 9,
            phase: PointerPhase::Move,
            position_document: Point { x, y: 0.0 },
            pressure,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 0,
        }
    }

    fn preset() -> BrushPreset {
        BrushPreset {
            id: BrushPresetId(7),
            schema_version: ROUND_BRUSH_PRESET_SCHEMA_VERSION,
            engine_version: ROUND_BRUSH_ENGINE_VERSION,
            size_px: 10.0,
            opacity: 0.8,
            flow: 0.5,
            spacing_ratio: 0.5,
            size_pressure: true,
            opacity_pressure: true,
            size_min_ratio: 0.0,
            opacity_min_ratio: 0.0,
            hardness: 1.0,
        }
    }

    #[test]
    fn arc_length_resampling_is_deterministic_and_preserves_pressure_dynamics() {
        let first = sample(10, 0.0, 0.25);
        let last = sample(11, 20.0, 1.0);
        let mut left = RoundBrushEvaluator::new(99);
        let mut right = RoundBrushEvaluator::new(99);
        let mut left_dabs = Vec::new();
        let mut right_dabs = Vec::new();

        let mut left_token = begin_round_stroke(&mut left, &preset(), first, &mut left_dabs);
        left.push(&mut left_token, &[last], &mut left_dabs);
        let left_record = left.end(left_token, &mut left_dabs);
        let mut right_token = begin_round_stroke(&mut right, &preset(), first, &mut right_dabs);
        right.push(&mut right_token, &[last], &mut right_dabs);
        let right_record = right.end(right_token, &mut right_dabs);

        assert_eq!(left_dabs, right_dabs, "same stroke must replay identically");
        assert_eq!(
            left_record, right_record,
            "record metadata must replay identically"
        );
        assert_eq!(
            left_dabs.len(),
            5,
            "5px spacing must cover the 20px segment"
        );
        assert_eq!(
            left_dabs.iter().map(|dab| dab.center.x).collect::<Vec<_>>(),
            vec![0.0, 5.0, 10.0, 15.0, 20.0]
        );
        assert!(left_dabs[0].radius_px < left_dabs[4].radius_px);
        assert!(left_dabs[0].opacity < left_dabs[4].opacity);
        assert!((left_dabs[0].flow - left_dabs[4].flow).abs() < f32::EPSILON);
    }

    #[test]
    fn pressure_axes_and_legacy_replay_preserve_the_recorded_artwork() {
        // Risk: disabling opacity pressure must not change size pressure, and
        // opening a v1 stroke must never reinterpret its tip or pressure curve.
        for (size_pressure, opacity_pressure, expected_radius, expected_opacity) in [
            (false, false, 5.0, 0.8),
            (true, false, 3.0, 0.8),
            (false, true, 5.0, 0.5),
            (true, true, 3.0, 0.5),
        ] {
            let configured = BrushPreset {
                size_pressure,
                opacity_pressure,
                size_min_ratio: 0.2,
                opacity_min_ratio: 0.5,
                hardness: 0.3,
                ..preset()
            };
            let dab = dab_for(&configured, sample(1, 0.0, 0.25));
            assert!((dab.radius_px - expected_radius).abs() < 0.000_001);
            assert!((dab.opacity - expected_opacity).abs() < 0.000_001);
            let mut evaluator = RoundBrushEvaluator::default();
            let mut dabs = Vec::new();
            let samples = [sample(1, 0.0, 0.25), sample(2, 10.0, 1.0)];
            let mut token = begin_round_stroke(&mut evaluator, &configured, samples[0], &mut dabs);
            evaluator.push(&mut token, &samples[1..], &mut dabs);
            let record = evaluator.end(token, &mut dabs);
            let mut replay = Vec::new();
            replay_round_stroke(
                &BrushSnapshot { preset: configured },
                &record,
                &samples,
                &mut replay,
            )
            .unwrap();
            assert_eq!(dabs, replay);
        }
        let legacy = BrushPreset {
            engine_version: 1,
            schema_version: 1,
            size_pressure: false,
            opacity_pressure: false,
            hardness: 0.0,
            size_min_ratio: 0.9,
            opacity_min_ratio: 0.9,
            ..preset()
        };
        let dab = dab_for(&legacy, sample(1, 0.0, 0.25));
        assert_eq!(
            dab,
            BrushDab {
                center: Point { x: 0.0, y: 0.0 },
                radius_px: 2.5,
                opacity: 0.2,
                flow: 0.5,
                hardness: 1.0
            }
        );
        for value in [f32::NAN, f32::INFINITY, -1.0, 2.0] {
            let configured = BrushPreset {
                size_min_ratio: value,
                opacity_min_ratio: value,
                hardness: value,
                ..preset()
            };
            let dab = dab_for(&configured, sample(1, 0.0, value));
            assert!(
                dab.radius_px.is_finite() && dab.opacity.is_finite() && dab.hardness.is_finite()
            );
        }
    }
}

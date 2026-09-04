#![forbid(unsafe_code)]

//! Deterministic, platform-neutral round-brush evaluation.
//!
//! `BrushDab::opacity` is the pressure-mapped stroke opacity. `BrushDab::flow`
//! is intentionally separate: a build-up renderer uses their product as the
//! per-dab alpha, while future non-build-up modes can retain opacity without
//! changing the dab contract.

use nyatidraw_input::{Point, StylusSample};

pub const ROUND_BRUSH_ENGINE_VERSION: u32 = 1;

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

/// A round tip with square-root pressure-to-size and linear pressure-to-opacity.
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
    if preset.engine_version != ROUND_BRUSH_ENGINE_VERSION
        || recorded.brush_engine_version != ROUND_BRUSH_ENGINE_VERSION
    {
        return Err(RoundBrushReplayError::UnsupportedEngineVersion(
            recorded.brush_engine_version,
        ));
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
    (preset.size_px.max(0.0) * preset.spacing_ratio.clamp(0.01, 4.0)).max(0.25)
}

#[must_use]
fn dab_for(preset: &BrushPreset, sample: StylusSample) -> BrushDab {
    let pressure = sample.pressure.clamp(0.0, 1.0);
    BrushDab {
        center: sample.position_document,
        radius_px: preset.size_px.max(0.0) * pressure.sqrt() * 0.5,
        opacity: preset.opacity.clamp(0.0, 1.0) * pressure,
        flow: preset.flow.clamp(0.0, 1.0),
    }
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
            schema_version: 1,
            engine_version: ROUND_BRUSH_ENGINE_VERSION,
            size_px: 10.0,
            opacity: 0.8,
            flow: 0.5,
            spacing_ratio: 0.5,
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
}

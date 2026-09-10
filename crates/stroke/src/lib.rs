#![forbid(unsafe_code)]

//! Closed-stroke contract and deterministic CPU materialization.

use std::{collections::BTreeSet, sync::Arc};

use nyatidraw_api::{LayerId, SnapshotId};
use nyatidraw_brush::{
    BrushDab, BrushSnapshot, RecordedStroke, RoundBrushReplayError, replay_round_stroke,
};
use nyatidraw_input::{PointerPhase, StylusSample};
pub use nyatidraw_paint_cpu::SelectionMask as StrokeSelection;
use nyatidraw_paint_cpu::{CpuCanvas, CpuCanvasError, PremultipliedRgba8};
use nyatidraw_tiles::{
    ContentRoot, ObjectHash, TILE_BYTE_LEN, TILE_EDGE, TileBounds, TileKey, TileSnapshot,
    TileSnapshotError,
};

const MAX_AFFECTED_TILES_PER_STROKE: u64 = 65_536;
/// Hard upper bound for one sealed stroke's retained raw sample stream.
///
/// At 240 samples/second this permits over 18 minutes of continuous input
/// while bounding hashing, replay, and project decode allocations.
pub const MAX_SAMPLES_PER_STROKE: usize = 262_144;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StrokeCommitId(pub ObjectHash);

/// Premultiplied linear-light RGBA8 colour captured with a sealed stroke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StrokeColor(pub [u8; 4]);

impl StrokeColor {
    /// # Errors
    ///
    /// Returns an error when an RGB channel exceeds alpha.
    pub const fn new(bytes: [u8; 4]) -> Result<Self, StrokeCommitError> {
        if bytes[0] > bytes[3] || bytes[1] > bytes[3] || bytes[2] > bytes[3] {
            return Err(StrokeCommitError::NonPremultipliedColor);
        }
        Ok(Self(bytes))
    }
}

/// Immutable semantic stroke sealed against one content root.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeCommit {
    pub id: StrokeCommitId,
    pub parent_snapshot: SnapshotId,
    pub layer: LayerId,
    pub before_root: ContentRoot,
    pub affected_tiles: TileBounds,
    pub brush: BrushSnapshot,
    pub recorded: RecordedStroke,
    pub color: StrokeColor,
    samples: Arc<[StylusSample]>,
    selection: Option<Arc<StrokeSelection>>,
    alpha_locked: bool,
}

impl StrokeCommit {
    /// Seals a closed sample stream and its exact before root.
    ///
    /// # Errors
    ///
    /// Returns an error when phase transitions, sequence ordering, brush
    /// metadata, samples, or affected bounds cannot be replayed safely.
    pub fn seal(
        parent_snapshot: SnapshotId,
        layer: LayerId,
        before: &TileSnapshot,
        brush: BrushSnapshot,
        recorded: RecordedStroke,
        color: StrokeColor,
        samples: Vec<StylusSample>,
    ) -> Result<Self, StrokeCommitError> {
        Self::seal_with_selection(
            parent_snapshot,
            layer,
            before,
            brush,
            recorded,
            color,
            samples,
            None,
        )
    }

    /// Seals the exact immutable selection observed at Begin with the stroke.
    ///
    /// # Errors
    /// Returns the same replay and phase validation failures as [`Self::seal`].
    #[allow(clippy::too_many_arguments)]
    pub fn seal_with_selection(
        parent_snapshot: SnapshotId,
        layer: LayerId,
        before: &TileSnapshot,
        brush: BrushSnapshot,
        recorded: RecordedStroke,
        color: StrokeColor,
        samples: Vec<StylusSample>,
        selection: Option<Arc<StrokeSelection>>,
    ) -> Result<Self, StrokeCommitError> {
        Self::seal_with_options(
            parent_snapshot,
            layer,
            before,
            brush,
            recorded,
            color,
            samples,
            selection,
            false,
        )
    }

    /// Seals Begin-captured selection and alpha-lock behavior independently of
    /// future layer metadata. Legacy wrappers keep the original replay hash.
    ///
    /// # Errors
    /// Returns stroke validation failures or rejects alpha-locked erasing.
    #[allow(clippy::too_many_arguments)]
    pub fn seal_with_options(
        parent_snapshot: SnapshotId,
        layer: LayerId,
        before: &TileSnapshot,
        brush: BrushSnapshot,
        recorded: RecordedStroke,
        color: StrokeColor,
        samples: Vec<StylusSample>,
        selection: Option<Arc<StrokeSelection>>,
        alpha_locked: bool,
    ) -> Result<Self, StrokeCommitError> {
        validate_closed_samples(&samples)?;
        if alpha_locked && samples.first().is_some_and(|sample| sample.eraser) {
            return Err(StrokeCommitError::AlphaLockedEraser);
        }
        let mut dabs = Vec::new();
        replay_round_stroke(&brush, &recorded, &samples, &mut dabs)
            .map_err(StrokeCommitError::BrushReplay)?;
        let affected_keys = affected_tile_keys(layer, &dabs)?;
        let affected_tiles = TileBounds::from_keys(affected_keys.iter().copied())
            .ok_or(StrokeCommitError::NoAffectedTiles)?;
        let samples: Arc<[StylusSample]> = samples.into();
        let mut commit = Self {
            id: StrokeCommitId(ObjectHash([0; 32])),
            parent_snapshot,
            layer,
            before_root: before.root(),
            affected_tiles,
            brush,
            recorded,
            color,
            samples,
            selection,
            alpha_locked,
        };
        commit.id = StrokeCommitId(hash_commit(&commit));
        Ok(commit)
    }

    #[must_use]
    pub fn samples(&self) -> &[StylusSample] {
        &self.samples
    }

    #[must_use]
    pub fn selection(&self) -> Option<&StrokeSelection> {
        self.selection.as_deref()
    }

    #[must_use]
    pub const fn alpha_locked(&self) -> bool {
        self.alpha_locked
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrokeCommitError {
    EmptySampleStream,
    TooManySamples,
    InvalidPhaseTransition,
    NonIncreasingSequence,
    DecreasingTimestamp,
    DeviceChanged,
    EraserModeChanged,
    AlphaLockedEraser,
    NonPremultipliedColor,
    BrushReplay(RoundBrushReplayError),
    NoAffectedTiles,
    CoordinateOutOfRange,
    TooManyAffectedTiles,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterializationStrategy {
    CpuReplay,
    GpuReadback,
    HybridCheckpoint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaterializationError {
    UnsupportedStrategy(MaterializationStrategy),
    BeforeRootMismatch {
        expected: ContentRoot,
        actual: ContentRoot,
    },
    BrushReplay(RoundBrushReplayError),
    AffectedTilesChanged,
    CoordinateOutOfRange,
    TooManyAffectedTiles,
    CpuCanvas(CpuCanvasError),
    TileSnapshot(TileSnapshotError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterializedStroke {
    pub strategy: MaterializationStrategy,
    pub commit_id: StrokeCommitId,
    pub before_root: ContentRoot,
    pub after: TileSnapshot,
    pub changed_tiles: Vec<(TileKey, Option<ObjectHash>)>,
}

pub trait StrokeMaterializer {
    fn strategy(&self) -> MaterializationStrategy;

    /// # Errors
    ///
    /// Returns an error when the before root or deterministic replay contract
    /// does not match the sealed commit.
    fn materialize(
        &self,
        commit: &StrokeCommit,
        before: &TileSnapshot,
    ) -> Result<MaterializedStroke, MaterializationError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuReplayMaterializer;

impl StrokeMaterializer for CpuReplayMaterializer {
    fn strategy(&self) -> MaterializationStrategy {
        MaterializationStrategy::CpuReplay
    }

    fn materialize(
        &self,
        commit: &StrokeCommit,
        before: &TileSnapshot,
    ) -> Result<MaterializedStroke, MaterializationError> {
        if before.root() != commit.before_root {
            return Err(MaterializationError::BeforeRootMismatch {
                expected: commit.before_root,
                actual: before.root(),
            });
        }

        let mut dabs = Vec::new();
        replay_round_stroke(&commit.brush, &commit.recorded, commit.samples(), &mut dabs)
            .map_err(MaterializationError::BrushReplay)?;
        let affected_keys =
            affected_tile_keys(commit.layer, &dabs).map_err(|error| match error {
                StrokeCommitError::CoordinateOutOfRange => {
                    MaterializationError::CoordinateOutOfRange
                }
                StrokeCommitError::TooManyAffectedTiles => {
                    MaterializationError::TooManyAffectedTiles
                }
                _ => MaterializationError::AffectedTilesChanged,
            })?;
        if TileBounds::from_keys(affected_keys.iter().copied()) != Some(commit.affected_tiles) {
            return Err(MaterializationError::AffectedTilesChanged);
        }

        let color = PremultipliedRgba8(commit.color.0);
        let mut replacements = Vec::with_capacity(affected_keys.len());
        for key in &affected_keys {
            if commit
                .selection()
                .is_some_and(|selection| !selection_intersects_tile(selection, *key))
            {
                continue;
            }
            let base_pixels = before
                .get(*key)
                .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
            let mut canvas = CpuCanvas::from_rgba8_premultiplied(TILE_EDGE, TILE_EDGE, base_pixels)
                .map_err(MaterializationError::CpuCanvas)?;
            let (origin_x, origin_y) = key.pixel_origin();
            for dab in &dabs {
                if !dab_intersects_tile(*dab, origin_x, origin_y) {
                    continue;
                }
                let dab = tile_local_dab(*dab, origin_x, origin_y);
                if commit.samples()[0].eraser {
                    canvas.erase_dab(dab);
                } else if commit.alpha_locked() {
                    canvas.apply_dab_alpha_locked(dab, color);
                } else {
                    canvas.apply_dab(dab, color);
                }
            }
            let mut pixels = canvas.pixels_rgba8_premultiplied().to_vec();
            if let Some(selection) = commit.selection() {
                for y in 0..TILE_EDGE {
                    for x in 0..TILE_EDGE {
                        let document_x = origin_x + i64::from(x);
                        let document_y = origin_y + i64::from(y);
                        let selected = match (i32::try_from(document_x), i32::try_from(document_y))
                        {
                            (Ok(x), Ok(y)) => selection.contains_signed(x, y),
                            _ => false,
                        };
                        if !selected {
                            let offset = usize::try_from((y * TILE_EDGE + x) * 4)
                                .expect("tile offset fits usize");
                            let original = before
                                .get(*key)
                                .map_or(&[0; 4][..], |tile| &tile.pixels()[offset..offset + 4]);
                            pixels[offset..offset + 4].copy_from_slice(original);
                        }
                    }
                }
            }
            replacements.push((*key, pixels));
        }

        let after = before
            .with_replacements(replacements)
            .map_err(MaterializationError::TileSnapshot)?;
        let changed_tiles = affected_keys
            .into_iter()
            .filter_map(|key| {
                let before_hash = before.get(key).map(nyatidraw_tiles::TileObject::hash);
                let after_hash = after.get(key).map(nyatidraw_tiles::TileObject::hash);
                (before_hash != after_hash).then_some((key, after_hash))
            })
            .collect();
        Ok(MaterializedStroke {
            strategy: MaterializationStrategy::CpuReplay,
            commit_id: commit.id,
            before_root: commit.before_root,
            after,
            changed_tiles,
        })
    }
}

#[allow(clippy::cast_precision_loss)]
fn dab_intersects_tile(dab: BrushDab, origin_x: i64, origin_y: i64) -> bool {
    let radius = f64::from(dab.radius_px);
    let origin_x = origin_x as f64;
    let origin_y = origin_y as f64;
    let edge = f64::from(TILE_EDGE);
    dab.center.x + radius > origin_x
        && dab.center.x - radius < origin_x + edge
        && dab.center.y + radius > origin_y
        && dab.center.y - radius < origin_y + edge
}

fn selection_intersects_tile(selection: &StrokeSelection, key: TileKey) -> bool {
    let [width, height] = selection.dimensions();
    let [selection_x, selection_y] = selection.origin().map(i64::from);
    let (origin_x, origin_y) = key.pixel_origin();
    let x0 = origin_x.max(selection_x);
    let y0 = origin_y.max(selection_y);
    let x1 = (origin_x + i64::from(TILE_EDGE)).min(selection_x + i64::from(width));
    let y1 = (origin_y + i64::from(TILE_EDGE)).min(selection_y + i64::from(height));
    (y0..y1).any(|y| {
        (x0..x1).any(|x| {
            selection.contains_signed(
                i32::try_from(x).expect("validated selection x"),
                i32::try_from(y).expect("validated selection y"),
            )
        })
    })
}

#[allow(clippy::cast_precision_loss)]
fn tile_local_dab(mut dab: BrushDab, origin_x: i64, origin_y: i64) -> BrushDab {
    // Tile origins derive from an i32 tile index times 128 and are therefore
    // well within f64's exact integer range.
    dab.center.x -= origin_x as f64;
    dab.center.y -= origin_y as f64;
    dab
}

/// Runs the selected materialization strategy without silently substituting a
/// different implementation.
///
/// # Errors
///
/// GPU readback and hybrid checkpoint return `UnsupportedStrategy` until a
/// measured implementation is supplied by their owning backend.
pub fn materialize_with_strategy(
    strategy: MaterializationStrategy,
    commit: &StrokeCommit,
    before: &TileSnapshot,
) -> Result<MaterializedStroke, MaterializationError> {
    match strategy {
        MaterializationStrategy::CpuReplay => CpuReplayMaterializer.materialize(commit, before),
        MaterializationStrategy::GpuReadback | MaterializationStrategy::HybridCheckpoint => {
            Err(MaterializationError::UnsupportedStrategy(strategy))
        }
    }
}

fn validate_closed_samples(samples: &[StylusSample]) -> Result<(), StrokeCommitError> {
    if samples.len() > MAX_SAMPLES_PER_STROKE {
        return Err(StrokeCommitError::TooManySamples);
    }
    let Some((first, remaining)) = samples.split_first() else {
        return Err(StrokeCommitError::EmptySampleStream);
    };
    if first.phase != PointerPhase::Begin
        || samples.last().map(|sample| sample.phase) != Some(PointerPhase::End)
        || remaining
            .iter()
            .take(remaining.len().saturating_sub(1))
            .any(|sample| sample.phase != PointerPhase::Move)
    {
        return Err(StrokeCommitError::InvalidPhaseTransition);
    }
    for pair in samples.windows(2) {
        if pair[1].sequence <= pair[0].sequence {
            return Err(StrokeCommitError::NonIncreasingSequence);
        }
        if pair[1].timestamp_ns < pair[0].timestamp_ns {
            return Err(StrokeCommitError::DecreasingTimestamp);
        }
        if pair[1].device_id != pair[0].device_id {
            return Err(StrokeCommitError::DeviceChanged);
        }
        if pair[1].eraser != pair[0].eraser {
            return Err(StrokeCommitError::EraserModeChanged);
        }
    }
    Ok(())
}

fn affected_tile_keys(
    layer: LayerId,
    dabs: &[BrushDab],
) -> Result<BTreeSet<TileKey>, StrokeCommitError> {
    let tile_edge = i32::try_from(TILE_EDGE).expect("tile edge fits i32");
    let mut keys = BTreeSet::new();
    for dab in dabs {
        let radius = f64::from(dab.radius_px);
        let alpha = dab.opacity * dab.flow;
        if !radius.is_finite() || !alpha.is_finite() {
            return Err(StrokeCommitError::CoordinateOutOfRange);
        }
        if radius <= 0.0 || alpha <= 0.0 {
            continue;
        }
        let left = floor_to_i32(dab.center.x - radius)?;
        let top = floor_to_i32(dab.center.y - radius)?;
        let right_exclusive = ceil_to_i32(dab.center.x + radius)?;
        let bottom_exclusive = ceil_to_i32(dab.center.y + radius)?;
        if right_exclusive <= left || bottom_exclusive <= top {
            continue;
        }
        let min_tile_x = left.div_euclid(tile_edge);
        let min_tile_y = top.div_euclid(tile_edge);
        let max_tile_x = right_exclusive.saturating_sub(1).div_euclid(tile_edge);
        let max_tile_y = bottom_exclusive.saturating_sub(1).div_euclid(tile_edge);
        let width = i64::from(max_tile_x) - i64::from(min_tile_x) + 1;
        let height = i64::from(max_tile_y) - i64::from(min_tile_y) + 1;
        let tile_count = u64::try_from(width)
            .ok()
            .and_then(|width| {
                u64::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(StrokeCommitError::TooManyAffectedTiles)?;
        if tile_count > MAX_AFFECTED_TILES_PER_STROKE {
            return Err(StrokeCommitError::TooManyAffectedTiles);
        }
        for y in min_tile_y..=max_tile_y {
            for x in min_tile_x..=max_tile_x {
                keys.insert(TileKey {
                    layer,
                    mip: 0,
                    x,
                    y,
                });
            }
        }
        if u64::try_from(keys.len()).unwrap_or(u64::MAX) > MAX_AFFECTED_TILES_PER_STROKE {
            return Err(StrokeCommitError::TooManyAffectedTiles);
        }
    }
    Ok(keys)
}

fn floor_to_i32(value: f64) -> Result<i32, StrokeCommitError> {
    bounded_f64_to_i32(value.floor())
}

fn ceil_to_i32(value: f64) -> Result<i32, StrokeCommitError> {
    bounded_f64_to_i32(value.ceil())
}

#[allow(clippy::cast_possible_truncation)]
fn bounded_f64_to_i32(value: f64) -> Result<i32, StrokeCommitError> {
    if !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX) {
        return Err(StrokeCommitError::CoordinateOutOfRange);
    }
    Ok(value as i32)
}

fn hash_commit(commit: &StrokeCommit) -> ObjectHash {
    let legacy = hash_legacy_commit(commit);
    if commit.alpha_locked() {
        ObjectHash::digest_tagged(b"nyatidraw-stroke-alpha-lock-v1", &legacy.0)
    } else {
        legacy
    }
}

fn hash_legacy_commit(commit: &StrokeCommit) -> ObjectHash {
    let mut payload = Vec::new();
    payload.extend_from_slice(&commit.parent_snapshot.0.to_le_bytes());
    payload.extend_from_slice(&commit.layer.0.to_le_bytes());
    payload.extend_from_slice(&commit.before_root.hash.0);
    payload.extend_from_slice(&commit.affected_tiles.min_x.to_le_bytes());
    payload.extend_from_slice(&commit.affected_tiles.min_y.to_le_bytes());
    payload.extend_from_slice(&commit.affected_tiles.max_x_exclusive.to_le_bytes());
    payload.extend_from_slice(&commit.affected_tiles.max_y_exclusive.to_le_bytes());
    let preset = commit.brush.preset;
    payload.extend_from_slice(&preset.id.0.to_le_bytes());
    payload.extend_from_slice(&preset.schema_version.to_le_bytes());
    payload.extend_from_slice(&preset.engine_version.to_le_bytes());
    payload.extend_from_slice(&preset.size_px.to_bits().to_le_bytes());
    payload.extend_from_slice(&preset.opacity.to_bits().to_le_bytes());
    payload.extend_from_slice(&preset.flow.to_bits().to_le_bytes());
    payload.extend_from_slice(&preset.spacing_ratio.to_bits().to_le_bytes());
    if preset.engine_version >= 2 {
        payload.extend_from_slice(&[
            u8::from(preset.size_pressure),
            u8::from(preset.opacity_pressure),
        ]);
        for value in [
            preset.size_min_ratio,
            preset.opacity_min_ratio,
            preset.hardness,
        ] {
            payload.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    payload.extend_from_slice(&commit.recorded.random_seed.to_le_bytes());
    payload.extend_from_slice(&commit.recorded.sample_count.to_le_bytes());
    payload.extend_from_slice(&commit.recorded.first_sample_sequence.to_le_bytes());
    payload.extend_from_slice(&commit.recorded.last_sample_sequence.to_le_bytes());
    payload.extend_from_slice(&commit.color.0);
    payload.extend_from_slice(
        &u64::try_from(commit.samples.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for sample in commit.samples() {
        encode_sample(&mut payload, sample);
    }
    if let Some(selection) = commit.selection() {
        let origin = selection.origin();
        if origin != [0, 0] {
            payload.extend_from_slice(&origin[0].to_le_bytes());
            payload.extend_from_slice(&origin[1].to_le_bytes());
        }
        let [width, height] = selection.dimensions();
        payload.extend_from_slice(&width.to_le_bytes());
        payload.extend_from_slice(&height.to_le_bytes());
        payload.extend_from_slice(&selection.packed_bits());
        ObjectHash::digest_tagged(
            if origin == [0, 0] {
                b"nyatidraw-stroke-commit-selected-v1"
            } else {
                b"nyatidraw-stroke-commit-selected-v2"
            },
            &payload,
        )
    } else {
        ObjectHash::digest_tagged(b"nyatidraw-stroke-commit-v1", &payload)
    }
}

fn encode_sample(output: &mut Vec<u8>, sample: &StylusSample) {
    output.extend_from_slice(&sample.sequence.to_le_bytes());
    output.extend_from_slice(&sample.timestamp_ns.to_le_bytes());
    output.extend_from_slice(&sample.device_id.to_le_bytes());
    output.push(match sample.phase {
        PointerPhase::Begin => 0,
        PointerPhase::Move => 1,
        PointerPhase::End => 2,
        PointerPhase::Cancel => 3,
    });
    output.extend_from_slice(&sample.position_document.x.to_bits().to_le_bytes());
    output.extend_from_slice(&sample.position_document.y.to_bits().to_le_bytes());
    output.extend_from_slice(&sample.pressure.to_bits().to_le_bytes());
    encode_optional_pair(output, sample.tilt);
    encode_optional_f32(output, sample.twist_radians);
    encode_optional_f32(output, sample.tangential_pressure);
    output.extend_from_slice(&sample.buttons.0.to_le_bytes());
    output.push(u8::from(sample.eraser));
    output.extend_from_slice(&sample.viewport_revision.to_le_bytes());
}

fn encode_optional_pair(output: &mut Vec<u8>, value: Option<[f32; 2]>) {
    match value {
        Some([x, y]) => {
            output.push(1);
            output.extend_from_slice(&x.to_bits().to_le_bytes());
            output.extend_from_slice(&y.to_bits().to_le_bytes());
        }
        None => output.push(0),
    }
}

fn encode_optional_f32(output: &mut Vec<u8>, value: Option<f32>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        None => output.push(0),
    }
}

#[cfg(test)]
mod tests {
    use nyatidraw_brush::{
        BrushEvaluator, BrushPreset, BrushPresetId, ROUND_BRUSH_ENGINE_VERSION,
        RoundBrushEvaluator, begin_round_stroke,
    };
    use nyatidraw_input::{PenButtons, Point};

    use super::*;

    fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64, pressure: f32) -> StylusSample {
        StylusSample {
            sequence,
            timestamp_ns: sequence * 1_000,
            device_id: 3,
            phase,
            position_document: Point { x, y },
            pressure,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: PenButtons::default(),
            eraser: false,
            viewport_revision: 7,
        }
    }

    fn sealed_commit(before: &TileSnapshot) -> StrokeCommit {
        let preset = BrushPreset {
            id: BrushPresetId(11),
            schema_version: nyatidraw_brush::ROUND_BRUSH_PRESET_SCHEMA_VERSION,
            engine_version: ROUND_BRUSH_ENGINE_VERSION,
            size_px: 20.0,
            opacity: 0.8,
            flow: 0.5,
            spacing_ratio: 0.25,
            size_pressure: true,
            opacity_pressure: false,
            size_min_ratio: 0.2,
            opacity_min_ratio: 0.1,
            hardness: 0.35,
        };
        let samples = vec![
            sample(1, PointerPhase::Begin, -4.0, 10.0, 0.4),
            sample(2, PointerPhase::Move, 20.0, 20.0, 0.7),
            sample(3, PointerPhase::End, 140.0, 40.0, 1.0),
        ];
        let mut evaluator = RoundBrushEvaluator::new(99);
        let mut dabs = Vec::new();
        let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
        evaluator.push(&mut token, &samples[1..], &mut dabs);
        let recorded = evaluator.end(token, &mut dabs);
        StrokeCommit::seal(
            SnapshotId(1),
            LayerId(8),
            before,
            BrushSnapshot { preset },
            recorded,
            StrokeColor::new([24, 12, 6, 32]).expect("premultiplied"),
            samples,
        )
        .expect("valid fixture")
    }

    #[test]
    fn deterministic_materialization_prevents_closed_stroke_pixel_drift() {
        let before = TileSnapshot::empty();
        let commit = sealed_commit(&before);
        let left = materialize_with_strategy(MaterializationStrategy::CpuReplay, &commit, &before)
            .expect("CPU replay");
        let right = materialize_with_strategy(MaterializationStrategy::CpuReplay, &commit, &before)
            .expect("CPU replay");

        assert_eq!(
            left.after, right.after,
            "same seal must produce exact bytes"
        );
        assert_eq!(
            left.after.root(),
            right.after.root(),
            "same seal must produce the same immutable root"
        );
        assert!(!left.changed_tiles.is_empty());
        assert!(matches!(
            materialize_with_strategy(MaterializationStrategy::GpuReadback, &commit, &before),
            Err(MaterializationError::UnsupportedStrategy(
                MaterializationStrategy::GpuReadback
            ))
        ));
    }

    #[test]
    fn alpha_locked_replay_preserves_exact_alpha_and_legacy_identity() {
        // Product risk: a locked stroke changes silhouettes, replays differently
        // after layer settings change, or silently shares an unlocked hash.
        let tiles = [(8, -1), (8, 0), (8, 1), (9, 0)].map(|(layer, x)| {
            let pixels = (0..TILE_BYTE_LEN / 4)
                .flat_map(|index| {
                    let alpha = [0, 1, 128, 255][index % 4];
                    [0, alpha / 2, 0, alpha]
                })
                .collect::<Vec<_>>();
            (
                TileKey {
                    layer: LayerId(layer),
                    mip: 0,
                    x,
                    y: 0,
                },
                pixels,
            )
        });
        let before = TileSnapshot::from_tiles(tiles).unwrap();
        let template = sealed_commit(&before);
        for origin in [None, Some([0, 0]), Some([-8, 0])] {
            let selection = origin.map(|origin| {
                Arc::new(
                    StrokeSelection::from_packed_bits_at(
                        origin,
                        [144, 64],
                        &vec![255; 144 * 64 / 8],
                    )
                    .unwrap(),
                )
            });
            let seal = |alpha_locked, eraser| {
                let mut samples = template.samples().to_vec();
                for sample in &mut samples {
                    sample.eraser = eraser;
                }
                StrokeCommit::seal_with_options(
                    template.parent_snapshot,
                    template.layer,
                    &before,
                    template.brush,
                    template.recorded.clone(),
                    template.color,
                    samples,
                    selection.clone(),
                    alpha_locked,
                )
            };
            let unlocked = seal(false, false).unwrap();
            let legacy = StrokeCommit::seal_with_selection(
                template.parent_snapshot,
                template.layer,
                &before,
                template.brush,
                template.recorded.clone(),
                template.color,
                template.samples().to_vec(),
                selection.clone(),
            )
            .unwrap();
            assert_eq!(unlocked.id, legacy.id);
            assert_eq!(unlocked.id.0, hash_legacy_commit(&unlocked));
            let locked = seal(true, false).unwrap();
            assert_ne!(locked.id, unlocked.id);
            assert_eq!(seal(true, true), Err(StrokeCommitError::AlphaLockedEraser));
            let first = CpuReplayMaterializer.materialize(&locked, &before).unwrap();
            let second = CpuReplayMaterializer.materialize(&locked, &before).unwrap();
            assert_eq!(first, second);
            assert!(
                !first.changed_tiles.is_empty(),
                "fixture must actually recolor existing artwork"
            );
            for (key, original) in before.iter() {
                let actual = first.after.get(key).unwrap();
                let (origin_x, origin_y) = key.pixel_origin();
                for (index, (before, after)) in original
                    .pixels()
                    .chunks_exact(4)
                    .zip(actual.pixels().chunks_exact(4))
                    .enumerate()
                {
                    assert_eq!(
                        before[3], after[3],
                        "alpha byte must remain exact across all dabs"
                    );
                    assert!(after[..3].iter().all(|channel| *channel <= after[3]));
                    let x = i32::try_from(origin_x + i64::try_from(index % 128).unwrap()).unwrap();
                    let y = i32::try_from(origin_y + i64::try_from(index / 128).unwrap()).unwrap();
                    if key.layer != template.layer
                        || selection
                            .as_ref()
                            .is_some_and(|mask| !mask.contains_signed(x, y))
                    {
                        assert_eq!(
                            before, after,
                            "unselected and other-layer pixels must not change"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn selected_brush_and_eraser_preserve_unselected_signed_and_padding_pixels() {
        let key = |layer, x| TileKey {
            layer: LayerId(layer),
            mip: 0,
            x,
            y: 0,
        };
        let before = TileSnapshot::from_tiles(
            [key(8, -1), key(8, 0), key(8, 1), key(9, 0)]
                .map(|key| (key, [80, 60, 40, 160].repeat(TILE_BYTE_LEN / 4))),
        )
        .unwrap();
        let template = sealed_commit(&before);
        for eraser in [false, true] {
            for (selection_origin, empty) in [([0, 0], false), ([-8, -4], false), ([-8, -4], true)]
            {
                let mut packed = vec![0; (129_usize * 64).div_ceil(8)];
                if !empty {
                    for y in 0..64 {
                        for x in [0, 1, 2, 3, 128] {
                            let index = y * 129 + x;
                            packed[index / 8] |= 1 << (index % 8);
                        }
                    }
                }
                let selection = Arc::new(
                    StrokeSelection::from_packed_bits_at(selection_origin, [129, 64], &packed)
                        .unwrap(),
                );
                let mut samples = template.samples().to_vec();
                for sample in &mut samples {
                    sample.eraser = eraser;
                }
                let unrestricted = StrokeCommit::seal(
                    template.parent_snapshot,
                    template.layer,
                    &before,
                    template.brush,
                    template.recorded.clone(),
                    template.color,
                    samples.clone(),
                )
                .unwrap();
                let selected = StrokeCommit::seal_with_selection(
                    template.parent_snapshot,
                    template.layer,
                    &before,
                    template.brush,
                    template.recorded.clone(),
                    template.color,
                    samples,
                    Some(selection),
                )
                .unwrap();
                assert_ne!(
                    selected.id, unrestricted.id,
                    "coverage is part of replay identity"
                );
                let all = CpuReplayMaterializer
                    .materialize(&unrestricted, &before)
                    .unwrap();
                let clipped = CpuReplayMaterializer
                    .materialize(&selected, &before)
                    .unwrap();
                for (key, tile) in before.iter() {
                    let actual = clipped.after.get(key).unwrap();
                    let unrestricted = all.after.get(key).unwrap();
                    let (origin_x, origin_y) = key.pixel_origin();
                    for y in 0..128_usize {
                        for x in 0..128_usize {
                            let document_x = origin_x + i64::try_from(x).unwrap();
                            let document_y = origin_y + i64::try_from(y).unwrap();
                            let selection_x = document_x - i64::from(selection_origin[0]);
                            let selection_y = document_y - i64::from(selection_origin[1]);
                            let selected = !empty
                                && key.layer == LayerId(8)
                                && (0..64).contains(&selection_y)
                                && ((0..4).contains(&selection_x) || selection_x == 128);
                            let offset = (y * 128 + x) * 4;
                            let expected = if selected { unrestricted } else { tile };
                            assert_eq!(
                                &actual.pixels()[offset..offset + 4],
                                &expected.pixels()[offset..offset + 4],
                                "eraser={eraser} empty={empty} key={key:?} x={x} y={y}"
                            );
                        }
                    }
                }
                if empty {
                    assert_eq!(clipped.after, before);
                    assert!(clipped.changed_tiles.is_empty());
                } else {
                    assert_ne!(
                        clipped.after.root(),
                        before.root(),
                        "selected area was actually painted"
                    );
                }
            }
        }
    }
}

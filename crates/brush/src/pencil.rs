//! `NyatiDraw`'s bounded upright dry-pencil model, not a physical graphite solver.
//! No external brush implementation or image assets are used.
use crate::{BrushDab, BrushPreset, BrushPresetId, pressure_factor, unit};
use nyatidraw_input::{Point, StylusSample};

pub const PENCIL_ENGINE_VERSION: u32 = 3;
pub const PENCIL_PRESET_SCHEMA_VERSION: u32 = 3;
pub const PENCIL_GRAIN_VERSION: u8 = 1;
/// One immutable procedural paper for v3. It does not restart for each stroke.
pub const PENCIL_PAPER_SEED: u32 = 0x4e59_4154;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PencilKind {
    Mechanical2H,
    Graphite2B,
}

impl PencilKind {
    #[must_use]
    pub const fn preset_id(self) -> BrushPresetId {
        BrushPresetId(match self {
            Self::Mechanical2H => 0x4e59_4154_4950_454e_4349_4c00_0000_0001,
            Self::Graphite2B => 0x4e59_4154_4950_454e_4349_4c00_0000_0002,
        })
    }

    #[must_use]
    pub const fn from_preset_id(id: BrushPresetId) -> Option<Self> {
        if id.0 == Self::Mechanical2H.preset_id().0 {
            Some(Self::Mechanical2H)
        } else if id.0 == Self::Graphite2B.preset_id().0 {
            Some(Self::Graphite2B)
        } else {
            None
        }
    }
}

/// The two supported immutable material identities; UI controls may override
/// size/opacity and pressure axes, but identity never means round-tip hardness.
#[must_use]
pub const fn pencil_preset(kind: PencilKind) -> BrushPreset {
    let (size_px, flow, spacing_ratio, size_min_ratio) = match kind {
        PencilKind::Mechanical2H => (2.0, 0.32, 0.15, 0.82),
        PencilKind::Graphite2B => (5.0, 0.65, 0.12, 0.35),
    };
    BrushPreset {
        id: kind.preset_id(),
        schema_version: PENCIL_PRESET_SCHEMA_VERSION,
        engine_version: PENCIL_ENGINE_VERSION,
        size_px,
        opacity: 1.0,
        flow,
        spacing_ratio,
        size_pressure: true,
        opacity_pressure: true,
        size_min_ratio,
        opacity_min_ratio: 0.05,
        hardness: 1.0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PencilGrain {
    /// Pressure-mapped graphite reaching paper valleys, in integer [0,255].
    pub deposit: u8,
    /// Raster surface origin modulo 2^32 document pixels. Never a view origin.
    pub origin: [u32; 2],
}

pub(super) fn valid_preset(preset: &BrushPreset) -> bool {
    preset.schema_version == PENCIL_PRESET_SCHEMA_VERSION
        && PencilKind::from_preset_id(preset.id).is_some()
        && preset.size_px.is_finite()
        && (0.1..=200.0).contains(&preset.size_px)
        && preset.spacing_ratio.is_finite()
        && (0.01..=4.0).contains(&preset.spacing_ratio)
        && [
            preset.opacity,
            preset.flow,
            preset.size_min_ratio,
            preset.opacity_min_ratio,
        ]
        .into_iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(&value))
        && preset.hardness.to_bits() == 1.0_f32.to_bits()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn dab_for(preset: &BrushPreset, sample: StylusSample) -> BrushDab {
    let pressure = unit(sample.pressure);
    let kind = PencilKind::from_preset_id(preset.id).unwrap_or(PencilKind::Mechanical2H);
    let size = pressure_factor(preset.size_pressure, preset.size_min_ratio, pressure);
    let deposit_pressure =
        pressure_factor(preset.opacity_pressure, preset.opacity_min_ratio, pressure);
    let (minimum, range) = match kind {
        PencilKind::Mechanical2H => (80.0, 128.0),
        PencilKind::Graphite2B => (128.0, 127.0),
    };
    // Upright fallback is intentional for ALL v3 inputs. Raw tilt is currently
    // screen-space degrees, not a document-space tip direction; do not invent it.
    // A 1/16 px grid makes circle membership identical in CPU and WGSL integers.
    let quantize = |value: f64| (value * 16.0).round() / 16.0;
    BrushDab {
        center: Point {
            x: quantize(sample.position_document.x),
            y: quantize(sample.position_document.y),
        },
        radius_px: ((preset.size_px * size * 0.5 * 16.0).round() / 16.0).max(0.0),
        opacity: unit(preset.opacity) * deposit_pressure,
        flow: unit(preset.flow),
        hardness: 1.0,
        grain: Some(PencilGrain {
            deposit: (minimum + range * deposit_pressure).round() as u8,
            origin: [0, 0],
        }),
    }
}

/// Paper hash uses only wrapping integer operations shared verbatim as a
/// mathematical contract with WGSL. Tile signs and draw batching cannot reseed it.
#[must_use]
pub fn paper_tooth(x: u32, y: u32) -> u32 {
    let mut value = x.wrapping_mul(0x9e37_79b9) ^ y.wrapping_mul(0x85eb_ca6b) ^ PENCIL_PAPER_SEED;
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    value >> 24
}

/// Exact integer numerator for pencil coverage, denominator 16 * 255 = 4080.
/// Call with raster-local pixels and dabs translated using `BrushDab::to_local`.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn pencil_coverage_units(dab: BrushDab, x: u32, y: u32) -> u32 {
    let Some(grain) = dab.grain else {
        return 0;
    };
    let tooth = paper_tooth(
        x.wrapping_add(grain.origin[0]),
        y.wrapping_add(grain.origin[1]),
    );
    let deposit = u32::from(grain.deposit).saturating_sub(tooth);
    if deposit == 0 {
        return 0;
    }
    let cx = (dab.center.x * 16.0).round() as i64;
    let cy = (dab.center.y * 16.0).round() as i64;
    let radius = (f64::from(dab.radius_px) * 16.0).round() as i64;
    let mut count = 0;
    for dy in [2_i64, 6, 10, 14] {
        for dx in [2_i64, 6, 10, 14] {
            let dx = i64::from(x) * 16 + dx - cx;
            let dy = i64::from(y) * 16 + dy - cy;
            if dx * dx + dy * dy <= radius * radius {
                count += 1;
            }
        }
    }
    count * deposit
}

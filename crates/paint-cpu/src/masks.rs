use std::collections::{BTreeMap, BTreeSet};

use nyatidraw_api::MaskId;
use nyatidraw_brush::BrushDab;
use nyatidraw_document::LayerMask;
use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey, TILE_EDGE, TileSnapshot};

use crate::{CpuCanvas, EditError, EditLimits, PremultipliedRgba8, SelectionMask};

#[must_use]
#[allow(
    clippy::missing_panics_doc,
    reason = "byte products are bounded by 255 squared"
)]
pub fn apply_mask_pixel(
    snapshot: &TileSnapshot,
    mask: Option<LayerMask>,
    point: [i32; 2],
    pixel: [u8; 4],
) -> [u8; 4] {
    let Some(mask) = mask.filter(|mask| mask.enabled) else {
        return pixel;
    };
    let coverage = u32::from(snapshot.coverage_at(mask.id, mask.default_coverage, point));
    pixel.map(|channel| {
        u8::try_from((u32::from(channel) * coverage + 127) / 255).expect("coverage is bounded")
    })
}

/// Creates a default-hidden mask without modifying the image plane.
/// # Errors
/// Rejects unbounded selection extents before allocating coverage tiles.
#[allow(
    clippy::missing_panics_doc,
    reason = "signed remainders are bounded tile offsets and entries are inserted first"
)]
pub fn mask_from_selection(
    snapshot: &TileSnapshot,
    mask: MaskId,
    selection: &SelectionMask,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    let limits = limits.bounded();
    let (origin, size) = selection.bounds_signed().ok_or(EditError::InvalidMask)?;
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    if pixels > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    let mut tiles = BTreeMap::new();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let point = [
                i64::from(origin[0]) + i64::from(x),
                i64::from(origin[1]) + i64::from(y),
            ];
            let point = [
                i32::try_from(point[0]).map_err(|_| EditError::CoordinateOutOfRange)?,
                i32::try_from(point[1]).map_err(|_| EditError::CoordinateOutOfRange)?,
            ];
            let coverage = selection.coverage_signed(point[0], point[1]);
            if coverage == 0 {
                continue;
            }
            let key = CoverageTileKey::from_pixel(mask, 128, point[0], point[1]);
            if !tiles.contains_key(&key) {
                check_workspace(tiles.len() + 1, snapshot.total_len(), limits)?;
                tiles.insert(key, vec![0; COVERAGE_TILE_BYTE_LEN]);
            }
            let index = usize::try_from(point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128))
                .expect("tile offset");
            tiles.get_mut(&key).expect("inserted")[index] = coverage;
        }
    }
    snapshot
        .with_coverage_replacements(tiles)
        .map_err(EditError::Tiles)
}

pub(super) fn check_workspace(
    changed: usize,
    resident: usize,
    limits: EditLimits,
) -> Result<(), EditError> {
    let bytes = (changed as u64)
        .saturating_mul((COVERAGE_TILE_BYTE_LEN * 2 + 256) as u64)
        .saturating_add((resident as u64).saturating_mul(256))
        .saturating_add((COVERAGE_TILE_BYTE_LEN * 4) as u64);
    if bytes > limits.max_workspace_bytes {
        Err(EditError::LimitExceeded)
    } else {
        Ok(())
    }
}

/// Cumulative mask-stroke admission matching the closed replay work limit.
#[derive(Clone, Copy, Debug)]
pub struct MaskStrokeBudget {
    used: u64,
    maximum: u64,
}

impl Default for MaskStrokeBudget {
    fn default() -> Self {
        Self::new(EditLimits::default())
    }
}

impl MaskStrokeBudget {
    #[must_use]
    pub fn new(limits: EditLimits) -> Self {
        Self {
            used: 0,
            maximum: limits.bounded().max_filter_work,
        }
    }

    /// Checks a batch before accepting any of its dabs. Rejection leaves the budget unchanged.
    /// # Errors
    /// Rejects invalid coordinates or cumulative work exceeding closed replay admission.
    pub fn admit(&mut self, dabs: &[BrushDab]) -> Result<(), EditError> {
        let mut next = self.used;
        for dab in dabs {
            next = next
                .checked_add(mask_dab_bounds(dab)?.work)
                .ok_or(EditError::LimitExceeded)?;
            if next > self.maximum {
                return Err(EditError::LimitExceeded);
            }
        }
        self.used = next;
        Ok(())
    }
}

struct MaskDabBounds {
    first: [i32; 2],
    last: [i32; 2],
    work: u64,
}

fn mask_dab_bounds(dab: &BrushDab) -> Result<MaskDabBounds, EditError> {
    let radius = f64::from(dab.radius_px);
    let min = [dab.center.x - radius, dab.center.y - radius];
    let max = [dab.center.x + radius, dab.center.y + radius];
    if min.into_iter().chain(max).any(|value| {
        !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX)
    }) {
        return Err(EditError::CoordinateOutOfRange);
    }
    #[allow(clippy::cast_possible_truncation)]
    let first = min.map(|value| (value.floor() as i32).div_euclid(128));
    #[allow(clippy::cast_possible_truncation)]
    let last = max.map(|value| (value.ceil() as i32).div_euclid(128));
    let count = u64::try_from(i64::from(last[0]) - i64::from(first[0]) + 1)
        .map_err(|_| EditError::LimitExceeded)?
        .checked_mul(
            u64::try_from(i64::from(last[1]) - i64::from(first[1]) + 1)
                .map_err(|_| EditError::LimitExceeded)?,
        )
        .ok_or(EditError::LimitExceeded)?;
    let work = count
        .checked_mul(u64::from(TILE_EDGE).pow(2))
        .ok_or(EditError::LimitExceeded)?;
    Ok(MaskDabBounds { first, last, work })
}

/// Uses the same deterministic dab rasterizer for preview and durable coverage.
/// # Errors
/// Rejects signed-coordinate, raster-work and allocation limits before mutation.
#[allow(
    clippy::missing_panics_doc,
    reason = "weighted byte and inserted tile invariants are bounded"
)]
pub fn paint_mask_dabs(
    snapshot: &TileSnapshot,
    mask: LayerMask,
    dabs: &[BrushDab],
    color: PremultipliedRgba8,
    erase: bool,
    selection: Option<&SelectionMask>,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    let limits = limits.bounded();
    let mut tiles = BTreeMap::<CoverageTileKey, Vec<u8>>::new();
    MaskStrokeBudget::new(limits).admit(dabs)?;
    let gray = if erase {
        PremultipliedRgba8([255; 4])
    } else {
        mask_gray(color)
    };
    for dab in dabs {
        let bounds = mask_dab_bounds(dab)?;
        for y in bounds.first[1]..=bounds.last[1] {
            for x in bounds.first[0]..=bounds.last[0] {
                let key = CoverageTileKey {
                    mask: mask.id,
                    mip: 0,
                    x,
                    y,
                };
                if !tiles.contains_key(&key) {
                    check_workspace(tiles.len() + 1, snapshot.total_len(), limits)?;
                    tiles.insert(
                        key,
                        snapshot.get_coverage(key).map_or_else(
                            || vec![mask.default_coverage; COVERAGE_TILE_BYTE_LEN],
                            |tile| tile.pixels().to_vec(),
                        ),
                    );
                }
                paint_coverage_tile(
                    tiles.get_mut(&key).expect("inserted"),
                    key,
                    *dab,
                    gray,
                    selection,
                )?;
            }
        }
    }
    tiles.retain(|key, pixels| {
        snapshot.get_coverage(*key).map_or_else(
            || pixels.iter().any(|value| *value != mask.default_coverage),
            |tile| tile.pixels() != pixels,
        )
    });
    snapshot
        .with_coverage_replacements(tiles)
        .map_err(EditError::Tiles)
}

pub(super) fn mask_gray(color: PremultipliedRgba8) -> PremultipliedRgba8 {
    let [r, g, b, a] =
        nyatidraw_tiles::color::linear_premultiplied_to_srgb8(color.0).unwrap_or([0; 4]);
    let gray = (u32::from(r) * 2126 + u32::from(g) * 7152 + u32::from(b) * 722 + 5000) / 10000;
    let premultiplied = u8::try_from((gray * u32::from(a) + 127) / 255).expect("weighted byte");
    PremultipliedRgba8([premultiplied, premultiplied, premultiplied, a])
}

/// Keeps coverage owned by retained layers when rebuilding image-only objects.
/// # Errors
/// Reserved for typed reconstruction errors; removing existing planes is infallible.
pub fn retain_owned_masks(
    snapshot: &TileSnapshot,
    tree: &nyatidraw_document::LayerTree,
) -> Result<TileSnapshot, EditError> {
    let ids: BTreeSet<_> = tree.mask_ids().into_iter().collect();
    Ok(snapshot.without_masks(
        snapshot
            .coverage_iter()
            .filter(|(key, _)| !ids.contains(&key.mask))
            .map(|(key, _)| key.mask),
    ))
}

fn paint_coverage_tile(
    original: &mut [u8],
    key: CoverageTileKey,
    dab: BrushDab,
    gray: PremultipliedRgba8,
    selection: Option<&SelectionMask>,
) -> Result<(), EditError> {
    let rgba: Vec<_> = original
        .iter()
        .flat_map(|&value| [value, value, value, 255])
        .collect();
    let mut canvas = CpuCanvas::from_rgba8_premultiplied(TILE_EDGE, TILE_EDGE, rgba)
        .map_err(|_| EditError::InvalidPaint)?;
    let (left, top) = key.pixel_origin();
    let local = dab.to_local(left, top);
    if let Some(selection) = selection {
        canvas.apply_dab_with_coverage(local, crate::DabOperation::Paint(gray), |x, y| {
            match [
                i32::try_from(left + i64::from(x)),
                i32::try_from(top + i64::from(y)),
            ] {
                [Ok(x), Ok(y)] => selection.coverage_signed(x, y),
                _ => 0,
            }
        });
    } else {
        canvas.apply_dab(local, gray);
    }
    for (value, pixel) in original
        .iter_mut()
        .zip(canvas.pixels_rgba8_premultiplied().chunks_exact(4))
    {
        *value = pixel[0];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_mask_batches_fit_replay_and_rejection_is_atomic() {
        for center in [[32.0, 32.0], [-128.0, -128.0], [128.0, 32.0]] {
            let dab = BrushDab {
                center: nyatidraw_input::Point {
                    x: center[0],
                    y: center[1],
                },
                radius_px: 1.0,
                opacity: 1.0,
                flow: 1.0,
                hardness: 1.0,
                grain: None,
            };
            let work = mask_dab_bounds(&dab).unwrap().work;
            for maximum in [work * 2 - 1, work * 2, work * 2 + 1] {
                let limits = EditLimits {
                    max_filter_work: maximum,
                    ..Default::default()
                };
                let mut split = MaskStrokeBudget::new(limits);
                split.admit(&[dab]).unwrap();
                let before = split.used;
                let second = split.admit(&[dab]);
                let combined = MaskStrokeBudget::new(limits).admit(&[dab, dab]);
                assert_eq!(second.is_ok(), combined.is_ok());
                if second.is_err() {
                    assert_eq!(split.used, before);
                }
                let mask = LayerMask {
                    id: MaskId(1),
                    enabled: true,
                    default_coverage: 255,
                };
                assert_eq!(
                    paint_mask_dabs(
                        &TileSnapshot::default(),
                        mask,
                        &[dab, dab],
                        PremultipliedRgba8([0, 0, 0, 255]),
                        false,
                        None,
                        limits
                    )
                    .is_ok(),
                    combined.is_ok()
                );
                let used = split.used;
                assert!(split.admit(&[dab, dab]).is_err());
                assert_eq!(
                    split.used, used,
                    "a rejected batch must not partially consume admission"
                );
            }
        }
    }

    #[test]
    fn displayed_mask_gray_preserves_coverage_opacity_and_original_pixels() {
        let source = TileSnapshot::from_tiles([(
            nyatidraw_tiles::TileKey::from_pixel(nyatidraw_api::LayerId(1), 128, 0, 0),
            vec![255; nyatidraw_tiles::TILE_BYTE_LEN],
        )])
        .unwrap();
        for default_coverage in [0, 255] {
            let mask = LayerMask {
                id: MaskId(2),
                enabled: true,
                default_coverage,
            };
            for gray in [0, 128, 255] {
                for alpha in [0, 64, 128, 255] {
                    for opacity in [0.5_f32, 1.0] {
                        let color = PremultipliedRgba8(
                            nyatidraw_tiles::color::srgb8_to_linear_premultiplied([
                                gray, gray, gray, alpha,
                            ]),
                        );
                        let dab = BrushDab {
                            center: nyatidraw_input::Point { x: 1.5, y: 1.5 },
                            radius_px: 1.0,
                            opacity,
                            flow: 1.0,
                            hardness: 1.0,
                            grain: None,
                        };
                        let after = paint_mask_dabs(
                            &source,
                            mask,
                            &[dab],
                            color,
                            false,
                            None,
                            EditLimits::default(),
                        )
                        .unwrap();
                        let strength = f64::from(alpha) / 255.0 * f64::from(opacity);
                        let expected = (f64::from(gray) * strength
                            + f64::from(default_coverage) * (1.0 - strength))
                            .round();
                        let actual = after.coverage_at(mask.id, default_coverage, [1, 1]);
                        assert!(
                            (f64::from(actual) - expected).abs() <= 1.0,
                            "gray={gray} alpha={alpha} opacity={opacity} default={default_coverage}: {actual} vs {expected}"
                        );
                        assert_eq!(
                            after.iter().collect::<Vec<_>>(),
                            source.iter().collect::<Vec<_>>()
                        );
                        let revealed = paint_mask_dabs(
                            &source,
                            mask,
                            &[BrushDab {
                                opacity: 1.0,
                                ..dab
                            }],
                            color,
                            true,
                            None,
                            EditLimits::default(),
                        )
                        .unwrap();
                        assert_eq!(revealed.coverage_at(mask.id, default_coverage, [1, 1]), 255);
                    }
                }
            }
        }
    }
}

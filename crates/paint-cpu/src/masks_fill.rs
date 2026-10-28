use std::collections::BTreeMap;

use nyatidraw_document::LayerMask;
use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey, TileSnapshot};

use crate::{EditError, EditLimits, PremultipliedRgba8, SelectionMask};

/// Fills finite selected mask coverage without changing source image pixels.
/// # Errors
/// Rejects invalid paint, signed bounds and workspace limits before publication.
pub fn fill_mask_selection(
    snapshot: &TileSnapshot,
    mask: LayerMask,
    selection: &SelectionMask,
    color: PremultipliedRgba8,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    if color.0[..3].iter().any(|channel| *channel > color.0[3]) {
        return Err(EditError::InvalidPaint);
    }
    let Some((origin, size)) = selection.bounds_signed() else {
        return Ok(snapshot.clone());
    };
    let limits = limits.bounded();
    let pixels = u64::from(size[0]) * u64::from(size[1]);
    if pixels > limits.max_pixels || pixels > limits.max_filter_work {
        return Err(EditError::LimitExceeded);
    }
    let gray = super::masks::mask_gray(color);
    let mut tiles = BTreeMap::<CoverageTileKey, Vec<u8>>::new();
    for y in 0..size[1] {
        for x in 0..size[0] {
            let point = [
                i32::try_from(i64::from(origin[0]) + i64::from(x))
                    .map_err(|_| EditError::CoordinateOutOfRange)?,
                i32::try_from(i64::from(origin[1]) + i64::from(y))
                    .map_err(|_| EditError::CoordinateOutOfRange)?,
            ];
            let coverage = selection.coverage_signed(point[0], point[1]);
            if coverage == 0 || gray.0[3] == 0 {
                continue;
            }
            let original = snapshot.coverage_at(mask.id, mask.default_coverage, point);
            let mut filled = [original, original, original, 255];
            crate::composite_surface(&mut filled, &gray.0, u16::from(coverage) * 257);
            if filled[0] == original {
                continue;
            }
            let key = CoverageTileKey::from_pixel(mask.id, 128, point[0], point[1]);
            if !tiles.contains_key(&key) {
                super::masks::check_workspace(tiles.len() + 1, snapshot.total_len(), limits)?;
            }
            let values = tiles.entry(key).or_insert_with(|| {
                snapshot.get_coverage(key).map_or_else(
                    || vec![mask.default_coverage; COVERAGE_TILE_BYTE_LEN],
                    |tile| tile.pixels().to_vec(),
                )
            });
            let index = usize::try_from(point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128))
                .map_err(|_| EditError::CoordinateOutOfRange)?;
            values[index] = filled[0];
        }
    }
    snapshot
        .with_coverage_replacements(tiles)
        .map_err(EditError::Tiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{LayerId, MaskId};
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    #[test]
    fn scalar_fill_preserves_signed_unselected_pixels_and_other_artwork_planes() {
        let source = TileSnapshot::from_tiles([(
            TileKey::from_pixel(LayerId(1), 128, -1, -1),
            vec![255; TILE_BYTE_LEN],
        )])
        .unwrap()
        .with_coverage_replacements([(
            CoverageTileKey::from_pixel(MaskId(3), 128, -1, -1),
            vec![93; COVERAGE_TILE_BYTE_LEN],
        )])
        .unwrap();
        let selection =
            SelectionMask::from_coverage_at([-1, -1], [4, 1], &[0, 64, 128, 255]).unwrap();
        for default_coverage in [0, 255] {
            let mask = LayerMask {
                id: MaskId(2),
                enabled: true,
                default_coverage,
            };
            for gray in [0, 128, 255] {
                for alpha in [0, 128, 255] {
                    let color =
                        PremultipliedRgba8(nyatidraw_tiles::color::srgb8_to_linear_premultiplied(
                            [gray, gray, gray, alpha],
                        ));
                    let after = fill_mask_selection(
                        &source,
                        mask,
                        &selection,
                        color,
                        EditLimits::default(),
                    )
                    .unwrap();
                    for (x, selected) in [(-1, 0), (0, 64), (1, 128), (2, 255)] {
                        let strength = f64::from(alpha) * f64::from(selected) / (255.0 * 255.0);
                        let expected = (f64::from(gray) * strength
                            + f64::from(default_coverage) * (1.0 - strength))
                            .round();
                        let actual = after.coverage_at(mask.id, default_coverage, [x, -1]);
                        assert!(
                            (f64::from(actual) - expected).abs() <= 2.0,
                            "gray={gray} alpha={alpha} selection={selected} default={default_coverage}: {actual} vs {expected}"
                        );
                    }
                    for point in [[-2, -1], [3, -1], [0, 0]] {
                        assert_eq!(
                            after.coverage_at(mask.id, default_coverage, point),
                            default_coverage
                        );
                    }
                    assert_eq!(
                        after.iter().collect::<Vec<_>>(),
                        source.iter().collect::<Vec<_>>()
                    );
                    assert_eq!(after.coverage_at(MaskId(3), 0, [-1, -1]), 93);
                }
            }
            for limits in [
                EditLimits {
                    max_pixels: 2,
                    ..Default::default()
                },
                EditLimits {
                    max_workspace_bytes: 1,
                    ..Default::default()
                },
            ] {
                let opposite = 255 - default_coverage;
                assert!(
                    fill_mask_selection(
                        &source,
                        mask,
                        &selection,
                        PremultipliedRgba8([opposite, opposite, opposite, 255]),
                        limits
                    )
                    .is_err()
                );
            }
        }
        assert_eq!(source.coverage_len(), 1);
    }
}

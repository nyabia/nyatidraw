use super::{EditError, EditLimits, SelectionMask, region_len, require_workspace};

/// Three separable box passes approximate a Gaussian with sigma near `radius`.
/// Outside the original selection is zero; signed padding preserves the full tail.
/// # Errors
/// Rejects radius > 64, coordinate overflow, and aggregate work/memory limits.
pub fn feather_selection(
    mask: &SelectionMask,
    radius: u8,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    if radius > 64 {
        return Err(EditError::InvalidMask);
    }
    let input_len = region_len(mask.origin(), mask.dimensions(), limits)?;
    if radius == 0 || mask.selected_pixels() == 0 {
        require_workspace(input_len as u64 * 2, limits)?;
        return Ok(mask.clone());
    }
    let pad = u32::from(radius) * 3;
    let mut origin = mask.origin();
    let mut size = mask.dimensions();
    for axis in 0..2 {
        origin[axis] = i32::try_from(i64::from(origin[axis]) - i64::from(pad))
            .map_err(|_| EditError::CoordinateOutOfRange)?;
        size[axis] = size[axis]
            .checked_add(pad * 2)
            .ok_or(EditError::LimitExceeded)?;
    }
    let len = region_len(origin, size, limits)?;
    require_workspace(input_len as u64 + len as u64 * 2, limits)?;
    if len as u64 * 6 > limits.max_filter_work {
        return Err(EditError::LimitExceeded);
    }
    let width = size[0] as usize;
    let height = size[1] as usize;
    let mut selected = vec![0; len];
    let pad = pad as usize;
    for (y, row) in mask
        .coverage_bytes()
        .chunks_exact(mask.width as usize)
        .enumerate()
    {
        let start = (y + pad) * width + pad;
        selected[start..start + row.len()].copy_from_slice(row);
    }
    let mut scratch = vec![0; len];
    for _ in 0..3 {
        box_pass(
            &selected,
            &mut scratch,
            width,
            height,
            usize::from(radius),
            false,
        );
        box_pass(
            &scratch,
            &mut selected,
            width,
            height,
            usize::from(radius),
            true,
        );
    }
    let count = selected.iter().filter(|&&value| value != 0).count() as u64;
    Ok(SelectionMask {
        origin,
        width: size[0],
        height: size[1],
        selected,
        count,
    })
}

fn box_pass(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    radius: usize,
    vertical: bool,
) {
    let (lines, length, stride) = if vertical {
        (width, height, width)
    } else {
        (height, width, 1)
    };
    let divisor = u32::try_from(radius * 2 + 1).expect("bounded feather radius");
    for line in 0..lines {
        let start = if vertical { line } else { line * width };
        let mut sum: u32 = (0..=radius.min(length - 1))
            .map(|position| u32::from(input[start + position * stride]))
            .sum();
        for position in 0..length {
            output[start + position * stride] =
                u8::try_from((sum + divisor / 2) / divisor).expect("average of byte coverage");
            if position >= radius {
                sum -= u32::from(input[start + (position - radius) * stride]);
            }
            if position + radius + 1 < length {
                sum += u32::from(input[start + (position + radius + 1) * stride]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectionCombine, combine_selection, invert_selection, selection_all};

    #[test]
    fn fractional_coverage_preserves_signed_edges_and_fails_before_oversized_allocation() {
        let limits = EditLimits::default();
        let original = selection_all([-8, -8], [16, 16], limits).unwrap();
        let feather = feather_selection(&original, 2, limits).unwrap();
        assert_eq!(original.coverage_signed(-8, 0), 255);
        assert_eq!(feather.coverage_signed(0, 0), 255);
        assert_eq!(
            feather.coverage_signed(-8, 0),
            feather.coverage_signed(7, 0)
        );
        assert!(feather.coverage_signed(-9, 0) > 0);
        assert_eq!(feather.coverage_signed(-15, 0), 0);
        assert!(!feather.is_binary());
        for radius in [0, 1, 2, 8] {
            let output = feather_selection(&original, radius, limits).unwrap();
            assert_eq!(
                output,
                feather_selection(&original, radius, limits).unwrap()
            );
            if radius == 0 {
                assert_eq!(output, original);
            }
        }
        assert_eq!(
            feather_selection(
                &original,
                2,
                EditLimits {
                    max_workspace_bytes: 100,
                    ..limits
                }
            ),
            Err(EditError::LimitExceeded)
        );
        assert_eq!(
            feather_selection(
                &original,
                2,
                EditLimits {
                    max_filter_work: 10,
                    ..limits
                }
            ),
            Err(EditError::LimitExceeded)
        );
        let edge = selection_all([i32::MIN, 0], [1, 1], limits).unwrap();
        assert_eq!(
            feather_selection(&edge, 1, limits),
            Err(EditError::CoordinateOutOfRange)
        );
    }

    #[test]
    fn coverage_algebra_never_silently_converts_soft_edges_to_binary() {
        let limits = EditLimits::default();
        let a = SelectionMask::from_coverage_at([-2, 1], [4, 1], &[0, 64, 128, 255]).unwrap();
        let b = SelectionMask::from_coverage_at([-2, 1], [4, 1], &[0, 128, 64, 255]).unwrap();
        for (operation, expected) in [
            (SelectionCombine::Replace, [0, 128, 64, 255]),
            (SelectionCombine::Add, [0, 128, 128, 255]),
            (SelectionCombine::Subtract, [0, 0, 64, 0]),
        ] {
            assert_eq!(
                combine_selection(&a, &b, operation, limits)
                    .unwrap()
                    .coverage_bytes(),
                expected
            );
        }
        let inverse = invert_selection(&a, a.origin(), a.dimensions(), limits).unwrap();
        assert_eq!(inverse.coverage_bytes(), [255, 191, 127, 0]);
        assert_eq!(
            invert_selection(&inverse, a.origin(), a.dimensions(), limits).unwrap(),
            a
        );
        let legacy = SelectionMask::from_packed_bits_at([-3, -129], [4, 1], &[0b0101]).unwrap();
        assert_eq!(legacy.coverage_bytes(), [255, 0, 255, 0]);
        assert_eq!(legacy.packed_bits(), [0b0101]);
        for (grow, radius) in [
            (true, 0),
            (true, 1),
            (true, 2),
            (false, 0),
            (false, 1),
            (false, 2),
        ] {
            let mask = SelectionMask::from_coverage_at(
                [-3, -4],
                [3, 3],
                &[0, 30, 80, 128, 255, 192, 0, 64, 255],
            )
            .unwrap();
            let actual = if grow {
                crate::grow_selection(&mask, radius, limits)
            } else {
                crate::shrink_selection(&mask, radius, limits)
            }
            .unwrap();
            for y in actual.origin()[1]
                ..actual.origin()[1] + i32::try_from(actual.dimensions()[1]).unwrap()
            {
                for x in actual.origin()[0]
                    ..actual.origin()[0] + i32::try_from(actual.dimensions()[0]).unwrap()
                {
                    let r = i32::from(radius);
                    let mut expected = if grow { 0 } else { 255 };
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if dx * dx + dy * dy <= r * r {
                                let value = mask.coverage_signed(x + dx, y + dy);
                                expected = if grow {
                                    expected.max(value)
                                } else {
                                    expected.min(value)
                                };
                            }
                        }
                    }
                    assert_eq!(
                        actual.coverage_signed(x, y),
                        expected,
                        "disk coverage at {x},{y} grow={grow} radius={radius}"
                    );
                }
            }
        }
    }
}

//! Exact binary Euclidean morphology over bounded signed pixel centers.
use super::{EditError, EditLimits, LayerId, LayerTree, SelectionMask, TileSnapshot};
use super::{TILE_EDGE, region_len, require_workspace};

/// Expands coverage by an integer-radius Euclidean disk, including off-page ink.
/// Radius zero preserves the exact original mask representation.
/// # Errors
/// Rejects radius > 64, coordinate overflow and bounded work/memory exhaustion.
pub fn grow_selection(
    mask: &SelectionMask,
    radius: u8,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    morphology(mask, radius, true, limits)
}

/// Contracts coverage by a Euclidean disk; outside the mask is unselected.
/// An entirely eroded selection remains a valid empty binary mask.
/// # Errors
/// Rejects radius > 64 and bounded work/memory exhaustion.
pub fn shrink_selection(
    mask: &SelectionMask,
    radius: u8,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    morphology(mask, radius, false, limits)
}

fn morphology(
    mask: &SelectionMask,
    radius: u8,
    grow: bool,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    if radius > 64 {
        return Err(EditError::InvalidMask);
    }
    let input_len = region_len(mask.origin(), mask.dimensions(), limits)?;
    if radius == 0 || mask.selected_pixels() == 0 {
        require_workspace(input_len as u64 * 2, limits)?;
        require_work(input_len as u64, limits)?;
        return Ok(mask.clone());
    }
    let pad = if grow { u32::from(radius) } else { 0 };
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
    // Two-byte horizontal squared distances, output mask, retained input,
    // and one column's costs/envelope indices/integer intersection boundaries.
    preflight_morphology(input_len as u64, len as u64, size[1], limits)?;
    let width = size[0] as usize;
    let height = size[1] as usize;
    let r = u16::from(radius);
    let cap = r * r + 1;
    let mut distances = vec![cap; len];
    horizontal_distances(mask, origin, width, &mut distances, r, grow);
    let mut selected = vec![0; len];
    let mut costs = vec![0; height];
    let mut sites = vec![0; height];
    let mut starts = vec![0; height];
    let mut count = 0;
    for x in 0..width {
        for y in 0..height {
            costs[y] = distances[y * width + x];
        }
        let last = lower_envelope(&costs, &mut sites, &mut starts);
        let mut segment = 0;
        for y in 0..height {
            while segment < last && starts[segment + 1] <= bounded_index(y) {
                segment += 1;
            }
            let dy = bounded_index(y) - bounded_index(sites[segment]);
            let mut distance = dy * dy + i64::from(costs[sites[segment]]);
            if !grow {
                // Nearest pixel center outside the finite source rectangle.
                let edge = bounded_index((x + 1).min(width - x).min(y + 1).min(height - y));
                distance = distance.min(edge * edge);
            }
            let near_seed = distance <= i64::from(r * r);
            let value = u8::from(if grow { near_seed } else { !near_seed });
            selected[y * width + x] = value;
            count += u64::from(value);
        }
    }
    Ok(SelectionMask {
        origin,
        width: size[0],
        height: size[1],
        selected,
        count,
    })
}

fn require_work(work: u64, limits: EditLimits) -> Result<(), EditError> {
    if work > limits.max_filter_work {
        Err(EditError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn preflight_morphology(
    input_len: u64,
    output_len: u64,
    height: u32,
    limits: EditLimits,
) -> Result<(), EditError> {
    let column_bytes = u64::from(height)
        * (2 + std::mem::size_of::<usize>() as u64 + std::mem::size_of::<i64>() as u64);
    require_workspace(input_len + output_len * 3 + column_bytes, limits)?;
    require_work(output_len * 10, limits)
}

fn bounded_index(value: usize) -> i64 {
    i64::try_from(value).expect("preflighted region/tile index is bounded to 16M pixels")
}

fn horizontal_distances(
    mask: &SelectionMask,
    origin: [i32; 2],
    width: usize,
    distances: &mut [u16],
    radius: u16,
    grow: bool,
) {
    for (y, row) in distances.chunks_exact_mut(width).enumerate() {
        let mut distance = radius + 1;
        for (x, cost) in row.iter_mut().enumerate() {
            // The entire signed ROI was checked before allocation.
            #[allow(clippy::cast_possible_truncation)]
            let point = [
                (i64::from(origin[0]) + bounded_index(x)) as i32,
                (i64::from(origin[1]) + bounded_index(y)) as i32,
            ];
            let seed = mask.contains_signed(point[0], point[1]) == grow;
            distance = if seed {
                0
            } else {
                (distance + 1).min(radius + 1)
            };
            *cost = distance * distance;
        }
        distance = radius + 1;
        for cost in row.iter_mut().rev() {
            distance = if *cost == 0 {
                0
            } else {
                (distance + 1).min(radius + 1)
            };
            *cost = (*cost).min(distance * distance).min(radius * radius + 1);
        }
    }
}

// Integer lower envelope of parabolas f(q) + (y-q)^2. Each site is inserted
// and removed at most once. No floating rounding or radius-squared loop.
fn lower_envelope(costs: &[u16], sites: &mut [usize], starts: &mut [i64]) -> usize {
    let mut last = 0;
    sites[0] = 0;
    starts[0] = i64::MIN;
    for q in 1..costs.len() {
        let mut start;
        loop {
            let p = sites[last];
            let numerator = i64::from(costs[q]) + bounded_index(q).pow(2)
                - i64::from(costs[p])
                - bounded_index(p).pow(2);
            let denominator = 2 * bounded_index(q - p);
            start = -(-numerator).div_euclid(denominator);
            if last == 0 || start > starts[last] {
                break;
            }
            last -= 1;
        }
        last += 1;
        sites[last] = q;
        starts[last] = start;
    }
    last
}

/// Selects every nonzero raw alpha pixel of one raster, regardless of display
/// visibility/opacity/lock. The ROI is the tight signed alpha bounds, not page.
/// Empty rasters return a 1x1 unselected mask at the origin.
/// # Errors
/// Rejects unknown layers, nonbase mips, invalid coordinates and work/memory
/// exhaustion before allocating a potentially huge sparse-to-dense mask.
pub fn select_layer_alpha(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    tree.raster(target).ok_or(EditError::UnknownLayer)?;
    require_work(snapshot.len() as u64, limits)?;
    let mut scan_pixels = 0_u64;
    for (key, _) in snapshot.iter().filter(|(key, _)| key.layer == target) {
        if key.mip != 0 {
            return Err(EditError::InvalidMask);
        }
        for coordinate in [key.x, key.y] {
            let start = i64::from(coordinate) * i64::from(TILE_EDGE);
            if i32::try_from(start).is_err()
                || i32::try_from(start + i64::from(TILE_EDGE) - 1).is_err()
            {
                return Err(EditError::CoordinateOutOfRange);
            }
        }
        scan_pixels += u64::from(TILE_EDGE).pow(2);
        require_work(scan_pixels * 2 + snapshot.len() as u64 * 3, limits)?;
    }
    let mut min = [i32::MAX; 2];
    let mut max = [i32::MIN; 2];
    let mut count = 0_u64;
    visit_alpha(snapshot, target, |point| {
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
        count += 1;
    });
    let (origin, size) = if count == 0 {
        ([0, 0], [1, 1])
    } else {
        let size =
            [0, 1].map(|axis| u32::try_from(i64::from(max[axis]) - i64::from(min[axis]) + 1));
        let [Ok(width), Ok(height)] = size else {
            return Err(EditError::LimitExceeded);
        };
        (min, [width, height])
    };
    let len = region_len(origin, size, limits)?;
    require_work(
        scan_pixels * 2 + snapshot.len() as u64 * 3 + len as u64,
        limits,
    )?;
    let mut selected = vec![0; len];
    visit_alpha(snapshot, target, |point| {
        // The two scans inspect the same immutable snapshot, so every point
        // lies inside the preflighted (usize-sized) tight alpha rectangle.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let x = (i64::from(point[0]) - i64::from(origin[0])) as usize;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let y = (i64::from(point[1]) - i64::from(origin[1])) as usize;
        selected[y * size[0] as usize + x] = 1;
    });
    Ok(SelectionMask {
        origin,
        width: size[0],
        height: size[1],
        selected,
        count,
    })
}

fn visit_alpha(snapshot: &TileSnapshot, target: LayerId, mut visit: impl FnMut([i32; 2])) {
    for (key, tile) in snapshot.iter().filter(|(key, _)| key.layer == target) {
        for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
            if pixel[3] != 0 {
                // Coordinates and every target tile were preflighted above.
                #[allow(clippy::cast_possible_truncation)]
                let point = [
                    (i64::from(key.x) * i64::from(TILE_EDGE)
                        + bounded_index(index % TILE_EDGE as usize)) as i32,
                    (i64::from(key.y) * i64::from(TILE_EDGE)
                        + bounded_index(index / TILE_EDGE as usize)) as i32,
                ];
                visit(point);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::selection_all;
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTreeNode};
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    #[test]
    fn disk_morphology_preserves_signed_geometry_holes_and_failure_atomicity() {
        // Product risk: ROI clipping, square dilation or wrong erosion borders
        // silently change where later brush/fill edits can damage artwork.
        let limits = EditLimits::default();
        // Filter work is independent of the smaller polygon-edge budget.
        // Realistic 4K preflight must pass without allocating this test fixture.
        assert!(preflight_morphology(3840 * 2160, 3842 * 2162, 2162, limits).is_ok());
        assert_eq!(
            preflight_morphology(
                3840 * 2160,
                3842 * 2162,
                2162,
                EditLimits {
                    max_workspace_bytes: 31 * 1024 * 1024,
                    ..limits
                }
            ),
            Err(EditError::LimitExceeded),
            "budget includes retained mask, intermediate distances and output",
        );
        for packed in [[0b0000_0001, 0], [0b1110_1111, 1], [0b0101_0101, 1], [0, 0]] {
            let mask = SelectionMask::from_packed_bits_at([-129, -1], [3, 3], &packed).unwrap();
            let original = mask.clone();
            for radius in [0, 1, 2, 4, 64] {
                let grown = grow_selection(&mask, radius, limits).unwrap();
                let shrunk = shrink_selection(&mask, radius, limits).unwrap();
                if radius == 0 {
                    assert_eq!(grown, mask);
                    assert_eq!(shrunk, mask);
                }
                for y in -6..7 {
                    for x in -134..-121 {
                        let mut any = false;
                        let mut all = true;
                        for dy in -i32::from(radius)..=i32::from(radius) {
                            for dx in -i32::from(radius)..=i32::from(radius) {
                                if dx * dx + dy * dy <= i32::from(radius).pow(2) {
                                    let bit = mask.contains_signed(x + dx, y + dy);
                                    any |= bit;
                                    all &= bit;
                                }
                            }
                        }
                        assert_eq!(grown.contains_signed(x, y), any, "grow {radius}: {x},{y}");
                        assert_eq!(
                            shrunk.contains_signed(x, y),
                            all,
                            "shrink {radius}: {x},{y}"
                        );
                    }
                }
            }
            for tight in [
                EditLimits {
                    max_workspace_bytes: 9,
                    ..limits
                },
                EditLimits {
                    max_filter_work: 1,
                    ..limits
                },
            ] {
                assert_eq!(
                    grow_selection(&mask, 1, tight),
                    Err(EditError::LimitExceeded)
                );
                assert_eq!(
                    shrink_selection(&mask, 1, tight),
                    Err(EditError::LimitExceeded)
                );
            }
            assert_eq!(
                grow_selection(&mask, 65, limits),
                Err(EditError::InvalidMask)
            );
            assert_eq!(mask, original);
        }
        let edge = selection_all([i32::MIN, 0], [1, 1], limits).unwrap();
        assert_eq!(
            grow_selection(&edge, 1, limits),
            Err(EditError::CoordinateOutOfRange)
        );
        assert_eq!(
            shrink_selection(&edge, 1, limits)
                .unwrap()
                .selected_pixels(),
            0
        );
    }

    #[test]
    fn raw_alpha_selection_keeps_signed_faint_ink_and_rejects_unsafe_dense_extents() {
        // Product risk: opacity/view state or another layer must not alter the
        // artwork silhouette; sparse far-apart tiles must not allocate unbounded RAM.
        let limits = EditLimits::default();
        let tree = LayerTree::new(GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(0),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: [1, 2]
                .map(|id| {
                    LayerTreeNode::Raster(LayerNode {
                        alpha_locked: false,
                        clip_to_below: false,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        id: LayerId(id),
                        name: "Ink".into(),
                        visible: false,
                        locked: true,
                        reference: false,
                        opacity_u16: 0,
                        content_root: ContentRootId(0),
                    })
                })
                .to_vec(),
        })
        .unwrap();
        let key = |layer, x| TileKey {
            layer: LayerId(layer),
            mip: 0,
            x,
            y: -1,
        };
        let mut faint = vec![0; TILE_BYTE_LEN];
        faint[3] = 1;
        faint[TILE_BYTE_LEN - 1] = 255;
        let snapshot = TileSnapshot::from_tiles([
            (key(1, -1), faint.clone()),
            (key(2, 3), vec![255; TILE_BYTE_LEN]),
        ])
        .unwrap();
        let before = snapshot.clone();
        let selected = select_layer_alpha(&snapshot, &tree, LayerId(1), limits).unwrap();
        assert_eq!(selected.origin(), [-128, -128]);
        assert_eq!(selected.dimensions(), [128, 128]);
        assert_eq!(selected.selected_pixels(), 2);
        assert!(selected.contains_signed(-128, -128));
        assert!(selected.contains_signed(-1, -1));
        assert!(!selected.contains_signed(384, -128));
        assert_eq!(
            select_layer_alpha(&TileSnapshot::default(), &tree, LayerId(1), limits)
                .unwrap()
                .selected_pixels(),
            0
        );
        for (bad_key, error) in [
            (
                TileKey {
                    mip: 1,
                    ..key(1, 0)
                },
                EditError::InvalidMask,
            ),
            (key(1, i32::MAX), EditError::CoordinateOutOfRange),
            (key(1, 100_000), EditError::LimitExceeded),
        ] {
            let bad =
                TileSnapshot::from_tiles([(key(1, -1), faint.clone()), (bad_key, faint.clone())])
                    .unwrap();
            assert_eq!(
                select_layer_alpha(&bad, &tree, LayerId(1), limits),
                Err(error)
            );
        }
        for tight in [
            EditLimits {
                max_workspace_bytes: 1,
                ..limits
            },
            EditLimits {
                max_filter_work: 1,
                ..limits
            },
        ] {
            assert_eq!(
                select_layer_alpha(&snapshot, &tree, LayerId(1), tight),
                Err(EditError::LimitExceeded)
            );
        }
        assert_eq!(snapshot, before);
    }
}

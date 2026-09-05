//! Bounded cut/transform/paste on one signed sparse raster. Page bounds never
//! discard artwork. Every read samples the immutable pre-edit snapshot.
use crate::{EditError, EditLimits, SelectionMask, SelectionPaintResult, composite_surface};
use nyatidraw_api::{LayerId, RasterTransform};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
struct Bounds {
    min: [i64; 2],
    max: [i64; 2],
}

impl Bounds {
    fn include(bounds: &mut Option<Self>, point: [i64; 2]) {
        let bounds = bounds.get_or_insert(Self {
            min: point,
            max: point,
        });
        for (axis, value) in point.into_iter().enumerate() {
            bounds.min[axis] = bounds.min[axis].min(value);
            bounds.max[axis] = bounds.max[axis].max(value);
        }
    }

    fn size(self, limits: EditLimits) -> Result<[u32; 2], EditError> {
        let size = [self.max[0] - self.min[0] + 1, self.max[1] - self.min[1] + 1];
        let [Ok(width), Ok(height)] = size.map(u32::try_from) else {
            return Err(EditError::LimitExceeded);
        };
        require_pixels(u64::from(width) * u64::from(height), limits)?;
        Ok([width, height])
    }
}

fn require_pixels(pixels: u64, limits: EditLimits) -> Result<(), EditError> {
    if pixels > limits.max_pixels {
        Err(EditError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn selected(mask: Option<&SelectionMask>, point: [i64; 2]) -> bool {
    mask.is_none_or(|mask| {
        let [Ok(x), Ok(y)] = point.map(u32::try_from) else {
            return false;
        };
        mask.contains(x, y)
    })
}

fn point_at(key: TileKey, index: usize) -> [i64; 2] {
    let (x, y) = key.pixel_origin();
    [
        x + i64::try_from(index % 128).expect("tile column"),
        y + i64::try_from(index / 128).expect("tile row"),
    ]
}

fn source_bounds(
    snapshot: &TileSnapshot,
    target: LayerId,
    mask: Option<&SelectionMask>,
    limits: EditLimits,
) -> Result<Option<Bounds>, EditError> {
    // Bound scanning independently of the occupied-pixel bounding rectangle.
    let mut tiles = 0_u64;
    for (key, _) in snapshot.iter().filter(|(key, _)| key.layer == target) {
        if key.mip != 0 {
            return Err(EditError::InvalidTransform);
        }
        tiles += 1;
        require_pixels(tiles * u64::from(TILE_EDGE).pow(2), limits)?;
    }
    let mut bounds = None;
    if let Some(mask) = mask {
        let [width, height] = mask.dimensions();
        require_pixels(u64::from(width) * u64::from(height), limits)?;
        for y in 0..height {
            for x in 0..width {
                if mask.contains(x, y) {
                    Bounds::include(&mut bounds, [i64::from(x), i64::from(y)]);
                }
            }
        }
    } else {
        for (key, tile) in snapshot.iter().filter(|(key, _)| key.layer == target) {
            for (index, color) in tile.pixels().chunks_exact(4).enumerate() {
                if color != [0; 4] {
                    Bounds::include(&mut bounds, point_at(key, index));
                }
            }
        }
    }
    if bounds.is_some_and(|b| {
        b.min
            .into_iter()
            .chain(b.max)
            .any(|v| i32::try_from(v).is_err())
    }) {
        return Err(EditError::CoordinateOutOfRange);
    }
    Ok(bounds)
}

struct Replacements<'a> {
    before: &'a TileSnapshot,
    tiles: BTreeMap<TileKey, Vec<u8>>,
    baseline_bytes: u64,
    limits: EditLimits,
}

impl Replacements<'_> {
    fn tile(&mut self, key: TileKey) -> Result<&mut Vec<u8>, EditError> {
        if !self.tiles.contains_key(&key) {
            // Include construction of immutable replacement objects and root
            // metadata while scratch buffers and the original root coexist.
            let bytes = self.baseline_bytes
                + (self.tiles.len() as u64 + 1) * (TILE_BYTE_LEN as u64 * 2 + 256);
            if bytes > self.limits.max_workspace_bytes {
                return Err(EditError::LimitExceeded);
            }
            let pixels = self
                .before
                .get(key)
                .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
            self.tiles.insert(key, pixels);
        }
        Ok(self.tiles.get_mut(&key).expect("replacement inserted"))
    }

    fn finish(mut self) -> Result<SelectionPaintResult, EditError> {
        self.tiles.retain(|key, pixels| {
            self.before.get(*key).map_or_else(
                || pixels.iter().any(|byte| *byte != 0),
                |tile| tile.pixels() != pixels,
            )
        });
        let changed_tiles = self.tiles.keys().copied().collect();
        let after = self
            .before
            .with_replacements(self.tiles)
            .map_err(EditError::Tiles)?;
        Ok(SelectionPaintResult {
            after,
            changed_tiles,
        })
    }
}

fn pixel_address(target: LayerId, point: [i64; 2]) -> (TileKey, usize) {
    // Both rectangles have been checked against signed i32 pixel coordinates.
    let [x, y] = point.map(|v| i32::try_from(v).expect("preflighted transform coordinate"));
    let offset = (y.rem_euclid(128) as usize * 128 + x.rem_euclid(128) as usize) * 4;
    (TileKey::from_pixel(target, 128, x, y), offset)
}

fn inverse_pixel(
    point: [u32; 2],
    source: [u32; 2],
    output: [u32; 2],
    transform: RasterTransform,
) -> [u32; 2] {
    let rotated = if transform.quarter_turns.is_multiple_of(2) {
        source
    } else {
        [source[1], source[0]]
    };
    let [rx, ry] = std::array::from_fn(|axis| {
        u32::try_from(
            (2 * u64::from(point[axis]) + 1) * u64::from(rotated[axis])
                / (2 * u64::from(output[axis])),
        )
        .expect("sample lies inside source")
    });
    let [mut x, mut y] = match transform.quarter_turns {
        0 => [rx, ry],
        1 => [ry, source[1] - 1 - rx],
        2 => [source[0] - 1 - rx, source[1] - 1 - ry],
        3 => [source[0] - 1 - ry, rx],
        _ => unreachable!("validated quarter turn"),
    };
    if transform.flip_x {
        x = source[0] - 1 - x;
    }
    if transform.flip_y {
        y = source[1] - 1 - y;
    }
    [x, y]
}

/// Rebase all raster coordinates without clipping to a page or resampling.
/// Layer locks/visibility do not alter this document-wide coordinate change.
///
/// # Errors
/// Rejects unsupported mips, coordinate overflow and bounded work/memory limits.
/// No partial replacement escapes on failure.
pub fn translate_artwork(
    snapshot: &TileSnapshot,
    offset: [i32; 2],
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    let limits = limits.bounded();
    require_pixels(snapshot.len() as u64 * u64::from(TILE_EDGE).pow(2), limits)?;
    if snapshot.iter().any(|(key, _)| key.mip != 0) {
        return Err(EditError::InvalidTransform);
    }
    if offset == [0, 0] {
        return Ok(SelectionPaintResult {
            after: snapshot.clone(),
            changed_tiles: Vec::new(),
        });
    }
    let mut tiles: BTreeMap<TileKey, Vec<u8>> = BTreeMap::new();
    // Include zero replacements removing old keys, new objects and map copies.
    let base_bytes = snapshot.len() as u64 * (TILE_BYTE_LEN as u64 * 2 + 256);
    if base_bytes > limits.max_workspace_bytes {
        return Err(EditError::LimitExceeded);
    }
    for (key, tile) in snapshot.iter() {
        for (index, color) in tile.pixels().chunks_exact(4).enumerate() {
            if color == [0; 4] {
                continue;
            }
            let point = point_at(key, index);
            let point = [
                point[0] + i64::from(offset[0]),
                point[1] + i64::from(offset[1]),
            ];
            if point.iter().any(|&v| i32::try_from(v).is_err()) {
                return Err(EditError::CoordinateOutOfRange);
            }
            let (destination, at) = pixel_address(key.layer, point);
            if !tiles.contains_key(&destination) {
                let bytes =
                    base_bytes + (tiles.len() as u64 + 1) * (TILE_BYTE_LEN as u64 * 2 + 256);
                if bytes > limits.max_workspace_bytes {
                    return Err(EditError::LimitExceeded);
                }
            }
            tiles
                .entry(destination)
                .or_insert_with(|| vec![0; TILE_BYTE_LEN])[at..at + 4]
                .copy_from_slice(color);
        }
    }
    for (key, _) in snapshot.iter() {
        tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
    }
    Replacements {
        before: snapshot,
        tiles,
        baseline_bytes: base_bytes,
        limits,
    }
    .finish()
}

/// Cuts selected pixels (or all active-layer pixels), applies the integer
/// transform around the source bounds' top-left and pastes source-over. Reads
/// always use `snapshot`, so overlapping moves neither smear nor double-blend.
/// Other layers and unselected pixels outside the destination stay exact.
///
/// # Errors
/// Rejects locked/missing targets, unsupported mips, invalid dimensions/turns,
/// signed coordinate overflow and work/memory limits. Input is immutable; all
/// failures leave the original artwork intact without a partial replacement.
#[allow(clippy::too_many_lines)]
pub fn transform_raster(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    mask: Option<&SelectionMask>,
    transform: RasterTransform,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    let limits = limits.bounded();
    let layer =
        crate::selection::find_raster(tree.root(), target).ok_or(EditError::UnknownLayer)?;
    if layer.locked {
        return Err(EditError::LockedLayer);
    }
    if transform.quarter_turns > 3 || transform.size.is_some_and(|size| size.contains(&0)) {
        return Err(EditError::InvalidTransform);
    }
    if let Some([width, height]) = transform.size {
        require_pixels(u64::from(width) * u64::from(height), limits)?;
    }
    let baseline_bytes = snapshot.len() as u64 * 256
        + mask.map_or(0, |mask| {
            let [w, h] = mask.dimensions();
            u64::from(w) * u64::from(h)
        });
    if baseline_bytes > limits.max_workspace_bytes {
        return Err(EditError::LimitExceeded);
    }
    let Some(bounds) = source_bounds(snapshot, target, mask, limits)? else {
        return Ok(SelectionPaintResult {
            after: snapshot.clone(),
            changed_tiles: Vec::new(),
        });
    };
    let source = bounds.size(limits)?;
    let output = transform
        .size
        .unwrap_or(if transform.quarter_turns.is_multiple_of(2) {
            source
        } else {
            [source[1], source[0]]
        });
    let origin: [i64; 2] =
        std::array::from_fn(|axis| bounds.min[axis] + i64::from(transform.offset[axis]));
    for axis in 0..2 {
        if i32::try_from(origin[axis]).is_err()
            || i32::try_from(origin[axis] + i64::from(output[axis]) - 1).is_err()
        {
            return Err(EditError::CoordinateOutOfRange);
        }
    }
    let mut replacements = Replacements {
        before: snapshot,
        tiles: BTreeMap::new(),
        baseline_bytes,
        limits,
    };
    for (key, tile) in snapshot.iter().filter(|(key, _)| key.layer == target) {
        for (index, color) in tile.pixels().chunks_exact(4).enumerate() {
            if color != [0; 4] && selected(mask, point_at(key, index)) {
                replacements.tile(key)?[index * 4..index * 4 + 4].fill(0);
            }
        }
    }
    for y in 0..output[1] {
        for x in 0..output[0] {
            let source_pixel = inverse_pixel([x, y], source, output, transform);
            let source_point =
                std::array::from_fn(|axis| bounds.min[axis] + i64::from(source_pixel[axis]));
            if !selected(mask, source_point) {
                continue;
            }
            let (key, offset) = pixel_address(target, source_point);
            let Some(tile) = snapshot.get(key) else {
                continue;
            };
            let color = &tile.pixels()[offset..offset + 4];
            if color == [0; 4] {
                continue;
            }
            let (key, offset) =
                pixel_address(target, [origin[0] + i64::from(x), origin[1] + i64::from(y)]);
            composite_surface(
                &mut replacements.tile(key)?[offset..offset + 4],
                color,
                u16::MAX,
            );
        }
    }
    replacements.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTreeNode};

    fn tree(locked: bool) -> LayerTree {
        LayerTree::new(GroupNode {
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: [1, 2]
                .map(|id| {
                    LayerTreeNode::Raster(LayerNode {
                        id: LayerId(id),
                        name: "Raster".into(),
                        visible: true,
                        locked,
                        reference: false,
                        opacity_u16: u16::MAX,
                        content_root: ContentRootId(0),
                    })
                })
                .into(),
        })
        .unwrap()
    }

    fn fixture(points: impl IntoIterator<Item = (u128, [i32; 2], [u8; 4])>) -> TileSnapshot {
        let mut tiles: BTreeMap<TileKey, Vec<u8>> = BTreeMap::new();
        for (layer, point, color) in points {
            let (key, offset) = pixel_address(LayerId(layer), point.map(i64::from));
            tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN])[offset..offset + 4]
                .copy_from_slice(&color);
        }
        TileSnapshot::from_tiles(tiles).unwrap()
    }

    #[test]
    fn page_rebase_preserves_all_layers_and_off_page_pixels_or_rejects_atomically() {
        // The disconnected mask must crop to its full rectangle, including gaps.
        let mask = SelectionMask::from_packed_bits(4, 3, &[0x20, 0x08]).unwrap();
        assert_eq!(mask.bounds(), Some([1, 1, 3, 2]));
        let points = [
            (1, [-129, -1], [7, 7, 7, 255]),
            (1, [1, 1], [128, 0, 0, 128]),
            (1, [3, 2], [0, 255, 0, 255]),
            (2, [128, 129], [0, 0, 64, 64]),
            (2, [1, 1], [9, 9, 9, 255]),
        ];
        let before = fixture(points);
        let limits = EditLimits::default();
        for offset in [[-1, -1], [-128, -129], [129, 127]] {
            let after = translate_artwork(&before, offset, limits).unwrap().after;
            let expected = fixture(
                points.map(|(layer, [x, y], color)| (layer, [x + offset[0], y + offset[1]], color)),
            );
            assert_eq!(after, expected);
            assert_eq!(
                translate_artwork(&after, offset.map(|v| -v), limits)
                    .unwrap()
                    .after,
                before
            );
        }
        for small in [
            EditLimits {
                max_pixels: 1,
                ..limits
            },
            EditLimits {
                max_workspace_bytes: 1024,
                ..limits
            },
        ] {
            assert_eq!(
                translate_artwork(&before, [-1, -1], small).unwrap_err(),
                EditError::LimitExceeded
            );
            assert_eq!(before, fixture(points));
        }
        for (position, delta) in [(i32::MIN, -1), (i32::MAX, 1)] {
            let edge = fixture([(1, [position, 0], [1, 0, 0, 255])]);
            assert_eq!(
                translate_artwork(&edge, [delta, 0], limits).unwrap_err(),
                EditError::CoordinateOutOfRange
            );
        }
    }

    #[test]
    fn transforms_preserve_exact_pixels_across_signed_tiles_without_smearing() {
        type Case = (u8, bool, bool, Option<[u32; 2]>, u32, &'static [u8]);
        let cases: [Case; 9] = [
            (0, false, false, None, 2, &[1, 2, 3, 4, 5, 6]),
            (1, false, false, None, 3, &[5, 3, 1, 6, 4, 2]),
            (2, false, false, None, 2, &[6, 5, 4, 3, 2, 1]),
            (3, false, false, None, 3, &[2, 4, 6, 1, 3, 5]),
            (0, true, false, None, 2, &[2, 1, 4, 3, 6, 5]),
            (0, false, true, None, 2, &[5, 6, 3, 4, 1, 2]),
            (1, true, false, None, 3, &[6, 4, 2, 5, 3, 1]),
            (
                0,
                false,
                false,
                Some([4, 3]),
                4,
                &[1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6],
            ),
            (0, false, false, Some([1, 2]), 1, &[2, 6]),
        ];
        let other = (2, [-1, 127], [0, 17, 0, 32]);
        let before = fixture(
            (0..6)
                .map(|i| {
                    (
                        1,
                        [-1 + i % 2, 127 + i / 2],
                        [u8::try_from(i + 1).unwrap(), 0, 0, 255],
                    )
                })
                .chain([other]),
        );
        for (turns, flip_x, flip_y, size, width, expected) in cases {
            for offset in [[0, 0], [-128, -128], [129, 1]] {
                let actual = transform_raster(
                    &before,
                    &tree(false),
                    LayerId(1),
                    None,
                    RasterTransform {
                        offset,
                        quarter_turns: turns,
                        flip_x,
                        flip_y,
                        size,
                    },
                    EditLimits::default(),
                )
                .unwrap();
                let golden = fixture(
                    expected
                        .iter()
                        .enumerate()
                        .map(|(i, &red)| {
                            (
                                1,
                                [
                                    -1 + offset[0] + i32::try_from(i % width as usize).unwrap(),
                                    127 + offset[1] + i32::try_from(i / width as usize).unwrap(),
                                ],
                                [red, 0, 0, 255],
                            )
                        })
                        .chain([other]),
                );
                assert_eq!(
                    actual.after, golden,
                    "turns={turns} offset={offset:?} size={size:?}"
                );
                if actual.after == before {
                    assert!(actual.changed_tiles.is_empty());
                }
            }
        }
    }

    #[test]
    fn selection_move_cuts_overlap_once_and_preserves_unselected_off_page_art() {
        let fixed = [
            (1, [-1, 0], [7, 7, 7, 255]),
            (1, [3, 0], [9, 9, 9, 255]),
            (2, [1, 0], [17, 0, 0, 255]),
        ];
        let before = fixture(
            [
                (1, [0, 0], [128, 0, 0, 128]),
                (1, [1, 0], [0, 64, 0, 64]),
                (1, [2, 0], [0, 0, 255, 255]),
            ]
            .into_iter()
            .chain(fixed),
        );
        let mask = SelectionMask::from_packed_bits(4, 1, &[3]).unwrap();
        let actual = transform_raster(
            &before,
            &tree(false),
            LayerId(1),
            Some(&mask),
            RasterTransform {
                offset: [1, 0],
                ..RasterTransform::default()
            },
            EditLimits::default(),
        )
        .unwrap();
        let golden = fixture(
            [
                (1, [1, 0], [128, 0, 0, 128]),
                (1, [2, 0], [0, 64, 191, 255]),
            ]
            .into_iter()
            .chain(fixed),
        );
        assert_eq!(actual.after, golden);
    }

    #[test]
    fn invalid_or_over_budget_transforms_reject_instead_of_clipping_artwork() {
        let basic = RasterTransform::default();
        let before = fixture([(1, [0, 0], [1, 0, 0, 255])]);
        let normal = EditLimits::default();
        for (transform, limits, error) in [
            (
                RasterTransform {
                    size: Some([0, 1]),
                    ..basic
                },
                normal,
                EditError::InvalidTransform,
            ),
            (
                RasterTransform {
                    quarter_turns: 4,
                    ..basic
                },
                normal,
                EditError::InvalidTransform,
            ),
            (
                RasterTransform {
                    size: Some([u32::MAX, 2]),
                    ..basic
                },
                normal,
                EditError::LimitExceeded,
            ),
            (
                basic,
                EditLimits {
                    max_workspace_bytes: 1024,
                    ..normal
                },
                EditError::LimitExceeded,
            ),
            (
                basic,
                EditLimits {
                    max_pixels: 10,
                    ..normal
                },
                EditError::LimitExceeded,
            ),
        ] {
            assert_eq!(
                transform_raster(&before, &tree(false), LayerId(1), None, transform, limits)
                    .unwrap_err(),
                error
            );
        }
        assert_eq!(
            transform_raster(&before, &tree(true), LayerId(1), None, basic, normal).unwrap_err(),
            EditError::LockedLayer
        );
        for (position, offset) in [(i32::MIN, -1), (i32::MAX, 1)] {
            let edge = fixture([(1, [position, 0], [1, 0, 0, 255])]);
            assert_eq!(
                transform_raster(
                    &edge,
                    &tree(false),
                    LayerId(1),
                    None,
                    RasterTransform {
                        offset: [offset, 0],
                        ..basic
                    },
                    normal
                )
                .unwrap_err(),
                EditError::CoordinateOutOfRange
            );
        }
        let sparse = fixture([
            (1, [i32::MIN, 0], [1, 0, 0, 255]),
            (1, [i32::MAX, 0], [2, 0, 0, 255]),
        ]);
        assert_eq!(
            transform_raster(&sparse, &tree(false), LayerId(1), None, basic, normal).unwrap_err(),
            EditError::LimitExceeded
        );
    }
}

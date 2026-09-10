//! Immutable-source, bounded affine cut/resample/paste for selection previews.
use crate::transform::{Replacements, pixel_address};
use crate::{EditError, EditLimits, SelectionMask, SelectionPaintResult, composite_surface};
use nyatidraw_api::{AffineTransform, LayerId};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileSnapshot};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct AffineResult {
    pub paint: SelectionPaintResult,
    pub selection: SelectionMask,
    /// Clockwise source rectangle corners, starting at its top left. These
    /// describe geometric edges, not the extra half-pixel filter support.
    pub corners: [[f64; 2]; 4],
}

#[derive(Clone, Copy)]
struct Mapping {
    center: [f64; 2],
    offset: [f64; 2],
    scale: [f64; 2],
    sin: f64,
    cos: f64,
}

impl Mapping {
    fn new(origin: [i32; 2], size: [u32; 2], value: &AffineTransform) -> Result<Self, EditError> {
        if value.scale_ppm.contains(&0) {
            return Err(EditError::InvalidTransform);
        }
        let radians = f64::from(value.rotation_millidegrees.rem_euclid(360_000))
            * std::f64::consts::PI
            / 180_000.0;
        let (sin, cos) = radians.sin_cos();
        // Exact orthogonal values avoid a spurious sampling fringe at 90°.
        let snap = |v: f64| {
            if v.abs() < 1e-12 {
                0.0
            } else if (v.abs() - 1.0).abs() < 1e-12 {
                v.signum()
            } else {
                v
            }
        };
        #[allow(clippy::cast_precision_loss)]
        let offset = value.offset_milli.map(|v| v as f64 / 1000.0);
        Ok(Self {
            center: std::array::from_fn(|axis| {
                f64::from(origin[axis]) + f64::from(size[axis]) / 2.0
            }),
            offset,
            scale: [
                f64::from(value.scale_ppm[0]) / 1_000_000.0 * if value.flip_x { -1.0 } else { 1.0 },
                f64::from(value.scale_ppm[1]) / 1_000_000.0 * if value.flip_y { -1.0 } else { 1.0 },
            ],
            sin: snap(sin),
            cos: snap(cos),
        })
    }

    fn forward(self, point: [f64; 2]) -> [f64; 2] {
        let x = (point[0] - self.center[0]) * self.scale[0];
        let y = (point[1] - self.center[1]) * self.scale[1];
        [
            self.center[0] + self.offset[0] + self.cos * x - self.sin * y,
            self.center[1] + self.offset[1] + self.sin * x + self.cos * y,
        ]
    }

    fn inverse(self, point: [f64; 2]) -> [f64; 2] {
        let x = point[0] - self.center[0] - self.offset[0];
        let y = point[1] - self.center[1] - self.offset[1];
        [
            self.center[0] + (self.cos * x + self.sin * y) / self.scale[0],
            self.center[1] + (-self.sin * x + self.cos * y) / self.scale[1],
        ]
    }

    fn corners(self, origin: [i32; 2], size: [u32; 2], pad: f64) -> [[f64; 2]; 4] {
        let left = f64::from(origin[0]) - pad;
        let top = f64::from(origin[1]) - pad;
        let right = f64::from(origin[0]) + f64::from(size[0]) + pad;
        let bottom = f64::from(origin[1]) + f64::from(size[1]) + pad;
        [[left, top], [right, top], [right, bottom], [left, bottom]].map(|p| self.forward(p))
    }
}

#[derive(Clone, Copy)]
struct Region {
    origin: [i32; 2],
    size: [u32; 2],
}

impl Region {
    fn enclosing(corners: [[f64; 2]; 4]) -> Result<Self, EditError> {
        let mut origin = [0; 2];
        let mut size = [0; 2];
        for axis in 0..2 {
            let min = corners
                .iter()
                .map(|p| p[axis])
                .fold(f64::INFINITY, f64::min)
                .floor();
            let end = corners
                .iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                .ceil();
            if !min.is_finite()
                || !end.is_finite()
                || min < f64::from(i32::MIN)
                || end > f64::from(i32::MAX) + 1.0
                || end <= min
            {
                return Err(EditError::CoordinateOutOfRange);
            }
            #[allow(clippy::cast_possible_truncation)]
            {
                origin[axis] = min as i32;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                size[axis] =
                    u32::try_from((end - min) as u64).map_err(|_| EditError::LimitExceeded)?;
            }
        }
        Ok(Self { origin, size })
    }

    fn pixels(self) -> u64 {
        u64::from(self.size[0]) * u64::from(self.size[1])
    }

    fn tile_count(self) -> u64 {
        let extent: [u64; 2] = std::array::from_fn(|axis| {
            let first = i64::from(self.origin[axis]).div_euclid(128);
            let last =
                (i64::from(self.origin[axis]) + i64::from(self.size[axis]) - 1).div_euclid(128);
            u64::try_from(last - first + 1).expect("positive checked region")
        });
        extent[0] * extent[1]
    }

    fn point(self, x: u32, y: u32) -> [i64; 2] {
        [
            i64::from(self.origin[0]) + i64::from(x),
            i64::from(self.origin[1]) + i64::from(y),
        ]
    }
}

/// Cuts the selected pixels once and resamples that immutable source. Callers
/// must reuse the original snapshot/mask for every preview, then accept just
/// the final result as one edit. Selection geometry includes nonzero bilinear
/// filter support, independently of whether selected artwork is transparent.
/// Positive rotation is clockwise in document coordinates (Y points down).
/// Bilinear minification is not an area filter: a shape shrunk between all
/// pixel centers is rejected, rather than committing an empty cut result.
///
/// # Errors
/// Rejects invalid, empty, locked, out-of-range or over-budget requests before
/// allocating scratch pixels. Failures never publish a partially cut source.
pub fn transform_selection_affine(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    mask: &SelectionMask,
    transform: &AffineTransform,
    limits: EditLimits,
) -> Result<AffineResult, EditError> {
    let limits = limits.bounded();
    let layer = tree.raster(target).ok_or(EditError::UnknownLayer)?;
    if layer.locked {
        return Err(EditError::LockedLayer);
    }
    if layer.alpha_locked {
        return Err(EditError::AlphaLockedLayer);
    }
    if snapshot
        .iter()
        .any(|(key, _)| key.layer == target && key.mip != 0)
    {
        return Err(EditError::InvalidTransform);
    }
    let [mw, mh] = mask.dimensions();
    let mask_pixels = u64::from(mw) * u64::from(mh);
    if mask_pixels > limits.max_pixels
        || mask_pixels > limits.max_polygon_work
        || mask_pixels
            .saturating_mul(2)
            .saturating_add(snapshot.len() as u64 * 256)
            > limits.max_workspace_bytes
    {
        return Err(EditError::LimitExceeded);
    }
    let (origin, size) = mask.bounds_signed().ok_or(EditError::InvalidMask)?;
    let source = Region { origin, size };
    let mapping = Mapping::new(origin, size, transform)?;
    let corners = mapping.corners(origin, size, 0.0);
    if *transform == AffineTransform::default() {
        return Ok(AffineResult {
            paint: SelectionPaintResult {
                after: snapshot.clone(),
                changed_tiles: Vec::new(),
            },
            selection: mask.clone(),
            corners,
        });
    }
    // A pixel-center tent kernel extends half a source pixel beyond the
    // geometric selection rectangle. Bound that support before rasterization.
    let output = Region::enclosing(mapping.corners(origin, size, 0.5))?;
    let work = mask_pixels
        .saturating_add(source.pixels())
        .saturating_add(output.pixels().saturating_mul(6));
    let baseline = (snapshot.len() as u64)
        .saturating_mul(256)
        .saturating_add(mask_pixels)
        .saturating_add(output.pixels().saturating_mul(2));
    let worst_bytes = baseline.saturating_add(
        source
            .tile_count()
            .saturating_add(output.tile_count())
            .saturating_mul(TILE_BYTE_LEN as u64 * 2 + 256),
    );
    if source.pixels() > limits.max_pixels
        || output.pixels() > limits.max_pixels
        || work > limits.max_polygon_work
        || worst_bytes > limits.max_workspace_bytes
    {
        return Err(EditError::LimitExceeded);
    }
    let mut replacements = Replacements {
        before: snapshot,
        tiles: BTreeMap::new(),
        baseline_bytes: baseline,
        limits,
    };
    cut_source(&mut replacements, source, target, mask)?;
    let pixel_count = usize::try_from(output.pixels()).map_err(|_| EditError::LimitExceeded)?;
    let mut packed = vec![0_u8; pixel_count.div_ceil(8)];
    for y in 0..output.size[1] {
        for x in 0..output.size[0] {
            let point = output.point(x, y);
            #[allow(clippy::cast_precision_loss)]
            let sample = mapping.inverse(point.map(|p| p as f64 + 0.5));
            let (color, selected) = bilinear(snapshot, target, mask, sample);
            if selected {
                let index = y as usize * output.size[0] as usize + x as usize;
                packed[index / 8] |= 1 << (index % 8);
            }
            if color[3] != 0 {
                let (key, offset) = pixel_address(target, point);
                composite_surface(
                    &mut replacements.tile(key)?[offset..offset + 4],
                    &color,
                    u16::MAX,
                );
            }
        }
    }
    let selection = SelectionMask::from_packed_bits_at(output.origin, output.size, &packed)?;
    // Extreme minification of a thin shape can miss all pixel centers. Never
    // publish the cut if no editable transformed selection remains.
    if selection.selected_pixels() == 0 {
        return Err(EditError::InvalidTransform);
    }
    Ok(AffineResult {
        paint: replacements.finish()?,
        selection,
        corners,
    })
}

fn cut_source(
    replacements: &mut Replacements<'_>,
    source: Region,
    target: LayerId,
    mask: &SelectionMask,
) -> Result<(), EditError> {
    for y in 0..source.size[1] {
        for x in 0..source.size[0] {
            let point = source.point(x, y);
            let [px, py] = point.map(|p| i32::try_from(p).expect("checked source"));
            if mask.contains_signed(px, py) {
                let (key, offset) = pixel_address(target, point);
                if replacements
                    .before
                    .get(key)
                    .is_some_and(|t| t.pixels()[offset..offset + 4] != [0; 4])
                {
                    replacements.tile(key)?[offset..offset + 4].fill(0);
                }
            }
        }
    }
    Ok(())
}

fn bilinear(
    snapshot: &TileSnapshot,
    layer: LayerId,
    mask: &SelectionMask,
    point: [f64; 2],
) -> ([u8; 4], bool) {
    let base = point.map(|v| (v - 0.5).floor());
    let frac: [f64; 2] =
        std::array::from_fn(|axis| (point[axis] - 0.5 - base[axis]).clamp(0.0, 1.0));
    let mut color = [0.0; 4];
    let mut coverage = 0.0;
    for y in 0..2 {
        for x in 0..2 {
            let weight = if x == 0 { 1.0 - frac[0] } else { frac[0] }
                * if y == 0 { 1.0 - frac[1] } else { frac[1] };
            if weight <= 1e-12 {
                continue;
            }
            let p = [base[0] + f64::from(x), base[1] + f64::from(y)];
            if p.iter()
                .any(|v| *v < f64::from(i32::MIN) || *v > f64::from(i32::MAX))
            {
                continue;
            }
            #[allow(clippy::cast_possible_truncation)]
            let p = p.map(|v| v as i32);
            if !mask.contains_signed(p[0], p[1]) {
                continue;
            }
            coverage += weight;
            let (key, offset) = pixel_address(layer, p.map(i64::from));
            if let Some(tile) = snapshot.get(key) {
                for (channel, value) in color.iter_mut().enumerate() {
                    *value += f64::from(tile.pixels()[offset + channel]) * weight;
                }
            }
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (
        color.map(|v| v.round().clamp(0.0, 255.0) as u8),
        coverage > 1e-12,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTreeNode};
    use nyatidraw_tiles::TileKey;

    fn tree() -> LayerTree {
        LayerTree::new(GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(100),
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
                        name: "Raster".into(),
                        visible: true,
                        locked: false,
                        reference: false,
                        opacity_u16: u16::MAX,
                        content_root: ContentRootId(0),
                    })
                })
                .into(),
        })
        .unwrap()
    }

    fn fixture(pixels: &[(LayerId, [i32; 2], [u8; 4])]) -> TileSnapshot {
        let mut tiles = BTreeMap::<TileKey, Vec<u8>>::new();
        for &(layer, point, color) in pixels {
            let (key, at) = pixel_address(layer, point.map(i64::from));
            tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN])[at..at + 4]
                .copy_from_slice(&color);
        }
        TileSnapshot::from_tiles(tiles).unwrap()
    }

    fn pixel(snapshot: &TileSnapshot, layer: LayerId, point: [i32; 2]) -> [u8; 4] {
        let (key, at) = pixel_address(layer, point.map(i64::from));
        snapshot
            .get(key)
            .map_or([0; 4], |tile| tile.pixels()[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn immutable_affine_preserves_identity_mask_holes_signed_pixels_and_other_layers() {
        // Product risk: repeated preview or an identity transform must not
        // degrade alpha, cut mask holes, or touch unrelated artwork/layers.
        let layer = LayerId(1);
        let tree = tree();
        let before = fixture(&[
            (layer, [-2, -1], [128, 0, 0, 128]),
            (layer, [-1, -1], [0, 255, 0, 255]),
            (layer, [0, -1], [0, 0, 64, 64]),
            (layer, [100, 10], [3, 2, 1, 4]),
            (LayerId(2), [-2, -1], [9, 9, 9, 9]),
        ]);
        // Includes a selected transparent fourth pixel and an unselected
        // opaque middle hole: selection cannot be reconstructed from alpha.
        let mask = SelectionMask::from_packed_bits_at([-2, -1], [4, 1], &[0b1101]).unwrap();
        let limits = EditLimits::default();
        let identity = transform_selection_affine(
            &before,
            &tree,
            layer,
            &mask,
            &AffineTransform::default(),
            limits,
        )
        .unwrap();
        assert_eq!(identity.paint.after, before);
        assert!(identity.paint.changed_tiles.is_empty());
        assert_eq!(identity.selection, mask);
        assert_eq!(
            identity.corners,
            [[-2.0, -1.0], [2.0, -1.0], [2.0, 0.0], [-2.0, 0.0]]
        );
        let translation = AffineTransform {
            offset_milli: [-5000, 3000],
            ..AffineTransform::default()
        };
        let result =
            transform_selection_affine(&before, &tree, layer, &mask, &translation, limits).unwrap();
        assert_eq!(pixel(&result.paint.after, layer, [-2, -1]), [0; 4]);
        assert_eq!(
            pixel(&result.paint.after, layer, [-1, -1]),
            [0, 255, 0, 255]
        );
        assert_eq!(pixel(&result.paint.after, layer, [-7, 2]), [128, 0, 0, 128]);
        assert_eq!(pixel(&result.paint.after, layer, [-5, 2]), [0, 0, 64, 64]);
        assert!(
            result.selection.contains_signed(-4, 2),
            "selected transparent pixel follows geometry"
        );
        assert!(
            !result.selection.contains_signed(-6, 2),
            "opaque unselected hole is not copied"
        );
        assert_eq!(pixel(&result.paint.after, layer, [100, 10]), [3, 2, 1, 4]);
        assert_eq!(
            pixel(&result.paint.after, LayerId(2), [-2, -1]),
            [9, 9, 9, 9]
        );
        let repeated =
            transform_selection_affine(&before, &tree, layer, &mask, &translation, limits).unwrap();
        assert_eq!(repeated.paint.after, result.paint.after);
        assert_eq!(repeated.selection, result.selection);
        assert_eq!(pixel(&before, layer, [-2, -1]), [128, 0, 0, 128]);
    }

    #[test]
    fn affine_filters_premultiplied_alpha_without_dark_fringes_or_clipped_support() {
        // Product risk: filtering straight RGB, clipping filter support or
        // replacing rather than source-over compositing destroys soft edges.
        let layer = LayerId(1);
        let tree = tree();
        let before = fixture(&[
            (layer, [0, 0], [128, 0, 0, 128]),
            (layer, [1, 0], [0, 255, 0, 255]),
        ]);
        let mask = SelectionMask::from_packed_bits(1, 1, &[1]).unwrap();
        let fractional = AffineTransform {
            offset_milli: [500, 0],
            ..AffineTransform::default()
        };
        let result = transform_selection_affine(
            &before,
            &tree,
            layer,
            &mask,
            &fractional,
            EditLimits::default(),
        )
        .unwrap();
        assert_eq!(pixel(&result.paint.after, layer, [0, 0]), [64, 0, 0, 64]);
        assert_eq!(pixel(&result.paint.after, layer, [1, 0]), [64, 191, 0, 255]);
        assert_eq!(result.selection.selected_pixels(), 2);
        let source = fixture(&[(layer, [0, 0], [128, 0, 0, 128])]);
        let scaled = AffineTransform {
            scale_ppm: [2_000_000, 1_000_000],
            ..AffineTransform::default()
        };
        let result = transform_selection_affine(
            &source,
            &tree,
            layer,
            &mask,
            &scaled,
            EditLimits::default(),
        )
        .unwrap();
        assert_eq!(pixel(&result.paint.after, layer, [-1, 0]), [64, 0, 0, 64]);
        assert_eq!(pixel(&result.paint.after, layer, [0, 0]), [128, 0, 0, 128]);
        assert_eq!(pixel(&result.paint.after, layer, [1, 0]), [64, 0, 0, 64]);
        assert!(result.selection.contains_signed(-1, 0));
        assert!(result.selection.contains_signed(1, 0));
        for (angle, flip_x, flip_y) in [
            (27_000, false, false),
            (-27_000, true, false),
            (90_000, false, true),
        ] {
            let transform = AffineTransform {
                offset_milli: [-5250, 125],
                scale_ppm: [1_700_000, 800_000],
                rotation_millidegrees: angle,
                flip_x,
                flip_y,
            };
            let result = transform_selection_affine(
                &source,
                &tree,
                layer,
                &mask,
                &transform,
                EditLimits::default(),
            )
            .unwrap();
            assert!(result.selection.selected_pixels() > 0);
            assert!(result.corners.iter().all(|p| p[0] < 0.0));
            assert_eq!(pixel(&result.paint.after, layer, [0, 0]), [0; 4]);
            let mut partial = false;
            for (key, tile) in result.paint.after.iter() {
                for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
                    assert_eq!(
                        pixel[0], pixel[3],
                        "pure red must keep RGB/alpha ratio after filtering"
                    );
                    assert_eq!(&pixel[1..3], &[0, 0]);
                    if pixel[3] > 0 {
                        partial |= pixel[3] < 128;
                        let (ox, oy) = key.pixel_origin();
                        let x = i32::try_from(ox + i64::try_from(index % 128).unwrap()).unwrap();
                        let y = i32::try_from(oy + i64::try_from(index / 128).unwrap()).unwrap();
                        assert!(
                            result.selection.contains_signed(x, y),
                            "AA fringe must remain selected for the next move"
                        );
                    }
                }
            }
            assert!(partial);
        }
    }

    #[test]
    fn affine_preflight_rejects_oversized_or_invalid_edits_without_cutting_artwork() {
        // Product risk: a failed scale/translate or memory-limit rejection
        // must not leave the selected source cut or allow unbounded work.
        let layer = LayerId(1);
        let mut tree = tree();
        let before = fixture(&[(layer, [0, 0], [128, 0, 0, 128])]);
        let mask = SelectionMask::from_packed_bits(1, 1, &[1]).unwrap();
        let identity = AffineTransform::default();
        let translation = AffineTransform {
            offset_milli: [1000, 0],
            ..identity
        };
        for (transform, limits) in [
            (
                AffineTransform {
                    scale_ppm: [0, 1_000_000],
                    ..identity
                },
                EditLimits::default(),
            ),
            (
                AffineTransform {
                    offset_milli: [i64::MAX, 0],
                    ..identity
                },
                EditLimits::default(),
            ),
            (
                AffineTransform {
                    scale_ppm: [u32::MAX, u32::MAX],
                    ..identity
                },
                EditLimits::default(),
            ),
            (
                translation,
                EditLimits {
                    max_pixels: 1,
                    ..EditLimits::default()
                },
            ),
            (
                translation,
                EditLimits {
                    max_workspace_bytes: 1024,
                    ..EditLimits::default()
                },
            ),
            (
                translation,
                EditLimits {
                    max_polygon_work: 1,
                    ..EditLimits::default()
                },
            ),
            (
                identity,
                EditLimits {
                    max_workspace_bytes: 1,
                    ..EditLimits::default()
                },
            ),
        ] {
            assert!(
                transform_selection_affine(&before, &tree, layer, &mask, &transform, limits)
                    .is_err()
            );
            assert_eq!(pixel(&before, layer, [0, 0]), [128, 0, 0, 128]);
        }
        let wide_mask = SelectionMask::from_packed_bits(2, 1, &[3]).unwrap();
        let subpixel = AffineTransform {
            scale_ppm: [1, 1],
            ..identity
        };
        assert!(matches!(
            transform_selection_affine(
                &before,
                &tree,
                layer,
                &wide_mask,
                &subpixel,
                EditLimits::default()
            ),
            Err(EditError::InvalidTransform)
        ));
        assert_eq!(pixel(&before, layer, [0, 0]), [128, 0, 0, 128]);
        tree.set_locked(layer, true).unwrap();
        assert!(matches!(
            transform_selection_affine(
                &before,
                &tree,
                layer,
                &mask,
                &translation,
                EditLimits::default()
            ),
            Err(EditError::LockedLayer)
        ));
    }
}

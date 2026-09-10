//! Clipboard-neutral, bounded fragments of canonical artwork pixels.
use crate::transform::{Replacements, pixel_address};
use crate::{EditError, EditLimits, SelectionMask, SelectionPaintResult, composite_surface};
use nyatidraw_api::LayerId;
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::TileSnapshot;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RasterFragment {
    origin: [i32; 2],
    size: [u32; 2],
    pixels: Vec<u8>,
}

impl RasterFragment {
    /// # Errors
    /// Rejects unbounded, malformed or non-premultiplied clipboard data.
    pub fn new(origin: [i32; 2], size: [u32; 2], pixels: Vec<u8>) -> Result<Self, EditError> {
        let length = Self::byte_len(origin, size)?;
        if pixels.len() != length
            || pixels
                .chunks_exact(4)
                .any(|c| c[..3].iter().any(|v| *v > c[3]))
        {
            return Err(EditError::InvalidPaint);
        }
        Ok(Self {
            origin,
            size,
            pixels,
        })
    }

    /// Checks dimensions before an adapter allocates or copies untrusted bytes.
    /// # Errors
    /// Rejects empty/oversized extents and signed coordinate overflow.
    pub fn byte_len(origin: [i32; 2], size: [u32; 2]) -> Result<usize, EditError> {
        let count = u64::from(size[0]) * u64::from(size[1]);
        if size.contains(&0) || count > EditLimits::default().max_pixels {
            return Err(EditError::LimitExceeded);
        }
        for axis in 0..2 {
            if i32::try_from(i64::from(origin[axis]) + i64::from(size[axis]) - 1).is_err() {
                return Err(EditError::CoordinateOutOfRange);
            }
        }
        usize::try_from(count * 4).map_err(|_| EditError::LimitExceeded)
    }

    #[must_use]
    pub const fn origin(&self) -> [i32; 2] {
        self.origin
    }
    #[must_use]
    pub const fn size(&self) -> [u32; 2] {
        self.size
    }
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Copies only selected active-raster pixels. Transparent holes stay holes;
/// layer visibility/opacity and viewport decorations are not sampled.
/// # Errors
/// Rejects an absent/empty selection, unknown layer or exceeded resource limits.
pub fn copy_selection(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    layer: LayerId,
    mask: &SelectionMask,
    limits: EditLimits,
) -> Result<RasterFragment, EditError> {
    tree.raster(layer).ok_or(EditError::UnknownLayer)?;
    let limits = limits.bounded();
    let (origin, [width, height]) = mask.bounds_signed().ok_or(EditError::InvalidMask)?;
    let length = RasterFragment::byte_len(origin, [width, height])?;
    if length as u64 / 4 > limits.max_pixels || length as u64 * 2 > limits.max_workspace_bytes {
        return Err(EditError::LimitExceeded);
    }
    let mut pixels = vec![0; length];
    for y in 0..height {
        for x in 0..width {
            let point = [
                i64::from(origin[0]) + i64::from(x),
                i64::from(origin[1]) + i64::from(y),
            ];
            if !mask.contains_signed(
                i32::try_from(point[0]).map_err(|_| EditError::CoordinateOutOfRange)?,
                i32::try_from(point[1]).map_err(|_| EditError::CoordinateOutOfRange)?,
            ) {
                continue;
            }
            let (key, offset) = pixel_address(layer, point);
            if let Some(tile) = snapshot.get(key) {
                let to = (y as usize * width as usize + x as usize) * 4;
                pixels[to..to + 4].copy_from_slice(&tile.pixels()[offset..offset + 4]);
            }
        }
    }
    RasterFragment::new(origin, [width, height], pixels)
}

/// Computes a cut without publishing it; the caller must successfully place
/// the copy on the clipboard before durably accepting these replacements.
/// # Errors
/// Rejects locked/missing layers, empty masks and resource exhaustion atomically.
pub fn cut_selection(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    layer: LayerId,
    mask: &SelectionMask,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    if tree.raster(layer).ok_or(EditError::UnknownLayer)?.locked {
        return Err(EditError::LockedLayer);
    }
    if tree
        .raster(layer)
        .ok_or(EditError::UnknownLayer)?
        .alpha_locked
    {
        return Err(EditError::AlphaLockedLayer);
    }
    let (origin, [width, height]) = mask.bounds_signed().ok_or(EditError::InvalidMask)?;
    let limits = limits.bounded();
    if u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    let mut replacements = Replacements {
        before: snapshot,
        tiles: BTreeMap::new(),
        baseline_bytes: snapshot.len() as u64 * 256 + u64::from(width) * u64::from(height) * 8,
        limits,
    };
    for y in 0..height {
        for x in 0..width {
            let point = [
                i64::from(origin[0]) + i64::from(x),
                i64::from(origin[1]) + i64::from(y),
            ];
            if !mask.contains_signed(
                i32::try_from(point[0]).map_err(|_| EditError::CoordinateOutOfRange)?,
                i32::try_from(point[1]).map_err(|_| EditError::CoordinateOutOfRange)?,
            ) {
                continue;
            }
            let (key, offset) = pixel_address(layer, point);
            if snapshot
                .get(key)
                .is_some_and(|tile| tile.pixels()[offset..offset + 4] != [0; 4])
            {
                replacements.tile(key)?[offset..offset + 4].fill(0);
            }
        }
    }
    replacements.finish()
}

/// Source-over pastes at the fragment's signed origin without page clipping.
/// # Errors
/// Rejects locked/missing targets and memory exhaustion without partial edits.
pub fn paste_fragment(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    layer: LayerId,
    fragment: &RasterFragment,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    if tree.raster(layer).ok_or(EditError::UnknownLayer)?.locked {
        return Err(EditError::LockedLayer);
    }
    if tree
        .raster(layer)
        .ok_or(EditError::UnknownLayer)?
        .alpha_locked
    {
        return Err(EditError::AlphaLockedLayer);
    }
    let limits = limits.bounded();
    if fragment.pixels.len() as u64 / 4 > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    let mut replacements = Replacements {
        before: snapshot,
        tiles: BTreeMap::new(),
        baseline_bytes: snapshot.len() as u64 * 256 + fragment.pixels.len() as u64 * 2,
        limits,
    };
    for y in 0..fragment.size[1] {
        for x in 0..fragment.size[0] {
            let offset = (y as usize * fragment.size[0] as usize + x as usize) * 4;
            let color = &fragment.pixels[offset..offset + 4];
            if color[3] == 0 {
                continue;
            }
            let point = [
                i64::from(fragment.origin[0]) + i64::from(x),
                i64::from(fragment.origin[1]) + i64::from(y),
            ];
            let (key, offset) = pixel_address(layer, point);
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
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey};

    #[test]
    fn fragments_preserve_holes_alpha_other_artwork_and_fail_atomically() {
        // Product risk: cut/paste must not clear mask holes, clip signed pixels,
        // corrupt alpha, mutate locked layers or expose partial limited edits.
        let mut tree = LayerTree::new(GroupNode {
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
        .unwrap();
        let key = |layer, x| TileKey {
            layer: LayerId(layer),
            mip: 0,
            x,
            y: 0,
        };
        let mut pixels = vec![0; TILE_BYTE_LEN];
        pixels[..12].copy_from_slice(&[128, 0, 0, 128, 0, 255, 0, 255, 0, 0, 64, 64]);
        let before =
            TileSnapshot::from_tiles([(key(1, 0), pixels), (key(2, 0), vec![64; TILE_BYTE_LEN])])
                .unwrap();
        let mask = SelectionMask::from_packed_bits(3, 1, &[0b101]).unwrap();
        let limits = EditLimits::default();
        let fragment = copy_selection(&before, &tree, LayerId(1), &mask, limits).unwrap();
        assert_eq!(
            fragment.pixels(),
            &[128, 0, 0, 128, 0, 0, 0, 0, 0, 0, 64, 64]
        );
        let cut = cut_selection(&before, &tree, LayerId(1), &mask, limits).unwrap();
        assert_eq!(
            &cut.after.get(key(1, 0)).unwrap().pixels()[..12],
            &[0, 0, 0, 0, 0, 255, 0, 255, 0, 0, 0, 0]
        );
        assert_eq!(cut.after.get(key(2, 0)), before.get(key(2, 0)));
        let restored = paste_fragment(&cut.after, &tree, LayerId(1), &fragment, limits).unwrap();
        assert_eq!(restored.after, before);
        let signed =
            RasterFragment::new([-1, 0], fragment.size(), fragment.pixels().to_vec()).unwrap();
        let outside =
            paste_fragment(&TileSnapshot::default(), &tree, LayerId(1), &signed, limits).unwrap();
        assert_eq!(
            &outside.after.get(key(1, -1)).unwrap().pixels()[508..512],
            &[128, 0, 0, 128]
        );
        for locked in [false, true] {
            tree.set_locked(LayerId(1), locked).unwrap();
            let tiny = EditLimits {
                max_workspace_bytes: 1,
                ..limits
            };
            assert!(paste_fragment(&before, &tree, LayerId(1), &fragment, tiny).is_err());
            assert!(cut_selection(&before, &tree, LayerId(1), &mask, tiny).is_err());
        }
        assert!(copy_selection(&before, &tree, LayerId(1), &mask, limits).is_ok());
        for (origin, size, bytes) in [
            ([0, 0], [0, 1], vec![]),
            ([i32::MAX, 0], [2, 1], vec![0; 8]),
            ([0, 0], [1, 1], vec![255, 0, 0, 1]),
            ([0, 0], [u32::MAX, u32::MAX], vec![]),
        ] {
            assert!(RasterFragment::new(origin, size, bytes).is_err());
        }
    }
}

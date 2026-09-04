#![forbid(unsafe_code)]

//! Immutable, content-addressed RGBA8 tiles.

use std::{collections::BTreeMap, sync::Arc};

use nyatidraw_api::{
    CanvasSpec, CanvasSpecError, CompositeInvalidation, CompositeTileKey, ContentRootId, LayerId,
    TileCoordinate,
};

pub const TILE_EDGE: u32 = 128;
pub const TILE_BYTE_LEN: usize = TILE_EDGE as usize * TILE_EDGE as usize * 4;
const TILE_EDGE_USIZE: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileKey {
    pub layer: LayerId,
    pub mip: u8,
    pub x: i32,
    pub y: i32,
}

impl TileKey {
    /// Maps a signed pixel coordinate to its containing mip-zero tile.
    ///
    /// # Panics
    ///
    /// Panics when `tile_edge` is not positive.
    #[must_use]
    pub fn from_pixel(layer: LayerId, tile_edge: i32, x: i32, y: i32) -> Self {
        assert!(tile_edge > 0, "tile edge must be positive");
        Self {
            layer,
            mip: 0,
            x: x.div_euclid(tile_edge),
            y: y.div_euclid(tile_edge),
        }
    }

    #[must_use]
    pub const fn pixel_origin(self) -> (i64, i64) {
        (
            self.x as i64 * TILE_EDGE as i64,
            self.y as i64 * TILE_EDGE as i64,
        )
    }

    #[must_use]
    pub const fn coordinate(self) -> TileCoordinate {
        TileCoordinate {
            mip: self.mip,
            x: self.x,
            y: self.y,
        }
    }
}

/// Backend-neutral index of cached group-composite tiles.
///
/// Values may be CPU metadata or renderer-owned opaque handles. The cache
/// itself only applies document invalidation keys and never interprets pixels.
#[derive(Clone, Debug, Default)]
pub struct CompositeCache<Value> {
    entries: BTreeMap<CompositeTileKey, Value>,
}

impl<Value> CompositeCache<Value> {
    pub fn insert(&mut self, key: CompositeTileKey, value: Value) -> Option<Value> {
        self.entries.insert(key, value)
    }

    #[must_use]
    pub fn get(&self, key: CompositeTileKey) -> Option<&Value> {
        self.entries.get(&key)
    }

    /// Removes one exact cached coordinate.
    ///
    /// Renderer residency managers use this when a bounded GPU atlas evicts a
    /// composite slot. The semantic invalidation contract remains unchanged.
    pub fn remove(&mut self, key: CompositeTileKey) -> Option<Value> {
        self.entries.remove(&key)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Applies exact-coordinate and whole-group invalidations in one pass.
    ///
    /// Returns the number of actual cache entries removed.
    pub fn apply_invalidation(&mut self, invalidation: &CompositeInvalidation) -> usize {
        let before = self.entries.len();
        for key in invalidation.exact_tiles() {
            self.entries.remove(&key);
        }
        let all_groups: std::collections::BTreeSet<_> = invalidation.all_tiles().collect();
        if !all_groups.is_empty() {
            self.entries
                .retain(|key, _| !all_groups.contains(&key.group));
        }
        before - self.entries.len()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectHash(pub [u8; 32]);

impl ObjectHash {
    #[must_use]
    pub fn digest_tagged(tag: &[u8], payload: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(tag.len() as u64).to_le_bytes());
        hasher.update(tag);
        hasher.update(&(payload.len() as u64).to_le_bytes());
        hasher.update(payload);
        Self(*hasher.finalize().as_bytes())
    }

    #[must_use]
    pub const fn content_root_id(self) -> ContentRootId {
        let mut bytes = [0_u8; 16];
        let mut index = 0;
        while index < bytes.len() {
            bytes[index] = self.0[index];
            index += 1;
        }
        ContentRootId(u128::from_le_bytes(bytes))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentRoot {
    pub id: ContentRootId,
    pub hash: ObjectHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TileBounds {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x_exclusive: i32,
    pub max_y_exclusive: i32,
}

impl TileBounds {
    #[must_use]
    pub fn from_keys(keys: impl IntoIterator<Item = TileKey>) -> Option<Self> {
        let mut keys = keys.into_iter();
        let first = keys.next()?;
        let mut bounds = Self {
            min_x: first.x,
            min_y: first.y,
            max_x_exclusive: first.x.saturating_add(1),
            max_y_exclusive: first.y.saturating_add(1),
        };
        for key in keys {
            bounds.min_x = bounds.min_x.min(key.x);
            bounds.min_y = bounds.min_y.min(key.y);
            bounds.max_x_exclusive = bounds.max_x_exclusive.max(key.x.saturating_add(1));
            bounds.max_y_exclusive = bounds.max_y_exclusive.max(key.y.saturating_add(1));
        }
        Some(bounds)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TileSnapshotError {
    InvalidByteLength { actual: usize },
    DuplicateKey(TileKey),
}

/// A finite, tightly packed RGBA8 export surface assembled from one base-mip layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlattenedRgba8 {
    /// Signed document-pixel origin of the top-left output pixel.
    pub origin_x: i64,
    /// Signed document-pixel origin of the top-left output pixel.
    pub origin_y: i64,
    pub width: u32,
    pub height: u32,
    /// Top-left-origin premultiplied linear-light RGBA8 pixels.
    pub pixels: Vec<u8>,
}

impl FlattenedRgba8 {
    /// Stable identity for the export pixels and their signed document bounds.
    #[must_use]
    pub fn hash(&self) -> ObjectHash {
        let mut payload = Vec::with_capacity(self.pixels.len().saturating_add(24));
        payload.extend_from_slice(&self.origin_x.to_le_bytes());
        payload.extend_from_slice(&self.origin_y.to_le_bytes());
        payload.extend_from_slice(&self.width.to_le_bytes());
        payload.extend_from_slice(&self.height.to_le_bytes());
        payload.extend_from_slice(&self.pixels);
        ObjectHash::digest_tagged(b"nyatidraw-flattened-rgba8-linear-premul-v1", &payload)
    }
}

/// Failure to flatten a signed tile map into a bounded RGBA8 surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlattenError {
    InvalidCanvas(CanvasSpecError),
    UnsupportedMip { mip: u8 },
    DimensionsTooLarge { width: u64, height: u64 },
    PixelLimitExceeded { pixels: u64, max_pixels: u64 },
}

/// Largest flattened output accepted by [`TileSnapshot::flatten_base_layer_rgba8`].
///
/// The fixed 64 Mi-pixel limit caps the raw allocation at 256 MiB before any
/// encoder buffers are created.
pub const MAX_FLATTENED_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileObject {
    hash: ObjectHash,
    pixels: Arc<[u8]>,
}

impl TileObject {
    /// Creates one immutable premultiplied linear-light RGBA8 tile.
    ///
    /// # Errors
    ///
    /// Returns an error unless `pixels` contains exactly one 128x128 RGBA8 tile.
    pub fn new(pixels: Vec<u8>) -> Result<Self, TileSnapshotError> {
        if pixels.len() != TILE_BYTE_LEN {
            return Err(TileSnapshotError::InvalidByteLength {
                actual: pixels.len(),
            });
        }
        let hash = ObjectHash::digest_tagged(b"nyatidraw-tile-rgba8-linear-premul-v1", &pixels);
        Ok(Self {
            hash,
            pixels: pixels.into(),
        })
    }

    #[must_use]
    pub const fn hash(&self) -> ObjectHash {
        self.hash
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Canonical immutable tile map. Fully transparent tiles are omitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileSnapshot {
    root: ContentRoot,
    tiles: BTreeMap<TileKey, Arc<TileObject>>,
}

impl TileSnapshot {
    #[must_use]
    pub fn empty() -> Self {
        Self::from_canonical_tiles(BTreeMap::new())
    }

    /// Constructs a canonical snapshot from tightly packed tile pixels.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate keys or invalid tile byte lengths.
    pub fn from_tiles(
        tiles: impl IntoIterator<Item = (TileKey, Vec<u8>)>,
    ) -> Result<Self, TileSnapshotError> {
        Self::empty().with_replacements(tiles)
    }

    /// Replaces tiles while structurally sharing every unchanged object.
    /// Transparent replacements remove their key from the canonical root.
    ///
    /// # Errors
    ///
    /// Returns an error for duplicate replacement keys or invalid tile byte lengths.
    pub fn with_replacements(
        &self,
        replacements: impl IntoIterator<Item = (TileKey, Vec<u8>)>,
    ) -> Result<Self, TileSnapshotError> {
        let mut tiles = self.tiles.clone();
        let mut seen = std::collections::BTreeSet::new();
        for (key, pixels) in replacements {
            if !seen.insert(key) {
                return Err(TileSnapshotError::DuplicateKey(key));
            }
            if pixels.len() != TILE_BYTE_LEN {
                return Err(TileSnapshotError::InvalidByteLength {
                    actual: pixels.len(),
                });
            }
            if pixels.iter().all(|byte| *byte == 0) {
                tiles.remove(&key);
            } else {
                tiles.insert(key, Arc::new(TileObject::new(pixels)?));
            }
        }
        Ok(Self::from_canonical_tiles(tiles))
    }

    #[must_use]
    pub const fn root(&self) -> ContentRoot {
        self.root
    }

    #[must_use]
    pub fn get(&self, key: TileKey) -> Option<&TileObject> {
        self.tiles.get(&key).map(Arc::as_ref)
    }

    pub fn iter(&self) -> impl Iterator<Item = (TileKey, &TileObject)> {
        self.tiles
            .iter()
            .map(|(key, object)| (*key, object.as_ref()))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Flattens one mip-zero layer into its finite signed-tile bounds.
    ///
    /// Other layers are intentionally not composed here; Sprint 2 has no layer
    /// ordering or blend-mode contract yet. An empty layer becomes a 1x1
    /// transparent surface at the document origin so a headless empty-project
    /// export remains finite.
    ///
    /// # Errors
    ///
    /// Returns an error rather than allocating when a requested layer has a
    /// non-base mip, dimensions exceed `u32`, or its output exceeds the fixed
    /// flattening limit.
    pub fn flatten_base_layer_rgba8(&self, layer: LayerId) -> Result<FlattenedRgba8, FlattenError> {
        let layer_keys: Vec<_> = self
            .tiles
            .keys()
            .copied()
            .filter(|key| key.layer == layer)
            .collect();
        if let Some(key) = layer_keys.iter().find(|key| key.mip != 0) {
            return Err(FlattenError::UnsupportedMip { mip: key.mip });
        }
        let Some(first) = layer_keys.first() else {
            return Ok(FlattenedRgba8 {
                origin_x: 0,
                origin_y: 0,
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            });
        };

        let min_tile_x = layer_keys.iter().map(|key| key.x).min().unwrap_or(first.x);
        let min_tile_y = layer_keys.iter().map(|key| key.y).min().unwrap_or(first.y);
        let max_tile_x = layer_keys.iter().map(|key| key.x).max().unwrap_or(first.x);
        let max_tile_y = layer_keys.iter().map(|key| key.y).max().unwrap_or(first.y);
        let tile_width =
            u64::try_from(i64::from(max_tile_x) - i64::from(min_tile_x) + 1).map_err(|_| {
                FlattenError::DimensionsTooLarge {
                    width: u64::MAX,
                    height: u64::MAX,
                }
            })?;
        let tile_height = u64::try_from(i64::from(max_tile_y) - i64::from(min_tile_y) + 1)
            .map_err(|_| FlattenError::DimensionsTooLarge {
                width: u64::MAX,
                height: u64::MAX,
            })?;
        let width = tile_width.saturating_mul(u64::from(TILE_EDGE));
        let height = tile_height.saturating_mul(u64::from(TILE_EDGE));
        let width =
            u32::try_from(width).map_err(|_| FlattenError::DimensionsTooLarge { width, height })?;
        let height = u32::try_from(height).map_err(|_| FlattenError::DimensionsTooLarge {
            width: u64::from(width),
            height,
        })?;
        let pixel_count = u64::from(width).saturating_mul(u64::from(height));
        if pixel_count > MAX_FLATTENED_PIXELS {
            return Err(FlattenError::PixelLimitExceeded {
                pixels: pixel_count,
                max_pixels: MAX_FLATTENED_PIXELS,
            });
        }
        let byte_count = usize::try_from(pixel_count.saturating_mul(4)).map_err(|_| {
            FlattenError::PixelLimitExceeded {
                pixels: pixel_count,
                max_pixels: MAX_FLATTENED_PIXELS,
            }
        })?;
        let mut pixels = vec![0; byte_count];
        let output_width =
            usize::try_from(width).map_err(|_| FlattenError::DimensionsTooLarge {
                width: u64::from(width),
                height: u64::from(height),
            })?;
        let origin_x = i64::from(min_tile_x) * i64::from(TILE_EDGE);
        let origin_y = i64::from(min_tile_y) * i64::from(TILE_EDGE);

        for key in layer_keys {
            let Some(tile) = self.tiles.get(&key) else {
                continue;
            };
            let destination_x =
                usize::try_from(i64::from(key.x) - i64::from(min_tile_x)).map_err(|_| {
                    FlattenError::DimensionsTooLarge {
                        width: u64::from(width),
                        height: u64::from(height),
                    }
                })? * TILE_EDGE_USIZE;
            let destination_y =
                usize::try_from(i64::from(key.y) - i64::from(min_tile_y)).map_err(|_| {
                    FlattenError::DimensionsTooLarge {
                        width: u64::from(width),
                        height: u64::from(height),
                    }
                })? * TILE_EDGE_USIZE;
            for row in 0..TILE_EDGE_USIZE {
                let source_start = row * TILE_EDGE_USIZE * 4;
                let destination_start = ((destination_y + row) * output_width + destination_x) * 4;
                let row_bytes = TILE_EDGE_USIZE * 4;
                pixels[destination_start..destination_start + row_bytes]
                    .copy_from_slice(&tile.pixels()[source_start..source_start + row_bytes]);
            }
        }
        Ok(FlattenedRgba8 {
            origin_x,
            origin_y,
            width,
            height,
            pixels,
        })
    }

    /// Crops one base-mip layer to the finite document canvas.
    ///
    /// The result always has origin `(0, 0)` and exactly the canvas dimensions.
    /// Signed sparse tiles wholly or partly outside `[0, width) x [0, height)`
    /// cannot expand or otherwise affect the output outside their intersection.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid canvas, any non-base tile on `layer`,
    /// or a canvas too large for the bounded export allocation.
    pub fn crop_base_layer_rgba8_to_canvas(
        &self,
        layer: LayerId,
        canvas: CanvasSpec,
    ) -> Result<FlattenedRgba8, FlattenError> {
        let canvas = canvas.validate().map_err(FlattenError::InvalidCanvas)?;
        if let Some(key) = self
            .tiles
            .keys()
            .find(|key| key.layer == layer && key.mip != 0)
        {
            return Err(FlattenError::UnsupportedMip { mip: key.mip });
        }

        let width = canvas.width_px;
        let height = canvas.height_px;
        let pixel_count = u64::from(width) * u64::from(height);
        if pixel_count > MAX_FLATTENED_PIXELS {
            return Err(FlattenError::PixelLimitExceeded {
                pixels: pixel_count,
                max_pixels: MAX_FLATTENED_PIXELS,
            });
        }
        let byte_count = usize::try_from(pixel_count.checked_mul(4).ok_or(
            FlattenError::PixelLimitExceeded {
                pixels: pixel_count,
                max_pixels: MAX_FLATTENED_PIXELS,
            },
        )?)
        .map_err(|_| FlattenError::PixelLimitExceeded {
            pixels: pixel_count,
            max_pixels: MAX_FLATTENED_PIXELS,
        })?;
        let output_width =
            usize::try_from(width).map_err(|_| FlattenError::DimensionsTooLarge {
                width: u64::from(width),
                height: u64::from(height),
            })?;
        let mut pixels = vec![0; byte_count];
        let canvas_right = i64::from(width);
        let canvas_bottom = i64::from(height);

        for (key, tile) in self.iter().filter(|(key, _)| key.layer == layer) {
            let tile_left = i64::from(key.x) * i64::from(TILE_EDGE);
            let tile_top = i64::from(key.y) * i64::from(TILE_EDGE);
            let tile_right = tile_left + i64::from(TILE_EDGE);
            let tile_bottom = tile_top + i64::from(TILE_EDGE);
            let left = tile_left.max(0);
            let top = tile_top.max(0);
            let right = tile_right.min(canvas_right);
            let bottom = tile_bottom.min(canvas_bottom);
            if left >= right || top >= bottom {
                continue;
            }

            let dimensions_error = || FlattenError::DimensionsTooLarge {
                width: u64::from(width),
                height: u64::from(height),
            };
            let source_x = usize::try_from(left - tile_left).map_err(|_| dimensions_error())?;
            let source_y = usize::try_from(top - tile_top).map_err(|_| dimensions_error())?;
            let destination_x = usize::try_from(left).map_err(|_| dimensions_error())?;
            let destination_y = usize::try_from(top).map_err(|_| dimensions_error())?;
            let copy_width = usize::try_from(right - left).map_err(|_| dimensions_error())?;
            let copy_height = usize::try_from(bottom - top).map_err(|_| dimensions_error())?;
            for row in 0..copy_height {
                let source_start = ((source_y + row) * TILE_EDGE_USIZE + source_x) * 4;
                let destination_start = ((destination_y + row) * output_width + destination_x) * 4;
                let row_bytes = copy_width * 4;
                pixels[destination_start..destination_start + row_bytes]
                    .copy_from_slice(&tile.pixels()[source_start..source_start + row_bytes]);
            }
        }

        Ok(FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width,
            height,
            pixels,
        })
    }

    fn from_canonical_tiles(tiles: BTreeMap<TileKey, Arc<TileObject>>) -> Self {
        let hash = hash_root(&tiles);
        Self {
            root: ContentRoot {
                id: hash.content_root_id(),
                hash,
            },
            tiles,
        }
    }
}

impl Default for TileSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

fn hash_root(tiles: &BTreeMap<TileKey, Arc<TileObject>>) -> ObjectHash {
    let mut payload = Vec::with_capacity(tiles.len().saturating_mul(57).saturating_add(8));
    payload.extend_from_slice(&TILE_EDGE.to_le_bytes());
    payload.extend_from_slice(&(tiles.len() as u64).to_le_bytes());
    for (key, object) in tiles {
        payload.extend_from_slice(&key.layer.0.to_le_bytes());
        payload.push(key.mip);
        payload.extend_from_slice(&key.x.to_le_bytes());
        payload.extend_from_slice(&key.y.to_le_bytes());
        payload.extend_from_slice(&object.hash.0);
    }
    ObjectHash::digest_tagged(b"nyatidraw-content-root-v1", &payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_pixels_use_euclidean_tiles() {
        let layer = LayerId(7);
        assert_eq!(
            TileKey::from_pixel(layer, 128, -1, -129),
            TileKey {
                layer,
                mip: 0,
                x: -1,
                y: -2
            }
        );
    }

    #[test]
    fn flattening_signed_tiles_preserves_bounds_pixels_and_hash() {
        let layer = LayerId(7);
        let mut left = vec![0; TILE_BYTE_LEN];
        left[..4].copy_from_slice(&[12, 6, 3, 16]);
        let mut right = vec![0; TILE_BYTE_LEN];
        right[..4].copy_from_slice(&[24, 12, 6, 32]);
        let snapshot = TileSnapshot::from_tiles([
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: -1,
                    y: 0,
                },
                left,
            ),
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                right,
            ),
        ])
        .expect("valid fixture tiles");

        let flattened = snapshot
            .flatten_base_layer_rgba8(layer)
            .expect("bounded base-mip layer");
        assert_eq!((flattened.origin_x, flattened.origin_y), (-128, 0));
        assert_eq!((flattened.width, flattened.height), (256, 128));
        assert_eq!(&flattened.pixels[..4], &[12, 6, 3, 16]);
        assert_eq!(&flattened.pixels[128 * 4..128 * 4 + 4], &[24, 12, 6, 32]);
        assert_eq!(
            flattened.hash(),
            ObjectHash([
                26, 218, 28, 50, 52, 58, 79, 86, 227, 69, 127, 42, 181, 136, 17, 146, 69, 208, 42,
                71, 166, 224, 223, 113, 36, 92, 141, 55, 188, 141, 72, 203,
            ])
        );
    }

    #[test]
    fn finite_canvas_crop_ignores_signed_tiles_outside_its_half_open_bounds() {
        // Product risk: sparse tiles outside the document must never change the
        // finite export size or pixels, even when their coordinates are negative.
        let layer = LayerId(7);
        let mut inside = vec![0; TILE_BYTE_LEN];
        inside[..4].copy_from_slice(&[12, 6, 3, 16]);
        let mut outside = vec![0; TILE_BYTE_LEN];
        outside[..4].copy_from_slice(&[250, 125, 63, 255]);
        let baseline = TileSnapshot::from_tiles([(
            TileKey {
                layer,
                mip: 0,
                x: 0,
                y: 0,
            },
            inside.clone(),
        )])
        .expect("valid baseline tile");
        let with_outside = TileSnapshot::from_tiles([
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                inside,
            ),
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: -1,
                    y: 0,
                },
                outside.clone(),
            ),
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: 1,
                    y: 0,
                },
                outside.clone(),
            ),
            (
                TileKey {
                    layer,
                    mip: 0,
                    x: 0,
                    y: 1,
                },
                outside,
            ),
        ])
        .expect("valid signed sparse tiles");
        let canvas = CanvasSpec {
            width_px: TILE_EDGE,
            height_px: TILE_EDGE,
            pixels_per_inch: 96,
        };

        let expected = baseline
            .crop_base_layer_rgba8_to_canvas(layer, canvas)
            .expect("crop baseline");
        let actual = with_outside
            .crop_base_layer_rgba8_to_canvas(layer, canvas)
            .expect("crop signed sparse tiles");
        assert_eq!((actual.origin_x, actual.origin_y), (0, 0));
        assert_eq!((actual.width, actual.height), (TILE_EDGE, TILE_EDGE));
        assert_eq!(actual, expected);
    }
}

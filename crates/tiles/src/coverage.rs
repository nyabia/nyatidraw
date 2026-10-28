use super::{ObjectHash, TILE_EDGE, TileKey, TileObject, TileSnapshot, TileSnapshotError};
use nyatidraw_api::{MaskId, TileCoordinate};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const COVERAGE_TILE_BYTE_LEN: usize = TILE_EDGE as usize * TILE_EDGE as usize;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoverageTileKey {
    pub mask: MaskId,
    pub mip: u8,
    pub x: i32,
    pub y: i32,
}

impl CoverageTileKey {
    /// # Panics
    /// Requires a positive tile edge.
    #[must_use]
    pub fn from_pixel(mask: MaskId, tile_edge: i32, x: i32, y: i32) -> Self {
        assert!(tile_edge > 0);
        Self {
            mask,
            mip: 0,
            x: x.div_euclid(tile_edge),
            y: y.div_euclid(tile_edge),
        }
    }
    #[must_use]
    pub const fn coordinate(self) -> TileCoordinate {
        TileCoordinate {
            mip: self.mip,
            x: self.x,
            y: self.y,
        }
    }
    #[must_use]
    pub const fn pixel_origin(self) -> (i64, i64) {
        (
            self.x as i64 * TILE_EDGE as i64,
            self.y as i64 * TILE_EDGE as i64,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverageTileObject {
    hash: ObjectHash,
    pixels: Arc<[u8]>,
}

impl CoverageTileObject {
    /// # Errors
    /// Requires exactly one signed-coordinate R8 coverage tile.
    pub fn new(pixels: Vec<u8>) -> Result<Self, TileSnapshotError> {
        if pixels.len() != COVERAGE_TILE_BYTE_LEN {
            return Err(TileSnapshotError::InvalidByteLength {
                actual: pixels.len(),
            });
        }
        let hash = ObjectHash::digest_tagged(b"nyatidraw-tile-coverage-r8-v1", &pixels);
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

impl TileSnapshot {
    /// # Errors
    /// Rejects duplicate keys, zero mask identities and unsupported coverage mips.
    pub fn from_planes(
        images: impl IntoIterator<Item = (TileKey, TileObject)>,
        masks: impl IntoIterator<Item = (CoverageTileKey, CoverageTileObject)>,
    ) -> Result<Self, TileSnapshotError> {
        let images = Self::from_objects(images)?;
        let mut coverage = BTreeMap::new();
        for (key, object) in masks {
            validate_key(key)?;
            if coverage.insert(key, Arc::new(object)).is_some() {
                return Err(TileSnapshotError::DuplicateCoverageKey(key));
            }
        }
        Ok(Self::from_canonical_planes(images.tiles, coverage))
    }

    /// Replaces R8 tiles while preserving all image pixels. Zero coverage is meaningful.
    /// # Errors
    /// Rejects malformed pixels, duplicate keys and invalid coverage identities.
    pub fn with_coverage_replacements(
        &self,
        replacements: impl IntoIterator<Item = (CoverageTileKey, Vec<u8>)>,
    ) -> Result<Self, TileSnapshotError> {
        let mut coverage = self.coverage.clone();
        let mut seen = BTreeSet::new();
        for (key, pixels) in replacements {
            validate_key(key)?;
            if !seen.insert(key) {
                return Err(TileSnapshotError::DuplicateCoverageKey(key));
            }
            coverage.insert(key, Arc::new(CoverageTileObject::new(pixels)?));
        }
        Ok(Self::from_canonical_planes(self.tiles.clone(), coverage))
    }

    #[must_use]
    pub fn without_masks(&self, masks: impl IntoIterator<Item = MaskId>) -> Self {
        let masks: BTreeSet<_> = masks.into_iter().collect();
        let mut coverage = self.coverage.clone();
        coverage.retain(|key, _| !masks.contains(&key.mask));
        Self::from_canonical_planes(self.tiles.clone(), coverage)
    }

    pub fn coverage_iter(&self) -> impl Iterator<Item = (CoverageTileKey, &CoverageTileObject)> {
        self.coverage
            .iter()
            .map(|(key, object)| (*key, object.as_ref()))
    }
    #[must_use]
    pub fn get_coverage(&self, key: CoverageTileKey) -> Option<&CoverageTileObject> {
        self.coverage.get(&key).map(Arc::as_ref)
    }
    #[must_use]
    pub fn coverage_len(&self) -> usize {
        self.coverage.len()
    }
    #[must_use]
    pub fn total_len(&self) -> usize {
        self.tiles.len() + self.coverage.len()
    }
    pub fn object_hashes(&self) -> impl Iterator<Item = ObjectHash> + '_ {
        self.iter()
            .map(|(_, object)| object.hash())
            .chain(self.coverage_iter().map(|(_, object)| object.hash()))
    }
    #[must_use]
    pub fn coverage_at(&self, mask: MaskId, default: u8, point: [i32; 2]) -> u8 {
        let key = CoverageTileKey::from_pixel(mask, 128, point[0], point[1]);
        self.get_coverage(key).map_or(default, |tile| {
            tile.pixels()
                [point[1].rem_euclid(128) as usize * 128 + point[0].rem_euclid(128) as usize]
        })
    }
}

fn validate_key(key: CoverageTileKey) -> Result<(), TileSnapshotError> {
    if key.mask.0 == 0 || key.mip != 0 {
        return Err(TileSnapshotError::InvalidCoverageKey(key));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::LayerId;

    #[test]
    fn coverage_plane_edits_preserve_image_roots_and_signed_default_semantics() {
        let image_key = TileKey::from_pixel(LayerId(1), 128, -1, -1);
        let image = TileSnapshot::from_tiles([(image_key, vec![127; super::super::TILE_BYTE_LEN])])
            .unwrap();
        let key = CoverageTileKey::from_pixel(MaskId(2), 128, -1, -1);
        let masked = image
            .with_coverage_replacements([(key, vec![0; COVERAGE_TILE_BYTE_LEN])])
            .unwrap();
        assert_ne!(masked.root(), image.root());
        assert_eq!(masked.coverage_at(MaskId(2), 255, [-1, -1]), 0);
        assert_eq!(masked.coverage_at(MaskId(2), 255, [0, 0]), 255);
        assert_eq!(masked.get(image_key), image.get(image_key));
        let repainted = masked
            .with_replacements([(image_key, vec![15; super::super::TILE_BYTE_LEN])])
            .unwrap();
        assert_eq!(repainted.get_coverage(key), masked.get_coverage(key));
        assert_eq!(masked.without_masks([MaskId(2)]), image);
        assert_eq!(
            TileSnapshot::from_planes(
                image.iter().map(|(key, tile)| (key, tile.clone())),
                masked
                    .coverage_iter()
                    .map(|(key, tile)| (key, tile.clone()))
            )
            .unwrap(),
            masked
        );
        assert!(
            masked
                .with_coverage_replacements([
                    (key, vec![1; COVERAGE_TILE_BYTE_LEN]),
                    (key, vec![2; COVERAGE_TILE_BYTE_LEN])
                ])
                .is_err()
        );
    }
}

use std::collections::BTreeMap;

use nyatidraw_api::RasterTransform;
use nyatidraw_document::{LayerMask, LayerTree};
use nyatidraw_tiles::{COVERAGE_TILE_BYTE_LEN, CoverageTileKey, TILE_BYTE_LEN, TileSnapshot};

use crate::{EditError, EditLimits, SelectionMask};

#[derive(Clone, Copy)]
pub(crate) struct IntegerMaskGeometry {
    pub origin: [i64; 2],
    pub source: [u32; 2],
    pub output: [u32; 2],
    pub transform: RasterTransform,
}

type Region = ([i64; 2], [i64; 2]);

impl IntegerMaskGeometry {
    fn rotated_size(self) -> [i128; 2] {
        let size = self.source.map(i128::from);
        if self.transform.quarter_turns.is_multiple_of(2) {
            size
        } else {
            [size[1], size[0]]
        }
    }

    fn forward_edge(self, point: [i64; 2]) -> [i128; 2] {
        let [w, h] = self.source.map(i128::from);
        let [mut x, mut y] =
            [0, 1].map(|axis| i128::from(point[axis]) - i128::from(self.origin[axis]));
        if self.transform.flip_x {
            x = w - x;
        }
        if self.transform.flip_y {
            y = h - y;
        }
        match self.transform.quarter_turns {
            0 => [x, y],
            1 => [h - y, x],
            2 => [w - x, h - y],
            _ => [y, w - x],
        }
    }

    fn destination(self, (low, high): Region) -> Result<Region, EditError> {
        let rotated = self.rotated_size();
        let corners = [
            low,
            [high[0] + 1, low[1]],
            [low[0], high[1] + 1],
            high.map(|value| value + 1),
        ];
        let mut min = [i128::MAX; 2];
        let mut max = [i128::MIN; 2];
        for corner in corners {
            let edge = self.forward_edge(corner);
            for axis in 0..2 {
                let scaled = edge[axis] * i128::from(self.output[axis]);
                min[axis] = min[axis].min(scaled.div_euclid(rotated[axis]));
                max[axis] = max[axis].max(-(-scaled).div_euclid(rotated[axis]) - 1);
            }
        }
        let origin = [0, 1]
            .map(|axis| i128::from(self.origin[axis]) + i128::from(self.transform.offset[axis]));
        let convert = |values: [i128; 2]| -> Result<[i64; 2], EditError> {
            let mut point = [0; 2];
            for axis in 0..2 {
                point[axis] = i64::from(
                    i32::try_from(origin[axis] + values[axis])
                        .map_err(|_| EditError::CoordinateOutOfRange)?,
                );
            }
            Ok(point)
        };
        Ok((convert(min)?, convert(max)?))
    }

    fn sample(self, point: [i64; 2]) -> Result<[i32; 2], EditError> {
        let rotated = self.rotated_size();
        let [rx, ry] = [0, 1].map(|axis| {
            let local = i128::from(point[axis])
                - i128::from(self.origin[axis])
                - i128::from(self.transform.offset[axis]);
            ((2 * local + 1) * rotated[axis]).div_euclid(2 * i128::from(self.output[axis]))
        });
        let [w, h] = self.source.map(i128::from);
        let [mut x, mut y] = match self.transform.quarter_turns {
            0 => [rx, ry],
            1 => [ry, h - 1 - rx],
            2 => [w - 1 - rx, h - 1 - ry],
            _ => [w - 1 - ry, rx],
        };
        if self.transform.flip_x {
            x = w - 1 - x;
        }
        if self.transform.flip_y {
            y = h - 1 - y;
        }
        Ok([
            i32::try_from(x + i128::from(self.origin[0]))
                .map_err(|_| EditError::CoordinateOutOfRange)?,
            i32::try_from(y + i128::from(self.origin[1]))
                .map_err(|_| EditError::CoordinateOutOfRange)?,
        ])
    }
}

fn count((low, high): Region, limits: EditLimits) -> Result<u64, EditError> {
    let count = u64::try_from(high[0] - low[0] + 1)
        .map_err(|_| EditError::LimitExceeded)?
        .checked_mul(u64::try_from(high[1] - low[1] + 1).map_err(|_| EditError::LimitExceeded)?)
        .ok_or(EditError::LimitExceeded)?;
    if count > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    Ok(count)
}

struct CoverageEdit<'a> {
    source: &'a TileSnapshot,
    changed: BTreeMap<CoverageTileKey, Vec<u8>>,
    retained: u64,
    limits: EditLimits,
}

impl CoverageEdit<'_> {
    fn write(&mut self, mask: LayerMask, point: [i32; 2], value: u8) -> Result<(), EditError> {
        let key = CoverageTileKey::from_pixel(mask.id, 128, point[0], point[1]);
        if !self.changed.contains_key(&key) {
            let bytes = self.retained.saturating_add(
                (self.changed.len() as u64 + 1) * (COVERAGE_TILE_BYTE_LEN as u64 * 2 + 256),
            );
            if bytes > self.limits.max_workspace_bytes {
                return Err(EditError::LimitExceeded);
            }
            self.changed.insert(
                key,
                self.source.get_coverage(key).map_or_else(
                    || vec![mask.default_coverage; COVERAGE_TILE_BYTE_LEN],
                    |tile| tile.pixels().to_vec(),
                ),
            );
        }
        let index = usize::try_from(point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128))
            .expect("tile offset");
        self.changed.get_mut(&key).expect("inserted")[index] = value;
        Ok(())
    }

    fn value(&self, mask: LayerMask, point: [i32; 2]) -> u8 {
        let key = CoverageTileKey::from_pixel(mask.id, 128, point[0], point[1]);
        let index = usize::try_from(point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128))
            .expect("tile offset");
        self.changed.get(&key).map_or_else(
            || {
                self.source
                    .coverage_at(mask.id, mask.default_coverage, point)
            },
            |tile| tile[index],
        )
    }
}

fn retained_bytes(source: &TileSnapshot, images: &TileSnapshot) -> u64 {
    (source.total_len() as u64 + images.total_len() as u64) * 256
        + images
            .iter()
            .filter(|(key, tile)| {
                source
                    .get(*key)
                    .is_none_or(|before| before.hash() != tile.hash())
            })
            .count() as u64
            * TILE_BYTE_LEN as u64
            * 2
}

pub(crate) fn transform_mask(
    source: &TileSnapshot,
    images: &TileSnapshot,
    mask: LayerMask,
    selection: Option<&SelectionMask>,
    geometry: IntegerMaskGeometry,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    let limits = limits.bounded();
    let mut region = (
        geometry.origin,
        [0, 1].map(|axis| geometry.origin[axis] + i64::from(geometry.source[axis]) - 1),
    );
    if selection.is_none() {
        for (key, _) in source
            .coverage_iter()
            .filter(|(key, _)| key.mask == mask.id)
        {
            let (x, y) = key.pixel_origin();
            for (axis, start) in [x, y].into_iter().enumerate() {
                region.0[axis] = region.0[axis].min(start);
                region.1[axis] = region.1[axis].max(start + 127);
            }
        }
    }
    if region
        .0
        .into_iter()
        .chain(region.1)
        .any(|value| i32::try_from(value).is_err())
    {
        return Err(EditError::CoordinateOutOfRange);
    }
    let destination = geometry.destination(region)?;
    if count(region, limits)?.saturating_add(count(destination, limits)?) > limits.max_filter_work {
        return Err(EditError::LimitExceeded);
    }
    let mut edits = CoverageEdit {
        source,
        changed: BTreeMap::new(),
        retained: retained_bytes(source, images),
        limits,
    };
    for y in region.0[1]..=region.1[1] {
        for x in region.0[0]..=region.1[0] {
            let point = [
                i32::try_from(x).expect("bounded"),
                i32::try_from(y).expect("bounded"),
            ];
            let selected = selection.map_or(255, |selection| {
                selection.coverage_signed(point[0], point[1])
            });
            let original = source.coverage_at(mask.id, mask.default_coverage, point);
            let remaining = mix(original, mask.default_coverage, selected);
            if remaining != original {
                edits.write(mask, point, remaining)?;
            }
        }
    }
    for y in destination.0[1]..=destination.1[1] {
        for x in destination.0[0]..=destination.1[0] {
            let sample = geometry.sample([x, y])?;
            if (0..2).any(|axis| {
                i64::from(sample[axis]) < region.0[axis] || i64::from(sample[axis]) > region.1[axis]
            }) {
                continue;
            }
            let selected = selection.map_or(255, |selection| {
                selection.coverage_signed(sample[0], sample[1])
            });
            if selected == 0 {
                continue;
            }
            let point = [
                i32::try_from(x).expect("bounded"),
                i32::try_from(y).expect("bounded"),
            ];
            let original = source.coverage_at(mask.id, mask.default_coverage, sample);
            let previous = edits.value(mask, point);
            let value = if point == sample {
                original
            } else {
                mix(previous, original, selected)
            };
            if value != previous {
                edits.write(mask, point, value)?;
            }
        }
    }
    images
        .with_coverage_replacements(edits.changed)
        .map_err(EditError::Tiles)
}

fn mix(before: u8, after: u8, coverage: u8) -> u8 {
    u8::try_from(
        (u32::from(before) * u32::from(255 - coverage)
            + u32::from(after) * u32::from(coverage)
            + 127)
            / 255,
    )
    .expect("weighted byte")
}

pub(crate) fn translate_masks(
    source: &TileSnapshot,
    images: &TileSnapshot,
    tree: &LayerTree,
    offset: [i32; 2],
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    if offset == [0, 0] || source.coverage_len() == 0 {
        return Ok(images.clone());
    }
    let limits = limits.bounded();
    let pixels = source.coverage_len() as u64 * COVERAGE_TILE_BYTE_LEN as u64;
    if pixels > limits.max_pixels || pixels > limits.max_filter_work {
        return Err(EditError::LimitExceeded);
    }
    let mut defaults = BTreeMap::new();
    collect_mask_defaults(tree.root(), &mut defaults);
    let empty = images.without_masks(defaults.keys().copied());
    let mut edits = CoverageEdit {
        source: &empty,
        changed: BTreeMap::new(),
        retained: retained_bytes(source, images),
        limits,
    };
    for (key, tile) in source.coverage_iter() {
        let mask = *defaults.get(&key.mask).ok_or(EditError::InvalidMask)?;
        let (left, top) = key.pixel_origin();
        for (index, &value) in tile.pixels().iter().enumerate() {
            if value == mask.default_coverage {
                continue;
            }
            let x = left + i64::try_from(index % 128).expect("tile offset") + i64::from(offset[0]);
            let y = top + i64::try_from(index / 128).expect("tile offset") + i64::from(offset[1]);
            let point = [
                i32::try_from(x).map_err(|_| EditError::CoordinateOutOfRange)?,
                i32::try_from(y).map_err(|_| EditError::CoordinateOutOfRange)?,
            ];
            edits.write(mask, point, value)?;
        }
    }
    empty
        .with_coverage_replacements(edits.changed)
        .map_err(EditError::Tiles)
}

fn collect_mask_defaults(
    group: &nyatidraw_document::GroupNode,
    defaults: &mut BTreeMap<nyatidraw_api::MaskId, LayerMask>,
) {
    for node in &group.children {
        match node {
            nyatidraw_document::LayerTreeNode::Raster(layer) => {
                if let Some(mask) = layer.mask {
                    defaults.insert(mask.id, mask);
                }
            }
            nyatidraw_document::LayerTreeNode::Group(group) => {
                collect_mask_defaults(group, defaults);
            }
        }
    }
}

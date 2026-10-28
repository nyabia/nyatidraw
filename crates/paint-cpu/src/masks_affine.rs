use std::collections::BTreeMap;

use nyatidraw_api::{AffineTransform, LayerId};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{CoverageTileKey, CoverageTileObject, TILE_BYTE_LEN, TileKey, TileSnapshot};

use crate::{EditError, EditLimits, SelectionMask};

/// Transforms attached coverage from the immutable source with the image pivot.
/// The temporary RGBA encoding carries scalar coverage over its explicit default;
/// it is never inserted into the document tree or durable image plane.
/// # Errors
/// Rejects aggregate scratch allocation, geometry or typed plane reconstruction.
pub fn transform_linked_masks(
    source: &TileSnapshot,
    image_result: &TileSnapshot,
    tree: &LayerTree,
    layers: &[LayerId],
    selection: &SelectionMask,
    transform: &AffineTransform,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    transform_linked_masks_scoped(
        source,
        image_result,
        tree,
        layers,
        selection,
        false,
        transform,
        limits,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn transform_linked_masks_scoped(
    source: &TileSnapshot,
    image_result: &TileSnapshot,
    tree: &LayerTree,
    layers: &[LayerId],
    selection: &SelectionMask,
    whole_layer: bool,
    transform: &AffineTransform,
    limits: EditLimits,
) -> Result<TileSnapshot, EditError> {
    if *transform == AffineTransform::default() {
        return Ok(image_result.clone());
    }
    let mut result = image_result.clone();
    let mut budget = limits.bounded();
    for &layer in layers {
        let Some(mask) = tree.raster(layer).and_then(|node| node.mask) else {
            continue;
        };
        let pivot = selection.bounds_signed().ok_or(EditError::InvalidMask)?;
        let full_selection = if whole_layer {
            Some(full_mask_selection(source, mask, pivot, budget)?)
        } else {
            None
        };
        let selection = full_selection.as_ref().unwrap_or(selection);
        let (origin, size) = selection.bounds_signed().ok_or(EditError::InvalidMask)?;
        let end = [
            i64::from(origin[0]) + i64::from(size[0]) - 1,
            i64::from(origin[1]) + i64::from(size[1]) - 1,
        ];
        let end = [
            i32::try_from(end[0]).map_err(|_| EditError::CoordinateOutOfRange)?,
            i32::try_from(end[1]).map_err(|_| EditError::CoordinateOutOfRange)?,
        ];
        let first = TileKey::from_pixel(layer, 128, origin[0], origin[1]);
        let last = TileKey::from_pixel(layer, 128, end[0], end[1]);
        let selected_tiles = u64::try_from(i64::from(last.x) - i64::from(first.x) + 1)
            .map_err(|_| EditError::LimitExceeded)?
            .checked_mul(
                u64::try_from(i64::from(last.y) - i64::from(first.y) + 1)
                    .map_err(|_| EditError::LimitExceeded)?,
            )
            .ok_or(EditError::LimitExceeded)?;
        let coverage_count = source
            .coverage_iter()
            .filter(|(key, _)| key.mask == mask.id)
            .count() as u64;
        let retained = selected_tiles
            .checked_add(coverage_count)
            .and_then(|count| count.checked_mul(TILE_BYTE_LEN as u64 * 2 + 256))
            .and_then(|bytes| {
                bytes.checked_add(
                    u64::from(selection.dimensions()[0]) * u64::from(selection.dimensions()[1]),
                )
            })
            .ok_or(EditError::LimitExceeded)?;
        let mut work_limits = budget;
        work_limits.max_workspace_bytes = work_limits
            .max_workspace_bytes
            .checked_sub(retained)
            .ok_or(EditError::LimitExceeded)?;
        let temporary = encode_mask(source, mask, layer, first, last)?;
        let mut temporary_tree = tree.clone();
        temporary_tree
            .set_mask(layer, None)
            .map_err(|_| EditError::UnknownLayer)?;
        let transformed = crate::affine::transform_selection_affine_with_pivot(
            &temporary,
            &temporary_tree,
            layer,
            selection,
            transform,
            work_limits,
            Some(pivot),
        )?;
        let coverage = decode_mask(&transformed.paint.after, mask)?;
        let retained_result = (coverage.len() as u64)
            .saturating_mul((nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN * 2 + 256) as u64);
        budget.max_workspace_bytes = budget
            .max_workspace_bytes
            .checked_sub(retained_result)
            .ok_or(EditError::LimitExceeded)?;
        result = TileSnapshot::from_planes(
            result.iter().map(|(key, tile)| (key, tile.clone())),
            result
                .coverage_iter()
                .filter(|(key, _)| key.mask != mask.id)
                .map(|(key, tile)| (key, tile.clone()))
                .chain(coverage),
        )
        .map_err(EditError::Tiles)?;
    }
    Ok(result)
}

fn full_mask_selection(
    source: &TileSnapshot,
    mask: nyatidraw_document::LayerMask,
    pivot: ([i32; 2], [u32; 2]),
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let mut low = pivot.0.map(i64::from);
    let mut high: [i64; 2] = std::array::from_fn(|axis| low[axis] + i64::from(pivot.1[axis]) - 1);
    let count = source
        .coverage_iter()
        .filter(|(key, _)| key.mask == mask.id)
        .count() as u64;
    if count.saturating_mul(nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN as u64) > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    for (key, tile) in source
        .coverage_iter()
        .filter(|(key, _)| key.mask == mask.id)
    {
        let (x, y) = key.pixel_origin();
        for (index, &value) in tile.pixels().iter().enumerate() {
            if value == mask.default_coverage {
                continue;
            }
            let index = i64::try_from(index).expect("tile index");
            let point = [x + index % 128, y + index / 128];
            for axis in 0..2 {
                low[axis] = low[axis].min(point[axis]);
                high[axis] = high[axis].max(point[axis]);
            }
        }
    }
    let [Ok(x), Ok(y)] = low.map(i32::try_from) else {
        return Err(EditError::CoordinateOutOfRange);
    };
    let size = [
        u32::try_from(high[0] - low[0] + 1).map_err(|_| EditError::LimitExceeded)?,
        u32::try_from(high[1] - low[1] + 1).map_err(|_| EditError::LimitExceeded)?,
    ];
    crate::selection_all([x, y], size, limits)
}

fn encode_mask(
    source: &TileSnapshot,
    mask: nyatidraw_document::LayerMask,
    layer: LayerId,
    first: TileKey,
    last: TileKey,
) -> Result<TileSnapshot, EditError> {
    let mut rgba = BTreeMap::new();
    for (key, tile) in source
        .coverage_iter()
        .filter(|(key, _)| key.mask == mask.id)
    {
        if key.mip != 0 {
            return Err(EditError::InvalidTransform);
        }
        rgba.insert(
            TileKey {
                layer,
                mip: 0,
                x: key.x,
                y: key.y,
            },
            tile.pixels()
                .iter()
                .flat_map(|&value| [value, value, value, 255])
                .collect::<Vec<_>>(),
        );
    }
    for y in first.y..=last.y {
        for x in first.x..=last.x {
            rgba.entry(TileKey {
                layer,
                mip: 0,
                x,
                y,
            })
            .or_insert_with(|| {
                [
                    mask.default_coverage,
                    mask.default_coverage,
                    mask.default_coverage,
                    255,
                ]
                .repeat(TILE_BYTE_LEN / 4)
            });
        }
    }
    TileSnapshot::from_tiles(rgba).map_err(EditError::Tiles)
}

fn decode_mask(
    transformed: &TileSnapshot,
    mask: nyatidraw_document::LayerMask,
) -> Result<Vec<(CoverageTileKey, CoverageTileObject)>, EditError> {
    let mut coverage = Vec::new();
    for (key, tile) in transformed.iter() {
        let values: Vec<_> = tile
            .pixels()
            .chunks_exact(4)
            .map(|pixel| {
                let value = u32::from(pixel[0])
                    + (u32::from(mask.default_coverage) * (255 - u32::from(pixel[3])) + 127) / 255;
                u8::try_from(value.min(255)).expect("coverage byte")
            })
            .collect();
        if values.iter().all(|value| *value == mask.default_coverage) {
            continue;
        }
        coverage.push((
            CoverageTileKey {
                mask: mask.id,
                mip: 0,
                x: key.x,
                y: key.y,
            },
            CoverageTileObject::new(values).map_err(EditError::Tiles)?,
        ));
    }
    Ok(coverage)
}

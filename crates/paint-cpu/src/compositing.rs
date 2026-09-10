//! One byte-level contract for stack composition, picker, preview and export.
use std::collections::BTreeSet;

use nyatidraw_api::{CanvasSpec, LayerBlendMode, LayerId, LayerTreeNodeId, TileCoordinate};
use nyatidraw_document::{GroupNode, LayerTreeNode, layer_stacks};
use nyatidraw_tiles::{FlattenError, TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

use crate::CpuCompositeError;

#[cfg(test)]
#[path = "compositing_tests.rs"]
mod tests;

pub(crate) fn blend_pixel(
    destination: &mut [u8],
    source: &[u8],
    opacity: u16,
    mode: LayerBlendMode,
    atop: bool,
) {
    let scaled = |channel| (u32::from(source[channel]) * u32::from(opacity) + 32_767) / 65_535;
    let a = scaled(3);
    let b = u32::from(destination[3]);
    let alpha = if atop {
        b
    } else {
        a + (b * (255 - a) + 127) / 255
    };
    for (channel, destination) in destination.iter_mut().take(3).enumerate() {
        let s = scaled(channel);
        let d = u32::from(*destination);
        let value = match (mode, atop) {
            (LayerBlendMode::Normal, false) => s + (d * (255 - a) + 127) / 255,
            (LayerBlendMode::Multiply, false) => {
                (s * (255 - b) + d * (255 - a) + s * d + 127) / 255
            }
            (LayerBlendMode::Normal, true) => (s * b + d * (255 - a) + 127) / 255,
            (LayerBlendMode::Multiply, true) => (d * (255 - a + s) + 127) / 255,
        };
        *destination =
            u8::try_from(value.min(alpha)).expect("premultiplied channel bounded by alpha");
    }
    destination[3] = u8::try_from(alpha).expect("over/atop alpha is at most 255");
}

fn properties(node: &LayerTreeNode) -> (bool, u16, LayerBlendMode) {
    match node {
        LayerTreeNode::Raster(layer) => (layer.visible, layer.opacity_u16, layer.blend_mode),
        LayerTreeNode::Group(group) => (group.visible, group.opacity_u16, group.blend_mode),
    }
}

fn included(node: &LayerTreeNode, scope: Option<&BTreeSet<LayerTreeNodeId>>) -> bool {
    scope.map_or_else(|| properties(node).0, |nodes| nodes.contains(&node.id()))
}

pub(crate) fn group_pixel(
    snapshot: &TileSnapshot,
    group: &GroupNode,
    point: [i32; 2],
    scope: Option<&BTreeSet<LayerTreeNodeId>>,
) -> [u8; 4] {
    let raw = |node: &LayerTreeNode| match node {
        LayerTreeNode::Raster(layer) => crate::selection::raster_pixel(snapshot, layer.id, point),
        LayerTreeNode::Group(group) => group_pixel(snapshot, group, point, scope),
    };
    let mut output = [0; 4];
    for stack in layer_stacks(group) {
        let (_, opacity, mode) = properties(stack.base);
        if !included(stack.base, scope) || opacity == 0 {
            continue;
        }
        let mut pixels = raw(stack.base);
        for clip in stack.clips {
            let (_, opacity, mode) = properties(clip);
            if included(clip, scope) && opacity != 0 {
                blend_pixel(&mut pixels, &raw(clip), opacity, mode, true);
            }
        }
        blend_pixel(&mut output, &pixels, opacity, mode, false);
    }
    output
}

fn raw_tile(snapshot: &TileSnapshot, node: &LayerTreeNode, coordinate: TileCoordinate) -> Vec<u8> {
    match node {
        LayerTreeNode::Raster(layer) => snapshot
            .get(TileKey {
                layer: layer.id,
                mip: 0,
                x: coordinate.x,
                y: coordinate.y,
            })
            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec()),
        LayerTreeNode::Group(group) => group_tile(snapshot, group, coordinate),
    }
}

fn group_tile(snapshot: &TileSnapshot, group: &GroupNode, coordinate: TileCoordinate) -> Vec<u8> {
    let mut output = vec![0; TILE_BYTE_LEN];
    for stack in layer_stacks(group) {
        let (visible, opacity, mode) = properties(stack.base);
        if !visible || opacity == 0 {
            continue;
        }
        let mut pixels = raw_tile(snapshot, stack.base, coordinate);
        for clip in stack.clips {
            let (visible, opacity, mode) = properties(clip);
            if !visible || opacity == 0 {
                continue;
            }
            let source = raw_tile(snapshot, clip, coordinate);
            for (destination, source) in pixels.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
                blend_pixel(destination, source, opacity, mode, true);
            }
        }
        for (destination, source) in output.chunks_exact_mut(4).zip(pixels.chunks_exact(4)) {
            blend_pixel(destination, source, opacity, mode, false);
        }
    }
    output
}

fn visible_rasters(group: &GroupNode, output: &mut BTreeSet<LayerId>) {
    // Preserve legacy malformed-mip validation even on zero-opacity layers.
    for child in &group.children {
        match child {
            LayerTreeNode::Raster(layer) if layer.visible => {
                output.insert(layer.id);
            }
            LayerTreeNode::Group(group) if group.visible => visible_rasters(group, output),
            _ => {}
        }
    }
}

pub(crate) fn flatten_group(
    snapshot: &TileSnapshot,
    group: &GroupNode,
    canvas: CanvasSpec,
    output: &mut [u8],
) -> Result<(), CpuCompositeError> {
    let mut rasters = BTreeSet::new();
    visible_rasters(group, &mut rasters);
    let mut coordinates = BTreeSet::new();
    for (key, _) in snapshot
        .iter()
        .filter(|(key, _)| rasters.contains(&key.layer))
    {
        if key.mip != 0 {
            return Err(CpuCompositeError::Flatten(FlattenError::UnsupportedMip {
                mip: key.mip,
            }));
        }
        let (x, y) = key.pixel_origin();
        if x < i64::from(canvas.width_px)
            && x + i64::from(TILE_EDGE) > 0
            && y < i64::from(canvas.height_px)
            && y + i64::from(TILE_EDGE) > 0
        {
            coordinates.insert(TileCoordinate {
                mip: 0,
                x: key.x,
                y: key.y,
            });
        }
    }
    // Memory is bounded by one tile per active stack/group depth, rather than
    // allocating a full-page intermediate for each isolated layer or group.
    for coordinate in coordinates {
        let pixels = group_tile(snapshot, group, coordinate);
        let left = u32::try_from(i64::from(coordinate.x) * i64::from(TILE_EDGE))
            .map_err(|_| CpuCompositeError::InvalidCanvas)?;
        let top = u32::try_from(i64::from(coordinate.y) * i64::from(TILE_EDGE))
            .map_err(|_| CpuCompositeError::InvalidCanvas)?;
        let width = TILE_EDGE.min(canvas.width_px - left);
        let height = TILE_EDGE.min(canvas.height_px - top);
        for row in 0..height {
            let source = (row * TILE_EDGE * 4) as usize;
            let destination = usize::try_from(
                (u64::from(top + row) * u64::from(canvas.width_px) + u64::from(left)) * 4,
            )
            .map_err(|_| CpuCompositeError::InvalidCanvas)?;
            let bytes = width as usize * 4;
            output[destination..destination + bytes]
                .copy_from_slice(&pixels[source..source + bytes]);
        }
    }
    Ok(())
}

use super::{TransformError, rejected};
use nyatidraw_api::{GroupId, LayerId};
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask, movable_bounds, selection_all};
use nyatidraw_tiles::TileSnapshot;

pub(super) fn targets(tree: &LayerTree, id: GroupId) -> Result<Vec<LayerId>, TransformError> {
    fn find(group: &GroupNode, id: GroupId) -> Option<&GroupNode> {
        if group.id == id {
            return Some(group);
        }
        group.children.iter().find_map(|node| match node {
            LayerTreeNode::Group(child) => find(child, id),
            LayerTreeNode::Raster(_) => None,
        })
    }
    fn collect(group: &GroupNode, layers: &mut Vec<LayerId>) -> Result<(), TransformError> {
        for child in &group.children {
            match child {
                LayerTreeNode::Raster(layer) => {
                    if layer.locked || layer.alpha_locked {
                        return Err(rejected(
                            "그룹 안의 픽셀 잠금·투명도 잠금을 모두 해제하세요.",
                        ));
                    }
                    layers.push(layer.id);
                }
                LayerTreeNode::Group(child) => collect(child, layers)?,
            }
        }
        Ok(())
    }
    if id == tree.root().id {
        return Err(rejected("문서 루트는 변형할 수 없습니다."));
    }
    let group = find(tree.root(), id).ok_or_else(|| rejected("그룹이 없습니다."))?;
    let mut layers = Vec::new();
    collect(group, &mut layers)?;
    if layers.is_empty() {
        return Err(rejected("그룹 안에 래스터 레이어가 없습니다."));
    }
    Ok(layers)
}

pub(super) fn occupied_selection(
    source: &TileSnapshot,
    tree: &LayerTree,
    layers: &[LayerId],
) -> Result<SelectionMask, TransformError> {
    let mut bounds: Option<([i64; 2], [i64; 2])> = None;
    let mut scan_limits = EditLimits::default();
    scan_limits.max_pixels /= layers.len() as u64;
    for &layer in layers {
        if let Some(found) = movable_bounds(source, layer, None, scan_limits).map_err(rejected)? {
            let min = found.origin.map(i64::from);
            let end = std::array::from_fn(|axis| min[axis] + i64::from(found.size[axis]));
            let (left, right) = bounds.get_or_insert((min, end));
            for axis in 0..2 {
                left[axis] = left[axis].min(min[axis]);
                right[axis] = right[axis].max(end[axis]);
            }
        }
    }
    if bounds.is_none() {
        for &layer in layers {
            let Some(mask) = tree.raster(layer).and_then(|node| node.mask) else {
                continue;
            };
            let count = source
                .coverage_iter()
                .filter(|(key, _)| key.mask == mask.id)
                .count() as u64;
            if count.saturating_mul(nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN as u64)
                > scan_limits.max_pixels
            {
                return Err(rejected("Mask workspace limit"));
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
                    let (left, right) = bounds.get_or_insert((point, point.map(|v| v + 1)));
                    for axis in 0..2 {
                        left[axis] = left[axis].min(point[axis]);
                        right[axis] = right[axis].max(point[axis] + 1);
                    }
                }
            }
        }
    }
    let (min, end) = bounds.ok_or_else(|| rejected("그룹에 변형할 그림이 없습니다."))?;
    let origin = [
        i32::try_from(min[0]).map_err(rejected)?,
        i32::try_from(min[1]).map_err(rejected)?,
    ];
    let size = [
        u32::try_from(end[0] - min[0]).map_err(rejected)?,
        u32::try_from(end[1] - min[1]).map_err(rejected)?,
    ];
    selection_all(origin, size, EditLimits::default()).map_err(rejected)
}

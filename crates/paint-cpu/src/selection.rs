//! Bounded, deterministic finite-page selection and pixel editing.
use crate::{CpuCompositeError, PremultipliedRgba8, composite_surface, flatten_layer_tree_rgba8};
use nyatidraw_api::{CanvasSpec, LayerId, LayerTreeNodeId};
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot, TileSnapshotError};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditLimits {
    pub max_pixels: u64,
    pub max_workspace_bytes: u64,
    pub max_frontier: usize,
    pub max_polygon_work: u64,
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            max_pixels: 16 * 1024 * 1024,
            max_workspace_bytes: 256 * 1024 * 1024,
            max_frontier: 1024 * 1024,
            max_polygon_work: 64 * 1024 * 1024,
        }
    }
}

impl EditLimits {
    // Callers can tighten limits for their task, never disable global bounds.
    pub(crate) fn bounded(self) -> Self {
        let maximum = Self::default();
        Self {
            max_pixels: self.max_pixels.min(maximum.max_pixels),
            max_workspace_bytes: self.max_workspace_bytes.min(maximum.max_workspace_bytes),
            max_frontier: self.max_frontier.min(maximum.max_frontier),
            max_polygon_work: self.max_polygon_work.min(maximum.max_polygon_work),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionSource {
    ActiveLayer,
    ReferenceLayers,
    AllVisible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WandRequest {
    pub canvas: CanvasSpec,
    pub active: LayerId,
    pub source: SelectionSource,
    pub seed: [i32; 2],
    pub tolerance: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditError {
    InvalidCanvas,
    UnknownLayer,
    LockedLayer,
    MissingReference,
    InvalidSeed,
    InvalidPolygon,
    InvalidPaint,
    InvalidMask,
    InvalidTransform,
    CoordinateOutOfRange,
    LimitExceeded,
    Composite(CpuCompositeError),
    Tiles(TileSnapshotError),
}

/// Binary pixel-center coverage in the finite page; outside artwork is excluded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionMask {
    width: u32,
    height: u32,
    selected: Vec<u8>,
    count: u64,
}

impl SelectionMask {
    /// Canonical row-major, least-significant-bit-first coverage. Unused tail
    /// bits are zero; this representation is bounded to 2 MiB.
    #[must_use]
    pub fn packed_bits(&self) -> Vec<u8> {
        let mut packed = vec![0; self.selected.len().div_ceil(8)];
        for (index, selected) in self.selected.iter().enumerate() {
            packed[index / 8] |= selected << (index % 8);
        }
        packed
    }

    /// Restores canonical binary coverage without trusting a serialized count.
    ///
    /// # Errors
    /// Rejects oversized dimensions, wrong lengths and nonzero unused bits
    /// before allocating the decoded page.
    pub fn from_packed_bits(width: u32, height: u32, packed: &[u8]) -> Result<Self, EditError> {
        let count = page_len(
            CanvasSpec {
                width_px: width,
                height_px: height,
                pixels_per_inch: 96,
            },
            EditLimits::default(),
        )?;
        if packed.len() != count.div_ceil(8)
            || (count % 8 != 0 && packed.last().is_some_and(|last| last >> (count % 8) != 0))
        {
            return Err(EditError::InvalidMask);
        }
        let selected: Vec<_> = (0..count)
            .map(|index| (packed[index / 8] >> (index % 8)) & 1)
            .collect();
        let count = selected.iter().map(|&value| u64::from(value)).sum();
        Ok(Self {
            width,
            height,
            selected,
            count,
        })
    }

    #[must_use]
    pub const fn dimensions(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    #[must_use]
    pub const fn selected_pixels(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.selected[pixel_index(self.width, x, y)] == 1
    }

    /// Computes the selected pixel rectangle on the CPU editing worker.
    /// The result is [left, top, width, height]; empty coverage has no bounds.
    #[must_use]
    pub fn bounds(&self) -> Option<[u32; 4]> {
        if self.count == 0 {
            return None;
        }
        let mut left = self.width;
        let mut top = self.height;
        let mut right = 0;
        let mut bottom = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.contains(x, y) {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x);
                    bottom = bottom.max(y);
                }
            }
        }
        Some([left, top, right - left + 1, bottom - top + 1])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionPaint {
    Solid(PremultipliedRgba8),
    /// Clamped linear interpolation in premultiplied linear RGBA, evaluated at
    /// pixel centers. Integer page points define the start/end axis.
    LinearGradient {
        start: [i32; 2],
        end: [i32; 2],
        start_color: PremultipliedRgba8,
        end_color: PremultipliedRgba8,
    },
}

#[derive(Clone, Debug)]
pub struct SelectionPaintResult {
    pub after: TileSnapshot,
    pub changed_tiles: Vec<TileKey>,
}

fn page_len(canvas: CanvasSpec, limits: EditLimits) -> Result<usize, EditError> {
    canvas.validate().map_err(|_| EditError::InvalidCanvas)?;
    if canvas.width_px > i32::MAX as u32 || canvas.height_px > i32::MAX as u32 {
        return Err(EditError::InvalidCanvas);
    }
    let count = u64::from(canvas.width_px) * u64::from(canvas.height_px);
    if count > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    usize::try_from(count).map_err(|_| EditError::LimitExceeded)
}

fn require_workspace(bytes: u64, limits: EditLimits) -> Result<(), EditError> {
    if bytes > limits.max_workspace_bytes {
        Err(EditError::LimitExceeded)
    } else {
        Ok(())
    }
}

fn pixel_index(width: u32, x: u32, y: u32) -> usize {
    // Callers preflight the whole page length and keep coordinates in bounds.
    usize::try_from(u64::from(y) * u64::from(width) + u64::from(x))
        .expect("validated page fits the platform index")
}

// Match the export group's visibility/opacity semantics, retaining ancestors.
fn restrict_references(group: &mut GroupNode, ancestors_visible: bool) -> usize {
    let visible = ancestors_visible && group.visible && group.opacity_u16 != 0;
    group
        .children
        .iter_mut()
        .map(|child| match child {
            LayerTreeNode::Raster(layer) => {
                layer.visible &= layer.reference;
                usize::from(visible && layer.visible && layer.opacity_u16 != 0)
            }
            LayerTreeNode::Group(child) => restrict_references(child, visible),
        })
        .sum()
}

fn group_depth(group: &GroupNode) -> u64 {
    1 + group
        .children
        .iter()
        .filter_map(|node| match node {
            LayerTreeNode::Group(child) if child.visible => Some(group_depth(child)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// Selects the 4-connected region whose RGBA channel differences from the seed
/// are all at most tolerance. Sampling never includes session Solo.
///
/// # Errors
/// Rejects out-of-page seeds, absent visible references and resource limits.
pub fn wand_selection(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    request: WandRequest,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    if limits.max_frontier == 0 {
        return Err(EditError::LimitExceeded);
    }
    let count = page_len(request.canvas, limits)?;
    let [width, height] = [request.canvas.width_px, request.canvas.height_px];
    let [Ok(x), Ok(y)] = request.seed.map(u32::try_from) else {
        return Err(EditError::InvalidSeed);
    };
    if x >= width || y >= height {
        return Err(EditError::InvalidSeed);
    }
    let mut root = tree.root().clone();
    if request.source == SelectionSource::ReferenceLayers
        && restrict_references(&mut root, true) == 0
    {
        return Err(EditError::MissingReference);
    }
    let source_tree = LayerTree::new(root).map_err(|_| EditError::InvalidCanvas)?;
    let surfaces = if request.source == SelectionSource::ActiveLayer {
        1
    } else {
        group_depth(source_tree.root()) + 1
    };
    let frontier = u64::try_from(limits.max_frontier)
        .unwrap_or(u64::MAX)
        .min(count as u64);
    require_workspace(
        (count as u64)
            .saturating_mul(surfaces * 4 + 1)
            .saturating_add(frontier.saturating_mul(8)),
        limits,
    )?;
    let source = if request.source == SelectionSource::ActiveLayer {
        if tree
            .ancestors(LayerTreeNodeId::Raster(request.active))
            .is_none()
        {
            return Err(EditError::UnknownLayer);
        }
        snapshot
            .crop_base_layer_rgba8_to_canvas(request.active, request.canvas)
            .map_err(|error| EditError::Composite(CpuCompositeError::Flatten(error)))?
    } else {
        flatten_layer_tree_rgba8(snapshot, &source_tree, request.canvas)
            .map_err(EditError::Composite)?
    };
    let mut mask = SelectionMask {
        width,
        height,
        selected: vec![0; count],
        count: 0,
    };
    let seed = pixel_index(width, x, y);
    let seed_color: [u8; 4] = source.pixels[seed * 4..seed * 4 + 4]
        .try_into()
        .map_err(|_| EditError::InvalidSeed)?;
    let mut queue = VecDeque::with_capacity(limits.max_frontier.min(count));
    mask.selected[seed] = 1;
    mask.count = 1;
    queue.push_back((x, y));
    while let Some((x, y)) = queue.pop_front() {
        for (nx, ny) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if nx >= width || ny >= height {
                continue;
            }
            let index = pixel_index(width, nx, ny);
            if mask.selected[index] != 0 {
                continue;
            }
            if source.pixels[index * 4..index * 4 + 4]
                .iter()
                .zip(seed_color)
                .all(|(channel, seed)| channel.abs_diff(seed) <= request.tolerance)
            {
                if queue.len() >= limits.max_frontier {
                    return Err(EditError::LimitExceeded);
                }
                mask.selected[index] = 1;
                mask.count += 1;
                queue.push_back((nx, ny));
            } else {
                mask.selected[index] = 2;
            }
        }
    }
    for selected in &mut mask.selected {
        *selected = u8::from(*selected == 1);
    }
    Ok(mask)
}

/// Selects an even-odd polygon at pixel centers, clipped to the finite page.
/// Horizontal edges and repeated vertices are permitted; no edge antialiasing.
///
/// # Errors
/// Rejects fewer than three/more than 4096 vertices or excessive work/memory.
pub fn lasso_selection(
    canvas: CanvasSpec,
    vertices: &[[i32; 2]],
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    if !(3..=4096).contains(&vertices.len()) {
        return Err(EditError::InvalidPolygon);
    }
    let count = page_len(canvas, limits)?;
    require_workspace(
        (count as u64).saturating_add(vertices.len() as u64 * 32),
        limits,
    )?;
    let min_y = vertices
        .iter()
        .map(|point| point[1])
        .min()
        .unwrap_or(0)
        .max(0)
        .cast_unsigned();
    let max_y = vertices
        .iter()
        .map(|point| point[1])
        .max()
        .unwrap_or(0)
        .max(0)
        .cast_unsigned();
    let end_y = max_y.min(canvas.height_px);
    if u64::from(end_y.saturating_sub(min_y)) * vertices.len() as u64 > limits.max_polygon_work {
        return Err(EditError::LimitExceeded);
    }
    let mut mask = SelectionMask {
        width: canvas.width_px,
        height: canvas.height_px,
        selected: vec![0; count],
        count: 0,
    };
    let mut crossings = Vec::with_capacity(vertices.len());
    for y in min_y..end_y {
        crossings.clear();
        let scan_y = i128::from(y) * 2 + 1;
        for (mut a, mut b) in vertices
            .iter()
            .copied()
            .zip(vertices.iter().copied().cycle().skip(1))
            .take(vertices.len())
        {
            if a[1] > b[1] {
                std::mem::swap(&mut a, &mut b);
            }
            if scan_y < i128::from(a[1]) * 2 || scan_y >= i128::from(b[1]) * 2 {
                continue;
            }
            let dy = i128::from(b[1]) - i128::from(a[1]);
            let dx = i128::from(b[0]) - i128::from(a[0]);
            let numerator = i128::from(a[0]) * 2 * dy + (scan_y - i128::from(a[1]) * 2) * dx;
            crossings.push((numerator, dy));
        }
        crossings.sort_unstable_by(|(a, b), (c, d)| (a * d).cmp(&(c * b)));
        for pair in crossings.chunks_exact(2) {
            let boundary = |(n, d): (i128, i128)| {
                let numerator = n - d;
                let denominator = 2 * d;
                u32::try_from(
                    (numerator.div_euclid(denominator)
                        + i128::from(numerator.rem_euclid(denominator) != 0))
                    .clamp(0, i128::from(canvas.width_px)),
                )
                .unwrap_or(canvas.width_px)
            };
            let start = boundary(pair[0]);
            let end = boundary(pair[1]);
            for x in start..end {
                mask.selected[pixel_index(canvas.width_px, x, y)] = 1;
                mask.count += 1;
            }
        }
    }
    Ok(mask)
}

fn valid_color(color: PremultipliedRgba8) -> bool {
    color.0[..3].iter().all(|channel| *channel <= color.0[3])
}

fn validate_paint(paint: SelectionPaint) -> Result<(), EditError> {
    let valid = match paint {
        SelectionPaint::Solid(color) => valid_color(color),
        SelectionPaint::LinearGradient {
            start,
            end,
            start_color,
            end_color,
        } => start != end && valid_color(start_color) && valid_color(end_color),
    };
    if valid {
        Ok(())
    } else {
        Err(EditError::InvalidPaint)
    }
}

fn paint_pixel(paint: SelectionPaint, x: u32, y: u32) -> [u8; 4] {
    match paint {
        SelectionPaint::Solid(color) => color.0,
        SelectionPaint::LinearGradient {
            start,
            end,
            start_color,
            end_color,
        } => {
            let dx = i128::from(end[0]) - i128::from(start[0]);
            let dy = i128::from(end[1]) - i128::from(start[1]);
            let denominator = 2 * (dx * dx + dy * dy);
            let numerator = ((i128::from(x) * 2 + 1 - i128::from(start[0]) * 2) * dx
                + (i128::from(y) * 2 + 1 - i128::from(start[1]) * 2) * dy)
                .clamp(0, denominator);
            std::array::from_fn(|index| {
                u8::try_from(
                    (i128::from(start_color.0[index]) * (denominator - numerator)
                        + i128::from(end_color.0[index]) * numerator
                        + denominator / 2)
                        / denominator,
                )
                .expect("convex combination of two bytes fits one byte")
            })
        }
    }
}

/// Paints source-over into selected pixels of one unlocked raster. Untouched
/// layers, off-page tiles and padding within a boundary tile remain exact.
///
/// # Errors
/// Rejects invalid colors/axes, missing/locked targets and resource overflow.
/// The immutable input is never changed, including when an error is returned.
pub fn paint_selection(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    mask: &SelectionMask,
    paint: SelectionPaint,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    let limits = limits.bounded();
    validate_paint(paint)?;
    let count = page_len(
        CanvasSpec {
            width_px: mask.width,
            height_px: mask.height,
            pixels_per_inch: 1,
        },
        limits,
    )?;
    let layer = find_raster(tree.root(), target).ok_or(EditError::UnknownLayer)?;
    if layer.locked {
        return Err(EditError::LockedLayer);
    }
    if snapshot
        .iter()
        .any(|(key, _)| key.layer == target && key.mip != 0)
    {
        return Err(EditError::InvalidPaint);
    }
    require_workspace(count as u64, limits)?;
    let keys = selected_tiles(mask, target, limits)?;
    let mut replacements = Vec::with_capacity(keys.len());
    for key in keys {
        let mut pixels = snapshot
            .get(key)
            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
        let left = u32::try_from(key.x).map_err(|_| EditError::InvalidCanvas)? * TILE_EDGE;
        let top = u32::try_from(key.y).map_err(|_| EditError::InvalidCanvas)? * TILE_EDGE;
        for y in top..(top + TILE_EDGE).min(mask.height) {
            for x in left..(left + TILE_EDGE).min(mask.width) {
                if !mask.contains(x, y) {
                    continue;
                }
                let offset = ((y % TILE_EDGE) * TILE_EDGE + x % TILE_EDGE) as usize * 4;
                composite_surface(
                    &mut pixels[offset..offset + 4],
                    &paint_pixel(paint, x, y),
                    u16::MAX,
                );
            }
        }
        replacements.push((key, pixels));
    }
    replacements.retain(|(key, pixels)| {
        snapshot.get(*key).map_or_else(
            || pixels.iter().any(|byte| *byte != 0),
            |tile| tile.pixels() != pixels,
        )
    });
    let changed_tiles = replacements.iter().map(|(key, _)| *key).collect();
    let after = snapshot
        .with_replacements(replacements)
        .map_err(EditError::Tiles)?;
    Ok(SelectionPaintResult {
        after,
        changed_tiles,
    })
}

fn selected_tiles(
    mask: &SelectionMask,
    target: LayerId,
    limits: EditLimits,
) -> Result<Vec<TileKey>, EditError> {
    let mut keys = Vec::new();
    for tile_x in 0..mask.width.div_ceil(TILE_EDGE) {
        for tile_y in 0..mask.height.div_ceil(TILE_EDGE) {
            let left = tile_x * TILE_EDGE;
            let top = tile_y * TILE_EDGE;
            let right = (left + TILE_EDGE).min(mask.width);
            if !(top..(top + TILE_EDGE).min(mask.height)).any(|y| {
                mask.selected
                    [pixel_index(mask.width, left, y)..=pixel_index(mask.width, right - 1, y)]
                    .contains(&1)
            }) {
                continue;
            }
            let bytes =
                mask.selected.len() as u64 + (keys.len() as u64 + 1) * (TILE_BYTE_LEN as u64 + 128);
            require_workspace(bytes, limits)?;
            keys.push(TileKey {
                layer: target,
                mip: 0,
                x: i32::try_from(tile_x).map_err(|_| EditError::InvalidCanvas)?,
                y: i32::try_from(tile_y).map_err(|_| EditError::InvalidCanvas)?,
            });
        }
    }
    Ok(keys)
}

pub(crate) fn find_raster(
    group: &GroupNode,
    id: LayerId,
) -> Option<&nyatidraw_document::LayerNode> {
    group.children.iter().find_map(|node| match node {
        LayerTreeNode::Raster(layer) if layer.id == id => Some(layer),
        LayerTreeNode::Group(group) => find_raster(group, id),
        LayerTreeNode::Raster(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId};
    use nyatidraw_document::LayerNode;

    fn raster(id: u128, reference: bool, locked: bool) -> LayerTreeNode {
        LayerTreeNode::Raster(LayerNode {
            id: LayerId(id),
            name: format!("Layer{id}"),
            visible: true,
            locked,
            reference,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })
    }
    fn tree(locked: bool) -> LayerTree {
        LayerTree::new(GroupNode {
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![
                raster(1, false, locked),
                LayerTreeNode::Group(GroupNode {
                    id: GroupId(10),
                    name: "References".into(),
                    visible: true,
                    opacity_u16: 32768,
                    children: vec![raster(2, true, false)],
                }),
                raster(3, false, false),
            ],
        })
        .expect("valid fixture")
    }
    fn canvas(width_px: u32, height_px: u32) -> CanvasSpec {
        CanvasSpec {
            width_px,
            height_px,
            pixels_per_inch: 96,
        }
    }
    fn key(layer: u128, x: i32) -> TileKey {
        TileKey {
            layer: LayerId(layer),
            mip: 0,
            x,
            y: 0,
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn wand_preserves_seed_tolerance_connectivity_and_reference_scope() {
        // Product risk: reference/scope mistakes or chained tolerance can fill
        // through a boundary or sample unrelated layers instead of the seed.
        let tree = tree(false);
        let mut alpha = vec![0; TILE_BYTE_LEN];
        alpha[7] = 4;
        alpha[11] = 5;
        let mut reference = vec![0; TILE_BYTE_LEN];
        reference[4..8].copy_from_slice(&[255, 0, 0, 255]);
        let snapshot = TileSnapshot::from_tiles([
            (key(1, 0), alpha),
            (key(2, 0), reference),
            (key(3, 0), [0, 0, 255, 255].repeat(TILE_BYTE_LEN / 4)),
        ])
        .expect("tiles");
        let request = WandRequest {
            canvas: canvas(3, 1),
            active: LayerId(1),
            source: SelectionSource::ActiveLayer,
            seed: [0, 0],
            tolerance: 0,
        };
        for (tolerance, expected) in [(0, 1), (3, 1), (4, 2), (5, 3)] {
            assert_eq!(
                wand_selection(
                    &snapshot,
                    &tree,
                    WandRequest {
                        tolerance,
                        ..request
                    },
                    EditLimits::default()
                )
                .expect("wand")
                .count,
                expected
            );
        }
        for (source, tolerance, expected) in [
            (SelectionSource::ReferenceLayers, 127, 1),
            (SelectionSource::ReferenceLayers, 128, 3),
            (SelectionSource::AllVisible, 0, 3),
        ] {
            assert_eq!(
                wand_selection(
                    &snapshot,
                    &tree,
                    WandRequest {
                        source,
                        tolerance,
                        ..request
                    },
                    EditLimits::default()
                )
                .expect("scope")
                .count,
                expected
            );
        }
        let mut hidden = tree.clone();
        hidden
            .set_visibility(LayerTreeNodeId::Group(GroupId(10)), false)
            .expect("hide source");
        assert_eq!(
            wand_selection(
                &snapshot,
                &hidden,
                WandRequest {
                    source: SelectionSource::ReferenceLayers,
                    ..request
                },
                EditLimits::default()
            ),
            Err(EditError::MissingReference)
        );
        let mut barrier = vec![0; TILE_BYTE_LEN];
        barrier[4..8].fill(255);
        barrier[128 * 4..128 * 4 + 4].fill(255);
        let barrier = TileSnapshot::from_tiles([(key(1, 0), barrier)]).expect("diagonal fixture");
        assert_eq!(
            wand_selection(
                &barrier,
                &tree,
                WandRequest {
                    canvas: canvas(3, 3),
                    ..request
                },
                EditLimits::default()
            )
            .expect("4-connected")
            .count,
            1
        );
        let before = snapshot.clone();
        assert_eq!(
            wand_selection(
                &snapshot,
                &tree,
                WandRequest {
                    canvas: canvas(3, 3),
                    source: SelectionSource::AllVisible,
                    ..request
                },
                EditLimits {
                    max_frontier: 1,
                    ..EditLimits::default()
                }
            ),
            Err(EditError::LimitExceeded)
        );
        assert_eq!(snapshot, before, "failed selection must not touch artwork");
    }

    #[test]
    fn lasso_pixel_centers_clip_signed_polygons_without_winding_or_overflow_drift() {
        // Product risk: selection edges must not drift with winding, negative
        // coordinates, self-crossings or oversized coordinates/work requests.
        let rectangle = [[-1, -1], [2, -1], [2, 2], [-1, 2]];
        let selected = lasso_selection(canvas(3, 3), &rectangle, EditLimits::default())
            .expect("clipped rectangle");
        assert_eq!(selected.count, 4);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(selected.contains(x, y), x < 2 && y < 2);
            }
        }
        let mut reversed = rectangle;
        reversed.reverse();
        assert_eq!(
            lasso_selection(canvas(3, 3), &reversed, EditLimits::default()),
            Ok(selected)
        );
        let bowtie = [[0, 0], [3, 3], [0, 3], [3, 0]];
        assert_eq!(
            lasso_selection(canvas(3, 3), &bowtie, EditLimits::default())
                .expect("even-odd")
                .count,
            4
        );
        let huge = [
            [i32::MIN, i32::MIN],
            [i32::MAX, i32::MIN],
            [i32::MAX, i32::MAX],
            [i32::MIN, i32::MAX],
        ];
        assert_eq!(
            lasso_selection(canvas(3, 3), &huge, EditLimits::default())
                .expect("wide rational arithmetic")
                .count,
            9
        );
        assert_eq!(
            lasso_selection(
                canvas(3, 3),
                &rectangle,
                EditLimits {
                    max_polygon_work: 1,
                    ..EditLimits::default()
                }
            ),
            Err(EditError::LimitExceeded)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn fill_and_gradient_preserve_other_artwork_padding_and_failure_atomicity() {
        // Product risk: page-edge fills must not erase signed artwork, adjacent
        // tile padding or other layers, and invalid work must return no mutation.
        let mut padding = vec![0; TILE_BYTE_LEN];
        padding[4..8].copy_from_slice(&[9, 9, 9, 255]);
        let snapshot = TileSnapshot::from_tiles([
            (key(1, 1), padding),
            (key(1, -1), [7, 7, 7, 255].repeat(TILE_BYTE_LEN / 4)),
            (key(2, 0), [5, 5, 5, 255].repeat(TILE_BYTE_LEN / 4)),
        ])
        .expect("signed fixture");
        let selection = lasso_selection(
            canvas(129, 2),
            &[[0, 0], [129, 0], [129, 2], [0, 2]],
            EditLimits::default(),
        )
        .expect("page mask");
        let fill = SelectionPaint::Solid(PremultipliedRgba8([0, 255, 0, 255]));
        let painted = paint_selection(
            &snapshot,
            &tree(false),
            LayerId(1),
            &selection,
            fill,
            EditLimits::default(),
        )
        .expect("fill");
        assert_eq!(painted.changed_tiles, vec![key(1, 0), key(1, 1)]);
        assert_eq!(painted.after.get(key(1, -1)), snapshot.get(key(1, -1)));
        assert_eq!(painted.after.get(key(2, 0)), snapshot.get(key(2, 0)));
        assert_eq!(
            &painted
                .after
                .get(key(1, 1))
                .expect("boundary tile")
                .pixels()[..8],
            &[0, 255, 0, 255, 9, 9, 9, 255]
        );
        let gradient = SelectionPaint::LinearGradient {
            start: [0, 0],
            end: [2, 0],
            start_color: PremultipliedRgba8([255, 0, 0, 255]),
            end_color: PremultipliedRgba8::transparent(),
        };
        let painted = paint_selection(
            &snapshot,
            &tree(false),
            LayerId(1),
            &selection,
            gradient,
            EditLimits::default(),
        )
        .expect("gradient");
        assert_eq!(
            &painted
                .after
                .get(key(1, 0))
                .expect("gradient tile")
                .pixels()[..12],
            &[191, 0, 0, 191, 64, 0, 0, 64, 0, 0, 0, 0]
        );
        let before = snapshot.clone();
        assert!(matches!(
            paint_selection(
                &snapshot,
                &tree(true),
                LayerId(1),
                &selection,
                fill,
                EditLimits::default()
            ),
            Err(EditError::LockedLayer)
        ));
        assert!(matches!(
            paint_selection(
                &snapshot,
                &tree(false),
                LayerId(1),
                &selection,
                fill,
                EditLimits {
                    max_workspace_bytes: 1,
                    ..EditLimits::default()
                }
            ),
            Err(EditError::LimitExceeded)
        ));
        assert!(matches!(
            paint_selection(
                &snapshot,
                &tree(false),
                LayerId(1),
                &selection,
                SelectionPaint::Solid(PremultipliedRgba8([255, 0, 0, 0])),
                EditLimits::default()
            ),
            Err(EditError::InvalidPaint)
        ));
        assert_eq!(snapshot, before);
        let small = lasso_selection(
            canvas(257, 1),
            &[[0, 0], [1, 0], [1, 1], [0, 1]],
            EditLimits::default(),
        )
        .expect("one selected pixel on a wider page");
        let bounded = paint_selection(
            &snapshot,
            &tree(false),
            LayerId(1),
            &small,
            fill,
            EditLimits {
                max_workspace_bytes: 70_000,
                ..EditLimits::default()
            },
        )
        .expect("budget follows selected tiles, not untouched page tiles");
        assert_eq!(bounded.changed_tiles, vec![key(1, 0)]);
        let noop = paint_selection(
            &snapshot,
            &tree(false),
            LayerId(1),
            &small,
            SelectionPaint::Solid(PremultipliedRgba8::transparent()),
            EditLimits::default(),
        )
        .expect("transparent source-over");
        assert!(noop.changed_tiles.is_empty());
        assert_eq!(noop.after, snapshot);
    }
}

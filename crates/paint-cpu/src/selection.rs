//! Bounded, deterministic signed-region selection and pixel editing.
use crate::{CpuCompositeError, PremultipliedRgba8, composite_surface, flatten_layer_tree_rgba8};
use nyatidraw_api::{CanvasSpec, LayerId, LayerTreeNodeId};
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot, TileSnapshotError};
use std::collections::VecDeque;

#[path = "selection_morph.rs"]
mod morphology;
pub use morphology::{grow_selection, select_layer_alpha, shrink_selection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditLimits {
    pub max_pixels: u64,
    pub max_workspace_bytes: u64,
    pub max_frontier: usize,
    pub max_polygon_work: u64,
    /// Bounded linear image-filter work, separate from polygon edge work.
    pub max_filter_work: u64,
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            max_pixels: 16 * 1024 * 1024,
            max_workspace_bytes: 256 * 1024 * 1024,
            max_frontier: 1024 * 1024,
            max_polygon_work: 64 * 1024 * 1024,
            max_filter_work: 1024 * 1024 * 1024,
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
            max_filter_work: self.max_filter_work.min(maximum.max_filter_work),
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
    AlphaLockedLayer,
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

/// Binary pixel-center coverage in a bounded signed document rectangle.
/// Bytes are currently 0/1, not fractional antialias coverage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionMask {
    origin: [i32; 2],
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
        Self::from_packed_bits_at([0, 0], [width, height], packed)
    }

    /// Restores a bounded signed mask; origin-zero payloads retain their encoding.
    /// # Errors
    /// Rejects malformed bits, oversized extents and coordinate overflow.
    pub fn from_packed_bits_at(
        origin: [i32; 2],
        dimensions: [u32; 2],
        packed: &[u8],
    ) -> Result<Self, EditError> {
        let [width, height] = dimensions;
        let count = region_len(origin, dimensions, EditLimits::default())?;
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
            origin,
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
    pub const fn origin(&self) -> [i32; 2] {
        self.origin
    }

    #[must_use]
    pub const fn selected_pixels(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn contains(&self, x: u32, y: u32) -> bool {
        let [Ok(x), Ok(y)] = [x, y].map(i32::try_from) else {
            return false;
        };
        self.contains_signed(x, y)
    }

    #[must_use]
    pub fn contains_signed(&self, x: i32, y: i32) -> bool {
        let [Ok(x), Ok(y)] = [
            i64::from(x) - i64::from(self.origin[0]),
            i64::from(y) - i64::from(self.origin[1]),
        ]
        .map(u32::try_from) else {
            return false;
        };
        x < self.width && y < self.height && self.selected[pixel_index(self.width, x, y)] != 0
    }

    /// Computes the selected pixel rectangle on the CPU editing worker.
    /// The result is [left, top, width, height]; empty coverage has no bounds.
    #[must_use]
    pub fn bounds(&self) -> Option<[u32; 4]> {
        let (origin, [width, height]) = self.bounds_signed()?;
        let [Ok(x), Ok(y)] = origin.map(u32::try_from) else {
            return None;
        };
        Some([x, y, width, height])
    }

    /// Selected document origin and extent, excluding unselected padding.
    #[must_use]
    pub fn bounds_signed(&self) -> Option<([i32; 2], [u32; 2])> {
        if self.count == 0 {
            return None;
        }
        let mut left = self.width;
        let mut top = self.height;
        let mut right = 0;
        let mut bottom = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.selected[pixel_index(self.width, x, y)] != 0 {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x);
                    bottom = bottom.max(y);
                }
            }
        }
        Some((
            [
                i32::try_from(i64::from(self.origin[0]) + i64::from(left)).ok()?,
                i32::try_from(i64::from(self.origin[1]) + i64::from(top)).ok()?,
            ],
            [right - left + 1, bottom - top + 1],
        ))
    }
}

fn region_len(origin: [i32; 2], size: [u32; 2], limits: EditLimits) -> Result<usize, EditError> {
    if size.contains(&0) {
        return Err(EditError::InvalidMask);
    }
    for axis in 0..2 {
        if i32::try_from(i64::from(origin[axis]) + i64::from(size[axis]) - 1).is_err() {
            return Err(EditError::CoordinateOutOfRange);
        }
    }
    let count = u64::from(size[0]) * u64::from(size[1]);
    if count > limits.max_pixels {
        return Err(EditError::LimitExceeded);
    }
    require_workspace(count, limits)?;
    usize::try_from(count).map_err(|_| EditError::LimitExceeded)
}

/// Selection boolean operations on document pixels, not mask-local indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionCombine {
    Replace,
    Add,
    Subtract,
}

/// Selects a caller-defined finite domain. No infinite allocation is implied.
/// # Errors
/// Rejects coordinate overflow and configured work/memory limits.
pub fn selection_all(
    origin: [i32; 2],
    size: [u32; 2],
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let count = region_len(origin, size, limits.bounded())?;
    Ok(SelectionMask {
        origin,
        width: size[0],
        height: size[1],
        selected: vec![1; count],
        count: count as u64,
    })
}

/// Half-open signed rectangle; dragging backwards gives the same pixels.
/// # Errors
/// Rejects empty rectangles, coordinate overflow or oversized regions.
pub fn rectangle_selection_signed(
    start: [i32; 2],
    end: [i32; 2],
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let origin = std::array::from_fn(|axis| start[axis].min(end[axis]));
    let size = std::array::from_fn(|axis| start[axis].abs_diff(end[axis]));
    selection_all(origin, size, limits)
}

/// Combines bounded document coverage without modifying either input.
/// # Errors
/// Rejects an oversized union before allocating its result.
pub fn combine_selection(
    current: &SelectionMask,
    incoming: &SelectionMask,
    operation: SelectionCombine,
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let limits = limits.bounded();
    let (origin, size) = match operation {
        SelectionCombine::Replace => (incoming.origin(), incoming.dimensions()),
        SelectionCombine::Subtract => (current.origin(), current.dimensions()),
        SelectionCombine::Add => {
            let origin =
                std::array::from_fn(|axis| current.origin[axis].min(incoming.origin[axis]));
            let end: [i64; 2] = std::array::from_fn(|axis| {
                (i64::from(current.origin[axis]) + i64::from(current.dimensions()[axis]))
                    .max(i64::from(incoming.origin[axis]) + i64::from(incoming.dimensions()[axis]))
            });
            let [Ok(w), Ok(h)] =
                std::array::from_fn::<_, 2, _>(|axis| end[axis] - i64::from(origin[axis]))
                    .map(u32::try_from)
            else {
                return Err(EditError::LimitExceeded);
            };
            (origin, [w, h])
        }
    };
    let count = region_len(origin, size, limits)?;
    require_workspace(
        count as u64 + current.selected.len() as u64 + incoming.selected.len() as u64,
        limits,
    )?;
    let mut mask = selection_all(origin, size, limits)?;
    mask.count = 0;
    for y in 0..size[1] {
        for x in 0..size[0] {
            let px = i32::try_from(i64::from(origin[0]) + i64::from(x))
                .map_err(|_| EditError::CoordinateOutOfRange)?;
            let py = i32::try_from(i64::from(origin[1]) + i64::from(y))
                .map_err(|_| EditError::CoordinateOutOfRange)?;
            let a = current.contains_signed(px, py);
            let b = incoming.contains_signed(px, py);
            let selected = match operation {
                SelectionCombine::Replace => b,
                SelectionCombine::Add => a || b,
                SelectionCombine::Subtract => a && !b,
            };
            mask.selected[pixel_index(size[0], x, y)] = u8::from(selected);
            mask.count += u64::from(selected);
        }
    }
    Ok(mask)
}

/// Complements coverage only inside the explicit finite domain.
/// # Errors
/// Rejects coordinate overflow or oversized working memory atomically.
pub fn invert_selection(
    mask: &SelectionMask,
    origin: [i32; 2],
    size: [u32; 2],
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    let domain = selection_all(origin, size, limits)?;
    combine_selection(&domain, mask, SelectionCombine::Subtract, limits)
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
pub(crate) fn restrict_references(group: &mut GroupNode, ancestors_visible: bool) -> usize {
    use nyatidraw_document::{CompositionScope, composition_scope_nodes};
    fn apply(group: &mut GroupNode, nodes: &std::collections::BTreeSet<LayerTreeNodeId>) -> usize {
        let mut references = 0;
        for child in &mut group.children {
            match child {
                LayerTreeNode::Raster(layer) => {
                    layer.visible = nodes.contains(&LayerTreeNodeId::Raster(layer.id));
                    references +=
                        usize::from(layer.visible && layer.reference && layer.opacity_u16 != 0);
                }
                LayerTreeNode::Group(nested) => {
                    nested.visible = nodes.contains(&LayerTreeNodeId::Group(nested.id));
                    references += apply(nested, nodes);
                }
            }
        }
        references
    }
    let nodes = if ancestors_visible {
        composition_scope_nodes(group, CompositionScope::Reference)
    } else {
        std::collections::BTreeSet::new()
    };
    // Resolve dependencies before mutating visibility; preserve sibling order,
    // orphan clips, and all metadata so hidden bases never rebind a stack.
    apply(group, &nodes)
}

pub(crate) fn group_depth(group: &GroupNode) -> u64 {
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

/// Samples durable artwork at a signed document pixel, including off-page ink.
/// Active-layer mode reads raw pixels; composite modes honor visibility and
/// group isolation/opacity, never viewport decorations or session Solo.
///
/// # Errors
/// Rejects a missing active layer or an empty visible reference set.
pub fn sample_artwork_pixel(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    active: LayerId,
    point: [i32; 2],
    source: SelectionSource,
) -> Result<[u8; 4], EditError> {
    if source == SelectionSource::ActiveLayer {
        tree.raster(active).ok_or(EditError::UnknownLayer)?;
        return Ok(raster_pixel(snapshot, active, point));
    }
    if source == SelectionSource::ReferenceLayers {
        let nodes = nyatidraw_document::composition_scope_nodes(
            tree.root(),
            nyatidraw_document::CompositionScope::Reference,
        );
        if nodes.is_empty() {
            return Err(EditError::MissingReference);
        }
        return Ok(crate::compositing::group_pixel(
            snapshot,
            tree.root(),
            point,
            Some(&nodes),
        ));
    }
    Ok(group_pixel(snapshot, tree.root(), point))
}

pub(crate) fn raster_pixel(snapshot: &TileSnapshot, layer: LayerId, point: [i32; 2]) -> [u8; 4] {
    let key = TileKey::from_pixel(layer, 128, point[0], point[1]);
    let offset = usize::try_from((point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128)) * 4)
        .expect("tile offset");
    snapshot.get(key).map_or([0; 4], |tile| {
        tile.pixels()[offset..offset + 4]
            .try_into()
            .expect("RGBA pixel")
    })
}

/// Samples only displayed artwork using the native compositor's Solo rules.
/// No UI decoration or presentation-only state is written to the document.
///
/// # Errors
/// Rejects a stale Solo target rather than returning a misleading color.
pub fn sample_display_pixel(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    point: [i32; 2],
    solo: Option<LayerTreeNodeId>,
) -> Result<[u8; 4], EditError> {
    if let Some(target) = solo
        && target != LayerTreeNodeId::Group(tree.root_id())
        && tree.parent_and_index(target).is_none()
    {
        return Err(EditError::UnknownLayer);
    }
    let scope = solo.map(|target| {
        nyatidraw_document::composition_scope_nodes(
            tree.root(),
            nyatidraw_document::CompositionScope::Solo(target),
        )
    });
    Ok(crate::compositing::group_pixel(
        snapshot,
        tree.root(),
        point,
        scope.as_ref(),
    ))
}

pub(crate) fn group_pixel(snapshot: &TileSnapshot, group: &GroupNode, point: [i32; 2]) -> [u8; 4] {
    crate::compositing::group_pixel(snapshot, group, point, None)
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
        origin: [0, 0],
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
        origin: [0, 0],
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

/// Selects a polygon without page clipping, in its bounded signed rectangle.
/// Coverage is binary pixel-center even-odd, not edge antialiasing.
/// # Errors
/// Rejects invalid polygons, excessive work and coordinate/region limits.
pub fn lasso_selection_signed(
    vertices: &[[i32; 2]],
    limits: EditLimits,
) -> Result<SelectionMask, EditError> {
    if !(3..=4096).contains(&vertices.len()) {
        return Err(EditError::InvalidPolygon);
    }
    let origin: [i32; 2] =
        std::array::from_fn(|axis| vertices.iter().map(|p| p[axis]).min().unwrap_or(0));
    let end: [i32; 2] =
        std::array::from_fn(|axis| vertices.iter().map(|p| p[axis]).max().unwrap_or(0));
    let size = std::array::from_fn(|axis| end[axis].abs_diff(origin[axis]));
    region_len(origin, size, limits.bounded())?;
    let local: Vec<[i32; 2]> = vertices
        .iter()
        .map(|p| {
            let [Ok(x), Ok(y)] =
                std::array::from_fn::<_, 2, _>(|axis| i64::from(p[axis]) - i64::from(origin[axis]))
                    .map(i32::try_from)
            else {
                return Err(EditError::CoordinateOutOfRange);
            };
            Ok([x, y])
        })
        .collect::<Result<_, EditError>>()?;
    let mut mask = lasso_selection(
        CanvasSpec {
            width_px: size[0],
            height_px: size[1],
            pixels_per_inch: 96,
        },
        &local,
        limits,
    )?;
    mask.origin = origin;
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

fn paint_pixel(paint: SelectionPaint, x: i32, y: i32) -> [u8; 4] {
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

/// Clears one raster across all signed tiles, keeping every other layer exact.
///
/// # Errors
/// Rejects missing or locked layers without modifying the immutable source.
pub fn clear_raster(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
) -> Result<SelectionPaintResult, EditError> {
    let layer = find_raster(tree.root(), target).ok_or(EditError::UnknownLayer)?;
    if layer.locked {
        return Err(EditError::LockedLayer);
    }
    if layer.alpha_locked {
        return Err(EditError::AlphaLockedLayer);
    }
    let changed_tiles: Vec<_> = snapshot
        .iter()
        .filter(|(key, _)| key.layer == target)
        .map(|(key, _)| key)
        .collect();
    let after = snapshot
        .with_replacements(
            changed_tiles
                .iter()
                .map(|key| (*key, vec![0; TILE_BYTE_LEN])),
        )
        .map_err(EditError::Tiles)?;
    Ok(SelectionPaintResult {
        after,
        changed_tiles,
    })
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
    let count = region_len(mask.origin, mask.dimensions(), limits)?;
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
        let (left, top) = key.pixel_origin();
        for row in 0..TILE_EDGE {
            for column in 0..TILE_EDGE {
                let [Ok(x), Ok(y)] =
                    [left + i64::from(column), top + i64::from(row)].map(i32::try_from)
                else {
                    continue;
                };
                if !mask.contains_signed(x, y) {
                    continue;
                }
                let offset = (row * TILE_EDGE + column) as usize * 4;
                let source = paint_pixel(paint, x, y);
                if layer.alpha_locked {
                    crate::composite_source_atop(
                        &mut pixels[offset..offset + 4],
                        PremultipliedRgba8(source),
                        1.0,
                    );
                } else {
                    composite_surface(&mut pixels[offset..offset + 4], &source, u16::MAX);
                }
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
    let mut keys = std::collections::BTreeSet::new();
    for y in 0..mask.height {
        for x in 0..mask.width {
            if mask.selected[pixel_index(mask.width, x, y)] == 0 {
                continue;
            }
            let point = [
                i64::from(mask.origin[0]) + i64::from(x),
                i64::from(mask.origin[1]) + i64::from(y),
            ]
            .map(|v| i32::try_from(v).expect("validated region"));
            let key = TileKey::from_pixel(target, 128, point[0], point[1]);
            if keys.contains(&key) {
                continue;
            }
            let bytes = mask.selected.len() as u64
                + (keys.len() as u64 + 1) * (TILE_BYTE_LEN as u64 * 2 + 256);
            require_workspace(bytes, limits)?;
            keys.insert(key);
        }
    }
    Ok(keys.into_iter().collect())
}

/// Deletes selected signed artwork, retaining the selection and other pixels.
/// # Errors
/// Rejects locked/missing layers or exceeded bounds without publishing edits.
pub fn clear_selection(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    mask: &SelectionMask,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    if tree
        .raster(target)
        .ok_or(EditError::UnknownLayer)?
        .alpha_locked
    {
        return Err(EditError::AlphaLockedLayer);
    }
    if mask.selected_pixels() == 0 {
        if tree.raster(target).ok_or(EditError::UnknownLayer)?.locked {
            return Err(EditError::LockedLayer);
        }
        return Ok(SelectionPaintResult {
            after: snapshot.clone(),
            changed_tiles: Vec::new(),
        });
    }
    crate::cut_selection(snapshot, tree, target, mask, limits)
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
            alpha_locked: false,
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
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
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![
                raster(1, false, locked),
                LayerTreeNode::Group(GroupNode {
                    clip_to_below: false,
                    blend_mode: nyatidraw_api::LayerBlendMode::Normal,
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
    fn signed_selection_algebra_and_edits_preserve_offpage_pixels_or_fail_atomically() {
        // Product risk: local/document coordinate confusion corrupts negative
        // artwork, while huge sparse unions must fail before allocating.
        let limits = EditLimits::default();
        let a = rectangle_selection_signed([-130, -2], [1, 1], limits).unwrap();
        let b = lasso_selection_signed(&[[-129, -1], [2, -1], [2, 2], [-129, 2]], limits).unwrap();
        assert_eq!(a.origin(), [-130, -2]);
        assert_eq!(a.bounds_signed(), Some(([-130, -2], [131, 3])));
        assert_eq!(
            SelectionMask::from_packed_bits_at(a.origin(), a.dimensions(), &a.packed_bits())
                .unwrap(),
            a
        );
        for operation in [
            SelectionCombine::Replace,
            SelectionCombine::Add,
            SelectionCombine::Subtract,
        ] {
            let result = combine_selection(&a, &b, operation, limits).unwrap();
            for y in -3..3 {
                for x in -131..3 {
                    let expected = match operation {
                        SelectionCombine::Replace => b.contains_signed(x, y),
                        SelectionCombine::Add => a.contains_signed(x, y) || b.contains_signed(x, y),
                        SelectionCombine::Subtract => {
                            a.contains_signed(x, y) && !b.contains_signed(x, y)
                        }
                    };
                    assert_eq!(result.contains_signed(x, y), expected);
                }
            }
        }
        let inverse = invert_selection(&a, [-131, -3], [134, 6], limits).unwrap();
        assert!(!inverse.contains_signed(-130, -2));
        assert!(inverse.contains_signed(-131, -3));
        let original = TileSnapshot::default();
        let painted = paint_selection(
            &original,
            &tree(false),
            LayerId(1),
            &a,
            SelectionPaint::Solid(PremultipliedRgba8([64, 0, 0, 128])),
            limits,
        )
        .unwrap()
        .after;
        assert_eq!(
            sample_artwork_pixel(
                &painted,
                &tree(false),
                LayerId(1),
                [-130, -2],
                SelectionSource::ActiveLayer
            )
            .unwrap(),
            [64, 0, 0, 128]
        );
        assert_eq!(
            sample_artwork_pixel(
                &painted,
                &tree(false),
                LayerId(1),
                [-131, -2],
                SelectionSource::ActiveLayer
            )
            .unwrap(),
            [0; 4]
        );
        let fragment =
            crate::copy_selection(&painted, &tree(false), LayerId(1), &a, limits).unwrap();
        assert_eq!(fragment.origin(), [-130, -2]);
        let cleared = clear_selection(&painted, &tree(false), LayerId(1), &a, limits)
            .unwrap()
            .after;
        assert_eq!(cleared, original);
        assert_eq!(
            crate::paste_fragment(&cleared, &tree(false), LayerId(1), &fragment, limits)
                .unwrap()
                .after,
            painted
        );
        let shifted = crate::transform_raster(
            &painted,
            &tree(false),
            LayerId(1),
            Some(&a),
            nyatidraw_api::RasterTransform {
                offset: [-128, 128],
                ..Default::default()
            },
            limits,
        )
        .unwrap()
        .after;
        assert_eq!(
            sample_artwork_pixel(
                &shifted,
                &tree(false),
                LayerId(1),
                [-258, 126],
                SelectionSource::ActiveLayer
            )
            .unwrap(),
            [64, 0, 0, 128]
        );
        assert_eq!(
            sample_artwork_pixel(
                &shifted,
                &tree(false),
                LayerId(1),
                [-130, -2],
                SelectionSource::ActiveLayer
            )
            .unwrap(),
            [0; 4]
        );
        let far = selection_all([i32::MAX, 0], [1, 1], limits).unwrap();
        assert_eq!(
            combine_selection(&a, &far, SelectionCombine::Add, limits),
            Err(EditError::LimitExceeded)
        );
        assert_eq!(
            selection_all([i32::MAX, 0], [2, 1], limits),
            Err(EditError::CoordinateOutOfRange)
        );
        assert!(
            paint_selection(
                &painted,
                &tree(false),
                LayerId(1),
                &a,
                SelectionPaint::Solid(PremultipliedRgba8([255; 4])),
                EditLimits {
                    max_workspace_bytes: 1,
                    ..limits
                }
            )
            .is_err()
        );
        assert_eq!(
            crate::copy_selection(&painted, &tree(false), LayerId(1), &a, limits).unwrap(),
            fragment
        );
    }

    #[test]
    fn sampling_preserves_signed_pixels_and_matches_isolated_layer_composition() {
        // Product risk: picking a decoration, wrong layer or double-applied
        // group opacity silently changes the color used in subsequent artwork.
        let mut tree = tree(true);
        let snapshot = TileSnapshot::from_tiles([
            (key(1, -1), [128, 0, 0, 128].repeat(TILE_BYTE_LEN / 4)),
            (key(1, 0), [255, 0, 0, 255].repeat(TILE_BYTE_LEN / 4)),
            (key(2, 0), [0, 0, 255, 255].repeat(TILE_BYTE_LEN / 4)),
        ])
        .unwrap();
        let original_root = snapshot.root();
        for (point, source, expected) in [
            ([-1, 0], SelectionSource::ActiveLayer, [128, 0, 0, 128]),
            ([0, 0], SelectionSource::ActiveLayer, [255, 0, 0, 255]),
            ([0, 0], SelectionSource::ReferenceLayers, [0, 0, 128, 128]),
            ([0, 0], SelectionSource::AllVisible, [127, 0, 128, 255]),
            ([128, 0], SelectionSource::AllVisible, [0; 4]),
            ([0, -1], SelectionSource::ActiveLayer, [0; 4]),
        ] {
            assert_eq!(
                sample_artwork_pixel(&snapshot, &tree, LayerId(1), point, source).unwrap(),
                expected
            );
        }
        let flat = flatten_layer_tree_rgba8(&snapshot, &tree, canvas(2, 1)).unwrap();
        assert_eq!(
            &flat.pixels[..4],
            &sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(1),
                [0, 0],
                SelectionSource::AllVisible,
            )
            .unwrap()
        );
        tree.set_visibility(LayerTreeNodeId::Raster(LayerId(1)), false)
            .unwrap();
        assert_eq!(
            sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(1),
                [0, 0],
                SelectionSource::ActiveLayer
            )
            .unwrap(),
            [255, 0, 0, 255]
        );
        assert_eq!(
            sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(1),
                [0, 0],
                SelectionSource::AllVisible
            )
            .unwrap(),
            [0, 0, 128, 128]
        );
        tree.set_visibility(LayerTreeNodeId::Group(GroupId(10)), false)
            .unwrap();
        // Display sampling follows session Solo, unlike durable AllVisible:
        // a hidden target path is revealed, preserving group opacity exactly.
        for (solo, expected) in [
            (None, [0; 4]),
            (Some(LayerTreeNodeId::Raster(LayerId(1))), [255, 0, 0, 255]),
            (Some(LayerTreeNodeId::Raster(LayerId(2))), [0, 0, 128, 128]),
            (Some(LayerTreeNodeId::Group(GroupId(10))), [0, 0, 128, 128]),
        ] {
            assert_eq!(
                sample_display_pixel(&snapshot, &tree, [0, 0], solo).unwrap(),
                expected
            );
        }
        assert!(matches!(
            sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(1),
                [0, 0],
                SelectionSource::ReferenceLayers
            ),
            Err(EditError::MissingReference)
        ));
        assert!(matches!(
            sample_artwork_pixel(
                &snapshot,
                &tree,
                LayerId(99),
                [0, 0],
                SelectionSource::ActiveLayer
            ),
            Err(EditError::UnknownLayer)
        ));
        assert_eq!(snapshot.root(), original_root);
    }

    #[test]
    fn clear_raster_preserves_other_layers_and_removes_signed_outside_pixels() {
        // Product risk: clearing a layer must not delete another layer, leave
        // invisible off-page pixels behind, or mutate a locked layer/history.
        let original = TileSnapshot::from_tiles(
            [key(1, -20), key(1, 0), key(1, 300), key(2, 0)]
                .map(|key| (key, vec![255; TILE_BYTE_LEN])),
        )
        .expect("fixture tiles");
        let root = original.root();
        let cleared = clear_raster(&original, &tree(false), LayerId(1)).expect("clear");
        assert_eq!(cleared.changed_tiles.len(), 3);
        assert_eq!(cleared.after.len(), 1);
        assert_eq!(cleared.after.get(key(2, 0)), original.get(key(2, 0)));
        assert_eq!(original.root(), root);
        assert!(
            clear_raster(&cleared.after, &tree(false), LayerId(1))
                .expect("empty clear")
                .changed_tiles
                .is_empty()
        );
        assert!(matches!(
            clear_raster(&original, &tree(true), LayerId(1)),
            Err(EditError::LockedLayer)
        ));
        assert!(matches!(
            clear_raster(&original, &tree(false), LayerId(99)),
            Err(EditError::UnknownLayer)
        ));
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
                max_workspace_bytes: 140_000,
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

//! Bounded reference-line fill. Source sampling is immutable; tolerance,
//! morphological gap closing, and barrier-only backing expansion are separate.
use crate::selection::{group_pixel, raster_pixel, restrict_references};
use crate::{
    EditError, EditLimits, PremultipliedRgba8, SelectionMask, SelectionPaintResult,
    SelectionSource, composite_surface,
};
use nyatidraw_api::{CanvasSpec, FillSettings, LayerId};
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FloodFillRequest {
    pub canvas: CanvasSpec,
    pub active: LayerId,
    pub source: SelectionSource,
    pub seed: [i32; 2],
    pub tolerance: u8,
    pub settings: FillSettings,
}

struct Source<'a> {
    snapshot: &'a TileSnapshot,
    active: Option<LayerId>,
    root: GroupNode,
    layers: BTreeSet<LayerId>,
    work_per_pixel: u64,
    tree_bytes: u64,
}

impl<'a> Source<'a> {
    fn new(
        snapshot: &'a TileSnapshot,
        tree: &LayerTree,
        request: FloodFillRequest,
    ) -> Result<Self, EditError> {
        fn visit(
            group: &GroupNode,
            layers: &mut BTreeSet<LayerId>,
            work: &mut u64,
            bytes: &mut u64,
        ) {
            *work += 1;
            *bytes += 256 + group.name.len() as u64;
            for child in &group.children {
                *work += 1;
                match child {
                    LayerTreeNode::Raster(layer) => {
                        *bytes += 256 + layer.name.len() as u64;
                        if layer.visible && layer.opacity_u16 != 0 {
                            layers.insert(layer.id);
                        }
                    }
                    LayerTreeNode::Group(group) => {
                        // Count every node for bounded traversal. Hidden descendants
                        // do not enlarge the sampled source domain.
                        let mut hidden = BTreeSet::new();
                        visit(
                            group,
                            if group.visible && group.opacity_u16 != 0 {
                                layers
                            } else {
                                &mut hidden
                            },
                            work,
                            bytes,
                        );
                    }
                }
            }
        }
        let mut root = tree.root().clone();
        if request.source == SelectionSource::ReferenceLayers
            && restrict_references(&mut root, true) == 0
        {
            return Err(EditError::MissingReference);
        }
        let mut layers = BTreeSet::new();
        let mut work_per_pixel = 0;
        let mut tree_bytes = 0;
        visit(&root, &mut layers, &mut work_per_pixel, &mut tree_bytes);
        let active = (request.source == SelectionSource::ActiveLayer).then_some(request.active);
        if active.is_some() {
            layers.clear();
            layers.insert(request.active);
            work_per_pixel = 1;
        }
        Ok(Self {
            snapshot,
            active,
            root,
            layers,
            work_per_pixel,
            tree_bytes,
        })
    }

    fn pixel(&self, point: [i32; 2]) -> [u8; 4] {
        self.active.map_or_else(
            || group_pixel(self.snapshot, &self.root, point),
            |layer| raster_pixel(self.snapshot, layer, point),
        )
    }
}

#[derive(Clone, Copy)]
struct Domain {
    origin: [i32; 2],
    width: usize,
    height: usize,
}

impl Domain {
    fn new(
        source: &Source<'_>,
        request: FloodFillRequest,
        clip: Option<&SelectionMask>,
    ) -> Result<Self, EditError> {
        request
            .canvas
            .validate()
            .map_err(|_| EditError::InvalidCanvas)?;
        let mut lo = [0_i64; 2];
        let mut hi = [
            i64::from(request.canvas.width_px) - 1,
            i64::from(request.canvas.height_px) - 1,
        ];
        for axis in 0..2 {
            lo[axis] = lo[axis].min(i64::from(request.seed[axis]));
            hi[axis] = hi[axis].max(i64::from(request.seed[axis]));
        }
        for (key, _) in source.snapshot.iter() {
            if !source.layers.contains(&key.layer) {
                continue;
            }
            if key.mip != 0 {
                return Err(EditError::InvalidPaint);
            }
            let (x, y) = key.pixel_origin();
            for (axis, coordinate) in [x, y].into_iter().enumerate() {
                lo[axis] = lo[axis].min(coordinate);
                hi[axis] = hi[axis].max(coordinate + i64::from(TILE_EDGE) - 1);
            }
        }
        if let Some(clip) = clip {
            if !clip.contains_signed(request.seed[0], request.seed[1]) {
                return Err(EditError::InvalidSeed);
            }
            let (origin, size) = clip.bounds_signed().ok_or(EditError::InvalidSeed)?;
            for axis in 0..2 {
                lo[axis] = lo[axis].max(i64::from(origin[axis]));
                hi[axis] = hi[axis].min(i64::from(origin[axis]) + i64::from(size[axis]) - 1);
            }
        }
        let [Ok(x), Ok(y)] = lo.map(i32::try_from) else {
            return Err(EditError::CoordinateOutOfRange);
        };
        if hi.into_iter().any(|value| i32::try_from(value).is_err()) {
            return Err(EditError::CoordinateOutOfRange);
        }
        let [Ok(width), Ok(height)] = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1].map(usize::try_from)
        else {
            return Err(EditError::LimitExceeded);
        };
        Ok(Self {
            origin: [x, y],
            width,
            height,
        })
    }

    fn len(self) -> Result<usize, EditError> {
        self.width
            .checked_mul(self.height)
            .ok_or(EditError::LimitExceeded)
    }

    fn point(self, index: usize) -> [i32; 2] {
        // The complete signed extent was checked before allocation.
        [
            i32::try_from(
                i64::from(self.origin[0])
                    + i64::try_from(index % self.width).expect("bounded column"),
            )
            .expect("validated domain x"),
            i32::try_from(
                i64::from(self.origin[1]) + i64::try_from(index / self.width).expect("bounded row"),
            )
            .expect("validated domain y"),
        ]
    }

    fn index(self, point: [i32; 2]) -> usize {
        let x = usize::try_from(i64::from(point[0]) - i64::from(self.origin[0]))
            .expect("seed inside domain");
        let y = usize::try_from(i64::from(point[1]) - i64::from(self.origin[1]))
            .expect("seed inside domain");
        y * self.width + x
    }

    fn neighbors(self, index: usize) -> [Option<usize>; 4] {
        let x = index % self.width;
        let y = index / self.width;
        [
            (x > 0).then(|| index - 1),
            (x + 1 < self.width).then_some(index + 1),
            (y > 0).then(|| index - self.width),
            (y + 1 < self.height).then_some(index + self.width),
        ]
    }
}

/// A separable square min/max filter. Edge replication preserves frame lines;
/// the finite domain boundary is not treated as an opening into infinity.
fn morphology(input: &[u8], domain: Domain, radius: usize, dilate: bool) -> Vec<u8> {
    #[allow(
        clippy::too_many_arguments,
        reason = "Explicit matrix strides share one bounded horizontal/vertical filter implementation"
    )]
    fn pass(
        input: &[u8],
        output: &mut [u8],
        lines: usize,
        length: usize,
        stride: usize,
        step: usize,
        radius: usize,
        dilate: bool,
    ) {
        for line in 0..lines {
            let base = line * step;
            let mut sum = 0;
            let mut left = 0;
            let mut right = 0;
            for position in 0..length {
                let end = (position + radius + 1).min(length);
                while right < end {
                    sum += usize::from(input[base + right * stride]);
                    right += 1;
                }
                let start = position.saturating_sub(radius);
                while left < start {
                    sum -= usize::from(input[base + left * stride]);
                    left += 1;
                }
                output[base + position * stride] =
                    u8::from(if dilate { sum > 0 } else { sum == right - left });
            }
        }
    }
    let mut horizontal = vec![0; input.len()];
    let mut output = vec![0; input.len()];
    pass(
        input,
        &mut horizontal,
        domain.height,
        domain.width,
        1,
        domain.width,
        radius,
        dilate,
    );
    pass(
        &horizontal,
        &mut output,
        domain.width,
        domain.height,
        domain.width,
        1,
        radius,
        dilate,
    );
    output
}

fn enqueue(queue: &mut VecDeque<usize>, index: usize, limits: EditLimits) -> Result<(), EditError> {
    if queue.len() >= limits.max_frontier {
        return Err(EditError::LimitExceeded);
    }
    queue.push_back(index);
    Ok(())
}

/// Fills a four-connected seed region on an immutable sampled source. Domain
/// is page union relevant signed source-tile extents and seed, intersected with
/// selection bounds. Selection holes block traversal and expansion.
///
/// `gap_close_px` is square morphological barrier closing (0..=8), independent
/// of RGBA tolerance. `expand_px` (0..=64) is Manhattan backing distance only
/// into barriers, never another matching component. Antialias adds a minimum
/// one-pixel backing fringe under the source line, not supersampling or a
/// fractional selection. Barrier backing uses destination-over so existing
/// target lines survive even when target participates in the sampled source.
///
/// # Errors
/// Missing/locked targets, missing references, invalid settings/colors/seeds,
/// unsupported source mip levels and work/memory/frontier exhaustion reject
/// atomically; no snapshot, history, or input tree is mutated.
#[allow(
    clippy::too_many_lines,
    reason = "Sequential immutable source, region, backing and atomic publication phases are reviewed together"
)]
pub fn flood_fill(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    request: FloodFillRequest,
    clip: Option<&SelectionMask>,
    color: PremultipliedRgba8,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    let limits = limits.bounded();
    let target = tree.raster(request.active).ok_or(EditError::UnknownLayer)?;
    if target.locked {
        return Err(EditError::LockedLayer);
    }
    if request.settings.gap_close_px > 8
        || request.settings.expand_px > 64
        || color.0[..3].iter().any(|channel| *channel > color.0[3])
        || snapshot
            .iter()
            .any(|(key, _)| key.layer == request.active && key.mip != 0)
    {
        return Err(EditError::InvalidPaint);
    }
    if limits.max_frontier == 0 {
        return Err(EditError::LimitExceeded);
    }
    let source = Source::new(snapshot, tree, request)?;
    let domain = Domain::new(&source, request, clip)?;
    let count = domain.len()?;
    let count_u64 = count as u64;
    let radius = request
        .settings
        .expand_px
        .max(u8::from(request.settings.antialias));
    let work = count_u64.saturating_mul(
        source.work_per_pixel
            + 16
            + if request.settings.gap_close_px > 0 {
                16
            } else {
                0
            }
            + if radius > 0 { 8 } else { 0 },
    );
    // Include filter temporaries, flood/growth state, the bounded queue and
    // conservative cloned-tree bookkeeping before allocating any pixel map.
    let workspace = count_u64
        .saturating_mul(7)
        .saturating_add((limits.max_frontier.min(count) as u64).saturating_mul(8))
        .saturating_add(source.tree_bytes)
        .saturating_add(clip.map_or(0, |mask| {
            let [width, height] = mask.dimensions();
            u64::from(width) * u64::from(height)
        }));
    if count_u64 > limits.max_pixels
        || work > limits.max_filter_work
        || workspace > limits.max_workspace_bytes
    {
        return Err(EditError::LimitExceeded);
    }
    let seed_color = source.pixel(request.seed);
    let mut barrier = vec![0; count];
    let mut allowed = vec![0; count];
    for index in 0..count {
        let point = domain.point(index);
        allowed[index] = u8::from(clip.is_none_or(|mask| mask.contains_signed(point[0], point[1])));
        barrier[index] = u8::from(
            source
                .pixel(point)
                .into_iter()
                .zip(seed_color)
                .any(|(channel, seed)| channel.abs_diff(seed) > request.tolerance),
        );
    }
    let closed = if request.settings.gap_close_px == 0 {
        barrier.clone()
    } else {
        let expanded = morphology(
            &barrier,
            domain,
            usize::from(request.settings.gap_close_px),
            true,
        );
        let mut closed = morphology(
            &expanded,
            domain,
            usize::from(request.settings.gap_close_px),
            false,
        );
        for (closed, original) in closed.iter_mut().zip(&barrier) {
            *closed |= original;
        }
        closed
    };
    let seed = domain.index(request.seed);
    if closed[seed] != 0 || allowed[seed] == 0 {
        return Err(EditError::InvalidSeed);
    }
    let mut selected = vec![0; count];
    let mut queue = VecDeque::with_capacity(limits.max_frontier.min(count));
    selected[seed] = 1;
    enqueue(&mut queue, seed, limits)?;
    while let Some(index) = queue.pop_front() {
        for neighbor in domain.neighbors(index).into_iter().flatten() {
            if selected[neighbor] == 0 && closed[neighbor] == 0 && allowed[neighbor] != 0 {
                enqueue(&mut queue, neighbor, limits)?;
                selected[neighbor] = 1;
            }
        }
    }
    let mut distance = vec![u8::MAX; count];
    if radius > 0 {
        for (index, value) in selected.iter().enumerate() {
            if *value == 0 {
                continue;
            }
            for neighbor in domain.neighbors(index).into_iter().flatten() {
                if closed[neighbor] != 0 && allowed[neighbor] != 0 && distance[neighbor] == u8::MAX
                {
                    enqueue(&mut queue, neighbor, limits)?;
                    distance[neighbor] = 1;
                }
            }
        }
        while let Some(index) = queue.pop_front() {
            if distance[index] >= radius {
                continue;
            }
            for neighbor in domain.neighbors(index).into_iter().flatten() {
                if closed[neighbor] != 0 && allowed[neighbor] != 0 && distance[neighbor] == u8::MAX
                {
                    enqueue(&mut queue, neighbor, limits)?;
                    distance[neighbor] = distance[index] + 1;
                }
            }
        }
    }
    let mut replacements = BTreeMap::<TileKey, Vec<u8>>::new();
    for index in 0..count {
        if selected[index] == 0 && distance[index] > radius {
            continue;
        }
        let point = domain.point(index);
        let key = TileKey::from_pixel(request.active, 128, point[0], point[1]);
        if !replacements.contains_key(&key) {
            let bytes = workspace.saturating_add(
                (replacements.len() as u64 + 1).saturating_mul(TILE_BYTE_LEN as u64 * 2 + 256),
            );
            if bytes > limits.max_workspace_bytes {
                return Err(EditError::LimitExceeded);
            }
            replacements.insert(
                key,
                snapshot
                    .get(key)
                    .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec()),
            );
        }
        let pixels = replacements.get_mut(&key).ok_or(EditError::InvalidPaint)?;
        let offset =
            usize::try_from((point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128)) * 4)
                .map_err(|_| EditError::CoordinateOutOfRange)?;
        if target.alpha_locked {
            crate::composite_source_atop(&mut pixels[offset..offset + 4], color, 1.0);
        } else if selected[index] != 0 {
            composite_surface(&mut pixels[offset..offset + 4], &color.0, u16::MAX);
        } else {
            let mut underpaint = color.0;
            composite_surface(&mut underpaint, &pixels[offset..offset + 4], u16::MAX);
            pixels[offset..offset + 4].copy_from_slice(&underpaint);
        }
    }
    replacements.retain(|key, pixels| {
        snapshot.get(*key).map_or_else(
            || pixels.iter().any(|byte| *byte != 0),
            |tile| tile.pixels() != pixels,
        )
    });
    let changed_tiles = replacements.keys().copied().collect();
    Ok(SelectionPaintResult {
        after: snapshot
            .with_replacements(replacements)
            .map_err(EditError::Tiles)?,
        changed_tiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId};
    use nyatidraw_document::LayerNode;

    fn tree() -> LayerTree {
        LayerTree::new(GroupNode {
            clip_to_below: false,
            blend_mode: nyatidraw_api::LayerBlendMode::Normal,
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: (1..=2)
                .map(|id| {
                    LayerTreeNode::Raster(LayerNode {
                        alpha_locked: false,
                        clip_to_below: false,
                        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                        id: LayerId(id),
                        name: format!("Layer{id}"),
                        visible: true,
                        locked: false,
                        reference: id == 2,
                        opacity_u16: u16::MAX,
                        content_root: ContentRootId(0),
                    })
                })
                .collect(),
        })
        .unwrap()
    }

    fn fixture(pixels: &[(LayerId, [i32; 2], [u8; 4])]) -> TileSnapshot {
        let mut tiles = BTreeMap::new();
        for (layer, point, color) in pixels {
            let key = TileKey::from_pixel(*layer, 128, point[0], point[1]);
            let data = tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN]);
            let offset =
                usize::try_from((point[1].rem_euclid(128) * 128 + point[0].rem_euclid(128)) * 4)
                    .unwrap();
            data[offset..offset + 4].copy_from_slice(color);
        }
        TileSnapshot::empty().with_replacements(tiles).unwrap()
    }

    fn request(settings: FillSettings) -> FloodFillRequest {
        FloodFillRequest {
            canvas: CanvasSpec {
                width_px: 16,
                height_px: 16,
                pixels_per_inch: 96,
            },
            active: LayerId(1),
            source: SelectionSource::ReferenceLayers,
            seed: [7, 7],
            tolerance: 0,
            settings,
        }
    }

    fn ring(
        pixels: &mut Vec<(LayerId, [i32; 2], [u8; 4])>,
        low: i32,
        high: i32,
        alpha: u8,
        gap: bool,
    ) {
        for y in low..=high {
            for x in low..=high {
                if (x == low || x == high || y == low || y == high) && !(gap && x == 7 && y == low)
                {
                    pixels.push((LayerId(2), [x, y], [0, 0, 0, alpha]));
                }
            }
        }
    }

    fn fill(before: &TileSnapshot, settings: FillSettings) -> TileSnapshot {
        flood_fill(
            before,
            &tree(),
            request(settings),
            None,
            PremultipliedRgba8::new(255, 0, 0, 255),
            EditLimits::default(),
        )
        .unwrap()
        .after
    }

    fn over_background(source: [u8; 4], white: bool) -> [u8; 4] {
        let mut output = if white { [255; 4] } else { [0, 0, 0, 255] };
        composite_surface(&mut output, &source, u16::MAX);
        output
    }

    #[test]
    fn reference_fill_separates_gap_expansion_and_inner_aa_without_recoloring_neighbors() {
        // Product risk: closing a line gap must not leak into a neighboring
        // matching component; AA backing must eliminate white seams without
        // altering reference lines or the outside edge of a thick contour.
        let plain = FillSettings {
            gap_close_px: 0,
            expand_px: 0,
            antialias: false,
        };
        for (gap, close, exterior_filled) in [(false, 0, false), (true, 0, true), (true, 1, false)]
        {
            let mut pixels = Vec::new();
            ring(&mut pixels, 3, 11, 255, gap);
            pixels.push((LayerId(1), [3, 7], [0, 255, 0, 255]));
            let before = fixture(&pixels);
            let after = fill(
                &before,
                FillSettings {
                    gap_close_px: close,
                    expand_px: 4,
                    ..plain
                },
            );
            assert_eq!(raster_pixel(&after, LayerId(1), [7, 7]), [255, 0, 0, 255]);
            assert_eq!(
                raster_pixel(&after, LayerId(1), [1, 7])[3] != 0,
                exterior_filled
            );
            assert_eq!(
                raster_pixel(&after, LayerId(1), [3, 7]),
                [0, 255, 0, 255],
                "expansion must not paint over target line"
            );
            for (key, tile) in before.iter().filter(|(key, _)| key.layer == LayerId(2)) {
                assert_eq!(after.get(key).unwrap().pixels(), tile.pixels());
            }
        }

        let mut pixels = Vec::new();
        ring(&mut pixels, 2, 12, 64, false);
        ring(&mut pixels, 3, 11, 255, false);
        ring(&mut pixels, 4, 10, 128, false);
        let before = fixture(&pixels);
        let without = fill(&before, plain);
        let with = fill(
            &before,
            FillSettings {
                antialias: true,
                ..plain
            },
        );
        let without_inner = group_pixel(&without, tree().root(), [4, 7]);
        let with_inner = group_pixel(&with, tree().root(), [4, 7]);
        assert_ne!(
            over_background(without_inner, true),
            over_background(without_inner, false)
        );
        assert_eq!(with_inner, [127, 0, 0, 255]);
        assert_eq!(
            over_background(with_inner, true),
            over_background(with_inner, false)
        );
        assert_eq!(
            raster_pixel(&with, LayerId(1), [3, 7]),
            [0; 4],
            "AA reach is one backing pixel"
        );
        assert_eq!(
            group_pixel(&with, tree().root(), [2, 7]),
            [0, 0, 0, 64],
            "outer AA must retain original transparency"
        );
        assert_eq!(raster_pixel(&with, LayerId(1), [1, 7]), [0; 4]);
    }

    #[test]
    fn signed_fill_clip_is_impermeable_and_disabled_options_match_existing_flood() {
        // Product risk: outside-page filling and optional selection must not
        // destroy pixels on the other side of a selection hole or silently
        // change ordinary tolerance-only bucket behavior.
        let mut pixels = Vec::new();
        ring(&mut pixels, 3, 11, 255, false);
        let before = fixture(&pixels);
        let plain = FillSettings {
            gap_close_px: 0,
            expand_px: 0,
            antialias: false,
        };
        let request = request(plain);
        let mask = crate::wand_selection(
            &before,
            &tree(),
            crate::WandRequest {
                canvas: request.canvas,
                active: request.active,
                source: request.source,
                seed: request.seed,
                tolerance: request.tolerance,
            },
            EditLimits::default(),
        )
        .unwrap();
        let legacy = crate::paint_selection(
            &before,
            &tree(),
            LayerId(1),
            &mask,
            crate::SelectionPaint::Solid(PremultipliedRgba8([255, 0, 0, 255])),
            EditLimits::default(),
        )
        .unwrap();
        let after = fill(&before, plain);
        assert_eq!(after.root(), legacy.after.root());

        let shifted: Vec<_> = pixels
            .into_iter()
            .map(|(layer, [x, y], color)| (layer, [x - 16, y - 16], color))
            .collect();
        let before = fixture(&shifted);
        let mut packed = vec![255; 16 * 16 / 8];
        for y in 0..16 {
            let index = y * 16 + 7;
            packed[index / 8] &= !(1 << (index % 8));
        }
        let clip = SelectionMask::from_packed_bits_at([-16, -16], [16, 16], &packed).unwrap();
        let after = flood_fill(
            &before,
            &tree(),
            FloodFillRequest {
                seed: [-10, -9],
                settings: FillSettings {
                    expand_px: 64,
                    antialias: true,
                    ..plain
                },
                ..request
            },
            Some(&clip),
            PremultipliedRgba8([255, 0, 0, 255]),
            EditLimits::default(),
        )
        .unwrap()
        .after;
        assert_eq!(
            raster_pixel(&after, LayerId(1), [-10, -9]),
            [255, 0, 0, 255]
        );
        assert_eq!(raster_pixel(&after, LayerId(1), [-9, -9]), [0; 4]);
        assert_eq!(raster_pixel(&after, LayerId(1), [-8, -9]), [0; 4]);
        assert_eq!(raster_pixel(&after, LayerId(1), [0, 0]), [0; 4]);
    }

    #[test]
    fn fill_preflight_and_frontier_fail_atomically_and_source_modes_preserve_line_alpha() {
        // Product risk: invalid resources/references must never partially fill
        // artwork; same-raster and visible/reference sources must share the
        // immutable line sampling contract.
        let mut pixels = Vec::new();
        ring(&mut pixels, 3, 11, 255, false);
        let before = fixture(&pixels);
        let original = before.root();
        for limits in [
            EditLimits {
                max_frontier: 1,
                ..EditLimits::default()
            },
            EditLimits {
                max_pixels: 2,
                ..EditLimits::default()
            },
            EditLimits {
                max_workspace_bytes: 64,
                ..EditLimits::default()
            },
            EditLimits {
                max_filter_work: 1,
                ..EditLimits::default()
            },
        ] {
            assert!(matches!(
                flood_fill(
                    &before,
                    &tree(),
                    request(FillSettings::default()),
                    None,
                    PremultipliedRgba8([255, 0, 0, 255]),
                    limits
                ),
                Err(EditError::LimitExceeded)
            ));
            assert_eq!(before.root(), original);
        }
        for (active, source) in [
            (LayerId(1), SelectionSource::ReferenceLayers),
            (LayerId(1), SelectionSource::AllVisible),
            (LayerId(2), SelectionSource::ActiveLayer),
        ] {
            let after = flood_fill(
                &before,
                &tree(),
                FloodFillRequest {
                    active,
                    source,
                    ..request(FillSettings::default())
                },
                None,
                PremultipliedRgba8([128, 0, 0, 128]),
                EditLimits::default(),
            )
            .unwrap()
            .after;
            assert_eq!(raster_pixel(&after, active, [7, 7]), [128, 0, 0, 128]);
            assert_eq!(raster_pixel(&after, LayerId(2), [3, 7]), [0, 0, 0, 255]);
        }
        let mut locked = tree();
        locked.set_locked(LayerId(1), true).unwrap();
        assert!(matches!(
            flood_fill(
                &before,
                &locked,
                request(FillSettings::default()),
                None,
                PremultipliedRgba8([255, 0, 0, 255]),
                EditLimits::default()
            ),
            Err(EditError::LockedLayer)
        ));
        let mut absent = tree();
        absent.set_reference(LayerId(2), false).unwrap();
        assert!(matches!(
            flood_fill(
                &before,
                &absent,
                request(FillSettings::default()),
                None,
                PremultipliedRgba8([255, 0, 0, 255]),
                EditLimits::default()
            ),
            Err(EditError::MissingReference)
        ));
        assert_eq!(before.root(), original);
    }
}

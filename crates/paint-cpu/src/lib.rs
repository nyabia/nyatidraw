#![forbid(unsafe_code)]

//! Deterministic CPU reference rasterization for Sprint 1 round brushes.
//!
//! Pixel bytes are RGBA8 premultiplied **linear-light** values. `BrushDab`
//! uses build-up semantics: its effective source alpha is
//! `coverage * dab.opacity * dab.flow`; source RGB is scaled by that same
//! amount and composited with source-over.

use nyatidraw_brush::BrushDab;
use nyatidraw_document::{GroupNode, LayerTree, LayerTreeNode};
use nyatidraw_input::Point;
use nyatidraw_tiles::{FlattenError, FlattenedRgba8, MAX_FLATTENED_PIXELS, TileSnapshot};

/// A premultiplied linear-light RGBA8 colour at full dab opacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PremultipliedRgba8(pub [u8; 4]);

impl PremultipliedRgba8 {
    #[must_use]
    pub const fn transparent() -> Self {
        Self([0, 0, 0, 0])
    }

    /// Clamps colour channels to alpha to preserve the premultiplied invariant.
    #[must_use]
    pub fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self([red.min(alpha), green.min(alpha), blue.min(alpha), alpha])
    }
}

/// A contiguous, top-left-origin RGBA8 premultiplied reference surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuCanvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCanvasError {
    DimensionOverflow,
    InvalidByteLength { expected: usize, actual: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCompositeError {
    InvalidCanvas,
    Flatten(FlattenError),
}

/// Flattens the visible raster/group hierarchy into the finite project page.
///
/// Children are stored bottom-to-top. Group opacity is applied once to the
/// completed child composite, matching the GPU compositor contract.
///
/// # Errors
///
/// Returns an error for invalid or excessively large canvas metadata and for
/// unsupported tile mip levels.
pub fn flatten_layer_tree_rgba8(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    canvas: nyatidraw_api::CanvasSpec,
) -> Result<FlattenedRgba8, CpuCompositeError> {
    canvas
        .validate()
        .map_err(|_| CpuCompositeError::InvalidCanvas)?;
    let pixel_count = u64::from(canvas.width_px)
        .checked_mul(u64::from(canvas.height_px))
        .filter(|count| *count <= MAX_FLATTENED_PIXELS)
        .ok_or(CpuCompositeError::InvalidCanvas)?;
    let byte_len = pixel_count
        .checked_mul(4)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or(CpuCompositeError::InvalidCanvas)?;
    let mut pixels = vec![0; byte_len];
    composite_group(snapshot, tree.root(), canvas, &mut pixels)?;
    Ok(FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: canvas.width_px,
        height: canvas.height_px,
        pixels,
    })
}

/// Renders a bounded, nearest-sampled preview of the finite project page.
///
/// Source lookup is restricted to `[0, width) x [0, height)`, so signed
/// outside artwork remains durable but cannot appear in a navigator preview.
/// Unlike export flattening this allocates only the requested preview surface.
///
/// # Errors
///
/// Returns an error for invalid canvas/preview dimensions or unsupported tile
/// mip levels.
pub fn render_page_preview_rgba8(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    canvas: nyatidraw_api::CanvasSpec,
    max_width: u32,
    max_height: u32,
) -> Result<FlattenedRgba8, CpuCompositeError> {
    canvas
        .validate()
        .map_err(|_| CpuCompositeError::InvalidCanvas)?;
    if max_width == 0 || max_height == 0 {
        return Err(CpuCompositeError::InvalidCanvas);
    }
    if let Some((key, _)) = snapshot.iter().find(|(key, _)| key.mip != 0) {
        return Err(CpuCompositeError::Flatten(FlattenError::UnsupportedMip {
            mip: key.mip,
        }));
    }

    let (width, height) = preview_dimensions(canvas, max_width, max_height);
    let byte_len = usize::try_from(u64::from(width) * u64::from(height) * 4)
        .map_err(|_| CpuCompositeError::InvalidCanvas)?;
    let mut pixels = vec![0; byte_len];
    for output_y in 0..height {
        let source_y = preview_source_coordinate(output_y, height, canvas.height_px);
        for output_x in 0..width {
            let source_x = preview_source_coordinate(output_x, width, canvas.width_px);
            let offset =
                usize::try_from((u64::from(output_y) * u64::from(width) + u64::from(output_x)) * 4)
                    .map_err(|_| CpuCompositeError::InvalidCanvas)?;
            composite_preview_group(
                snapshot,
                tree.root(),
                source_x,
                source_y,
                &mut pixels[offset..offset + 4],
            );
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

/// Renders one raster layer as a bounded, nearest-sampled preview of the
/// finite project page.
///
/// This intentionally ignores layer visibility and opacity: a row thumbnail
/// describes that raster's durable pixels, while the row controls describe how
/// it participates in the composite. Source lookup remains restricted to the
/// finite page, so signed artwork outside the page cannot leak into a
/// thumbnail.
///
/// # Errors
///
/// Returns an error for invalid canvas/preview dimensions or unsupported tile
/// mip levels.
pub fn render_raster_page_preview_rgba8(
    snapshot: &TileSnapshot,
    layer: nyatidraw_api::LayerId,
    canvas: nyatidraw_api::CanvasSpec,
    max_width: u32,
    max_height: u32,
) -> Result<FlattenedRgba8, CpuCompositeError> {
    canvas
        .validate()
        .map_err(|_| CpuCompositeError::InvalidCanvas)?;
    if max_width == 0 || max_height == 0 {
        return Err(CpuCompositeError::InvalidCanvas);
    }
    if let Some((key, _)) = snapshot.iter().find(|(key, _)| key.mip != 0) {
        return Err(CpuCompositeError::Flatten(FlattenError::UnsupportedMip {
            mip: key.mip,
        }));
    }

    let (width, height) = preview_dimensions(canvas, max_width, max_height);
    let byte_len = usize::try_from(u64::from(width) * u64::from(height) * 4)
        .map_err(|_| CpuCompositeError::InvalidCanvas)?;
    let mut pixels = vec![0; byte_len];
    for output_y in 0..height {
        let source_y = preview_source_coordinate(output_y, height, canvas.height_px);
        for output_x in 0..width {
            let source_x = preview_source_coordinate(output_x, width, canvas.width_px);
            let offset =
                usize::try_from((u64::from(output_y) * u64::from(width) + u64::from(output_x)) * 4)
                    .map_err(|_| CpuCompositeError::InvalidCanvas)?;
            pixels[offset..offset + 4]
                .copy_from_slice(&preview_raster_pixel(snapshot, layer, source_x, source_y));
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

fn preview_dimensions(
    canvas: nyatidraw_api::CanvasSpec,
    max_width: u32,
    max_height: u32,
) -> (u32, u32) {
    let page_width = u64::from(canvas.width_px);
    let page_height = u64::from(canvas.height_px);
    let width_from_height = (u64::from(max_height) * page_width / page_height).max(1);
    let height_from_width = (u64::from(max_width) * page_height / page_width).max(1);
    if width_from_height <= u64::from(max_width) {
        (
            u32::try_from(width_from_height).expect("preview width is bounded by max width"),
            max_height,
        )
    } else {
        (
            max_width,
            u32::try_from(height_from_width).expect("preview height is bounded by max height"),
        )
    }
}

fn preview_source_coordinate(output: u32, output_extent: u32, source_extent: u32) -> u32 {
    let numerator = (u64::from(output) * 2 + 1) * u64::from(source_extent);
    u32::try_from(numerator / (u64::from(output_extent) * 2))
        .expect("preview source coordinate is within the canvas")
}

fn composite_preview_group(
    snapshot: &TileSnapshot,
    group: &GroupNode,
    x: u32,
    y: u32,
    destination: &mut [u8],
) {
    for child in &group.children {
        match child {
            LayerTreeNode::Raster(layer) if layer.visible => {
                let source = preview_raster_pixel(snapshot, layer.id, x, y);
                composite_surface(destination, &source, layer.opacity_u16);
            }
            LayerTreeNode::Group(child_group) if child_group.visible => {
                let mut source = [0; 4];
                composite_preview_group(snapshot, child_group, x, y, &mut source);
                composite_surface(destination, &source, child_group.opacity_u16);
            }
            LayerTreeNode::Raster(_) | LayerTreeNode::Group(_) => {}
        }
    }
}

fn preview_raster_pixel(
    snapshot: &TileSnapshot,
    layer: nyatidraw_api::LayerId,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let Some(tile_x) = i32::try_from(x / nyatidraw_tiles::TILE_EDGE).ok() else {
        return [0; 4];
    };
    let Some(tile_y) = i32::try_from(y / nyatidraw_tiles::TILE_EDGE).ok() else {
        return [0; 4];
    };
    let key = nyatidraw_tiles::TileKey {
        layer,
        mip: 0,
        x: tile_x,
        y: tile_y,
    };
    let Some(tile) = snapshot.get(key) else {
        return [0; 4];
    };
    let local_x = usize::try_from(x % nyatidraw_tiles::TILE_EDGE).expect("tile local x fits usize");
    let local_y = usize::try_from(y % nyatidraw_tiles::TILE_EDGE).expect("tile local y fits usize");
    let edge = usize::try_from(nyatidraw_tiles::TILE_EDGE).expect("tile edge fits usize");
    let offset = (local_y * edge + local_x) * 4;
    tile.pixels()[offset..offset + 4]
        .try_into()
        .expect("one RGBA tile pixel has four bytes")
}

fn composite_group(
    snapshot: &TileSnapshot,
    group: &GroupNode,
    canvas: nyatidraw_api::CanvasSpec,
    destination: &mut [u8],
) -> Result<(), CpuCompositeError> {
    for child in &group.children {
        match child {
            LayerTreeNode::Raster(layer) if layer.visible => {
                let source = snapshot
                    .crop_base_layer_rgba8_to_canvas(layer.id, canvas)
                    .map_err(CpuCompositeError::Flatten)?;
                composite_surface(destination, &source.pixels, layer.opacity_u16);
            }
            LayerTreeNode::Group(child_group) if child_group.visible => {
                let mut source = vec![0; destination.len()];
                composite_group(snapshot, child_group, canvas, &mut source)?;
                composite_surface(destination, &source, child_group.opacity_u16);
            }
            LayerTreeNode::Raster(_) | LayerTreeNode::Group(_) => {}
        }
    }
    Ok(())
}

fn composite_surface(destination: &mut [u8], source: &[u8], opacity_u16: u16) {
    debug_assert_eq!(destination.len(), source.len());
    let opacity = u32::from(opacity_u16);
    for (destination, source) in destination.chunks_exact_mut(4).zip(source.chunks_exact(4)) {
        let source_alpha = (u32::from(source[3]) * opacity + 32_767) / 65_535;
        let inverse_alpha = 255 - source_alpha;
        for channel in 0..3 {
            let source_channel = (u32::from(source[channel]) * opacity + 32_767) / 65_535;
            let value =
                source_channel + (u32::from(destination[channel]) * inverse_alpha + 127) / 255;
            destination[channel] = u8::try_from(value.min(255)).expect("channel is clamped");
        }
        let alpha = source_alpha + (u32::from(destination[3]) * inverse_alpha + 127) / 255;
        destination[3] = u8::try_from(alpha.min(255)).expect("alpha is clamped");
    }
}

/// Acceptance thresholds for a CPU/GPU premultiplied RGBA8 image comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageDiffTolerance {
    /// Per-channel difference permitted before a pixel is counted as out of tolerance.
    pub max_channel_delta: u8,
    /// Largest permitted number of pixels exceeding `max_channel_delta`.
    pub max_out_of_tolerance_pixels: u64,
}

impl ImageDiffTolerance {
    #[must_use]
    pub const fn exact() -> Self {
        Self {
            max_channel_delta: 0,
            max_out_of_tolerance_pixels: 0,
        }
    }
}

/// GPU acceptance limit for the closed-batch `Rgba8Unorm` probe corpus.
///
/// The CPU reference rounds each source-over result to an 8-bit channel after
/// every dab. Hardware blending into an `Rgba8Unorm` attachment can round the
/// equivalent intermediate value differently. Repeated low-alpha build-up can
/// accumulate those differences: the current 4/5/8/12-dab hardware corpus
/// reaches four bytes on an eight-dab low-alpha overlap. This permits that
/// measured four-byte ceiling while requiring that no pixel exceeds it. It is
/// intentionally not a percentage-based allowance and must be remeasured when
/// the probe corpus or blend contract changes.
pub const GPU_UNORM_CLOSED_STROKE_TOLERANCE: ImageDiffTolerance = ImageDiffTolerance {
    max_channel_delta: 4,
    max_out_of_tolerance_pixels: 0,
};

/// Statistics returned for a complete RGBA8 image comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageDiffStats {
    pub compared_pixels: u64,
    pub differing_pixels: u64,
    pub out_of_tolerance_pixels: u64,
    pub max_channel_delta: u8,
    pub total_absolute_error: u64,
    pub tolerance: ImageDiffTolerance,
}

impl ImageDiffStats {
    #[must_use]
    pub const fn is_within(self) -> bool {
        self.out_of_tolerance_pixels <= self.tolerance.max_out_of_tolerance_pixels
    }
}

/// Failure to compare a raw GPU readback with a [`CpuCanvas`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageDiffError {
    DimensionMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    InvalidActualByteLength {
        expected: usize,
        actual: usize,
    },
}

impl CpuCanvas {
    /// # Panics
    ///
    /// Panics when `width * height * 4` cannot be represented in memory on the
    /// current target. Callers creating untrusted-size canvases must validate
    /// dimensions before constructing the reference surface.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let byte_count =
            canvas_byte_len(width, height).expect("CPU reference canvas byte length overflow");
        Self {
            width,
            height,
            pixels: vec![0; byte_count],
        }
    }

    /// Restores a tightly packed premultiplied linear-light RGBA8 surface.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions overflow or the byte length does not
    /// exactly match `width * height * 4`.
    pub fn from_rgba8_premultiplied(
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Self, CpuCanvasError> {
        let expected = canvas_byte_len(width, height).ok_or(CpuCanvasError::DimensionOverflow)?;
        if pixels.len() != expected {
            return Err(CpuCanvasError::InvalidByteLength {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels_rgba8_premultiplied(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<PremultipliedRgba8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = pixel_offset(self.width, x, y);
        Some(PremultipliedRgba8(
            self.pixels[index..index + 4].try_into().ok()?,
        ))
    }

    pub fn apply_dabs(&mut self, dabs: &[BrushDab], colour: PremultipliedRgba8) {
        for &dab in dabs {
            self.apply_dab(dab, colour);
        }
    }

    pub fn erase_dabs(&mut self, dabs: &[BrushDab]) {
        for &dab in dabs {
            self.erase_dab(dab);
        }
    }

    /// Rasterizes with deterministic 4×4 coverage supersampling.
    pub fn apply_dab(&mut self, dab: BrushDab, colour: PremultipliedRgba8) {
        self.rasterize_dab(dab, |destination, coverage_alpha| {
            composite_source_over(destination, colour, coverage_alpha);
        });
    }

    /// Rasterizes an alpha eraser with deterministic 4x4 coverage supersampling.
    pub fn erase_dab(&mut self, dab: BrushDab) {
        self.rasterize_dab(dab, composite_destination_out);
    }

    fn rasterize_dab(&mut self, dab: BrushDab, mut composite: impl FnMut(&mut [u8], f32)) {
        let radius = f64::from(dab.radius_px.max(0.0));
        if radius == 0.0 || !radius.is_finite() {
            return;
        }
        let alpha = (dab.opacity * dab.flow).clamp(0.0, 1.0);
        if alpha == 0.0 || !alpha.is_finite() {
            return;
        }
        let Some(bounds) = pixel_bounds(self.width, self.height, dab.center, radius) else {
            return;
        };

        for y in bounds.top..bounds.bottom {
            for x in bounds.left..bounds.right {
                let coverage = circle_coverage(dab.center, radius, x, y);
                if coverage == 0.0 {
                    continue;
                }
                composite(
                    &mut self.pixels[pixel_offset(self.width, x, y)..][..4],
                    coverage * alpha,
                );
            }
        }
    }
}

#[must_use]
fn canvas_byte_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(4)
}

/// Compares a tightly packed RGBA8 premultiplied image with the CPU reference.
///
/// This function is GPU-agnostic so callers can pass a `GpuWorkingTile`
/// readback without making this crate depend on `wgpu`.
///
/// # Errors
///
/// Returns an error when the source dimensions or byte length do not describe
/// the same tightly packed RGBA8 image as `expected`.
pub fn compare_rgba8_premultiplied(
    expected: &CpuCanvas,
    actual_width: u32,
    actual_height: u32,
    actual: &[u8],
    tolerance: ImageDiffTolerance,
) -> Result<ImageDiffStats, ImageDiffError> {
    if expected.width != actual_width || expected.height != actual_height {
        return Err(ImageDiffError::DimensionMismatch {
            expected_width: expected.width,
            expected_height: expected.height,
            actual_width,
            actual_height,
        });
    }
    if expected.pixels.len() != actual.len() {
        return Err(ImageDiffError::InvalidActualByteLength {
            expected: expected.pixels.len(),
            actual: actual.len(),
        });
    }

    let mut stats = ImageDiffStats {
        compared_pixels: 0,
        differing_pixels: 0,
        out_of_tolerance_pixels: 0,
        max_channel_delta: 0,
        total_absolute_error: 0,
        tolerance,
    };
    for (expected_pixel, actual_pixel) in
        expected.pixels.chunks_exact(4).zip(actual.chunks_exact(4))
    {
        stats.compared_pixels = stats.compared_pixels.saturating_add(1);
        let mut pixel_max_delta = 0_u8;
        for (expected_channel, actual_channel) in expected_pixel.iter().zip(actual_pixel) {
            let delta = expected_channel.abs_diff(*actual_channel);
            pixel_max_delta = pixel_max_delta.max(delta);
            stats.total_absolute_error =
                stats.total_absolute_error.saturating_add(u64::from(delta));
        }
        if pixel_max_delta > 0 {
            stats.differing_pixels = stats.differing_pixels.saturating_add(1);
        }
        if pixel_max_delta > tolerance.max_channel_delta {
            stats.out_of_tolerance_pixels = stats.out_of_tolerance_pixels.saturating_add(1);
        }
        stats.max_channel_delta = stats.max_channel_delta.max(pixel_max_delta);
    }
    Ok(stats)
}

#[derive(Clone, Copy, Debug)]
struct PixelBounds {
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
}

#[must_use]
fn pixel_bounds(width: u32, height: u32, center: Point, radius: f64) -> Option<PixelBounds> {
    if width == 0 || height == 0 || !center.x.is_finite() || !center.y.is_finite() {
        return None;
    }
    let left = bounded_pixel_coordinate((center.x - radius).floor().max(0.0), width);
    let top = bounded_pixel_coordinate((center.y - radius).floor().max(0.0), height);
    let right = bounded_pixel_coordinate((center.x + radius).ceil().min(f64::from(width)), width);
    let bottom =
        bounded_pixel_coordinate((center.y + radius).ceil().min(f64::from(height)), height);
    (left < right && top < bottom).then_some(PixelBounds {
        left,
        right,
        top,
        bottom,
    })
}

#[must_use]
fn bounded_pixel_coordinate(value: f64, upper_bound: u32) -> u32 {
    debug_assert!(value.is_finite() && value >= 0.0 && value <= f64::from(upper_bound));
    // The caller clamps both bounds to the closed [0, upper_bound] range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value as u32
    }
}

#[must_use]
fn pixel_offset(width: u32, x: u32, y: u32) -> usize {
    (usize::try_from(y).expect("u32 fits usize") * usize::try_from(width).expect("u32 fits usize")
        + usize::try_from(x).expect("u32 fits usize"))
        * 4
}

#[must_use]
fn circle_coverage(center: Point, radius: f64, x: u32, y: u32) -> f32 {
    const OFFSETS: [f64; 4] = [0.125, 0.375, 0.625, 0.875];
    let radius_squared = radius * radius;
    let mut covered = 0_u8;
    for offset_y in OFFSETS {
        for offset_x in OFFSETS {
            let dx = f64::from(x) + offset_x - center.x;
            let dy = f64::from(y) + offset_y - center.y;
            if dx.mul_add(dx, dy * dy) <= radius_squared {
                covered = covered.saturating_add(1);
            }
        }
    }
    f32::from(covered) / 16.0
}

fn composite_source_over(destination: &mut [u8], colour: PremultipliedRgba8, coverage_alpha: f32) {
    let source_alpha = f32::from(colour.0[3]) / 255.0 * coverage_alpha;
    let inverse_source_alpha = 1.0 - source_alpha;
    for (channel, destination_channel_byte) in destination.iter_mut().take(3).enumerate() {
        let source = f32::from(colour.0[channel]) / 255.0 * coverage_alpha;
        let destination_channel = f32::from(*destination_channel_byte) / 255.0;
        *destination_channel_byte =
            normalized_byte(source + destination_channel * inverse_source_alpha);
    }
    let destination_alpha = f32::from(destination[3]) / 255.0;
    destination[3] = normalized_byte(source_alpha + destination_alpha * inverse_source_alpha);
}

fn composite_destination_out(destination: &mut [u8], coverage_alpha: f32) {
    let remaining = 1.0 - coverage_alpha;
    for channel in destination.iter_mut() {
        *channel = normalized_byte(f32::from(*channel) / 255.0 * remaining);
    }
}

#[must_use]
fn normalized_byte(value: f32) -> u8 {
    let rounded = (255.0 * value).round().clamp(0.0, 255.0);
    // Clamping proves this finite conversion is inside the u8 range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        rounded as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId, LayerId};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};

    #[test]
    fn round_dab_golden_prevents_gpu_cpu_reference_drift() {
        let mut canvas = CpuCanvas::new(3, 3);
        canvas.apply_dab(
            BrushDab {
                center: Point { x: 1.5, y: 1.5 },
                radius_px: 1.0,
                opacity: 0.5,
                flow: 0.5,
            },
            PremultipliedRgba8::new(128, 64, 32, 128),
        );

        assert_eq!(
            canvas.pixel(1, 1),
            Some(PremultipliedRgba8([32, 16, 8, 32])),
            "CPU reference output is the golden contract for GPU comparison"
        );

        let mut one_channel_drift = canvas.pixels_rgba8_premultiplied().to_vec();
        one_channel_drift[0] = one_channel_drift[0].saturating_add(1);
        let strict = compare_rgba8_premultiplied(
            &canvas,
            canvas.width(),
            canvas.height(),
            &one_channel_drift,
            ImageDiffTolerance::exact(),
        )
        .expect("matching image layout");
        assert_eq!(strict.differing_pixels, 1);
        assert_eq!(strict.out_of_tolerance_pixels, 1);
        assert_eq!(strict.max_channel_delta, 1);
        assert_eq!(strict.total_absolute_error, 1);
        assert!(!strict.is_within());
        let mut within_unorm_bound = canvas.pixels_rgba8_premultiplied().to_vec();
        within_unorm_bound[0] = within_unorm_bound[0].saturating_add(4);
        let unorm = compare_rgba8_premultiplied(
            &canvas,
            canvas.width(),
            canvas.height(),
            &within_unorm_bound,
            GPU_UNORM_CLOSED_STROKE_TOLERANCE,
        )
        .expect("matching image layout");
        assert!(unorm.is_within());

        let mut exceeds_unorm_bound = canvas.pixels_rgba8_premultiplied().to_vec();
        exceeds_unorm_bound[0] = exceeds_unorm_bound[0].saturating_add(5);
        let exceeds = compare_rgba8_premultiplied(
            &canvas,
            canvas.width(),
            canvas.height(),
            &exceeds_unorm_bound,
            GPU_UNORM_CLOSED_STROKE_TOLERANCE,
        )
        .expect("matching image layout");
        assert_eq!(exceeds.out_of_tolerance_pixels, 1);
        assert!(!exceeds.is_within());
    }

    #[test]
    fn eraser_scales_every_premultiplied_channel_by_remaining_coverage() {
        // Product risk: erasing RGB differently from alpha leaves coloured
        // fringes and corrupts the premultiplied pixel invariant on reopen.
        let mut canvas = CpuCanvas::from_rgba8_premultiplied(1, 1, vec![128, 64, 32, 128])
            .expect("valid premultiplied fixture");
        canvas.erase_dab(BrushDab {
            center: Point { x: 0.5, y: 0.5 },
            radius_px: 1.0,
            opacity: 0.5,
            flow: 1.0,
        });

        assert_eq!(
            canvas.pixel(0, 0),
            Some(PremultipliedRgba8([64, 32, 16, 64]))
        );
    }

    #[test]
    fn group_opacity_is_applied_once_to_the_completed_child_composite() {
        // Product risk: applying group opacity once per child changes artwork.
        let layer = LayerId(7);
        let mut tile = vec![0; TILE_BYTE_LEN];
        tile[..4].copy_from_slice(&[128, 0, 0, 128]);
        let snapshot = TileSnapshot::from_tiles([(
            TileKey {
                layer,
                mip: 0,
                x: 0,
                y: 0,
            },
            tile,
        )])
        .expect("valid fixture tile");
        let tree = LayerTree::new(GroupNode {
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![LayerTreeNode::Group(GroupNode {
                id: GroupId(2),
                name: "Half".into(),
                visible: true,
                opacity_u16: 32_768,
                children: vec![LayerTreeNode::Raster(LayerNode {
                    id: layer,
                    name: "Ink".into(),
                    visible: true,
                    locked: false,
                    reference: false,
                    opacity_u16: u16::MAX,
                    content_root: ContentRootId(0),
                })],
            })],
        })
        .expect("valid fixture tree");
        let output = flatten_layer_tree_rgba8(
            &snapshot,
            &tree,
            nyatidraw_api::CanvasSpec {
                width_px: 1,
                height_px: 1,
                pixels_per_inch: 96,
            },
        )
        .expect("composite succeeds");
        assert_eq!(&output.pixels[..4], &[64, 0, 0, 64]);
    }

    #[test]
    fn navigator_preview_crops_signed_outside_artwork_without_losing_it_from_tiles() {
        // Product risk: an outside-workspace stroke must remain durable while
        // never leaking into the finite page navigator.
        let layer = LayerId(7);
        let mut inside = vec![0; TILE_BYTE_LEN];
        let inside_offset = (usize::from(1_u8) * 128 + usize::from(1_u8)) * 4;
        inside[inside_offset..inside_offset + 4].copy_from_slice(&[20, 40, 60, 255]);
        let mut outside = vec![0; TILE_BYTE_LEN];
        outside[(127 * 128 + 127) * 4..(127 * 128 + 127) * 4 + 4]
            .copy_from_slice(&[200, 10, 10, 255]);
        let baseline = TileSnapshot::from_tiles([(
            TileKey {
                layer,
                mip: 0,
                x: 0,
                y: 0,
            },
            inside.clone(),
        )])
        .expect("valid inside tile");
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
                    y: -1,
                },
                outside,
            ),
        ])
        .expect("valid signed sparse tiles");
        let tree = LayerTree::new(GroupNode {
            id: GroupId(1),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![LayerTreeNode::Raster(LayerNode {
                id: layer,
                name: "Ink".into(),
                visible: true,
                locked: false,
                reference: false,
                opacity_u16: u16::MAX,
                content_root: ContentRootId(0),
            })],
        })
        .expect("valid tree");
        let canvas = nyatidraw_api::CanvasSpec {
            width_px: 4,
            height_px: 4,
            pixels_per_inch: 96,
        };
        let expected =
            render_page_preview_rgba8(&baseline, &tree, canvas, 4, 4).expect("page preview");
        let actual = render_page_preview_rgba8(&with_outside, &tree, canvas, 4, 4)
            .expect("page preview with outside tile");
        assert_eq!(actual, expected);
        assert_eq!(with_outside.len(), 2, "outside artwork remains durable");
    }

    #[test]
    fn raster_thumbnail_crops_signed_outside_artwork_without_hiding_layer_pixels() {
        // Product risk: a per-layer thumbnail could expose signed outside
        // artwork or accidentally composite another layer into its row.
        let selected = LayerId(7);
        let other = LayerId(8);
        let mut selected_inside = vec![0; TILE_BYTE_LEN];
        selected_inside[..4].copy_from_slice(&[20, 40, 60, 255]);
        let mut selected_outside = vec![0; TILE_BYTE_LEN];
        selected_outside[(127 * 128 + 127) * 4..(127 * 128 + 127) * 4 + 4]
            .copy_from_slice(&[200, 10, 10, 255]);
        let mut other_inside = vec![0; TILE_BYTE_LEN];
        other_inside[..4].copy_from_slice(&[1, 2, 3, 255]);
        let snapshot = TileSnapshot::from_tiles([
            (
                TileKey {
                    layer: selected,
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                selected_inside,
            ),
            (
                TileKey {
                    layer: selected,
                    mip: 0,
                    x: -1,
                    y: -1,
                },
                selected_outside,
            ),
            (
                TileKey {
                    layer: other,
                    mip: 0,
                    x: 0,
                    y: 0,
                },
                other_inside,
            ),
        ])
        .expect("valid signed sparse tiles");
        let preview = render_raster_page_preview_rgba8(
            &snapshot,
            selected,
            nyatidraw_api::CanvasSpec {
                width_px: 1,
                height_px: 1,
                pixels_per_inch: 96,
            },
            50,
            56,
        )
        .expect("raster page preview");
        assert_eq!((preview.width, preview.height), (50, 50));
        assert!(
            preview
                .pixels
                .chunks_exact(4)
                .all(|pixel| pixel == [20, 40, 60, 255])
        );
        assert_eq!(snapshot.len(), 3, "outside artwork remains durable");
    }
}

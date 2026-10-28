use nyatidraw_api::{CanvasSpec, MaskId};
use nyatidraw_tiles::{FlattenedRgba8, TileSnapshot};

use crate::preview::{
    LAYER_THUMBNAIL_MAX_HEIGHT, LAYER_THUMBNAIL_MAX_WIDTH, MaskThumbnailFrame, png_data_uri,
};

/// Samples the same finite-page pixel centers as the original image thumbnail.
/// # Errors
/// Rejects invalid page/preview dimensions or PNG encoding failure.
pub fn mask_thumbnail_frame(
    mask: MaskId,
    default_coverage: u8,
    snapshot: &TileSnapshot,
    canvas: CanvasSpec,
    dimensions: [u16; 2],
) -> Result<MaskThumbnailFrame, String> {
    canvas
        .validate()
        .map_err(|error| format!("mask-preview-page:{error:?}"))?;
    let [width, height] = dimensions;
    if width == 0
        || height == 0
        || u32::from(width) > LAYER_THUMBNAIL_MAX_WIDTH
        || u32::from(height) > LAYER_THUMBNAIL_MAX_HEIGHT
    {
        return Err("mask-preview-dimensions".into());
    }
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for y in 0..height {
        let source_y = source_coordinate(y, height, canvas.height_px)?;
        for x in 0..width {
            let source_x = source_coordinate(x, width, canvas.width_px)?;
            let coverage = snapshot.coverage_at(mask, default_coverage, [source_x, source_y]);
            pixels.extend_from_slice(&[coverage; 4]);
        }
    }
    let surface = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: u32::from(width),
        height: u32::from(height),
        pixels,
    };
    let png = nyatidraw_png_io::encode_png_bytes(&surface)
        .map_err(|error| format!("mask-preview-png:{error}"))?;
    Ok(MaskThumbnailFrame {
        mask,
        width,
        height,
        data_uri: png_data_uri(&png, 32 * 1024, "mask-thumbnail")?,
    })
}

fn source_coordinate(output: u16, extent: u16, page_extent: u32) -> Result<i32, String> {
    let numerator = (u64::from(output) * 2 + 1) * u64::from(page_extent);
    i32::try_from(numerator / (u64::from(extent) * 2)).map_err(|_| "mask-preview-coordinate".into())
}

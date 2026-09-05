#![forbid(unsafe_code)]

//! PNG import/export at the application boundary.
//!
//! `NyatiDraw`'s tile bytes are premultiplied RGBA8. PNG files use straight
//! alpha, so conversion is explicit in both directions and never leaks into
//! the document model.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use nyatidraw_api::{CanvasSpec, LayerId};
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

// Import may temporarily hold both decoded RGBA and expanded edge tiles. Keep
// tile padding bounded even for extreme aspect-ratio images whose pixel count
// alone looks acceptable.
const MAX_EXPANDED_TILE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug)]
pub struct ImportedPng {
    pub canvas: CanvasSpec,
    pub tiles: TileSnapshot,
}

#[derive(Debug)]
pub enum PngIoError {
    Io(std::io::Error),
    Decode(png::DecodingError),
    Encode(png::EncodingError),
    UnsupportedColor(png::ColorType),
    DimensionsTooLarge,
    InvalidSurface,
    InvalidTiles(nyatidraw_tiles::TileSnapshotError),
}

impl std::fmt::Display for PngIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O: {error}"),
            Self::Decode(error) => write!(formatter, "PNG decode: {error}"),
            Self::Encode(error) => write!(formatter, "PNG encode: {error}"),
            Self::UnsupportedColor(color) => write!(formatter, "unsupported PNG color: {color:?}"),
            Self::DimensionsTooLarge => formatter.write_str("PNG dimensions exceed safe limits"),
            Self::InvalidSurface => formatter.write_str("invalid RGBA export surface"),
            Self::InvalidTiles(error) => write!(formatter, "invalid imported tiles: {error:?}"),
        }
    }
}

impl std::error::Error for PngIoError {}

impl From<std::io::Error> for PngIoError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Decodes a PNG into signed-tile-compatible base-mip pixels at document origin.
///
/// # Errors
///
/// Returns an error for unreadable, malformed, unsupported, or excessively
/// large input without modifying a project file.
pub fn decode_png(path: &Path, layer: LayerId) -> Result<ImportedPng, PngIoError> {
    let file = File::open(path)?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(PngIoError::Decode)?;
    let info = reader.info();
    let canvas = CanvasSpec {
        width_px: info.width,
        height_px: info.height,
        pixels_per_inch: 96,
    };
    canvas
        .validate()
        .map_err(|_| PngIoError::DimensionsTooLarge)?;
    let pixels = u64::from(info.width)
        .checked_mul(u64::from(info.height))
        .ok_or(PngIoError::DimensionsTooLarge)?;
    if pixels > nyatidraw_tiles::MAX_FLATTENED_PIXELS {
        return Err(PngIoError::DimensionsTooLarge);
    }
    let mut frame_buffer = vec![0; reader.output_buffer_size()];
    let output = reader
        .next_frame(&mut frame_buffer)
        .map_err(PngIoError::Decode)?;
    let source = &frame_buffer[..output.buffer_size()];
    let rgba = to_premultiplied_rgba(source, output.color_type)?;
    let tiles = rgba_to_tiles(&rgba, canvas, layer)?;
    Ok(ImportedPng { canvas, tiles })
}

/// Encodes a tightly-packed premultiplied surface as a standard straight-alpha PNG.
///
/// # Errors
///
/// Returns an error for an invalid surface or when the destination cannot be
/// created and fully encoded.
pub fn encode_png(path: &Path, surface: &FlattenedRgba8) -> Result<(), PngIoError> {
    let file = File::create(path)?;
    encode_png_writer(BufWriter::new(file), surface)
}

/// Encodes a tightly-packed premultiplied surface into an in-memory PNG.
///
/// This supports disposable previews without touching a user path.
///
/// # Errors
///
/// Returns an error for invalid surface dimensions or PNG encoding failure.
pub fn encode_png_bytes(surface: &FlattenedRgba8) -> Result<Vec<u8>, PngIoError> {
    let mut bytes = Vec::new();
    encode_png_writer(&mut bytes, surface)?;
    Ok(bytes)
}

fn encode_png_writer(writer: impl Write, surface: &FlattenedRgba8) -> Result<(), PngIoError> {
    let expected = usize::try_from(u64::from(surface.width) * u64::from(surface.height) * 4)
        .map_err(|_| PngIoError::InvalidSurface)?;
    if surface.width == 0 || surface.height == 0 || surface.pixels.len() != expected {
        return Err(PngIoError::InvalidSurface);
    }
    let mut encoder = png::Encoder::new(writer, surface.width, surface.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(PngIoError::Encode)?;
    let straight = unpremultiply(&surface.pixels);
    writer
        .write_image_data(&straight)
        .map_err(PngIoError::Encode)?;
    // Drop ignores IEND and buffered flush errors. A caller must never replace
    // the last good export after an incomplete encoding was reported as success.
    writer.finish().map_err(PngIoError::Encode)
}

fn to_premultiplied_rgba(source: &[u8], color: png::ColorType) -> Result<Vec<u8>, PngIoError> {
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other @ png::ColorType::Indexed => return Err(PngIoError::UnsupportedColor(other)),
    };
    if !source.len().is_multiple_of(channels) {
        return Err(PngIoError::InvalidSurface);
    }
    let mut rgba = Vec::with_capacity(source.len() / channels * 4);
    for pixel in source.chunks_exact(channels) {
        let (red, green, blue, alpha) = match color {
            png::ColorType::Grayscale => (pixel[0], pixel[0], pixel[0], 255),
            png::ColorType::GrayscaleAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
            png::ColorType::Rgb => (pixel[0], pixel[1], pixel[2], 255),
            png::ColorType::Rgba => (pixel[0], pixel[1], pixel[2], pixel[3]),
            png::ColorType::Indexed => unreachable!(),
        };
        rgba.extend_from_slice(&[
            premultiply(red, alpha),
            premultiply(green, alpha),
            premultiply(blue, alpha),
            alpha,
        ]);
    }
    Ok(rgba)
}

fn rgba_to_tiles(
    rgba: &[u8],
    canvas: CanvasSpec,
    layer: LayerId,
) -> Result<TileSnapshot, PngIoError> {
    let width = usize::try_from(canvas.width_px).map_err(|_| PngIoError::DimensionsTooLarge)?;
    let columns = canvas.width_px.div_ceil(TILE_EDGE);
    let rows = canvas.height_px.div_ceil(TILE_EDGE);
    let expanded_tile_bytes = u64::from(columns)
        .checked_mul(u64::from(rows))
        .and_then(|tiles| tiles.checked_mul(TILE_BYTE_LEN as u64))
        .ok_or(PngIoError::DimensionsTooLarge)?;
    if expanded_tile_bytes > MAX_EXPANDED_TILE_BYTES {
        return Err(PngIoError::DimensionsTooLarge);
    }
    let mut tiles = Vec::new();
    for tile_y in 0..rows {
        for tile_x in 0..columns {
            let mut tile = vec![0; TILE_BYTE_LEN];
            let copy_width = TILE_EDGE.min(canvas.width_px - tile_x * TILE_EDGE);
            let copy_height = TILE_EDGE.min(canvas.height_px - tile_y * TILE_EDGE);
            for row in 0..copy_height {
                let source_y = usize::try_from(tile_y * TILE_EDGE + row)
                    .map_err(|_| PngIoError::DimensionsTooLarge)?;
                let source_x = usize::try_from(tile_x * TILE_EDGE)
                    .map_err(|_| PngIoError::DimensionsTooLarge)?;
                let source_start = (source_y * width + source_x) * 4;
                let byte_count =
                    usize::try_from(copy_width).map_err(|_| PngIoError::DimensionsTooLarge)? * 4;
                let destination_start = usize::try_from(row)
                    .map_err(|_| PngIoError::DimensionsTooLarge)?
                    * usize::try_from(TILE_EDGE).expect("tile edge fits usize")
                    * 4;
                tile[destination_start..destination_start + byte_count]
                    .copy_from_slice(&rgba[source_start..source_start + byte_count]);
            }
            tiles.push((
                TileKey {
                    layer,
                    mip: 0,
                    x: i32::try_from(tile_x).map_err(|_| PngIoError::DimensionsTooLarge)?,
                    y: i32::try_from(tile_y).map_err(|_| PngIoError::DimensionsTooLarge)?,
                },
                tile,
            ));
        }
    }
    TileSnapshot::from_tiles(tiles).map_err(PngIoError::InvalidTiles)
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    u8::try_from((u16::from(channel) * u16::from(alpha) + 127) / 255)
        .expect("premultiplied channel is bounded by 255")
}

fn unpremultiply(pixels: &[u8]) -> Vec<u8> {
    let mut straight = Vec::with_capacity(pixels.len());
    for pixel in pixels.chunks_exact(4) {
        let alpha = pixel[3];
        if alpha == 0 {
            straight.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            for channel in &pixel[..3] {
                let value = (u32::from(*channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
                straight.push(value.min(255) as u8);
            }
            straight.push(alpha);
        }
    }
    straight
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_png_flush_failure_cannot_report_a_complete_artwork_export() {
        struct FailsOnFlush;
        impl Write for FailsOnFlush {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("final write failed"))
            }
        }
        let surface = FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 1,
            height: 1,
            pixels: vec![32, 16, 8, 255],
        };
        assert!(
            encode_png_writer(FailsOnFlush, &surface).is_err(),
            "a truncated PNG must not replace the last complete artwork"
        );
    }

    #[test]
    fn png_round_trip_preserves_valid_premultiplied_artwork() {
        // Product risk: PNG pairing must not alter the durable pixel result.
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-png-roundtrip-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos(),
        ));
        let surface = FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 1,
            height: 1,
            pixels: vec![64, 32, 16, 128],
        };
        encode_png(&path, &surface).expect("encode fixture");
        let imported = decode_png(&path, LayerId(9)).expect("decode fixture");
        let restored = imported
            .tiles
            .crop_base_layer_rgba8_to_canvas(LayerId(9), imported.canvas)
            .expect("crop fixture");
        assert_eq!(restored.pixels, surface.pixels);
        std::fs::remove_file(path).expect("remove scratch PNG");
    }
}

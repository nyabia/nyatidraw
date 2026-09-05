#![forbid(unsafe_code)]

//! PNG import/export at the application boundary.
//!
//! Tiles are premultiplied linear-light RGBA8. File exports are tagged sRGB
//! with straight 16-bit channels to preserve existing tile values on import.
//! Disposable previews use tagged 8-bit sRGB. Alpha always stays linear.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
    sync::OnceLock,
};

use nyatidraw_api::{CanvasSpec, LayerId};
use nyatidraw_tiles::color::{linear_to_srgb, srgb_to_linear};
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
    UnsupportedColorProfile(&'static str),
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
            Self::UnsupportedColorProfile(reason) => {
                write!(formatter, "unsupported PNG color profile: {reason}")
            }
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
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(PngIoError::Decode)?;
    let info = reader.info();
    let transfer = source_transfer(info)?;
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
    let rgba = to_premultiplied_rgba(source, output.color_type, output.bit_depth, transfer)?;
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
    encode_png_writer(BufWriter::new(file), surface, png::BitDepth::Sixteen)
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
    encode_png_writer(&mut bytes, surface, png::BitDepth::Eight)?;
    Ok(bytes)
}

fn encode_png_writer(
    writer: impl Write,
    surface: &FlattenedRgba8,
    depth: png::BitDepth,
) -> Result<(), PngIoError> {
    let pixels = u64::from(surface.width) * u64::from(surface.height);
    let expected = usize::try_from(pixels * 4).map_err(|_| PngIoError::InvalidSurface)?;
    if surface.width == 0
        || surface.height == 0
        || pixels > nyatidraw_tiles::MAX_FLATTENED_PIXELS
        || surface.pixels.len() != expected
    {
        return Err(PngIoError::InvalidSurface);
    }
    let mut encoder = png::Encoder::new(writer, surface.width, surface.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(depth);
    encoder.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(PngIoError::Encode)?;
    let mut stream = writer.stream_writer().map_err(PngIoError::Encode)?;
    // One encoded row, not a second full 16-bit image. The lookup has a fixed
    // 128 KiB bound and performs no transfer-function pow in the pixel loop.
    let table = export_table();
    let row_bytes = usize::try_from(surface.width).map_err(|_| PngIoError::InvalidSurface)? * 4;
    let mut row = Vec::with_capacity(
        row_bytes
            * if depth == png::BitDepth::Sixteen {
                2
            } else {
                1
            },
    );
    for source_row in surface.pixels.chunks_exact(row_bytes) {
        row.clear();
        for pixel in source_row.chunks_exact(4) {
            let alpha = pixel[3];
            for channel in &pixel[..3] {
                let channel_srgb = table[usize::from(alpha) * 256 + usize::from(*channel)];
                if depth == png::BitDepth::Sixteen {
                    row.extend_from_slice(&channel_srgb.to_be_bytes());
                } else {
                    row.push(
                        u8::try_from((u32::from(channel_srgb) + 128) / 257).expect("sRGB8 channel"),
                    );
                }
            }
            if depth == png::BitDepth::Sixteen {
                row.extend_from_slice(&(u16::from(alpha) * 257).to_be_bytes());
            } else {
                row.push(alpha);
            }
        }
        stream.write_all(&row)?;
    }
    stream.finish().map_err(PngIoError::Encode)?;
    // Both stream completion and the final IEND/flush must succeed before the
    // export worker can replace its generation-guarded sibling temporary file.
    writer.finish().map_err(PngIoError::Encode)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn export_table() -> &'static [u16] {
    static TABLE: OnceLock<Box<[u16]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut values = vec![0; 256 * 256];
        for alpha in 1..=255_u16 {
            for channel in 0..=255_u16 {
                let linear = f64::from(channel.min(alpha)) / f64::from(alpha);
                values[usize::from(alpha) * 256 + usize::from(channel)] =
                    (linear_to_srgb(linear) * 65535.0).round() as u16;
            }
        }
        values.into_boxed_slice()
    })
}

#[derive(Clone, Copy)]
enum SourceTransfer {
    Srgb,
    Gamma(f64),
}

fn source_transfer(info: &png::Info<'_>) -> Result<SourceTransfer, PngIoError> {
    // Do not interpret unsupported higher-precedence color data as sRGB.
    if info.coding_independent_code_points.is_some() {
        return Err(PngIoError::UnsupportedColorProfile("cICP is not supported"));
    }
    if info.icc_profile.is_some() {
        return Err(PngIoError::UnsupportedColorProfile(
            "ICC is not supported; convert to sRGB first",
        ));
    }
    if info.srgb.is_some() {
        return Ok(SourceTransfer::Srgb);
    }
    let srgb_primaries =
        png::SourceChromaticities::new((0.3127, 0.3290), (0.64, 0.33), (0.30, 0.60), (0.15, 0.06));
    if info
        .source_chromaticities
        .is_some_and(|primaries| primaries != srgb_primaries)
    {
        return Err(PngIoError::UnsupportedColorProfile(
            "non-sRGB primaries are not supported",
        ));
    }
    if let Some(gamma) = info.source_gamma {
        let scaled = gamma.into_scaled();
        if scaled == 0 {
            return Err(PngIoError::UnsupportedColorProfile("zero gamma"));
        }
        return Ok(SourceTransfer::Gamma(f64::from(scaled) / 100_000.0));
    }
    // Untagged input follows the application's explicit sRGB assumption.
    Ok(SourceTransfer::Srgb)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn to_premultiplied_rgba(
    source: &[u8],
    color: png::ColorType,
    depth: png::BitDepth,
    transfer: SourceTransfer,
) -> Result<Vec<u8>, PngIoError> {
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        other @ png::ColorType::Indexed => return Err(PngIoError::UnsupportedColor(other)),
    };
    let bytes_per_channel = match depth {
        png::BitDepth::Eight => 1,
        png::BitDepth::Sixteen => 2,
        _ => return Err(PngIoError::InvalidSurface),
    };
    let stride = channels * bytes_per_channel;
    if !source.len().is_multiple_of(stride) {
        return Err(PngIoError::InvalidSurface);
    }
    let maximum = if bytes_per_channel == 1 {
        255_u32
    } else {
        65535
    };
    // Bounded per-import lookup, at most 512 KiB, including gamma-only sources.
    let table: Vec<f64> = (0..=maximum)
        .map(|value| {
            let encoded = f64::from(value) / f64::from(maximum);
            match transfer {
                SourceTransfer::Srgb => srgb_to_linear(encoded),
                SourceTransfer::Gamma(gamma) => encoded.powf(1.0 / gamma),
            }
        })
        .collect();
    let mut rgba = Vec::with_capacity(source.len() / stride * 4);
    for pixel in source.chunks_exact(stride) {
        let sample = |index: usize| {
            if bytes_per_channel == 1 {
                usize::from(pixel[index])
            } else {
                usize::from(u16::from_be_bytes([pixel[index * 2], pixel[index * 2 + 1]]))
            }
        };
        let (red, green, blue, alpha) = match color {
            png::ColorType::Grayscale => (sample(0), sample(0), sample(0), maximum as usize),
            png::ColorType::GrayscaleAlpha => (sample(0), sample(0), sample(0), sample(1)),
            png::ColorType::Rgb => (sample(0), sample(1), sample(2), maximum as usize),
            png::ColorType::Rgba => (sample(0), sample(1), sample(2), sample(3)),
            png::ColorType::Indexed => unreachable!(),
        };
        let alpha =
            f64::from(u32::try_from(alpha).expect("16-bit alpha")) * 255.0 / f64::from(maximum);
        rgba.extend_from_slice(&[
            (table[red] * alpha).round() as u8,
            (table[green] * alpha).round() as u8,
            (table[blue] * alpha).round() as u8,
            alpha.round() as u8,
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
            encode_png_writer(FailsOnFlush, &surface, png::BitDepth::Sixteen).is_err(),
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
        let mut pixels = Vec::with_capacity(256 * 129 * 4);
        for alpha in 0..=255_u8 {
            for channel in 0..=alpha {
                pixels.extend_from_slice(&[channel, alpha - channel, channel / 2, alpha]);
            }
        }
        pixels.resize(256 * 129 * 4, 0);
        let surface = FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 256,
            height: 129,
            pixels,
        };
        encode_png(&path, &surface).expect("encode fixture");
        // Independent encoded midpoint: straight linear 0.5 is sRGB16 48192.
        // Checking raw PNG bytes catches two inverse but equally wrong paths.
        let mut raw = png::Decoder::new(BufReader::new(File::open(&path).unwrap()))
            .read_info()
            .unwrap();
        assert_eq!(raw.info().bit_depth, png::BitDepth::Sixteen);
        assert_eq!(
            raw.info().srgb,
            Some(png::SrgbRenderingIntent::RelativeColorimetric)
        );
        let mut encoded = vec![0; raw.output_buffer_size()];
        raw.next_frame(&mut encoded).unwrap();
        let index = (128 * 129 / 2 + 64) * 8;
        assert_eq!(
            u16::from_be_bytes([encoded[index], encoded[index + 1]]),
            48192
        );
        assert_eq!(
            u16::from_be_bytes([encoded[index + 6], encoded[index + 7]]),
            32896
        );
        let imported = decode_png(&path, LayerId(9)).expect("decode fixture");
        let restored = imported
            .tiles
            .crop_base_layer_rgba8_to_canvas(LayerId(9), imported.canvas)
            .expect("crop fixture");
        assert_eq!(restored.pixels, surface.pixels);
        drop(raw);
        std::fs::remove_file(path).expect("remove scratch PNG");
    }

    #[test]
    fn png_color_metadata_cannot_silently_reinterpret_imported_artwork() {
        let untagged = png::Info::default();
        let mut srgb = png::Info::default();
        srgb.srgb = Some(png::SrgbRenderingIntent::RelativeColorimetric);
        srgb.source_gamma = Some(png::ScaledFloat::from_scaled(100_000));
        let mut linear = png::Info::default();
        linear.source_gamma = Some(png::ScaledFloat::from_scaled(100_000));
        for (info, expected) in [(&untagged, 55), (&srgb, 55), (&linear, 128)] {
            let transfer = source_transfer(info).unwrap();
            for (color, source, expected_alpha) in [
                (png::ColorType::Grayscale, vec![128], 255),
                (png::ColorType::Rgb, vec![128; 3], 255),
                (png::ColorType::Rgba, vec![128, 128, 128, 255], 255),
            ] {
                assert_eq!(
                    to_premultiplied_rgba(&source, color, png::BitDepth::Eight, transfer).unwrap(),
                    [expected, expected, expected, expected_alpha]
                );
            }
        }
        assert_eq!(
            to_premultiplied_rgba(
                &[128, 128],
                png::ColorType::GrayscaleAlpha,
                png::BitDepth::Eight,
                SourceTransfer::Srgb
            )
            .unwrap(),
            [28, 28, 28, 128]
        );
        let mut icc = srgb.clone();
        icc.icc_profile = Some(vec![0; 128].into());
        let mut cicp = srgb.clone();
        cicp.coding_independent_code_points = Some(png::CodingIndependentCodePoints {
            color_primaries: 9,
            transfer_function: 16,
            matrix_coefficients: 0,
            is_video_full_range_image: true,
        });
        let mut zero_gamma = png::Info::default();
        zero_gamma.source_gamma = Some(png::ScaledFloat::from_scaled(0));
        let mut p3 = png::Info::default();
        p3.source_chromaticities = Some(png::SourceChromaticities::new(
            (0.3127, 0.3290),
            (0.68, 0.32),
            (0.265, 0.69),
            (0.15, 0.06),
        ));
        for info in [icc, cicp, zero_gamma, p3] {
            assert!(
                matches!(
                    source_transfer(&info),
                    Err(PngIoError::UnsupportedColorProfile(_))
                ),
                "unsupported profiles must fail before artwork import"
            );
        }
    }
}

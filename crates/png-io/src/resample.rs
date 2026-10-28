use std::io::{BufWriter, Write};
use std::path::Path;

use nyatidraw_tiles::{FlattenedRgba8, color::linear_to_srgb};

use crate::PngIoError;

/// Encodes a new reduced copy without changing or allocating a second source image.
/// # Errors
/// Rejects invalid surfaces, enlargement, and I/O or PNG encoding failures.
pub fn encode_png_reduced(
    path: &Path,
    surface: &FlattenedRgba8,
    size: [u32; 2],
) -> Result<(), PngIoError> {
    validate_reduction(surface, size)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let result = encode_reduced(BufWriter::new(file), surface, size);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

/// Encodes an alpha-correct, linear-light area reduction as tagged sRGB16 PNG.
/// # Errors
/// Rejects invalid surfaces, enlargement, and PNG encoding failures.
pub fn encode_png_reduced_bytes(
    surface: &FlattenedRgba8,
    size: [u32; 2],
) -> Result<Vec<u8>, PngIoError> {
    validate_reduction(surface, size)?;
    let mut bytes = Vec::new();
    encode_reduced(&mut bytes, surface, size)?;
    Ok(bytes)
}

fn validate_reduction(
    surface: &FlattenedRgba8,
    [width, height]: [u32; 2],
) -> Result<(), PngIoError> {
    let pixels = u64::from(surface.width) * u64::from(surface.height);
    if width == 0 || height == 0 || width > surface.width || height > surface.height {
        return Err(PngIoError::DimensionsTooLarge);
    }
    if pixels > nyatidraw_tiles::MAX_FLATTENED_PIXELS
        || usize::try_from(pixels * 4).ok() != Some(surface.pixels.len())
    {
        return Err(PngIoError::InvalidSurface);
    }
    Ok(())
}

fn encode_reduced(
    destination: impl Write,
    surface: &FlattenedRgba8,
    [width, height]: [u32; 2],
) -> Result<(), PngIoError> {
    if [surface.width, surface.height] == [width, height] {
        return crate::encode_png_writer(destination, surface, png::BitDepth::Sixteen);
    }
    let mut encoder = png::Encoder::new(destination, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Sixteen);
    encoder.set_source_srgb(png::SrgbRenderingIntent::RelativeColorimetric);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(PngIoError::Encode)?;
    let mut stream = writer.stream_writer().map_err(PngIoError::Encode)?;
    let mut row = vec![0_u8; width.min(1024) as usize * 8];
    for y in 0..height {
        for start in (0..width).step_by(1024) {
            let end = (start + 1024).min(width);
            for x in start..end {
                let pixel = reduced_pixel(surface, [width, height], [x, y]);
                for (channel, value) in row[(x - start) as usize * 8..][..8]
                    .chunks_exact_mut(2)
                    .zip(pixel)
                {
                    channel.copy_from_slice(&value.to_be_bytes());
                }
            }
            stream.write_all(&row[..(end - start) as usize * 8])?;
        }
    }
    stream.finish().map_err(PngIoError::Encode)?;
    writer.finish().map_err(PngIoError::Encode)
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn reduced_pixel(surface: &FlattenedRgba8, size: [u32; 2], position: [u32; 2]) -> [u16; 4] {
    let [width, height] = size.map(u64::from);
    let [x, y] = position.map(u64::from);
    let source_width = u64::from(surface.width);
    let source_height = u64::from(surface.height);
    let left = x * source_width;
    let right = (x + 1) * source_width;
    let top = y * source_height;
    let bottom = (y + 1) * source_height;
    let mut sum = [0_u64; 4];
    // Integer overlap weights integrate every source pixel, including fractional
    // borders. Premultiplied linear channels prevent dark transparent fringes.
    for sy in top / height..bottom.div_ceil(height) {
        let wy = bottom.min((sy + 1) * height) - top.max(sy * height);
        for sx in left / width..right.div_ceil(width) {
            let wx = right.min((sx + 1) * width) - left.max(sx * width);
            let offset = ((sy * source_width + sx) * 4) as usize;
            let pixel = &surface.pixels[offset..offset + 4];
            for channel in 0..4 {
                sum[channel] += u64::from(pixel[channel].min(pixel[3])) * wx * wy;
            }
        }
    }
    let mut output = [0; 4];
    if sum[3] != 0 {
        for channel in 0..3 {
            output[channel] =
                (linear_to_srgb(sum[channel] as f64 / sum[3] as f64) * 65535.0).round() as u16;
        }
        output[3] =
            ((sum[3] as f64 * 257.0) / (source_width * source_height) as f64).round() as u16;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_reduction_preserves_transparent_edges_energy_and_source_pixels() {
        for (width, height, pixels, size, expected) in [
            (
                2,
                1,
                vec![255, 0, 0, 255, 0, 0, 0, 0],
                [1, 1],
                vec![255_u8, 0, 0, 128],
            ),
            (
                2,
                1,
                vec![0, 0, 0, 255, 255, 255, 255, 255],
                [1, 1],
                vec![188, 188, 188, 255],
            ),
            (
                3,
                1,
                vec![255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 255, 255],
                [2, 1],
                vec![255, 0, 0, 170, 0, 0, 255, 170],
            ),
            (
                3,
                3,
                (0..9)
                    .flat_map(|index| {
                        if index % 2 == 0 {
                            [255; 4]
                        } else {
                            [0, 0, 0, 255]
                        }
                    })
                    .collect(),
                [2, 2],
                [197, 197, 197, 255].repeat(4),
            ),
        ] {
            let surface = FlattenedRgba8 {
                origin_x: 0,
                origin_y: 0,
                width,
                height,
                pixels,
            };
            let original = surface.clone();
            let mut full_size = Vec::new();
            crate::encode_png_writer(&mut full_size, &surface, png::BitDepth::Sixteen).unwrap();
            assert_eq!(
                encode_png_reduced_bytes(&surface, [width, height]).unwrap(),
                full_size
            );
            let bytes = encode_png_reduced_bytes(&surface, size).unwrap();
            let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
            decoder.set_transformations(png::Transformations::STRIP_16);
            let mut reader = decoder.read_info().unwrap();
            assert!(reader.info().srgb.is_some());
            let mut result = vec![0; reader.output_buffer_size()];
            reader.next_frame(&mut result).unwrap();
            for (actual, expected) in result.iter().zip(&expected) {
                assert!(i16::from(*actual).abs_diff(i16::from(*expected)) <= 1);
            }
            assert_eq!(surface, original);
        }
    }

    #[test]
    fn invalid_reduction_cannot_truncate_an_existing_export() {
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-reduction-invalid-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, b"preserve existing artwork").unwrap();
        let surface = FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 1,
            height: 1,
            pixels: vec![255; 4],
        };
        for size in [[0, 1], [1, 0], [2, 1], [1, 2]] {
            assert!(encode_png_reduced(&path, &surface, size).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"preserve existing artwork");
        }
        std::fs::remove_file(path).unwrap();
    }
}

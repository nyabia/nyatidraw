use nyatidraw_paint_cpu::RasterFragment;
use nyatidraw_tiles::color::srgb8_to_linear_premultiplied;

const INVALID: &str = "잘못되거나 잘린 비트맵 클립보드 데이터입니다.";
const UNSUPPORTED: &str = "이 비트맵 형식은 지원하지 않습니다. PNG로 가져오세요.";

struct Dib<'a> {
    bytes: &'a [u8],
    width: u32,
    height: u32,
    top_down: bool,
    bytes_per_pixel: usize,
    stride: usize,
    offset: usize,
    masks: [u32; 4],
    output_len: usize,
}

impl<'a> Dib<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() > super::MAX_BYTES {
            return Err(INVALID.into());
        }
        let header = read_u32(bytes, 0)? as usize;
        if !matches!(header, 40 | 108 | 124) {
            return Err(UNSUPPORTED.into());
        }
        let header_bytes = bytes.get(..header).ok_or(INVALID)?;
        let width = read_i32(bytes, 4)?;
        let height = read_i32(bytes, 8)?;
        if width <= 0 || height == 0 || height == i32::MIN || read_u16(bytes, 12)? != 1 {
            return Err(INVALID.into());
        }
        let size = [width.unsigned_abs(), height.unsigned_abs()];
        let output_len =
            RasterFragment::byte_len([0, 0], size).map_err(|e| format!("Clipboard: {e:?}"))?;
        if output_len > super::MAX_BYTES {
            return Err(INVALID.into());
        }
        let depth = read_u16(bytes, 14)?;
        let compression = read_u32(bytes, 16)?;
        if !matches!((depth, compression), (24 | 32, 0) | (32, 3)) {
            return Err(UNSUPPORTED.into());
        }
        if header >= 108 {
            let color_space = read_u32(bytes, 56)?;
            let untagged = color_space == 0 && header_bytes[60..108].iter().all(|byte| *byte == 0);
            if !untagged && !matches!(color_space, 0x7352_4742 | 0x5769_6e20) {
                return Err("색 프로파일이 있는 비트맵은 sRGB PNG로 가져오세요.".into());
            }
            if header == 124 && (read_u32(bytes, 112)? != 0 || read_u32(bytes, 116)? != 0) {
                return Err(UNSUPPORTED.into());
            }
        }
        // BI_RGB's fourth byte is reserved, not alpha. Only explicit bitfields carry alpha.
        let masks = if compression == 3 {
            [
                read_u32(bytes, 40)?,
                read_u32(bytes, 44)?,
                read_u32(bytes, 48)?,
                if header >= 108 {
                    read_u32(bytes, 52)?
                } else {
                    0
                },
            ]
        } else {
            [0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0]
        };
        validate_masks(masks)?;
        let palette_entries = read_u32(bytes, 32)? as usize;
        let offset = header
            .checked_add(if header == 40 && compression == 3 {
                12
            } else {
                0
            })
            .and_then(|offset| {
                palette_entries
                    .checked_mul(4)
                    .and_then(|palette| offset.checked_add(palette))
            })
            .ok_or(INVALID)?;
        let bytes_per_pixel = usize::from(depth / 8);
        let stride = (size[0] as usize)
            .checked_mul(bytes_per_pixel)
            .and_then(|row| row.checked_add(3))
            .map(|row| row & !3)
            .ok_or(INVALID)?;
        let end = stride
            .checked_mul(size[1] as usize)
            .and_then(|len| offset.checked_add(len))
            .ok_or(INVALID)?;
        bytes.get(offset..end).ok_or(INVALID)?;
        Ok(Self {
            bytes,
            width: size[0],
            height: size[1],
            top_down: height < 0,
            bytes_per_pixel,
            stride,
            offset,
            masks,
            output_len,
        })
    }

    fn decode(&self) -> Result<RasterFragment, String> {
        let mut pixels = Vec::with_capacity(self.output_len);
        for y in 0..self.height as usize {
            let source_y = if self.top_down {
                y
            } else {
                self.height as usize - 1 - y
            };
            let row = self.offset + source_y * self.stride;
            for x in 0..self.width as usize {
                let at = row + x * self.bytes_per_pixel;
                let mut word = [0; 4];
                word[..self.bytes_per_pixel]
                    .copy_from_slice(&self.bytes[at..at + self.bytes_per_pixel]);
                let word = u32::from_le_bytes(word);
                let rgba =
                    std::array::from_fn(|channel| extract_channel(word, self.masks[channel]));
                pixels.extend_from_slice(&srgb8_to_linear_premultiplied(rgba));
            }
        }
        RasterFragment::new([0, 0], [self.width, self.height], pixels)
            .map_err(|e| format!("Clipboard: {e:?}"))
    }
}

pub(crate) fn decode(bytes: &[u8]) -> Result<RasterFragment, String> {
    Dib::parse(bytes)?.decode()
}

fn validate_masks(masks: [u32; 4]) -> Result<(), String> {
    let mut used = 0;
    for (index, mask) in masks.into_iter().enumerate() {
        if mask == 0 {
            if index == 3 {
                continue;
            }
            return Err(INVALID.into());
        }
        let normalized = mask >> mask.trailing_zeros();
        if normalized & normalized.wrapping_add(1) != 0 || used & mask != 0 {
            return Err(INVALID.into());
        }
        used |= mask;
    }
    Ok(())
}

fn extract_channel(word: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let shift = mask.trailing_zeros();
    let maximum = u64::from(mask >> shift);
    let value = u64::from((word & mask) >> shift);
    u8::try_from((value * 255 + maximum / 2) / maximum).expect("normalized channel")
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(INVALID)?
            .try_into()
            .expect("checked word"),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(INVALID)?
            .try_into()
            .expect("checked word"),
    ))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, String> {
    Ok(i32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(INVALID)?
            .try_into()
            .expect("checked word"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(
        header: usize,
        width: i32,
        height: i32,
        depth: u16,
        compression: u32,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut bytes = vec![0; header];
        bytes[0..4].copy_from_slice(&u32::try_from(header).unwrap().to_le_bytes());
        bytes[4..8].copy_from_slice(&width.to_le_bytes());
        bytes[8..12].copy_from_slice(&height.to_le_bytes());
        bytes[12..14].copy_from_slice(&1_u16.to_le_bytes());
        bytes[14..16].copy_from_slice(&depth.to_le_bytes());
        bytes[16..20].copy_from_slice(&compression.to_le_bytes());
        if header >= 108 {
            for (index, mask) in [0x00ff_0000_u32, 0x0000_ff00, 0x0000_00ff, 0xff00_0000]
                .into_iter()
                .enumerate()
            {
                bytes[40 + index * 4..44 + index * 4].copy_from_slice(&mask.to_le_bytes());
            }
            bytes[56..60].copy_from_slice(&0x7352_4742_u32.to_le_bytes());
        }
        bytes.extend_from_slice(pixels);
        bytes
    }

    #[test]
    fn dib_import_preserves_orientation_color_and_explicit_alpha_without_losing_bgrx_artwork() {
        let cases = [
            (
                bitmap(40, 1, 2, 24, 0, &[255, 0, 0, 0, 0, 0, 255, 0]),
                vec![255, 0, 0, 255, 0, 0, 255, 255],
            ),
            (
                bitmap(40, 1, -2, 32, 0, &[0, 0, 255, 0, 255, 0, 0, 0]),
                vec![255, 0, 0, 255, 0, 0, 255, 255],
            ),
            (
                bitmap(124, 1, -2, 32, 3, &[0, 0, 255, 128, 255, 255, 255, 0]),
                vec![128, 0, 0, 128, 0, 0, 0, 0],
            ),
            (
                bitmap(108, 1, 1, 32, 3, &[128, 128, 128, 255]),
                vec![55, 55, 55, 255],
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(decode(&bytes).unwrap().pixels(), expected);
        }
    }

    #[test]
    fn malformed_dib_cannot_allocate_unbounded_pixels_or_guess_unsupported_color() {
        let valid = bitmap(124, 1, 1, 32, 3, &[0, 0, 255, 128]);
        for length in 0..valid.len() {
            assert!(
                decode(&valid[..length]).is_err(),
                "truncated length {length}"
            );
        }
        for (offset, value) in [
            (0, u32::MAX),
            (4, i32::MAX.cast_unsigned()),
            (8, i32::MIN.cast_unsigned()),
            (4, 0),
            (8, 0),
            (16, 1),
            (32, u32::MAX),
            (40, 0),
            (40, 0xff),
            (40, 0x000a_0000),
            (56, 0x4c49_4e4b),
            (112, 124),
            (116, 4),
        ] {
            let mut invalid = valid.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                decode(&invalid).is_err(),
                "invalid header field {offset} = {value}"
            );
        }
    }
}

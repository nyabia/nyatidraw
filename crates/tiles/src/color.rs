//! Color transfer at the boundary of premultiplied linear-light tile bytes.

use std::sync::OnceLock;

/// Decodes one normalized sRGB channel. Alpha must not use this transfer.
#[must_use]
pub fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Encodes one normalized linear-light channel. Alpha remains linear.
#[must_use]
pub fn linear_to_srgb(value: f64) -> f64 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Converts straight UI sRGB8 to the canonical premultiplied tile color.
///
/// The fixed 256-entry transfer table is initialized once. Premultiplication
/// happens before rounding so translucent colors do not suffer double rounding.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn srgb8_to_linear_premultiplied(color: [u8; 4]) -> [u8; 4] {
    static DECODE: OnceLock<[f64; 256]> = OnceLock::new();
    let table = DECODE.get_or_init(|| {
        std::array::from_fn(|index| srgb_to_linear(f64::from(index as u8) / 255.0))
    });
    let alpha = color[3];
    let channel = |value: u8| (table[usize::from(value)] * f64::from(alpha)).round() as u8;
    [
        channel(color[0]),
        channel(color[1]),
        channel(color[2]),
        alpha,
    ]
}

/// Unpremultiplies durable linear RGBA8 and converts RGB to straight UI sRGB.
/// Fully transparent pixels have no recoverable color.
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn linear_premultiplied_to_srgb8(color: [u8; 4]) -> Option<[u8; 4]> {
    if color[3] == 0 {
        return None;
    }
    let channel = |value: u8| {
        (linear_to_srgb(f64::from(value.min(color[3])) / f64::from(color[3])) * 255.0).round() as u8
    };
    Some([
        channel(color[0]),
        channel(color[1]),
        channel(color[2]),
        color[3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_transfer_preserves_artwork_alpha_and_does_not_treat_srgb_as_linear() {
        for (input, expected) in [
            ([128, 128, 128, 255], [55, 55, 55, 255]),
            ([128, 128, 128, 128], [28, 28, 28, 128]),
            ([26, 199, 232, 255], [3, 146, 206, 255]),
            ([255, 128, 0, 0], [0, 0, 0, 0]),
            ([255, 255, 255, 1], [1, 1, 1, 1]),
        ] {
            assert_eq!(srgb8_to_linear_premultiplied(input), expected);
        }
        for alpha in 0..=255 {
            for channel in 0..=255 {
                let actual = srgb8_to_linear_premultiplied([channel, channel, channel, alpha]);
                assert_eq!(actual[3], alpha);
                assert!(actual[..3].iter().all(|value| *value <= alpha));
            }
        }
        // Sampling must not darken translucent artwork or invent a color from
        // alpha-zero bytes. Low-alpha quantization cannot be losslessly undone.
        for (pixel, expected) in [
            ([55, 55, 55, 255], Some([128, 128, 128, 255])),
            ([128, 0, 0, 128], Some([255, 0, 0, 128])),
            ([1, 1, 1, 1], Some([255, 255, 255, 1])),
            ([0, 0, 0, 0], None),
        ] {
            assert_eq!(linear_premultiplied_to_srgb8(pixel), expected);
        }
    }
}

//! Color transfer at the boundary of premultiplied linear-light tile bytes.

use nyatidraw_api::LayerColorEffect;
use std::sync::OnceLock;

/// Two bounded alpha/channel transfer tables: straight sRGB8 encoding followed
/// by premultiplied linear RGBA8 decoding. GPU uploads these same integers.
pub const LAYER_COLOR_TRANSFER_ENTRIES: usize = 2 * 256 * 256;

#[must_use]
pub fn layer_color_transfer_tables() -> &'static [u32] {
    static TABLES: OnceLock<Box<[u32]>> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut values = vec![0; LAYER_COLOR_TRANSFER_ENTRIES];
        for alpha in 0..=255_u8 {
            for channel in 0..=255_u8 {
                let index = usize::from(alpha) * 256 + usize::from(channel);
                values[index] = u32::from(
                    linear_premultiplied_to_srgb8([channel, channel, channel, alpha])
                        .map_or(0, |color| color[0]),
                );
                values[65_536 + index] =
                    u32::from(srgb8_to_linear_premultiplied([channel, channel, channel, alpha])[0]);
            }
        }
        values.into_boxed_slice()
    })
}

/// Composition-only two-color mapping. Rec.709 coefficients are applied to
/// straight sRGB8 (2126/7152/722), rounded to a byte, then endpoint interpolation
/// is rounded to sRGB8 before decoding with the original, exact alpha.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn apply_layer_color_effect(pixel: [u8; 4], effect: Option<LayerColorEffect>) -> [u8; 4] {
    let Some(effect) = effect.filter(|effect| effect.enabled) else {
        return pixel;
    };
    if pixel[3] == 0 {
        return [0; 4];
    }
    let tables = layer_color_transfer_tables();
    let offset = usize::from(pixel[3]) * 256;
    let straight =
        [pixel[0], pixel[1], pixel[2]].map(|channel| tables[offset + usize::from(channel)]);
    let luma = (straight[0] * 2126 + straight[1] * 7152 + straight[2] * 722 + 5000) / 10_000;
    let rgb = std::array::from_fn::<_, 3, _>(|channel| {
        let srgb = (u32::from(effect.dark_rgb[channel]) * (255 - luma)
            + u32::from(effect.light_rgb[channel]) * luma
            + 127)
            / 255;
        tables[65_536 + offset + srgb as usize] as u8
    });
    [rgb[0], rgb[1], rgb[2], pixel[3]]
}

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
    fn layer_color_mapping_preserves_alpha_and_disabled_artwork_bytes() {
        let effect = LayerColorEffect::default();
        let disabled = LayerColorEffect {
            enabled: false,
            ..effect
        };
        for pixel in [[0; 4], [0, 0, 0, 255], [255; 4], [128, 0, 0, 128], [1; 4]] {
            assert_eq!(apply_layer_color_effect(pixel, None), pixel);
            assert_eq!(apply_layer_color_effect(pixel, Some(disabled)), pixel);
        }
        for alpha in 0..=255_u8 {
            for channel in 0..=alpha {
                let pixel = [channel, channel / 2, alpha - channel, alpha];
                let mapped = apply_layer_color_effect(pixel, Some(effect));
                assert_eq!(mapped[3], alpha);
                assert!(mapped[..3].iter().all(|value| *value <= alpha));
            }
            assert_eq!(
                apply_layer_color_effect([0, 0, 0, alpha], Some(effect)),
                srgb8_to_linear_premultiplied([96, 144, 224, alpha])
            );
            if alpha != 0 {
                assert_eq!(
                    apply_layer_color_effect([alpha; 4], Some(effect)),
                    [alpha; 4]
                );
            }
        }
        // Pure red's straight sRGB luma rounds to 54, mapping to [130,168,231].
        assert_eq!(
            apply_layer_color_effect([128, 0, 0, 128], Some(effect)),
            srgb8_to_linear_premultiplied([130, 168, 231, 128])
        );
    }

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

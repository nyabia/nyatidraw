//! Bounded active-raster corrections in straight sRGB, retaining exact alpha.
use crate::{EditError, EditLimits, SelectionMask, SelectionPaintResult};
pub use nyatidraw_api::ColorAdjustment;
use nyatidraw_api::LayerId;
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot, color};

fn validate_adjustment(value: ColorAdjustment) -> Result<(), EditError> {
    if !(-180..=180).contains(&value.hue_degrees)
        || [
            value.saturation_percent,
            value.value_percent,
            value.brightness_percent,
            value.contrast_percent,
        ]
        .iter()
        .any(|value| !(-100..=100).contains(value))
    {
        return Err(EditError::InvalidPaint);
    }
    Ok(())
}

/// Recomputes from the immutable source, including signed off-page tiles.
/// # Errors
/// Rejects locked/missing layers, invalid parameters, mips and resource limits.
pub fn adjust_raster_colors(
    snapshot: &TileSnapshot,
    tree: &LayerTree,
    target: LayerId,
    selection: Option<&SelectionMask>,
    adjustment: ColorAdjustment,
    limits: EditLimits,
) -> Result<SelectionPaintResult, EditError> {
    validate_adjustment(adjustment)?;
    let layer = tree.raster(target).ok_or(EditError::UnknownLayer)?;
    if layer.locked {
        return Err(EditError::LockedLayer);
    }
    let limits = limits.bounded();
    let mask_bytes = selection.map_or(0, |mask| {
        let [width, height] = mask.dimensions();
        u64::from(width) * u64::from(height)
    });
    let mut keys = Vec::new();
    let bounds = selection.and_then(SelectionMask::bounds_signed);
    for (key, _) in snapshot.iter().filter(|(key, _)| key.layer == target) {
        if key.mip != 0 {
            return Err(EditError::InvalidPaint);
        }
        if selection.is_some() && bounds.is_none() {
            continue;
        }
        if bounds.is_some_and(|(origin, size)| !tile_intersects(key, origin, size)) {
            continue;
        }
        keys.push(key);
        let pixels = keys.len() as u64 * u64::from(TILE_EDGE).pow(2);
        // Two candidates can coexist while replacing a visible preview.
        let bytes = keys.len() as u64 * TILE_BYTE_LEN as u64 * 2 + mask_bytes;
        if pixels > limits.max_pixels
            || pixels > limits.max_filter_work
            || bytes > limits.max_workspace_bytes
        {
            return Err(EditError::LimitExceeded);
        }
    }
    if adjustment == ColorAdjustment::default() {
        return Ok(SelectionPaintResult {
            after: snapshot.clone(),
            changed_tiles: Vec::new(),
        });
    }
    let mut replacements = Vec::new();
    for key in keys {
        let source = snapshot.get(key).ok_or(EditError::InvalidPaint)?;
        let mut pixels = source.pixels().to_vec();
        let (left, top) = key.pixel_origin();
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let mut coverage = 255;
            if let Some(mask) = selection {
                let index = u32::try_from(index).map_err(|_| EditError::LimitExceeded)?;
                let [Ok(x), Ok(y)] = [
                    left + i64::from(index % TILE_EDGE),
                    top + i64::from(index / TILE_EDGE),
                ]
                .map(i32::try_from) else {
                    continue;
                };
                coverage = mask.coverage_signed(x, y);
                if coverage == 0 {
                    continue;
                }
            }
            let original = [pixel[0], pixel[1], pixel[2], pixel[3]];
            pixel.copy_from_slice(&adjust_selected_pixel(original, adjustment, coverage));
        }
        if pixels != source.pixels() {
            replacements.push((key, pixels));
        }
    }
    let changed_tiles = replacements.iter().map(|(key, _)| *key).collect();
    let after = snapshot
        .with_replacements(replacements)
        .map_err(EditError::Tiles)?;
    Ok(SelectionPaintResult {
        after,
        changed_tiles,
    })
}

fn adjust_selected_pixel(original: [u8; 4], adjustment: ColorAdjustment, coverage: u8) -> [u8; 4] {
    let adjusted = adjust_pixel(original, adjustment);
    let mut result = original;
    for channel in 0..3 {
        result[channel] = u8::try_from(
            (u32::from(original[channel]) * u32::from(255 - coverage)
                + u32::from(adjusted[channel]) * u32::from(coverage)
                + 127)
                / 255,
        )
        .expect("weighted channels retain the original alpha bound");
    }
    result
}

fn tile_intersects(key: TileKey, origin: [i32; 2], size: [u32; 2]) -> bool {
    let (left, top) = key.pixel_origin();
    [left, top].into_iter().enumerate().all(|(axis, start)| {
        start < i64::from(origin[axis]) + i64::from(size[axis])
            && start + i64::from(TILE_EDGE) > i64::from(origin[axis])
    })
}

fn adjust_pixel(pixel: [u8; 4], adjustment: ColorAdjustment) -> [u8; 4] {
    if pixel[3] == 0 || adjustment == ColorAdjustment::default() {
        return pixel;
    }
    let alpha = f64::from(pixel[3]);
    let mut rgb = [pixel[0], pixel[1], pixel[2]]
        .map(|value| color::linear_to_srgb(f64::from(value.min(pixel[3])) / alpha));
    if adjustment.hue_degrees != 0
        || adjustment.saturation_percent != 0
        || adjustment.value_percent != 0
    {
        let [h, s, v] = rgb_to_hsv(rgb);
        rgb = hsv_to_rgb([
            (h + f64::from(adjustment.hue_degrees) / 60.0).rem_euclid(6.0),
            (s * (1.0 + f64::from(adjustment.saturation_percent) / 100.0)).clamp(0.0, 1.0),
            (v * (1.0 + f64::from(adjustment.value_percent) / 100.0)).clamp(0.0, 1.0),
        ]);
    }
    let brightness = f64::from(adjustment.brightness_percent) / 100.0;
    let contrast = 1.0 + f64::from(adjustment.contrast_percent) / 100.0;
    let rgb = rgb.map(|channel| {
        quantize(
            color::srgb_to_linear(((channel - 0.5) * contrast + 0.5 + brightness).clamp(0.0, 1.0))
                * alpha,
        )
        .min(pixel[3])
    });
    [rgb[0], rgb[1], rgb[2], pixel[3]]
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn rgb_to_hsv([red, green, blue]: [f64; 3]) -> [f64; 3] {
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    if delta <= f64::EPSILON {
        return [0.0, 0.0, max];
    }
    let hue = if red >= green && red >= blue {
        (green - blue) / delta
    } else if green >= blue {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    [hue.rem_euclid(6.0), delta / max, max]
}

fn hsv_to_rgb([hue, saturation, value]: [f64; 3]) -> [f64; 3] {
    let chroma = value * saturation;
    let cross = chroma * (1.0 - (hue.rem_euclid(2.0) - 1.0).abs());
    let rgb = if hue < 1.0 {
        [chroma, cross, 0.0]
    } else if hue < 2.0 {
        [cross, chroma, 0.0]
    } else if hue < 3.0 {
        [0.0, chroma, cross]
    } else if hue < 4.0 {
        [0.0, cross, chroma]
    } else if hue < 5.0 {
        [cross, 0.0, chroma]
    } else {
        [chroma, 0.0, cross]
    };
    rgb.map(|channel| channel + value - chroma)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{ContentRootId, GroupId, LayerBlendMode};
    use nyatidraw_document::{GroupNode, LayerNode, LayerTreeNode};

    fn tree(locked: bool) -> LayerTree {
        LayerTree::new(GroupNode {
            id: GroupId(10),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: vec![LayerTreeNode::Raster(LayerNode {
                mask: None,
                id: LayerId(1),
                name: "Paint".into(),
                visible: true,
                locked,
                reference: false,
                opacity_u16: u16::MAX,
                content_root: ContentRootId(0),
                color_effect: None,
                alpha_locked: true,
                clip_to_below: false,
                blend_mode: LayerBlendMode::Normal,
            })],
        })
        .unwrap()
    }

    #[test]
    fn corrections_preserve_neutral_bytes_alpha_and_premultiplied_edges() {
        // Risk: neutral round trips or low-alpha corrections corrupt edge colors.
        assert_eq!(
            adjust_pixel(
                [10, 20, 30, 0],
                ColorAdjustment {
                    brightness_percent: 100,
                    ..Default::default()
                }
            ),
            [10, 20, 30, 0]
        );
        for alpha in 0..=255 {
            for rgb in [[0, 0, 0], [alpha / 3, alpha / 2, alpha], [alpha; 3]] {
                let pixel = [rgb[0], rgb[1], rgb[2], alpha];
                assert_eq!(adjust_pixel(pixel, ColorAdjustment::default()), pixel);
                for adjustment in [
                    ColorAdjustment {
                        hue_degrees: 180,
                        saturation_percent: 100,
                        ..Default::default()
                    },
                    ColorAdjustment {
                        brightness_percent: 100,
                        contrast_percent: 100,
                        ..Default::default()
                    },
                    ColorAdjustment {
                        saturation_percent: -100,
                        value_percent: -100,
                        ..Default::default()
                    },
                ] {
                    let output = adjust_pixel(pixel, adjustment);
                    assert_eq!(output[3], alpha);
                    assert!(output[..3].iter().all(|channel| *channel <= alpha));
                    if alpha == 0 {
                        assert_eq!(output, pixel);
                    }
                }
            }
        }
        for (input, adjustment, expected) in [
            (
                [255, 0, 0, 255],
                ColorAdjustment {
                    hue_degrees: 120,
                    ..Default::default()
                },
                [0, 255, 0, 255],
            ),
            (
                [255, 0, 0, 255],
                ColorAdjustment {
                    hue_degrees: -120,
                    ..Default::default()
                },
                [0, 0, 255, 255],
            ),
            (
                [32, 16, 8, 128],
                ColorAdjustment {
                    brightness_percent: 100,
                    ..Default::default()
                },
                [128; 4],
            ),
            (
                [32, 16, 8, 128],
                ColorAdjustment {
                    brightness_percent: -100,
                    ..Default::default()
                },
                [0, 0, 0, 128],
            ),
        ] {
            assert_eq!(adjust_pixel(input, adjustment), expected);
        }
    }

    #[test]
    fn selected_signed_corrections_preserve_other_artwork_and_reject_limits_or_locks() {
        // Risk: correcting a selection changes off-mask pixels, other layers or locked artwork.
        let key = TileKey {
            layer: LayerId(1),
            mip: 0,
            x: -1,
            y: -1,
        };
        let other = TileKey {
            layer: LayerId(2),
            ..key
        };
        let mut bytes = vec![0; TILE_BYTE_LEN];
        bytes[..4].copy_from_slice(&[255, 0, 0, 255]);
        bytes[4..8].copy_from_slice(&[32, 16, 8, 128]);
        let source = TileSnapshot::from_tiles([(key, bytes.clone()), (other, bytes)]).unwrap();
        let mask = crate::selection_all([-128, -128], [1, 1], EditLimits::default()).unwrap();
        let adjustment = ColorAdjustment {
            hue_degrees: 120,
            ..Default::default()
        };
        let output = adjust_raster_colors(
            &source,
            &tree(false),
            LayerId(1),
            Some(&mask),
            adjustment,
            EditLimits::default(),
        )
        .unwrap();
        assert_eq!(
            &output.after.get(key).unwrap().pixels()[..4],
            &[0, 255, 0, 255]
        );
        assert_eq!(
            &output.after.get(key).unwrap().pixels()[4..],
            &source.get(key).unwrap().pixels()[4..]
        );
        assert_eq!(output.after.get(other), source.get(other));
        assert_eq!(output.changed_tiles, vec![key]);
        assert!(matches!(
            adjust_raster_colors(
                &source,
                &tree(true),
                LayerId(1),
                Some(&mask),
                adjustment,
                EditLimits::default()
            ),
            Err(EditError::LockedLayer)
        ));
        assert!(matches!(
            adjust_raster_colors(
                &source,
                &tree(false),
                LayerId(1),
                None,
                adjustment,
                EditLimits {
                    max_pixels: 1,
                    ..EditLimits::default()
                }
            ),
            Err(EditError::LimitExceeded)
        ));
        assert!(matches!(
            adjust_raster_colors(
                &source,
                &tree(false),
                LayerId(1),
                None,
                ColorAdjustment {
                    hue_degrees: 181,
                    ..Default::default()
                },
                EditLimits::default()
            ),
            Err(EditError::InvalidPaint)
        ));
    }
    #[test]
    fn fractional_correction_retains_alpha_and_unselected_pixels() {
        let key = TileKey {
            layer: LayerId(1),
            mip: 0,
            x: -1,
            y: -1,
        };
        let mut pixels = vec![0; TILE_BYTE_LEN];
        for pixel in pixels[..24].chunks_exact_mut(4) {
            pixel.copy_from_slice(&[32, 16, 8, 128]);
        }
        let before = TileSnapshot::from_tiles([(key, pixels)]).unwrap();
        let coverage = [0, 1, 64, 128, 254, 255];
        let mask = SelectionMask::from_coverage_at([-128, -128], [6, 1], &coverage).unwrap();
        let result = adjust_raster_colors(
            &before,
            &tree(false),
            LayerId(1),
            Some(&mask),
            ColorAdjustment {
                brightness_percent: 100,
                ..Default::default()
            },
            EditLimits::default(),
        )
        .unwrap();
        let pixels = result.after.get(key).unwrap().pixels();
        assert_eq!(&pixels[..4], &[32, 16, 8, 128]);
        assert_eq!(&pixels[12..16], &[80, 72, 68, 128]);
        assert_eq!(&pixels[20..24], &[128; 4]);
        for pixel in pixels[..24].chunks_exact(4) {
            assert_eq!(pixel[3], 128);
        }
        assert_eq!(&pixels[24..], &before.get(key).unwrap().pixels()[24..]);
    }
}

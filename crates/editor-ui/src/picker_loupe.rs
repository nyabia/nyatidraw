use dioxus::prelude::*;
use std::sync::Arc;
const PATCH_SIZE: u16 = 13;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickerFrame {
    pub size: u16,
    pub data_uri: Arc<str>,
    /// Opaque sRGB, exactly as a successful release would set foreground RGB.
    /// Transparent artwork has no candidate; the PNG still shows its neighbors.
    pub candidate: Option<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PickerSnapshot {
    /// `WebView` client CSS pixels, including the native canvas layout origin.
    pub cursor_css: [f64; 2],
    pub frame: Option<Arc<PickerFrame>>,
    pub pending: bool,
    pub error: Option<String>,
}

/// # Errors
/// Propagates sampling and PNG encoding failures without accepting a color.
pub fn sample_frame(
    point: [i32; 2],
    mut sample: impl FnMut([i32; 2]) -> Result<[u8; 4], String>,
) -> Result<PickerFrame, String> {
    let half = i32::from(PATCH_SIZE / 2);
    let mut pixels = Vec::with_capacity(usize::from(PATCH_SIZE).pow(2) * 4);
    let mut candidate = None;
    for y in -half..=half {
        for x in -half..=half {
            let point = [point[0].checked_add(x), point[1].checked_add(y)];
            let pixel = if let [Some(x), Some(y)] = point {
                sample([x, y])?
            } else {
                [0; 4]
            };
            if x == 0 && y == 0 {
                candidate = nyatidraw_tiles::color::linear_premultiplied_to_srgb8(pixel).map(
                    |mut color| {
                        color[3] = 255;
                        color
                    },
                );
            }
            pixels.extend_from_slice(&pixel);
        }
    }
    let surface = nyatidraw_tiles::FlattenedRgba8 {
        origin_x: i64::from(point[0]) - i64::from(half),
        origin_y: i64::from(point[1]) - i64::from(half),
        width: u32::from(PATCH_SIZE),
        height: u32::from(PATCH_SIZE),
        pixels,
    };
    let png = nyatidraw_png_io::encode_png_bytes(&surface).map_err(|error| error.to_string())?;
    Ok(PickerFrame {
        size: PATCH_SIZE,
        candidate,
        data_uri: crate::preview::png_data_uri(&png, 4096, "picker")?,
    })
}

/// Read-only feedback; gesture ownership stays with the host.
/// # Errors
/// Propagates Dioxus rendering errors.
pub fn picker_loupe(snapshot: PickerSnapshot) -> Element {
    let [x, y] = snapshot.cursor_css;
    let top = if y >= 190.0 { y - 180.0 } else { y + 26.0 };
    let position = format!(
        "left:clamp(6px,{}px,calc(100vw - 138px));top:clamp(6px,{top}px,calc(100vh - 160px))",
        x - 66.0
    );
    let candidate = snapshot.frame.as_ref().and_then(|frame| frame.candidate);
    let swatch = candidate.map_or_else(String::new, |[r, g, b, _]| {
        format!("background:rgb({r} {g} {b})")
    });
    let status = if snapshot.pending {
        "…"
    } else if snapshot.error.is_some() && snapshot.frame.is_none() {
        "채취 실패"
    } else if candidate.is_none() {
        "투명"
    } else {
        ""
    };
    rsx! {
        aside { class: "picker-loupe", style: position, aria_label: "스포이트 확대 미리보기",
            div { class: "picker-pixels",
                if let Some(frame) = snapshot.frame {
                    img { src: frame.data_uri.to_string(), alt: "커서 주변 픽셀", draggable: "false", "data-pixel-size": "{frame.size}" }
                }
                span { class: "picker-center" }
            }
            div { class: "picker-candidate",
                span { class: "picker-swatch", style: swatch }
                span { "{status}" }
            }
        }
    }
}

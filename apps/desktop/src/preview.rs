use std::sync::Arc;

use nyatidraw_api::{ContentRootId, LayerId};
use nyatidraw_tiles::FlattenedRgba8;

/// The navigator is a small, disposable UI artifact, never a second document
/// representation.
pub(crate) const NAVIGATOR_MAX_WIDTH: u32 = 160;
pub(crate) const NAVIGATOR_MAX_HEIGHT: u32 = 160;
const MAX_NAVIGATOR_DATA_URI_BYTES: usize = 160 * 1024;
const MAX_LAYER_THUMBNAIL_DATA_URI_BYTES: usize = 32 * 1024;
/// This is a chrome-memory bound, not a document limit. Layers beyond this
/// retain their durable pixels and display the transparent fallback.
pub(crate) const MAX_LAYER_THUMBNAILS: usize = 128;
pub(crate) const LAYER_THUMBNAIL_MAX_WIDTH: u32 = 50;
pub(crate) const LAYER_THUMBNAIL_MAX_HEIGHT: u32 = 56;
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NavigatorFrame {
    pub(crate) root: ContentRootId,
    pub(crate) width: u16,
    pub(crate) height: u16,
    /// Browser-safe encoded pixels. The source CPU surface has already been dropped.
    pub(crate) data_uri: Arc<str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NavigatorViewport {
    /// Page-relative ten-thousandths. Values outside the page are clipped by UI.
    pub(crate) quad_page_10k: [[i32; 2]; 4],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct NavigatorSnapshot {
    pub(crate) frame: Option<Arc<NavigatorFrame>>,
    pub(crate) viewport: Option<NavigatorViewport>,
}

/// One disposable, browser-safe raster-layer thumbnail. It is desktop-private
/// and deliberately absent from `UiProjection`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LayerThumbnailFrame {
    pub(crate) layer: LayerId,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) data_uri: Arc<str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LayerThumbnailSnapshot {
    pub(crate) generation: u64,
    pub(crate) frames: std::collections::BTreeMap<LayerId, Arc<LayerThumbnailFrame>>,
}

impl LayerThumbnailSnapshot {
    pub(crate) fn frame(&self, layer: LayerId) -> Option<Arc<LayerThumbnailFrame>> {
        self.frames.get(&layer).cloned()
    }
}

pub(crate) fn navigator_frame(
    root: ContentRootId,
    surface: &FlattenedRgba8,
) -> Result<NavigatorFrame, String> {
    let width = u16::try_from(surface.width)
        .map_err(|_| "navigator preview width does not fit u16".to_owned())?;
    let height = u16::try_from(surface.height)
        .map_err(|_| "navigator preview height does not fit u16".to_owned())?;
    let png = nyatidraw_png_io::encode_png_bytes(surface)
        .map_err(|error| format!("navigator-preview-png:{error}"))?;
    Ok(NavigatorFrame {
        root,
        width,
        height,
        data_uri: png_data_uri(&png, MAX_NAVIGATOR_DATA_URI_BYTES, "navigator")?,
    })
}

pub(crate) fn layer_thumbnail_frame(
    layer: LayerId,
    surface: &FlattenedRgba8,
) -> Result<LayerThumbnailFrame, String> {
    let width = u16::try_from(surface.width)
        .map_err(|_| "layer thumbnail width does not fit u16".to_owned())?;
    let height = u16::try_from(surface.height)
        .map_err(|_| "layer thumbnail height does not fit u16".to_owned())?;
    let png = nyatidraw_png_io::encode_png_bytes(surface)
        .map_err(|error| format!("layer-thumbnail-png:{error}"))?;
    Ok(LayerThumbnailFrame {
        layer,
        width,
        height,
        data_uri: png_data_uri(&png, MAX_LAYER_THUMBNAIL_DATA_URI_BYTES, "layer-thumbnail")?,
    })
}

fn png_data_uri(png: &[u8], maximum: usize, label: &str) -> Result<Arc<str>, String> {
    const PREFIX: &str = "data:image/png;base64,";
    let encoded_len = png.len().div_ceil(3).saturating_mul(4);
    let total_len = PREFIX.len().saturating_add(encoded_len);
    if total_len > maximum {
        return Err(format!(
            "{label}-too-large encoded_bytes={total_len} cap={maximum}"
        ));
    }
    let mut output = String::with_capacity(total_len);
    output.push_str(PREFIX);
    for chunk in png.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(BASE64_ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            BASE64_ALPHABET[usize::from(((first & 3) << 4) | (second >> 4))],
        ));
        output.push(if chunk.len() > 1 {
            char::from(BASE64_ALPHABET[usize::from(((second & 15) << 2) | (third >> 6))])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(BASE64_ALPHABET[usize::from(third & 63)])
        } else {
            '='
        });
    }
    Ok(Arc::from(output))
}

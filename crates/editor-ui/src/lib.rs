pub mod brush_preview;
pub mod canvas_size;
mod host;
pub mod layout_codec;
pub mod layout_store;
mod mask_preview;
pub mod palette;
pub mod picker_loupe;
pub mod preview;
pub mod workspace_appearance;
pub use host::{ExportStatus, PalettePins, UiBackend};

#[cfg(feature = "chrome")]
include!("chrome.rs");

#[must_use]
pub fn can_begin_transform(projection: &nyatidraw_api::UiProjection) -> bool {
    !projection.edit.busy
        && projection.edit.transform.is_none()
        && projection.edit.color_adjustment.is_none()
        && !projection.animation.playing
        && !matches!(
            projection.raster_target,
            Some(nyatidraw_api::RasterEditTarget::Mask { .. })
        )
        && projection.active_layer.is_some_and(|id| {
            projection.layers.iter().any(|layer| {
                layer.id == nyatidraw_api::LayerTreeNodeId::Raster(id)
                    && !layer.locked
                    && !layer.alpha_locked
            })
        })
}

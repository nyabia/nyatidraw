use nyatidraw_api::ContentRootId;
use nyatidraw_editor_ui::preview::{
    LayerThumbnailFrame, NavigatorFrame, layer_thumbnail_frame, mask_thumbnail_frame,
    navigator_frame,
};
use nyatidraw_paint_cpu::{render_page_preview_rgba8, render_raster_page_preview_rgba8};
use nyatidraw_web_core::{LayerId, WebDocument};

pub fn layer_thumbnail(document: &WebDocument, layer: LayerId) -> Option<LayerThumbnailFrame> {
    let image =
        render_raster_page_preview_rgba8(document.snapshot(), layer, document.canvas(), 50, 56)
            .ok()?;
    let mut frame = layer_thumbnail_frame(layer, &image).ok()?;
    if let Some(mask) = document
        .render_layers()
        .raster(layer)
        .and_then(|node| node.mask)
    {
        frame.mask = mask_thumbnail_frame(
            mask.id,
            mask.default_coverage,
            document.snapshot(),
            document.canvas(),
            [frame.width, frame.height],
        )
        .ok()
        .map(std::sync::Arc::new);
    }
    Some(frame)
}

pub fn navigator(document: &WebDocument, revision: u64) -> Option<NavigatorFrame> {
    let image = render_page_preview_rgba8(
        document.snapshot(),
        document.render_layers(),
        document.canvas(),
        160,
        160,
    )
    .ok()?;
    navigator_frame(ContentRootId(u128::from(revision)), &image).ok()
}

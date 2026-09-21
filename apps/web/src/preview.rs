use base64::{Engine as _, engine::general_purpose::STANDARD};
use nyatidraw_paint_cpu::render_raster_page_preview_rgba8;
use nyatidraw_tiles::color::linear_premultiplied_to_srgb8;
use nyatidraw_web_core::{LayerId, WebDocument};

pub fn layer_thumbnail(document: &WebDocument, layer: LayerId) -> Option<String> {
    document.layers().raster(layer)?;
    let mut preview =
        render_raster_page_preview_rgba8(document.snapshot(), layer, document.canvas(), 64, 48)
            .ok()?;
    for pixel in preview.pixels.chunks_exact_mut(4) {
        let color = [pixel[0], pixel[1], pixel[2], pixel[3]];
        pixel.copy_from_slice(&linear_premultiplied_to_srgb8(color).unwrap_or([0; 4]));
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, preview.width, preview.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&preview.pixels).ok()?;
        writer.finish().ok()?;
    }
    Some(format!("data:image/png;base64,{}", STANDARD.encode(bytes)))
}

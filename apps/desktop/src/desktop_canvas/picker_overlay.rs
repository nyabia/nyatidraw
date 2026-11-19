use super::TexturePresenter;
use nyatidraw_editor_ui::picker_loupe::{PickerFrame, PickerSnapshot};
use std::sync::Arc;

const WIDTH: u32 = 41;
const HEIGHT: u32 = 48;
const EXTENT: wgpu::Extent3d = wgpu::Extent3d {
    width: WIDTH,
    height: HEIGHT,
    depth_or_array_layers: 1,
};

pub(super) struct PickerOverlay {
    texture: wgpu::Texture,
    binding: wgpu::BindGroup,
    last: Option<Arc<PickerFrame>>,
    pending: Option<bool>,
    viewport: [f32; 4],
}

impl PickerOverlay {
    pub(super) fn new(device: &wgpu::Device, presenter: &TexturePresenter) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("native picker loupe"),
            size: EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let binding = presenter.bind(
            device,
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            &device.create_sampler(&wgpu::SamplerDescriptor::default()),
        );
        Self {
            texture,
            binding,
            last: None,
            pending: None,
            viewport: [0.; 4],
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "The clamped physical-pixel viewport is passed to the f32 WGPU viewport API."
    )]
    pub(super) fn update(
        &mut self,
        queue: &wgpu::Queue,
        snapshot: PickerSnapshot,
        origin: [i32; 2],
        scale: f64,
        surface: [u32; 2],
    ) {
        let px = snapshot.cursor_css[0] * scale - f64::from(origin[0]);
        let py = snapshot.cursor_css[1] * scale - f64::from(origin[1]);
        let width = (123. * scale).min(f64::from(surface[0]));
        let height = (144. * scale).min(f64::from(surface[1]));
        let x = (px - width * 0.5).clamp(0., f64::from(surface[0]) - width);
        let margin = 18. * scale;
        let y = if py >= height + margin {
            py - height - margin
        } else {
            py + margin
        };
        self.viewport = [
            x,
            y.clamp(0., f64::from(surface[1]) - height),
            width,
            height,
        ]
        .map(|v| v as f32);
        let unchanged = match (&self.last, &snapshot.frame) {
            (Some(previous), Some(current)) => Arc::ptr_eq(previous, current),
            (None, None) => true,
            _ => false,
        };
        if unchanged && self.pending == Some(snapshot.pending) {
            return;
        }
        let pixels = loupe_pixels(snapshot.frame.as_deref(), snapshot.pending);
        queue.write_texture(
            self.texture.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH * 4),
                rows_per_image: None,
            },
            EXTENT,
        );
        self.last = snapshot.frame;
        self.pending = Some(snapshot.pending);
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let [x, y, w, h] = self.viewport;
        pass.set_viewport(x, y, w, h, 0., 1.);
        pass.set_bind_group(0, &self.binding, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn loupe_pixels(frame: Option<&PickerFrame>, pending: bool) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let rgb = if x == 0 || x == WIDTH - 1 || y == 0 || y == HEIGHT - 1 || y == WIDTH {
                [7; 3]
            } else if y < WIDTH {
                let sx = (x - 1) / 3;
                let sy = (y - 1) / 3;
                let checker: u8 = if (sx + sy) % 2 == 0 { 134 } else { 86 };
                let center_outline = sx == 6 && sy == 6 && ((x - 1) % 3 != 1 || (y - 1) % 3 != 1);
                if center_outline {
                    [255; 3]
                } else {
                    frame
                        .and_then(|frame| {
                            let offset =
                                usize::try_from((sy * u32::from(frame.size) + sx) * 4).ok()?;
                            let color = frame.linear_pixels.get(offset..offset + 4)?;
                            Some(std::array::from_fn(|i| {
                                color[i].saturating_add(
                                    u8::try_from(
                                        (255 - u32::from(color[3])) * u32::from(checker) / 255,
                                    )
                                    .expect("checker contribution fits one channel"),
                                )
                            }))
                        })
                        .unwrap_or([checker; 3])
                }
            } else if let Some(frame) = frame.filter(|frame| frame.candidate.is_some()) {
                let center = (usize::from(frame.size).pow(2) / 2) * 4;
                let color = &frame.linear_pixels[center..center + 4];
                std::array::from_fn(|i| {
                    (u32::from(color[i]) * 255 / u32::from(color[3]).max(1)).min(255) as u8
                })
            } else if pending {
                [136, 95, 22]
            } else {
                [60; 3]
            };
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    pixels
}

use nyatidraw_input::{Point, ViewportTransform};
use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

mod barcode;

#[derive(Clone, Copy)]
pub(super) struct FrameContext {
    pub viewport: Option<ViewportTransform>,
    pub page_size: Option<[u32; 2]>,
    pub geometry_epoch: u64,
}

pub(super) struct MaskPresentTrace {
    remaining: u8,
    wake_budget: u8,
    pending: VecDeque<PendingPixel>,
    barcode: Option<barcode::Barcode>,
    serial: u32,
    started: Instant,
    last_context: Option<FrameContext>,
}

impl MaskPresentTrace {
    pub(super) fn new(
        device: &wgpu::Device,
        scratch_authorized: bool,
        capabilities: &wgpu::SurfaceCapabilities,
        config: &mut wgpu::SurfaceConfiguration,
    ) -> Self {
        let supported = capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC)
            && matches!(
                config.format,
                wgpu::TextureFormat::Rgba8Unorm
                    | wgpu::TextureFormat::Rgba8UnormSrgb
                    | wgpu::TextureFormat::Bgra8Unorm
                    | wgpu::TextureFormat::Bgra8UnormSrgb
            );
        let requested =
            std::env::var_os("NAYATI_MASK_PRESENT_TRACE").is_some_and(|value| value == "1");
        let remaining = if scratch_authorized && requested && supported {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
            32
        } else {
            0
        };
        if scratch_authorized {
            println!(
                "native-mask-present-trace enabled={} surface_copy_supported={supported}",
                remaining > 0
            );
        }
        Self {
            remaining,
            wake_budget: 128,
            pending: VecDeque::new(),
            barcode: (remaining > 0).then(|| barcode::Barcode::new(device, config.format)),
            serial: 0,
            started: Instant::now(),
            last_context: None,
        }
    }

    pub(super) fn stop(&mut self) {
        self.remaining = 0;
        self.barcode = None;
        for pending in self.pending.drain(..) {
            pending.buffer.unmap();
        }
    }

    pub(super) fn sample(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Texture,
        config: &wgpu::SurfaceConfiguration,
        context: FrameContext,
    ) {
        self.serial = self.serial.saturating_add(1);
        self.last_context = Some(context);
        if self.barcode.is_none()
            || self.serial > 255
            || (self.remaining == 0 && self.pending.is_empty())
        {
            return;
        }
        let point = context
            .viewport
            .and_then(|view| view.document_to_window(Point { x: 8.5, y: 8.5 }));
        let marker = context
            .viewport
            .zip(context.page_size)
            .is_some_and(|(view, page)| barcode::outside_page(view, page));
        let pixel = point.and_then(|point| window_pixel(point, config.width, config.height));
        let sample = pixel.filter(|_| self.remaining > 0 && self.pending.len() < 4);
        let context = format!(
            "serial={} unix_ms={} elapsed_us={} geometry_epoch={} point={point:?} marker={marker} marker_rect=[4,4,60,12] sampled={} format={:?} present={:?} size={}x{}",
            self.serial,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_millis()),
            self.started.elapsed().as_micros(),
            context.geometry_epoch,
            sample.is_some(),
            config.format,
            config.present_mode,
            config.width,
            config.height,
        );
        println!("native-mask-present-trace stage=before-present {context}");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if marker && let Some(barcode) = &self.barcode {
            let view = surface.create_view(&wgpu::TextureViewDescriptor::default());
            barcode.draw(&mut encoder, &view, self.serial);
        }
        let staging = sample.map(|point| enqueue_pixel(device, &mut encoder, surface, point));
        queue.submit([encoder.finish()]);
        let Some(buffer) = staging else {
            return;
        };
        self.remaining -= 1;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.pending.push_back(PendingPixel {
            buffer,
            receiver,
            format: config.format,
            context,
        });
    }

    pub(super) fn sample_retained(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Texture,
        config: &wgpu::SurfaceConfiguration,
    ) {
        if let Some(context) = self.last_context {
            self.sample(device, queue, surface, config, context);
        }
    }

    pub(super) fn after_present(
        &mut self,
        device: &wgpu::Device,
        live_ink: &crate::live_ink::LiveInkBridge,
    ) {
        if self.pending.is_empty() && self.remaining == 0 {
            return;
        }
        if let Err(error) = device.poll(wgpu::PollType::Poll) {
            eprintln!("native-mask-present-trace async-poll-error={error}");
            self.stop();
            return;
        }
        for _ in 0..self.pending.len() {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            match pending.receiver.try_recv() {
                Ok(Ok(())) => {
                    let bytes = pending.buffer.slice(..).get_mapped_range();
                    let mut pixel = [bytes[0], bytes[1], bytes[2], bytes[3]];
                    drop(bytes);
                    pending.buffer.unmap();
                    if matches!(
                        pending.format,
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                    ) {
                        pixel.swap(0, 2);
                    }
                    println!(
                        "native-mask-present-trace stage=readback {} surface_rgba={pixel:?} asynchronous=true",
                        pending.context
                    );
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => self.pending.push_back(pending),
                result => eprintln!(
                    "native-mask-present-trace {} map-error={result:?}",
                    pending.context
                ),
            }
        }
        if self.wake_budget > 0 && !self.pending.is_empty() {
            self.wake_budget -= 1;
            let _ = live_ink.request_redraw();
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite coordinates are checked inside the u32 surface bounds before conversion"
)]
fn window_pixel(point: Point, width: u32, height: u32) -> Option<[u32; 2]> {
    (point.x.is_finite()
        && point.y.is_finite()
        && point.x >= 0.0
        && point.y >= 0.0
        && point.x < f64::from(width)
        && point.y < f64::from(height))
    .then(|| [point.x.floor() as u32, point.y.floor() as u32])
}

struct PendingPixel {
    buffer: wgpu::Buffer,
    receiver: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    format: wgpu::TextureFormat,
    context: String,
}

fn enqueue_pixel(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    point: [u32; 2],
) -> wgpu::Buffer {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("scratch-mask-present-trace"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: point[0],
                y: point[1],
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    staging
}

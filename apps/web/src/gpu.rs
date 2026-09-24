use std::sync::{Arc, Mutex};

use nyatidraw_input::ViewportTransform;
use nyatidraw_paint_gpu::GpuCompositeScene;
use nyatidraw_web_core::{TILE_BYTE_LEN, TileKey, WebDocument};
use web_sys::HtmlCanvasElement;

pub struct WebRenderer {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    view_format: wgpu::TextureFormat,
    scene: GpuCompositeScene,
    presenter: TexturePresenter,
    failure: Arc<Mutex<Option<String>>>,
    zero_tile: Box<[u8]>,
    selection: Option<Arc<nyatidraw_paint_cpu::SelectionMask>>,
    transform_guide_active: bool,
}

impl WebRenderer {
    pub fn set_background(&mut self, colors: [[u8; 3]; 2]) {
        self.scene.set_workspace_background(colors);
    }
    #[allow(clippy::too_many_lines)]
    pub async fn new(canvas: HtmlCanvasElement, doc: &WebDocument) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| format!("WebGPU canvas could not be created: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| {
                format!(
                    "WebGPU is unavailable. Use a WebGPU-enabled browser over HTTPS or localhost: {error}"
                )
            })?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("nyatidraw-web-device"),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("WebGPU device could not be created: {error}"))?;
        let failure = Arc::new(Mutex::new(None));
        let lost_failure = Arc::clone(&failure);
        device.set_device_lost_callback(move |reason, message| {
            record_failure(
                &lost_failure,
                format!(
                    "WebGPU device lost ({reason:?}): {message}. Save the project before reloading."
                ),
            );
        });
        let uncaptured_failure = Arc::clone(&failure);
        device.on_uncaptured_error(Box::new(move |error| {
            record_failure(
                &uncaptured_failure,
                format!("WebGPU rendering failed: {error}"),
            );
        }));

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| {
                matches!(
                    format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .ok_or("WebGPU exposes no compatible 8-bit canvas format")?;
        // Working tiles are linear. The sRGB view performs the display transfer once.
        let view_format = format.add_srgb_suffix();
        let size = [canvas.width().max(1), canvas.height().max(1)];
        validate_texture_size(&device, size)?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size[0],
            height: size[1],
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![view_format],
            desired_maximum_frame_latency: 2,
        };
        let document_size = document_size(doc);
        validate_texture_size(&device, document_size)?;
        device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(&device, &config);
        let presenter = TexturePresenter::new(&device, view_format);
        let scene_result =
            GpuCompositeScene::new(&device, &queue, document_size, doc.layers().clone());
        let validation_error = device.pop_error_scope().await;
        let memory_error = device.pop_error_scope().await;
        if let Some(error) = validation_error.or(memory_error) {
            return Err(format!("WebGPU renderer initialization failed: {error}"));
        }
        let mut scene = scene_result.map_err(|error| format!("Create canvas scene: {error:?}"))?;
        scene.set_workspace_background([[40, 40, 40]; 2]);
        upload_snapshot(&mut scene, doc)?;
        let renderer = Self {
            canvas,
            surface,
            device,
            queue,
            config,
            view_format,
            scene,
            presenter,
            failure,
            zero_tile: vec![0; TILE_BYTE_LEN].into_boxed_slice(),
            selection: None,
            transform_guide_active: false,
        };
        renderer.check_device()?;
        Ok(renderer)
    }

    pub fn sync_document(&mut self, doc: &WebDocument) -> Result<(), String> {
        self.check_device()?;
        let size = document_size(doc);
        validate_texture_size(&self.device, size)?;
        self.scene
            .replace_document(size, doc.display_layers().clone())
            .map_err(|error| format!("Replace canvas scene: {error:?}"))?;
        upload_snapshot(&mut self.scene, doc)?;
        self.sync_chrome(doc)
    }

    pub fn sync_chrome(&mut self, doc: &WebDocument) -> Result<(), String> {
        self.scene
            .set_solo(doc.solo_node())
            .map_err(|e| format!("Solo: {e:?}"))?;
        let unchanged = match (&self.selection, doc.selection()) {
            (Some(before), Some(after)) => Arc::ptr_eq(before, after),
            (None, None) => true,
            _ => false,
        };
        if !unchanged {
            let mask = doc
                .selection()
                .map(|mask| {
                    nyatidraw_paint_gpu::GpuSelectionMask::new(
                        &self.device,
                        mask.origin(),
                        mask.dimensions(),
                        &mask.packed_bits(),
                    )
                })
                .transpose()
                .map_err(|e| format!("Selection: {e:?}"))?;
            self.scene.set_selection_overlay(mask.as_ref());
            self.selection = doc.selection().cloned();
        }
        Ok(())
    }

    pub fn set_gesture_preview(&mut self, points: &[[i32; 2]], closed: bool) {
        self.scene.set_gesture_preview(points, closed);
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn transform_guide(
        &mut self,
        transform: Option<&nyatidraw_api::TransformProjection>,
        zoom: f64,
    ) {
        if let Some(transform) = transform {
            let points: Vec<_> = nyatidraw_editor::transform_gesture::guide(transform, 7.0 / zoom)
                .into_iter()
                .map(|point| point.map(|v| v.round() as i32))
                .collect();
            self.scene.set_gesture_preview(&points, false);
            self.transform_guide_active = true;
        } else if self.transform_guide_active {
            self.scene.set_gesture_preview(&[], false);
            self.transform_guide_active = false;
        }
    }

    pub fn upload_dirty(&mut self, doc: &WebDocument, keys: &[TileKey]) -> Result<(), String> {
        self.check_device()?;
        for key in keys {
            let pixels = doc
                .snapshot()
                .get(*key)
                .map_or(self.zero_tile.as_ref(), |tile| tile.pixels());
            self.scene
                .upload_closed_tile(*key, pixels)
                .map_err(|error| format!("Upload changed canvas tile: {error:?}"))?;
        }
        Ok(())
    }

    pub fn render(&mut self, viewport: ViewportTransform) -> Result<(), String> {
        self.check_device()?;
        if viewport.physical_size.contains(&0) {
            return Ok(());
        }
        viewport
            .validate()
            .map_err(|error| format!("Invalid canvas viewport: {error:?}"))?;
        validate_texture_size(&self.device, viewport.physical_size)?;
        self.resize(viewport.physical_size);
        self.scene
            .render_viewport(viewport)
            .map_err(|error| format!("Compose canvas viewport: {error:?}"))?;
        let texture = self
            .scene
            .display_texture()
            .ok_or("Canvas compositor produced no display texture")?;
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|error| format!("WebGPU surface recovery failed: {error}"))?
            }
            Err(error) => return Err(format!("Acquire WebGPU canvas frame: {error}")),
        };
        let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view_format),
            ..Default::default()
        });
        self.presenter
            .present(&self.device, &self.queue, &texture, &target);
        frame.present();
        self.check_device()
    }

    fn resize(&mut self, [width, height]: [u32; 2]) {
        if self.config.width == width
            && self.config.height == height
            && self.canvas.width() == width
            && self.canvas.height() == height
        {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn check_device(&self) -> Result<(), String> {
        let failure = self
            .failure
            .lock()
            .map_err(|_| "WebGPU error state could not be read".to_owned())?;
        match failure.as_ref() {
            Some(message) => Err(message.clone()),
            None => Ok(()),
        }
    }
}

fn document_size(doc: &WebDocument) -> [u32; 2] {
    let canvas = doc.canvas();
    [canvas.width_px, canvas.height_px]
}

fn validate_texture_size(device: &wgpu::Device, size: [u32; 2]) -> Result<(), String> {
    let limit = device.limits().max_texture_dimension_2d;
    if size.contains(&0) || size.iter().any(|dimension| *dimension > limit) {
        return Err(format!(
            "Canvas texture {}×{} exceeds this GPU's 1–{limit} pixel dimension range",
            size[0], size[1]
        ));
    }
    Ok(())
}

fn upload_snapshot(scene: &mut GpuCompositeScene, doc: &WebDocument) -> Result<(), String> {
    for (key, tile) in doc.display_snapshot().iter() {
        scene
            .upload_closed_tile(key, tile.pixels())
            .map_err(|error| format!("Upload canvas snapshot: {error:?}"))?;
    }
    Ok(())
}

fn record_failure(slot: &Mutex<Option<String>>, message: String) {
    if let Ok(mut failure) = slot.lock()
        && failure.is_none()
    {
        *failure = Some(message);
    }
}

struct TexturePresenter {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

impl TexturePresenter {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nyatidraw-web-present-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nyatidraw-web-present-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nyatidraw-web-present-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nyatidraw-web-present-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nyatidraw-web-present-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Self {
            layout,
            sampler,
            pipeline,
        }
    }

    fn present(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        target: &wgpu::TextureView,
    ) {
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nyatidraw-web-present-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nyatidraw-web-present-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nyatidraw-web-present-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

//! Disposable, bounded viewport lines. No artwork texture is ever targeted.
use nyatidraw_input::{Point, ViewportTransform};
use wgpu::util::DeviceExt;

const MAX_POINTS: usize = 4096;
const SEGMENT_BYTES: u64 = 16;

pub(crate) struct GesturePreview {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    segments: wgpu::Buffer,
    points: Vec<[i32; 2]>,
    closed: bool,
    staging: Vec<u8>,
}

impl GesturePreview {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nyatidraw-gesture-screen-size"),
            contents: &[0; 16],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nyatidraw-gesture-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nyatidraw-gesture-binding"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let segments = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nyatidraw-gesture-segments"),
            size: MAX_POINTS as u64 * SEGMENT_BYTES,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nyatidraw-gesture-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gesture_preview.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nyatidraw-gesture-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nyatidraw-gesture-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: SEGMENT_BYTES,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::WORKING_TEXTURE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            group,
            uniform,
            segments,
            points: Vec::new(),
            closed: false,
            staging: Vec::new(),
        }
    }

    pub(crate) fn set(&mut self, points: &[[i32; 2]], closed: bool) -> bool {
        if points.len() > MAX_POINTS {
            self.points.clear();
            return false;
        }
        if self.points == points && self.closed == closed {
            return true;
        }
        self.points.clear();
        self.points.extend_from_slice(points);
        self.closed = closed;
        true
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(crate) fn render(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: ViewportTransform,
    ) -> bool {
        if self.points.len() < 2 {
            return false;
        }
        self.staging.clear();
        let segments = self.points.len() - usize::from(!self.closed);
        for index in 0..segments {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];
            let convert = |[x, y]: [i32; 2]| {
                viewport.document_to_window(Point {
                    x: f64::from(x),
                    y: f64::from(y),
                })
            };
            let (Some(a), Some(b)) = (convert(a), convert(b)) else {
                continue;
            };
            if a == b {
                continue;
            }
            let origin = viewport.window_origin_physical;
            for value in [
                a.x - origin.x,
                a.y - origin.y,
                b.x - origin.x,
                b.y - origin.y,
            ] {
                self.staging
                    .extend_from_slice(&(value as f32).to_ne_bytes());
            }
        }
        if self.staging.is_empty() {
            return false;
        }
        let mut dimensions = [0; 16];
        for (slot, value) in dimensions[..8]
            .chunks_exact_mut(4)
            .zip(viewport.physical_size)
        {
            slot.copy_from_slice(&(value as f32).to_ne_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &dimensions);
        queue.write_buffer(&self.segments, 0, &self.staging);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nyatidraw-gesture-preview"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, self.segments.slice(..));
        pass.draw(
            0..6,
            0..u32::try_from(self.staging.len() / 16).expect("bounded segments"),
        );
        true
    }
}

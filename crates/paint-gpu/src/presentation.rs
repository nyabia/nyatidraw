use crate::WORKING_TEXTURE_FORMAT;

pub(super) const SPARSE_MIP_LEVELS: u32 = 8;
pub(super) const ACCUMULATION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(super) fn mip_levels(size: [u32; 2]) -> u32 {
    u32::BITS - size[0].max(size[1]).leading_zeros()
}

pub(super) fn mip_bytes(size: [u32; 2]) -> u64 {
    (1..mip_levels(size))
        .map(|level| {
            u64::from((size[0] >> level).max(1)) * u64::from((size[1] >> level).max(1)) * 4
        })
        .sum()
}

pub(super) fn source_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    layer: u32,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_array_layer: layer,
        array_layer_count: Some(1),
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("nyatidraw-presentation-source"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    })
}

pub(super) struct AreaReducer {
    pipeline: wgpu::RenderPipeline,
}

impl AreaReducer {
    pub(super) fn new(device: &wgpu::Device, source_layout: &wgpu::BindGroupLayout) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nyatidraw-presentation-area-reduce"),
            source: wgpu::ShaderSource::Wgsl(include_str!("area_reduce.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nyatidraw-presentation-reduce-layout"),
            bind_group_layouts: &[source_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nyatidraw-presentation-reduce"),
            layout: Some(&layout),
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
                    format: WORKING_TEXTURE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Self { pipeline }
    }

    pub(super) fn rebuild(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_layout: &wgpu::BindGroupLayout,
        texture: &wgpu::Texture,
        layer: u32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nyatidraw-presentation-pyramid"),
        });
        for level in 1..texture.mip_level_count() {
            let view = |mip| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            };
            let source = view(level - 1);
            let target = view(level);
            let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nyatidraw-presentation-reduce-source"),
                layout: source_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source),
                }],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nyatidraw-presentation-reduce-level"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

use nyatidraw_input::{Point, ViewportTransform};

pub(super) struct Barcode(wgpu::RenderPipeline);

impl Barcode {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scratch-present-barcode"),
            source: wgpu::ShaderSource::Wgsl(include_str!("barcode.wgsl").into()),
        });
        Self(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("scratch-present-barcode"),
                layout: None,
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
            }),
        )
    }

    pub(super) fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        serial: u32,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scratch-present-barcode"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.0);
        pass.set_scissor_rect(4, 4, 60, 12);
        pass.draw(0..3, serial..serial + 1);
    }
}

pub(super) fn outside_page(view: ViewportTransform, page: [u32; 2]) -> bool {
    if view.physical_size[0] < 64 || view.physical_size[1] < 16 {
        return false;
    }
    let mut bounds = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for [x, y] in [[0, 0], [page[0], 0], page, [0, page[1]]] {
        let Some(point) = view.document_to_window(Point {
            x: f64::from(x),
            y: f64::from(y),
        }) else {
            return false;
        };
        bounds[0] = bounds[0].min(point.x);
        bounds[1] = bounds[1].min(point.y);
        bounds[2] = bounds[2].max(point.x);
        bounds[3] = bounds[3].max(point.y);
    }
    bounds[0] >= 65.0 || bounds[1] >= 17.0 || bounds[2] <= 3.0 || bounds[3] <= 3.0
}

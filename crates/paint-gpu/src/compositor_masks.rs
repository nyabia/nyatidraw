use wgpu::util::DeviceExt;

pub(super) const MAX_MASK_BYTES: usize = 128 * 1024 * 1024;

pub(super) struct MaskBuffers {
    pub layout: wgpu::BindGroupLayout,
    pub binding: wgpu::BindGroup,
    transfer: wgpu::Buffer,
    coverage: wgpu::Buffer,
    capacity: usize,
}

impl MaskBuffers {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nyatidraw-composite-color-and-coverage"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let bytes: Vec<_> = nyatidraw_tiles::color::layer_color_transfer_tables()
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        let transfer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nyatidraw-layer-color-transfer"),
            contents: &bytes,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let coverage = create_buffer(device, 4);
        let binding = bind(device, &layout, &transfer, &coverage);
        Self {
            layout,
            binding,
            transfer,
            coverage,
            capacity: 4,
        }
    }

    pub fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
    ) -> Result<(), super::CompositeRenderError> {
        let required = bytes.len().max(4);
        if required > MAX_MASK_BYTES
            || required as u64 > u64::from(device.limits().max_storage_buffer_binding_size)
        {
            return Err(super::CompositeRenderError::TooManyChildren);
        }
        if required > self.capacity {
            let capacity = required.next_power_of_two();
            if capacity as u64 > u64::from(device.limits().max_storage_buffer_binding_size) {
                return Err(super::CompositeRenderError::TooManyChildren);
            }
            self.capacity = capacity;
            self.coverage = create_buffer(device, self.capacity);
            self.binding = bind(device, &self.layout, &self.transfer, &self.coverage);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.coverage, 0, bytes);
        }
        Ok(())
    }
}

fn create_buffer(device: &wgpu::Device, bytes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nyatidraw-composite-mask-coverage"),
        size: bytes as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    transfer: &wgpu::Buffer,
    coverage: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("nyatidraw-composite-mask-binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: transfer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: coverage.as_entire_binding(),
            },
        ],
    })
}

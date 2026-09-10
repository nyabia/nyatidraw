use wgpu::util::DeviceExt;

/// Immutable, bounded coverage uploaded once and shared by brush/eraser bindings.
#[derive(Clone)]
pub struct GpuSelectionMask {
    bits: wgpu::Buffer,
    dimensions: [u32; 2],
    origin: [i32; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuSelectionError {
    InvalidDimensions,
    InvalidCoverage,
}

impl GpuSelectionMask {
    pub(crate) fn new(
        device: &wgpu::Device,
        origin: [i32; 2],
        dimensions: [u32; 2],
        packed: &[u8],
    ) -> Result<Self, GpuSelectionError> {
        let [width, height] = dimensions;
        let pixels = u64::from(width) * u64::from(height);
        if pixels == 0
            || pixels > 16 * 1024 * 1024
            || origin
                .into_iter()
                .zip(dimensions)
                .any(|(start, size)| i64::from(start) + i64::from(size) > i64::from(i32::MAX) + 1)
        {
            return Err(GpuSelectionError::InvalidDimensions);
        }
        if u64::try_from(packed.len()).ok() != Some(pixels.div_ceil(8))
            || (pixels % 8 != 0 && packed.last().is_some_and(|last| last >> (pixels % 8) != 0))
        {
            return Err(GpuSelectionError::InvalidCoverage);
        }
        let mut words = packed.to_vec();
        words.resize(words.len().div_ceil(4) * 4, 0);
        let bits = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nyatidraw-selection-coverage"),
            contents: &words,
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(Self {
            bits,
            dimensions,
            origin,
        })
    }
}

pub(crate) struct SelectionBinding {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    disabled: wgpu::Buffer,
    mask: Option<GpuSelectionMask>,
    origin: [i32; 2],
}

impl SelectionBinding {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nyatidraw-selection-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nyatidraw-selection-coordinates"),
            contents: &[0; 32],
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let disabled = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nyatidraw-selection-disabled"),
            contents: &[0; 4],
            usage: wgpu::BufferUsages::STORAGE,
        });
        let group = Self::group(device, &layout, &uniform, &disabled);
        Self {
            layout,
            group,
            uniform,
            disabled,
            mask: None,
            origin: [0, 0],
        }
    }

    fn group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        bits: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nyatidraw-selection-binding"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bits.as_entire_binding(),
                },
            ],
        })
    }

    pub(crate) fn replace(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: Option<&GpuSelectionMask>,
    ) {
        self.mask = mask.cloned();
        self.origin = [0, 0];
        self.group = Self::group(
            device,
            &self.layout,
            &self.uniform,
            self.mask.as_ref().map_or(&self.disabled, |mask| &mask.bits),
        );
        self.write_coordinates(queue);
    }

    pub(crate) fn dimensions(&self) -> Option<[u32; 2]> {
        self.mask.as_ref().map(|mask| mask.dimensions)
    }

    pub(crate) fn mask_origin(&self) -> [i32; 2] {
        self.mask.as_ref().map_or([0, 0], |mask| mask.origin)
    }

    pub(crate) fn set_origin(&mut self, queue: &wgpu::Queue, origin: [i64; 2]) -> bool {
        if self.mask.is_none() {
            return true;
        }
        let [Ok(x), Ok(y)] = origin.map(i32::try_from) else {
            return false;
        };
        if self.origin != [x, y] {
            self.origin = [x, y];
            self.write_coordinates(queue);
        }
        true
    }

    fn write_coordinates(&self, queue: &wgpu::Queue) {
        let [width, height] = self.dimensions().unwrap_or([0, 0]);
        let mut bytes = [0; 32];
        for (slot, value) in
            bytes[..16]
                .chunks_exact_mut(4)
                .zip([u32::from(self.mask.is_some()), width, height, 0])
        {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        for (slot, value) in bytes[16..].chunks_exact_mut(4).zip([
            self.origin[0],
            self.origin[1],
            self.mask_origin()[0],
            self.mask_origin()[1],
        ]) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.uniform, 0, &bytes);
    }
}

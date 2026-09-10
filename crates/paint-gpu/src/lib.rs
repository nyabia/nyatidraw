#![forbid(unsafe_code)]
//! WGPU-backed working texture for the first visible-ink vertical slice.
//!
//! The caller supplies the application's existing [`wgpu::Device`] and
//! [`wgpu::Queue`]. This crate never creates an instance, adapter, device,
//! queue, surface, or persistent CPU readback buffer.

mod compositor;
mod gesture_preview;
mod selection;
pub use selection::{GpuSelectionError, GpuSelectionMask};

pub use compositor::{
    CompositeRenderError, CompositeRenderStats, GpuCompositeScene, LayerUploadError,
    LiveStrokeDisposition, LiveStrokeError, LiveStrokeOutcome, LiveStrokeReplacement,
    LiveStrokeToken, MAX_PERSISTENT_SCENE_BYTES,
};

use std::sync::mpsc;

use nyatidraw_brush::BrushDab;
use wgpu::util::DeviceExt;

pub const WORKING_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const FLOATS_PER_INSTANCE: usize = 9;
const BYTES_PER_INSTANCE: usize = FLOATS_PER_INSTANCE * size_of::<f32>();
const INSTANCE_STRIDE: wgpu::BufferAddress = 36;
const INITIAL_INSTANCE_CAPACITY: usize = 64;
const RGBA8_BYTES_PER_PIXEL: u32 = 4;

/// Pixel-space region changed by one submitted dab batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirtyRect {
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

impl DirtyRect {
    fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }
}

/// Signed document-space region changed by one submitted dab batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignedDirtyRect {
    pub min_x: i64,
    pub min_y: i64,
    pub max_x: i64,
    pub max_y: i64,
}

impl SignedDirtyRect {
    pub(crate) fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }
}

/// Result of applying one CPU-produced [`BrushDab`] batch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PaintSubmission {
    /// Dabs that survived finite-value, opacity, and tile-intersection checks.
    pub dab_count: u32,
    /// Union of changed pixels, or `None` when no GPU pass was submitted.
    pub dirty: Option<DirtyRect>,
    /// Signed document-space union. This remains meaningful for workspace ink
    /// outside the finite page, where `dirty` is necessarily absent.
    pub document_dirty: Option<SignedDirtyRect>,
}

/// Tightly packed RGBA8 pixels copied from one [`GpuWorkingTile`].
///
/// `pixels` is top-left-origin, row-major RGBA8 premultiplied linear-light
/// data. It contains no GPU copy-row padding, so its length is always
/// `width * height * 4`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rgba8Readback {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Failure to restore a closed premultiplied RGBA8 snapshot into GPU memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuRestoreError {
    EmptyDimensions,
    DimensionOverflow,
    InvalidByteLength { expected: usize, actual: usize },
}

/// Failure while making the deliberately synchronous readback copy.
#[derive(Debug)]
pub enum GpuReadbackError {
    /// The image dimensions cannot be represented by the staging layout.
    SizeOverflow,
    /// The device failed while waiting for the submitted copy or map callback.
    Poll(wgpu::PollError),
    /// Mapping the staging buffer for CPU read access failed.
    Map(wgpu::BufferAsyncError),
    /// The GPU callback exited without reporting a mapping result.
    CallbackDisconnected,
}

/// Persistent RGBA8 texture or one renderer-owned texture-array slice mutated
/// by successive dab batches.
pub struct GpuWorkingTile {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    copy_origin: wgpu::Origin3d,
    width: u32,
    height: u32,
}

impl GpuWorkingTile {
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Clones the lightweight handle for registration with the compositor.
    #[must_use]
    pub fn texture(&self) -> wgpu::Texture {
        self.texture.clone()
    }

    pub(crate) fn from_array_layer(
        texture: wgpu::Texture,
        array_layer: u32,
        width: u32,
        height: u32,
    ) -> Self {
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("nayati-sparse-atlas-tile-view"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: array_layer,
            array_layer_count: Some(1),
            ..Default::default()
        });
        Self {
            texture,
            view,
            copy_origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: array_layer,
            },
            width,
            height,
        }
    }

    /// Copies this persistent texture to CPU memory and waits for completion.
    ///
    /// This is intentionally synchronous and allocates one staging buffer per
    /// call. Use it only for a closed-stroke/golden comparison boundary, never
    /// from the per-dab or present path.
    ///
    /// # Errors
    ///
    /// Returns an error when staging dimensions overflow, device polling fails,
    /// or the GPU cannot map the staging buffer.
    pub fn readback_rgba8(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Rgba8Readback, GpuReadbackError> {
        let layout = ReadbackLayout::new(self.width, self.height)?;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nayati-working-tile-readback"),
            size: layout.staging_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nayati-working-tile-readback-copy"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: self.copy_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(layout.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = staging.slice(..);
        let (sender, receiver) = mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait)
            .map_err(GpuReadbackError::Poll)?;
        receiver
            .recv()
            .map_err(|_| GpuReadbackError::CallbackDisconnected)?
            .map_err(GpuReadbackError::Map)?;

        let mapped = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity(layout.tight_bytes);
        for row in mapped
            .chunks_exact(layout.padded_bytes_per_row_usize)
            .take(layout.row_count)
        {
            pixels.extend_from_slice(&row[..layout.tight_bytes_per_row]);
        }
        drop(mapped);
        staging.unmap();

        Ok(Rgba8Readback {
            width: self.width,
            height: self.height,
            pixels,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct ReadbackLayout {
    padded_bytes_per_row: u32,
    padded_bytes_per_row_usize: usize,
    tight_bytes_per_row: usize,
    tight_bytes: usize,
    staging_bytes: u64,
    row_count: usize,
}

impl ReadbackLayout {
    fn new(width: u32, height: u32) -> Result<Self, GpuReadbackError> {
        let unpadded_bytes_per_row = width
            .checked_mul(RGBA8_BYTES_PER_PIXEL)
            .ok_or(GpuReadbackError::SizeOverflow)?;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .checked_add(alignment - 1)
            .ok_or(GpuReadbackError::SizeOverflow)?
            / alignment
            * alignment;
        let staging_bytes = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .ok_or(GpuReadbackError::SizeOverflow)?;
        let tight_bytes = usize::try_from(
            u64::from(unpadded_bytes_per_row)
                .checked_mul(u64::from(height))
                .ok_or(GpuReadbackError::SizeOverflow)?,
        )
        .map_err(|_| GpuReadbackError::SizeOverflow)?;

        Ok(Self {
            padded_bytes_per_row,
            padded_bytes_per_row_usize: usize::try_from(padded_bytes_per_row)
                .map_err(|_| GpuReadbackError::SizeOverflow)?,
            tight_bytes_per_row: usize::try_from(unpadded_bytes_per_row)
                .map_err(|_| GpuReadbackError::SizeOverflow)?,
            tight_bytes,
            staging_bytes,
            row_count: usize::try_from(height).map_err(|_| GpuReadbackError::SizeOverflow)?,
        })
    }
}

/// Instanced round-dab renderer for one or more persistent working textures.
pub struct GpuRoundDabPainter {
    selection: selection::SelectionBinding,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    brush_buffer: wgpu::Buffer,
    brush_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

impl GpuRoundDabPainter {
    /// Builds pipelines on the already-selected application device.
    ///
    /// `brush_rgba` is straight-alpha, linear-light RGBA. Fixed-function
    /// source-over blending stores premultiplied linear-light RGBA8 in the
    /// working texture, matching the CPU reference colour convention.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, brush_rgba: [f32; 4]) -> Self {
        Self::new_with_blend(
            device,
            queue,
            brush_rgba,
            wgpu::BlendState::ALPHA_BLENDING,
            false,
        )
    }

    /// Builds a source-atop recoloring painter. RGB is premultiplied in the
    /// fragment shader, and alpha writes are disabled to retain exact bytes.
    #[must_use]
    pub fn new_alpha_locked(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        brush_rgba: [f32; 4],
    ) -> Self {
        let source_atop = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::DstAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        Self::new_with_blend(device, queue, brush_rgba, source_atop, true)
    }

    /// Builds a destination-out painter for alpha erasing.
    #[must_use]
    pub fn new_eraser(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let destination_out = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        Self::new_with_blend(device, queue, [0.0, 0.0, 0.0, 1.0], destination_out, false)
    }

    #[allow(clippy::too_many_lines)]
    fn new_with_blend(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        brush_rgba: [f32; 4],
        blend: wgpu::BlendState,
        alpha_locked: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nayati-round-dab-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("round_dab.wgsl").into()),
        });
        let selection = selection::SelectionBinding::new(device);
        let brush_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nayati-round-dab-color"),
            contents: &encode_f32s(&brush_rgba),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let brush_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nayati-round-dab-brush-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let brush_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nayati-round-dab-brush-bind-group"),
            layout: &brush_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: brush_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nayati-round-dab-pipeline-layout"),
            bind_group_layouts: &[&brush_layout, &selection.layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nayati-round-dab-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 20,
                            shader_location: 3,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 28,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32,
                            offset: 32,
                            shader_location: 5,
                        },
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(if alpha_locked {
                    "fragment_alpha_locked"
                } else {
                    "fragment_main"
                }),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORKING_TEXTURE_FORMAT,
                    blend: Some(blend),
                    write_mask: if alpha_locked {
                        wgpu::ColorWrites::RED | wgpu::ColorWrites::GREEN | wgpu::ColorWrites::BLUE
                    } else {
                        wgpu::ColorWrites::ALL
                    },
                })],
            }),
            multiview: None,
            cache: None,
        });
        let instance_capacity = INITIAL_INSTANCE_CAPACITY;
        let instance_buffer = create_instance_buffer(device, instance_capacity);

        Self {
            device: device.clone(),
            selection,
            queue: queue.clone(),
            pipeline,
            brush_buffer,
            brush_bind_group,
            instance_buffer,
            instance_capacity,
        }
    }

    /// Updates the straight-alpha linear-light colour used by subsequent dab
    /// submissions without rebuilding the render pipeline.
    pub fn set_brush_rgba(&self, brush_rgba: [f32; 4]) {
        self.queue
            .write_buffer(&self.brush_buffer, 0, &encode_f32s(&brush_rgba));
    }

    /// Uploads canonical packed binary coverage once for both painter bindings.
    ///
    /// # Errors
    /// Rejects empty/oversized dimensions and noncanonical coverage.
    pub fn create_selection_mask(
        &self,
        dimensions: [u32; 2],
        packed: &[u8],
    ) -> Result<GpuSelectionMask, GpuSelectionError> {
        self.create_selection_mask_at([0, 0], dimensions, packed)
    }

    /// Uploads bounded coverage whose document origin may lie outside the page.
    ///
    /// # Errors
    /// Rejects invalid extents or noncanonical packed coverage.
    pub fn create_selection_mask_at(
        &self,
        origin: [i32; 2],
        dimensions: [u32; 2],
        packed: &[u8],
    ) -> Result<GpuSelectionMask, GpuSelectionError> {
        GpuSelectionMask::new(&self.device, origin, dimensions, packed)
    }

    /// Changes semantic coverage between strokes; no mask upload occurs per dab.
    pub fn set_selection_mask(&mut self, mask: Option<&GpuSelectionMask>) {
        self.selection.replace(&self.device, &self.queue, mask);
    }

    /// Allocates and clears one persistent working texture.
    ///
    /// # Panics
    ///
    /// Panics when either dimension is zero.
    #[must_use]
    pub fn create_working_tile(&self, width: u32, height: u32) -> GpuWorkingTile {
        assert!(width > 0 && height > 0, "working tile must be non-empty");
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nayati-gpu-working-tile"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nayati-gpu-working-tile-init"),
            });
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nayati-gpu-working-tile-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
        }
        self.queue.submit([encoder.finish()]);

        GpuWorkingTile {
            texture,
            view,
            copy_origin: wgpu::Origin3d::ZERO,
            width,
            height,
        }
    }

    /// Restores a closed, tightly packed premultiplied RGBA8 snapshot.
    ///
    /// This upload is intentionally outside the live-stroke path. It is the
    /// device-recreation seam for durable CPU tile bytes, not a replacement for
    /// the viewport's incremental GPU painting path.
    ///
    /// # Errors
    ///
    /// Returns an error when dimensions are empty or the byte length does not
    /// exactly describe one RGBA8 image.
    pub fn restore_closed_rgba8(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<GpuWorkingTile, GpuRestoreError> {
        if width == 0 || height == 0 {
            return Err(GpuRestoreError::EmptyDimensions);
        }
        let expected = rgba8_byte_len(width, height).ok_or(GpuRestoreError::DimensionOverflow)?;
        if pixels.len() != expected {
            return Err(GpuRestoreError::InvalidByteLength {
                expected,
                actual: pixels.len(),
            });
        }
        let tile = self.create_working_tile(width, height);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tile.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * RGBA8_BYTES_PER_PIXEL),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(tile)
    }

    /// Applies all visible dabs in one instanced pass, preserving prior pixels.
    ///
    /// No CPU readback or per-dab submission occurs.
    pub fn apply(&mut self, tile: &GpuWorkingTile, dabs: &[BrushDab]) -> PaintSubmission {
        let prepared = PreparedDabs::new(tile, dabs);
        self.apply_prepared(tile, &prepared)
    }

    pub(crate) fn prepare_live(tile: &GpuWorkingTile, dabs: &[BrushDab]) -> PreparedDabs {
        PreparedDabs::new(tile, dabs)
    }

    pub(crate) fn apply_prepared(
        &mut self,
        tile: &GpuWorkingTile,
        prepared: &PreparedDabs,
    ) -> PaintSubmission {
        self.apply_prepared_at_origin(tile, prepared, [0, 0])
    }

    pub(crate) fn apply_prepared_at_origin(
        &mut self,
        tile: &GpuWorkingTile,
        prepared: &PreparedDabs,
        origin: [i64; 2],
    ) -> PaintSubmission {
        if prepared.bytes.is_empty() {
            return PaintSubmission::default();
        }
        if !self.selection.set_origin(&self.queue, origin) {
            return PaintSubmission::default();
        }

        self.ensure_instance_capacity(prepared.count);
        self.queue
            .write_buffer(&self.instance_buffer, 0, &prepared.bytes);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nayati-round-dab-batch"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nayati-round-dab-batch-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &tile.view,
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
            pass.set_bind_group(0, &self.brush_bind_group, &[]);
            pass.set_bind_group(1, &self.selection.group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            pass.draw(0..6, 0..prepared.count);
        }
        self.queue.submit([encoder.finish()]);

        PaintSubmission {
            dab_count: prepared.count,
            dirty: prepared.dirty,
            document_dirty: prepared.dirty.map(|dirty| SignedDirtyRect {
                min_x: i64::from(dirty.min_x),
                min_y: i64::from(dirty.min_y),
                max_x: i64::from(dirty.max_x),
                max_y: i64::from(dirty.max_y),
            }),
        }
    }

    fn ensure_instance_capacity(&mut self, required: u32) {
        let required = usize::try_from(required).expect("u32 fits usize on supported targets");
        if required <= self.instance_capacity {
            return;
        }

        self.instance_capacity = required.next_power_of_two();
        self.instance_buffer = create_instance_buffer(&self.device, self.instance_capacity);
    }
}

pub(crate) struct PreparedDabs {
    bytes: Vec<u8>,
    count: u32,
    dirty: Option<DirtyRect>,
}

impl PreparedDabs {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn new(tile: &GpuWorkingTile, dabs: &[BrushDab]) -> Self {
        let width = f64::from(tile.width);
        let height = f64::from(tile.height);
        let mut bytes = Vec::with_capacity(dabs.len().saturating_mul(BYTES_PER_INSTANCE));
        let mut dirty: Option<DirtyRect> = None;

        for dab in dabs {
            let x = dab.center.x;
            let y = dab.center.y;
            let radius = f64::from(dab.radius_px);
            let opacity = (dab.opacity.clamp(0.0, 1.0) * dab.flow.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            if !x.is_finite()
                || !y.is_finite()
                || !radius.is_finite()
                || radius <= 0.0
                || opacity <= 0.0
                || !opacity.is_finite()
            {
                continue;
            }

            let min_x = (x - radius).floor().clamp(0.0, width) as u32;
            let min_y = (y - radius).floor().clamp(0.0, height) as u32;
            let max_x = (x + radius).ceil().clamp(0.0, width) as u32;
            let max_y = (y + radius).ceil().clamp(0.0, height) as u32;
            if min_x >= max_x || min_y >= max_y {
                continue;
            }

            let dab_dirty = DirtyRect {
                min_x,
                min_y,
                max_x,
                max_y,
            };
            dirty = Some(dirty.map_or(dab_dirty, |current| current.union(dab_dirty)));
            let quad_radius = radius + 0.5;
            let instance = [
                (x.mul_add(2.0, -width) / width) as f32,
                ((height - y * 2.0) / height) as f32,
                (quad_radius * 2.0 / width) as f32,
                (quad_radius * 2.0 / height) as f32,
                opacity,
                x as f32,
                y as f32,
                radius as f32,
                if dab.hardness.is_finite() {
                    dab.hardness.clamp(0.0, 1.0)
                } else {
                    0.0
                },
            ];
            bytes.extend_from_slice(&encode_f32s(&instance));
        }

        let count = u32::try_from(bytes.len() / BYTES_PER_INSTANCE)
            .expect("one batch cannot exceed u32 dab instances");
        Self {
            bytes,
            count,
            dirty,
        }
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let byte_capacity = capacity
        .checked_mul(BYTES_PER_INSTANCE)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .expect("instance buffer size fits u64");
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("nayati-round-dab-instances"),
        size: byte_capacity,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn rgba8_byte_len(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(usize::try_from(RGBA8_BYTES_PER_PIXEL).ok()?)
}

fn encode_f32s(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len().saturating_mul(size_of::<f32>()));
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

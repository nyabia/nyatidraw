use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

use nyatidraw_api::{
    CompositeTileKey, GroupId, LayerBlendMode, LayerId, LayerTreeNodeId, TileCoordinate,
};
use nyatidraw_brush::BrushDab;
use nyatidraw_document::{
    CompositionScope, GroupNode, LayerTree, LayerTreeError, LayerTreeNode, composition_scope_nodes,
    layer_stacks,
};
use nyatidraw_input::{Point, ViewportTransform, ViewportValidationError};
use nyatidraw_tiles::{CompositeCache, TILE_BYTE_LEN, TILE_EDGE, TileKey};

use crate::{
    GpuRoundDabPainter, GpuWorkingTile, PaintSubmission, RGBA8_BYTES_PER_PIXEL,
    WORKING_TEXTURE_FORMAT,
};

const INITIAL_COMPOSITE_CHILD_CAPACITY: usize = 32;
const UNIFORM_VALUE_BYTES: usize = 16;
const WORKSPACE_TILE_UNIFORM_BYTES: usize = 64;
const MAX_SPARSE_ATLAS_SLOTS: u32 = 2_048;
const COMPOSITE_SCRATCH_BYTES: u64 = TILE_BYTE_LEN as u64 * 4;
/// Explicit cap for the temporary full-document Sprint 3 texture strategy.
pub const MAX_PERSISTENT_SCENE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerUploadError {
    UnknownLayer(LayerId),
    LiveStrokeActive(LiveStrokeToken),
    UnsupportedMip(u8),
    InvalidByteLength { expected: usize, actual: usize },
    SparseAtlasExhausted,
    LayerTree(LayerTreeError),
}

/// Opaque identity of one renderer-owned live stroke.
///
/// The monotonically increasing ordinal lets the editor reject a late CPU
/// materialization belonging to an older, cancelled, or replaced stroke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiveStrokeToken {
    ordinal: u64,
    layer: LayerId,
}

impl LiveStrokeToken {
    #[must_use]
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }

    #[must_use]
    pub const fn layer(self) -> LayerId {
        self.layer
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveStrokeDisposition {
    Commit,
    Cancel,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveStrokeOutcome {
    pub token: LiveStrokeToken,
    pub disposition: LiveStrokeDisposition,
    /// Exact base-mip tiles touched by accepted dabs, in stable key order.
    pub dirty_tiles: Vec<TileKey>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveStrokeReplacement {
    pub cancelled: Option<LiveStrokeOutcome>,
    pub active: LiveStrokeToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveStrokeError {
    UnknownLayer(LayerId),
    AlreadyActive(LiveStrokeToken),
    NoActiveStroke,
    StaleToken {
        expected: LiveStrokeToken,
        actual: LiveStrokeToken,
    },
    OrdinalExhausted,
    SparseAtlasExhausted,
    LayerTree(LayerTreeError),
}

struct ActiveLiveStroke {
    token: LiveStrokeToken,
    dirty_tiles: std::collections::BTreeSet<TileKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompositeRenderError {
    InvalidDocumentSize,
    LiveStrokeActive,
    AllocationBudgetExceeded {
        required_bytes: u64,
        max_bytes: u64,
        surfaces: usize,
    },
    InvalidViewport(ViewportValidationError),
    ViewportSizeMismatch,
    UnknownGroup(GroupId),
    MissingRaster(LayerId),
    TooManyChildren,
    SparseAtlasExhausted,
    LayerTree(LayerTreeError),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompositeRenderStats {
    pub group_tiles_rebuilt: u64,
    pub display_passes: u64,
    pub cache_entries: usize,
}

struct CompositeSurface {
    tile: GpuWorkingTile,
    source_bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SparseSurfaceKey {
    Raster(TileKey),
    Group(CompositeTileKey),
    LiveBackup(TileKey),
}

struct SparseAtlasSurface {
    key: SparseSurfaceKey,
    tile: GpuWorkingTile,
    source_bind_group: wgpu::BindGroup,
    last_used: u64,
}

struct SparseAtlas {
    texture: wgpu::Texture,
    slots: Vec<Option<SparseAtlasSurface>>,
    key_to_slot: BTreeMap<SparseSurfaceKey, u32>,
    clock: u64,
}

impl SparseAtlas {
    fn new(device: &wgpu::Device, slot_count: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nayati-signed-workspace-atlas"),
            size: wgpu::Extent3d {
                width: TILE_EDGE,
                height: TILE_EDGE,
                depth_or_array_layers: slot_count,
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
        Self {
            texture,
            slots: (0..slot_count).map(|_| None).collect(),
            key_to_slot: BTreeMap::new(),
            clock: 1,
        }
    }

    fn slot(&self, key: SparseSurfaceKey) -> Option<u32> {
        self.key_to_slot.get(&key).copied()
    }

    fn surface(&self, key: SparseSurfaceKey) -> Option<&SparseAtlasSurface> {
        let slot = usize::try_from(self.slot(key)?).ok()?;
        self.slots.get(slot)?.as_ref()
    }

    fn surface_mut(&mut self, key: SparseSurfaceKey) -> Option<&mut SparseAtlasSurface> {
        let slot = usize::try_from(self.slot(key)?).ok()?;
        self.clock = self.clock.saturating_add(1);
        let surface = self.slots.get_mut(slot)?.as_mut()?;
        surface.last_used = self.clock;
        Some(surface)
    }

    fn allocate(
        &mut self,
        device: &wgpu::Device,
        source_layout: &wgpu::BindGroupLayout,
        key: SparseSurfaceKey,
        protected: &BTreeSet<SparseSurfaceKey>,
    ) -> Result<(u32, Option<SparseSurfaceKey>), ()> {
        if let Some(slot) = self.slot(key) {
            let _ = self.surface_mut(key);
            return Ok((slot, None));
        }
        let free = self.slots.iter().position(Option::is_none);
        let candidate = free.or_else(|| {
            self.slots
                .iter()
                .enumerate()
                .filter_map(|(slot, surface)| {
                    let surface = surface.as_ref()?;
                    (!protected.contains(&surface.key)).then_some((slot, surface.last_used))
                })
                .min_by_key(|(_, last_used)| *last_used)
                .map(|(slot, _)| slot)
        });
        let slot = candidate.ok_or(())?;
        let evicted = self.slots[slot].take().map(|surface| {
            self.key_to_slot.remove(&surface.key);
            surface.key
        });
        let array_layer = u32::try_from(slot).map_err(|_| ())?;
        let tile = GpuWorkingTile::from_array_layer(
            self.texture.clone(),
            array_layer,
            TILE_EDGE,
            TILE_EDGE,
        );
        let source_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nayati-sparse-atlas-source"),
            layout: source_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&tile.view),
            }],
        });
        self.clock = self.clock.saturating_add(1);
        self.slots[slot] = Some(SparseAtlasSurface {
            key,
            tile,
            source_bind_group,
            last_used: self.clock,
        });
        self.key_to_slot.insert(key, array_layer);
        Ok((array_layer, evicted))
    }
}

impl CompositeSurface {
    fn new(
        tile: GpuWorkingTile,
        device: &wgpu::Device,
        source_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let source_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nayati-composite-source"),
            layout: source_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&tile.view),
            }],
        });
        Self {
            tile,
            source_bind_group,
        }
    }
}

#[derive(Clone, Copy)]
enum StackInput {
    Page(LayerTreeNodeId),
    Sparse(SparseSurfaceKey),
    Scratch(usize),
    Transparent,
}
struct StackPass {
    input: StackInput,
    backdrop: usize,
    target: usize,
    params: [u32; 4],
}
fn node_opacity(node: &LayerTreeNode) -> u16 {
    match node {
        LayerTreeNode::Raster(layer) => layer.opacity_u16,
        LayerTreeNode::Group(group) => group.opacity_u16,
    }
}
fn blend_operation(node: &LayerTreeNode) -> u32 {
    let mode = match node {
        LayerTreeNode::Raster(layer) => layer.blend_mode,
        LayerTreeNode::Group(group) => group.blend_mode,
    };
    match mode {
        LayerBlendMode::Normal => 0,
        LayerBlendMode::Multiply => 1,
    }
}
struct DynamicOpacityBuffer {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    stride: usize,
    capacity: usize,
}

impl DynamicOpacityBuffer {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, capacity: usize) -> Self {
        let stride = usize::try_from(device.limits().min_uniform_buffer_offset_alignment)
            .expect("uniform alignment fits usize")
            .max(UNIFORM_VALUE_BYTES);
        let buffer = create_uniform_buffer(device, stride, capacity, "nayati-composite-opacity");
        let bind_group = create_uniform_bind_group(
            device,
            layout,
            &buffer,
            "nayati-composite-opacity-bind-group",
        );
        Self {
            buffer,
            bind_group,
            stride,
            capacity,
        }
    }

    fn ensure_capacity(
        &mut self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        required: usize,
    ) {
        if required <= self.capacity {
            return;
        }
        self.capacity = required.next_power_of_two();
        self.buffer = create_uniform_buffer(
            device,
            self.stride,
            self.capacity,
            "nayati-composite-opacity",
        );
        self.bind_group = create_uniform_bind_group(
            device,
            layout,
            &self.buffer,
            "nayati-composite-opacity-bind-group",
        );
    }

    fn write(&self, queue: &wgpu::Queue, parameters: &[[u32; 4]]) {
        let mut bytes = vec![0_u8; self.stride.saturating_mul(parameters.len())];
        for (index, values) in parameters.iter().enumerate() {
            let start = index * self.stride;
            for (component, value) in values.iter().enumerate() {
                bytes[start + component * 4..start + component * 4 + 4]
                    .copy_from_slice(&value.to_ne_bytes());
            }
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, &bytes);
        }
    }

    fn dynamic_offset(&self, index: usize) -> Result<u32, CompositeRenderError> {
        u32::try_from(index.saturating_mul(self.stride))
            .map_err(|_| CompositeRenderError::TooManyChildren)
    }
}

struct DynamicWorkspaceParams {
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    stride: usize,
    capacity: usize,
}

impl DynamicWorkspaceParams {
    fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nayati-workspace-tile-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(WORKSPACE_TILE_UNIFORM_BYTES as u64),
                },
                count: None,
            }],
        });
        let stride = usize::try_from(device.limits().min_uniform_buffer_offset_alignment)
            .expect("uniform alignment fits usize")
            .max(WORKSPACE_TILE_UNIFORM_BYTES);
        let capacity = 1;
        let buffer =
            create_uniform_buffer(device, stride, capacity, "nayati-workspace-tile-uniforms");
        let bind_group = create_workspace_bind_group(
            device,
            &layout,
            &buffer,
            "nayati-workspace-tile-bind-group",
        );
        Self {
            layout,
            buffer,
            bind_group,
            stride,
            capacity,
        }
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, required: usize) {
        if required <= self.capacity {
            return;
        }
        self.capacity = required.next_power_of_two();
        self.buffer = create_uniform_buffer(
            device,
            self.stride,
            self.capacity,
            "nayati-workspace-tile-uniforms",
        );
        self.bind_group = create_workspace_bind_group(
            device,
            &self.layout,
            &self.buffer,
            "nayati-workspace-tile-bind-group",
        );
    }

    #[allow(clippy::cast_precision_loss)]
    fn write(&self, queue: &wgpu::Queue, affine: [f32; 12], coordinates: &[TileCoordinate]) {
        let mut bytes = vec![0_u8; self.stride.saturating_mul(coordinates.len())];
        for (index, coordinate) in coordinates.iter().enumerate() {
            let mut values = [0.0_f32; 16];
            values[..12].copy_from_slice(&affine);
            values[12] = coordinate.x as f32 * TILE_EDGE as f32;
            values[13] = coordinate.y as f32 * TILE_EDGE as f32;
            let encoded = encode_f32s(&values);
            let start = index * self.stride;
            bytes[start..start + WORKSPACE_TILE_UNIFORM_BYTES].copy_from_slice(&encoded);
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, &bytes);
        }
    }

    fn dynamic_offset(&self, index: usize) -> Result<u32, CompositeRenderError> {
        u32::try_from(index.saturating_mul(self.stride))
            .map_err(|_| CompositeRenderError::TooManyChildren)
    }
}

/// Renderer-owned raster/group scene with coordinate-scoped composite caches.
///
/// Finite-page pixels retain the fast document-sized textures. Signed tiles
/// crossing or outside that page use a bounded texture-array atlas with lazy
/// closed-tile residency and exact cache eviction. The viewport texture is a
/// disposable display projection; workspace tiles never enter page export.
pub struct GpuCompositeScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    document_size: [u32; 2],
    tree: LayerTree,
    solo_node: Option<LayerTreeNodeId>,
    solo_scope: Option<BTreeSet<LayerTreeNodeId>>,
    rasters: BTreeMap<LayerId, CompositeSurface>,
    groups: BTreeMap<GroupId, CompositeSurface>,
    // A boundary coordinate has two different products: its page-clipped
    // texture and its complete signed-atlas tile. Rebuilding one must never
    // make the other look current. Sparse validity is bounded by atlas slots.
    page_cache: CompositeCache<()>,
    sparse_cache: CompositeCache<()>,
    source_layout: wgpu::BindGroupLayout,
    opacity_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    composite_scratch: [CompositeSurface; 4],
    opacity: DynamicOpacityBuffer,
    viewport_pipeline: wgpu::RenderPipeline,
    selection_overlay: crate::selection::SelectionBinding,
    selection_pipeline: wgpu::RenderPipeline,
    gesture_preview: crate::gesture_preview::GesturePreview,
    viewport_bind_group: wgpu::BindGroup,
    viewport_uniform: wgpu::Buffer,
    workspace_pipeline: wgpu::RenderPipeline,
    workspace_params: DynamicWorkspaceParams,
    sparse_atlas: SparseAtlas,
    sparse_closed_tiles: BTreeMap<TileKey, Box<[u8]>>,
    sparse_preview_tiles: BTreeSet<TileKey>,
    sparse_coordinates: BTreeSet<TileCoordinate>,
    display: Option<GpuWorkingTile>,
    live_backup: GpuWorkingTile,
    live_stroke: Option<ActiveLiveStroke>,
    next_live_ordinal: u64,
    zero_tile: Vec<u8>,
}

impl GpuCompositeScene {
    /// Checks a history/structural tree replacement before durable acceptance.
    ///
    /// # Errors
    /// Rejects active input and allocations beyond the scene plus atlas budget.
    pub fn validate_tree_replacement(&self, tree: &LayerTree) -> Result<(), CompositeRenderError> {
        self.replacement_atlas_slots(tree).map(|_| ())
    }

    fn replacement_atlas_slots(&self, tree: &LayerTree) -> Result<usize, CompositeRenderError> {
        self.replacement_atlas_slots_for(tree, self.document_size)
    }

    fn replacement_atlas_slots_for(
        &self,
        tree: &LayerTree,
        size: [u32; 2],
    ) -> Result<usize, CompositeRenderError> {
        if self.live_stroke.is_some() {
            return Err(CompositeRenderError::LiveStrokeActive);
        }
        if size.contains(&0)
            || size
                .iter()
                .any(|value| *value > self.device.limits().max_texture_dimension_2d)
        {
            return Err(CompositeRenderError::InvalidDocumentSize);
        }
        let mut rasters = Vec::new();
        let mut groups = vec![tree.root_id()];
        collect_node_ids(tree.root(), &mut rasters, &mut groups);
        let surfaces = rasters.len().saturating_add(groups.len()).saturating_add(1);
        let page_bytes = u64::from(size[0])
            .checked_mul(u64::from(size[1]))
            .and_then(|size| size.checked_mul(u64::from(RGBA8_BYTES_PER_PIXEL)))
            .and_then(|size| size.checked_mul(u64::try_from(surfaces).ok()?))
            .unwrap_or(u64::MAX);
        let required_bytes = page_bytes
            .saturating_add(COMPOSITE_SCRATCH_BYTES)
            .saturating_add(TILE_BYTE_LEN as u64);
        if required_bytes > MAX_PERSISTENT_SCENE_BYTES {
            return Err(CompositeRenderError::AllocationBudgetExceeded {
                required_bytes,
                max_bytes: MAX_PERSISTENT_SCENE_BYTES,
                surfaces,
            });
        }
        Ok(self.sparse_atlas.slots.len().min(
            usize::try_from(
                (MAX_PERSISTENT_SCENE_BYTES - page_bytes - COMPOSITE_SCRATCH_BYTES)
                    / TILE_BYTE_LEN as u64,
            )
            .unwrap_or(usize::MAX),
        ))
    }

    /// Preflights a page/history replacement against texture and scene budgets.
    ///
    /// # Errors
    /// Rejects active input, invalid dimensions and excessive allocation.
    pub fn validate_document_replacement(
        &self,
        size: [u32; 2],
        tree: &LayerTree,
    ) -> Result<(), CompositeRenderError> {
        self.replacement_atlas_slots_for(tree, size).map(|_| ())
    }

    /// Replaces disposable page-sized surfaces, retaining the device, pipelines,
    /// canvas owner and monotonic stroke identities. Caller reuploads CPU tiles.
    ///
    /// # Errors
    /// Returns the same preflight errors before discarding any surface.
    pub fn replace_document(
        &mut self,
        size: [u32; 2],
        tree: LayerTree,
    ) -> Result<(), CompositeRenderError> {
        self.validate_document_replacement(size, &tree)?;
        self.rasters.clear();
        self.groups.clear();
        self.sparse_closed_tiles.clear();
        self.sparse_preview_tiles.clear();
        self.sparse_coordinates.clear();
        self.document_size = size;
        self.live_backup = create_document_texture(
            &self.device,
            &self.queue,
            size,
            "nayati-resized-stroke-backup",
        );
        self.replace_tree(tree)
    }

    /// Reconciles disposable surfaces with an authoritative history tree.
    /// Existing raster surfaces survive; the caller uploads changed CPU tiles.
    ///
    /// # Errors
    /// Returns the same checks as `validate_tree_replacement` before mutation.
    pub fn replace_tree(&mut self, tree: LayerTree) -> Result<(), CompositeRenderError> {
        let atlas_slots = self.replacement_atlas_slots(&tree)?;
        if atlas_slots < self.sparse_atlas.slots.len() {
            self.sparse_atlas = SparseAtlas::new(
                &self.device,
                u32::try_from(atlas_slots)
                    .map_err(|_| CompositeRenderError::SparseAtlasExhausted)?,
            );
        }
        let mut rasters = Vec::new();
        let mut groups = vec![tree.root_id()];
        collect_node_ids(tree.root(), &mut rasters, &mut groups);
        let rasters: BTreeSet<_> = rasters.into_iter().collect();
        let groups: BTreeSet<_> = groups.into_iter().collect();
        self.rasters.retain(|id, _| rasters.contains(id));
        self.groups.retain(|id, _| groups.contains(id));
        for id in rasters {
            self.rasters.entry(id).or_insert_with(|| {
                CompositeSurface::new(
                    create_document_texture(
                        &self.device,
                        &self.queue,
                        self.document_size,
                        "nayati-restored-raster",
                    ),
                    &self.device,
                    &self.source_layout,
                )
            });
        }
        for id in groups {
            self.groups.entry(id).or_insert_with(|| {
                CompositeSurface::new(
                    create_document_texture(
                        &self.device,
                        &self.queue,
                        self.document_size,
                        "nayati-restored-group",
                    ),
                    &self.device,
                    &self.source_layout,
                )
            });
        }
        self.tree = tree;
        if self
            .solo_node
            .is_some_and(|id| self.tree.ancestors(id).is_none())
        {
            self.solo_node = None;
        }
        self.page_cache.clear();
        self.sparse_cache.clear();
        self.sparse_atlas.key_to_slot.clear();
        self.sparse_atlas
            .slots
            .iter_mut()
            .for_each(|slot| *slot = None);
        self.sparse_closed_tiles
            .retain(|key, _| self.rasters.contains_key(&key.layer));
        self.sparse_preview_tiles.clear();
        self.sparse_coordinates = self
            .sparse_closed_tiles
            .keys()
            .map(|key| key.coordinate())
            .collect();
        Ok(())
    }

    /// Allocates one persistent document texture per raster and group node.
    ///
    /// # Errors
    ///
    /// Returns an error for empty document dimensions.
    #[allow(clippy::too_many_lines)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        document_size: [u32; 2],
        tree: LayerTree,
    ) -> Result<Self, CompositeRenderError> {
        if document_size.contains(&0) {
            return Err(CompositeRenderError::InvalidDocumentSize);
        }
        let mut raster_ids = Vec::new();
        let mut group_ids = vec![tree.root_id()];
        collect_node_ids(tree.root(), &mut raster_ids, &mut group_ids);
        // Include the single reusable live-stroke backup texture in the hard
        // allocation budget. Only touched 128x128 regions are ever copied.
        let surface_count = raster_ids
            .len()
            .saturating_add(group_ids.len())
            .saturating_add(1);
        let required_bytes = u64::from(document_size[0])
            .checked_mul(u64::from(document_size[1]))
            .and_then(|pixels| pixels.checked_mul(u64::from(RGBA8_BYTES_PER_PIXEL)))
            .and_then(|surface_bytes| surface_bytes.checked_mul(u64::try_from(surface_count).ok()?))
            .unwrap_or(u64::MAX)
            .saturating_add(COMPOSITE_SCRATCH_BYTES);
        if required_bytes > MAX_PERSISTENT_SCENE_BYTES {
            return Err(CompositeRenderError::AllocationBudgetExceeded {
                required_bytes,
                max_bytes: MAX_PERSISTENT_SCENE_BYTES,
                surfaces: surface_count,
            });
        }
        let tile_byte_len = TILE_BYTE_LEN as u64;
        let sparse_slot_budget = (MAX_PERSISTENT_SCENE_BYTES - required_bytes) / tile_byte_len;
        let atlas_slots = u32::try_from(
            sparse_slot_budget.min(u64::from(device.limits().max_texture_array_layers)),
        )
        .unwrap_or(device.limits().max_texture_array_layers)
        .min(MAX_SPARSE_ATLAS_SLOTS);
        if atlas_slots == 0 {
            return Err(CompositeRenderError::AllocationBudgetExceeded {
                required_bytes: required_bytes.saturating_add(tile_byte_len),
                max_bytes: MAX_PERSISTENT_SCENE_BYTES,
                surfaces: surface_count.saturating_add(1),
            });
        }
        let source_layout = create_source_layout(device);
        let opacity_layout = create_opacity_layout(device);
        let composite_pipeline = create_composite_pipeline(device, &source_layout, &opacity_layout);
        let composite_scratch = std::array::from_fn(|_| {
            CompositeSurface::new(
                create_document_texture(
                    device,
                    queue,
                    [TILE_EDGE; 2],
                    "nyatidraw-stack-composite-scratch",
                ),
                device,
                &source_layout,
            )
        });
        let viewport_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nayati-viewport-affine"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (viewport_layout, viewport_bind_group) =
            create_viewport_binding(device, &viewport_uniform);
        let viewport_pipeline = create_viewport_pipeline(
            device,
            &source_layout,
            &viewport_layout,
            include_str!("viewport.wgsl"),
        );
        let selection_overlay = crate::selection::SelectionBinding::new(device);
        let selection_pipeline = create_viewport_pipeline(
            device,
            &selection_overlay.layout,
            &viewport_layout,
            include_str!("selection_overlay.wgsl"),
        );
        let workspace_params = DynamicWorkspaceParams::new(device);
        let workspace_pipeline =
            create_workspace_pipeline(device, &source_layout, &workspace_params.layout);
        let opacity =
            DynamicOpacityBuffer::new(device, &opacity_layout, INITIAL_COMPOSITE_CHILD_CAPACITY);

        let sparse_atlas = SparseAtlas::new(device, atlas_slots);

        let mut rasters = BTreeMap::new();
        for id in raster_ids {
            let tile = create_document_texture(device, queue, document_size, "nayati-raster-layer");
            rasters.insert(id, CompositeSurface::new(tile, device, &source_layout));
        }
        let mut groups = BTreeMap::new();
        for id in group_ids {
            let tile =
                create_document_texture(device, queue, document_size, "nayati-group-composite");
            groups.insert(id, CompositeSurface::new(tile, device, &source_layout));
        }
        let live_backup =
            create_document_texture(device, queue, document_size, "nayati-live-stroke-backup");

        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            document_size,
            tree,
            solo_node: None,
            solo_scope: None,
            rasters,
            groups,
            page_cache: CompositeCache::default(),
            sparse_cache: CompositeCache::default(),
            source_layout,
            opacity_layout,
            composite_pipeline,
            composite_scratch,
            opacity,
            viewport_pipeline,
            selection_overlay,
            selection_pipeline,
            gesture_preview: crate::gesture_preview::GesturePreview::new(device),
            viewport_bind_group,
            viewport_uniform,
            workspace_pipeline,
            workspace_params,
            sparse_atlas,
            sparse_closed_tiles: BTreeMap::new(),
            sparse_preview_tiles: BTreeSet::new(),
            sparse_coordinates: BTreeSet::new(),
            display: None,
            live_backup,
            live_stroke: None,
            next_live_ordinal: 1,
            zero_tile: vec![0; TILE_BYTE_LEN],
        })
    }

    #[must_use]
    pub const fn document_size(&self) -> [u32; 2] {
        self.document_size
    }

    #[must_use]
    pub const fn tree(&self) -> &LayerTree {
        &self.tree
    }

    /// Applies a disposable layer-isolation view without mutating the durable
    /// tree used by save/export.
    ///
    /// # Errors
    ///
    /// Returns `UnknownNode` when the requested solo target is absent.
    pub fn set_solo(&mut self, node: Option<LayerTreeNodeId>) -> Result<(), LayerTreeError> {
        if let Some(node) = node
            && node != LayerTreeNodeId::Group(self.tree.root_id())
            && self.tree.ancestors(node).is_none()
        {
            return Err(LayerTreeError::UnknownNode(node));
        }
        if self.solo_node != node {
            self.solo_node = node;
            self.page_cache.clear();
            self.sparse_cache.clear();
        }
        Ok(())
    }

    #[must_use]
    pub fn display_texture(&self) -> Option<wgpu::Texture> {
        self.display.as_ref().map(GpuWorkingTile::texture)
    }

    /// Session chrome only: never changes raster, composite, or export pixels.
    pub fn set_selection_overlay(&mut self, mask: Option<&crate::GpuSelectionMask>) {
        self.selection_overlay
            .replace(&self.device, &self.queue, mask);
    }

    /// Replaces transient document-space guide lines; rejects over 4096 points.
    pub fn set_gesture_preview(&mut self, points: &[[i32; 2]], closed: bool) -> bool {
        self.gesture_preview.set(points, closed)
    }

    /// Synchronous acceptance-only readback; never call from input or present.
    ///
    /// # Errors
    /// Returns GPU copy/mapping errors from the display texture readback.
    pub fn readback_display(
        &self,
    ) -> Result<Option<crate::Rgba8Readback>, crate::GpuReadbackError> {
        self.display
            .as_ref()
            .map(|display| display.readback_rgba8(&self.device, &self.queue))
            .transpose()
    }

    /// Returns the current disposable display projection for bounded probes or
    /// registration with the shell compositor.
    #[must_use]
    pub const fn display(&self) -> Option<&GpuWorkingTile> {
        self.display.as_ref()
    }

    /// # Errors
    ///
    /// Returns `UnknownLayer` when the target is not a raster in this scene or
    /// a layer-tree error if the dirty coordinates cannot be invalidated.
    ///
    /// # Panics
    ///
    /// Panics only if a device-supported document dimension cannot be reduced
    /// to the layer model's signed tile coordinate.
    pub fn apply_dabs(
        &mut self,
        painter: &mut GpuRoundDabPainter,
        layer: LayerId,
        dabs: &[BrushDab],
    ) -> Result<PaintSubmission, LayerUploadError> {
        if self
            .live_stroke
            .as_ref()
            .is_some_and(|stroke| stroke.token.layer == layer)
        {
            return Err(LayerUploadError::LiveStrokeActive(
                self.live_stroke.as_ref().expect("checked above").token,
            ));
        }
        let submission = {
            let surface = self
                .rasters
                .get(&layer)
                .ok_or(LayerUploadError::UnknownLayer(layer))?;
            painter.apply(&surface.tile, dabs)
        };
        if let Some(dirty) = submission.dirty {
            let min_x = dirty.min_x / TILE_EDGE;
            let min_y = dirty.min_y / TILE_EDGE;
            let max_x = (dirty.max_x.saturating_sub(1)) / TILE_EDGE;
            let max_y = (dirty.max_y.saturating_sub(1)) / TILE_EDGE;
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    self.invalidate_raster_tile(
                        layer,
                        TileCoordinate {
                            mip: 0,
                            x: i32::try_from(x).expect("document tile x fits i32"),
                            y: i32::try_from(y).expect("document tile y fits i32"),
                        },
                    )?;
                }
            }
        }
        Ok(submission)
    }

    /// Starts one transactional GPU stroke pinned to `layer`.
    ///
    /// No document-sized copy occurs here. Each affected page region or sparse
    /// atlas tile is copied to renderer-owned backup storage immediately before
    /// its first paint pass.
    ///
    /// # Errors
    ///
    /// Rejects an unknown raster layer, a nested stroke, or exhausted ordinal.
    pub fn begin_live_stroke(
        &mut self,
        layer: LayerId,
    ) -> Result<LiveStrokeToken, LiveStrokeError> {
        if let Some(active) = &self.live_stroke {
            return Err(LiveStrokeError::AlreadyActive(active.token));
        }
        if !self.rasters.contains_key(&layer) {
            return Err(LiveStrokeError::UnknownLayer(layer));
        }
        let ordinal = self.next_live_ordinal;
        self.next_live_ordinal = ordinal
            .checked_add(1)
            .ok_or(LiveStrokeError::OrdinalExhausted)?;
        let token = LiveStrokeToken { ordinal, layer };
        self.live_stroke = Some(ActiveLiveStroke {
            token,
            dirty_tiles: std::collections::BTreeSet::new(),
        });
        Ok(token)
    }

    /// Cancels any active stroke and begins a replacement on `layer`.
    ///
    /// The new layer is validated before the old stroke is restored, so an
    /// unknown replacement target does not disturb the current stroke.
    ///
    /// # Errors
    ///
    /// Rejects an unknown raster layer, exhausted ordinal, or inconsistent
    /// active stroke state.
    pub fn replace_live_stroke(
        &mut self,
        layer: LayerId,
    ) -> Result<LiveStrokeReplacement, LiveStrokeError> {
        if !self.rasters.contains_key(&layer) {
            return Err(LiveStrokeError::UnknownLayer(layer));
        }
        if self.next_live_ordinal == u64::MAX {
            return Err(LiveStrokeError::OrdinalExhausted);
        }
        let cancelled = if let Some(active) = &self.live_stroke {
            Some(self.finish_live_stroke(active.token, LiveStrokeDisposition::Cancel)?)
        } else {
            None
        };
        let active = self.begin_live_stroke(layer)?;
        Ok(LiveStrokeReplacement { cancelled, active })
    }

    /// Applies accepted dabs to the active layer after backing each newly
    /// touched tile. The token prevents stale or cross-layer input batches.
    ///
    /// # Errors
    ///
    /// Rejects a missing active stroke, stale token, or inconsistent layer
    /// tree.
    ///
    /// # Panics
    ///
    /// Panics only if a WGPU-supported document tile coordinate cannot fit the
    /// signed tile-key representation.
    #[allow(clippy::too_many_lines)]
    pub fn apply_live_dabs(
        &mut self,
        painter: &mut GpuRoundDabPainter,
        token: LiveStrokeToken,
        dabs: &[BrushDab],
    ) -> Result<PaintSubmission, LiveStrokeError> {
        self.validate_live_token(token)?;
        let mut batch_keys = signed_dab_keys(token.layer, dabs);
        if let Some([width, height]) = painter.selection.dimensions() {
            let [left, top] = painter.selection.mask_origin().map(i64::from);
            batch_keys.retain(|key| {
                let (x, y) = key.pixel_origin();
                x < left + i64::from(width)
                    && y < top + i64::from(height)
                    && x + i64::from(TILE_EDGE) > left
                    && y + i64::from(TILE_EDGE) > top
            });
        }
        if batch_keys.is_empty() {
            return Ok(PaintSubmission::default());
        }
        let page_prepared = {
            let surface = self
                .rasters
                .get(&token.layer)
                .ok_or(LiveStrokeError::UnknownLayer(token.layer))?;
            GpuRoundDabPainter::prepare_live(&surface.tile, dabs)
        };

        let new_coordinates: Vec<_> = {
            let active = self.live_stroke.as_ref().expect("token validated");
            batch_keys
                .iter()
                .filter_map(|&key| {
                    (!active.dirty_tiles.contains(&key)).then_some((key, key.coordinate()))
                })
                .collect()
        };
        if !new_coordinates.is_empty() {
            let source = &self
                .rasters
                .get(&token.layer)
                .ok_or(LiveStrokeError::UnknownLayer(token.layer))?
                .tile
                .texture;
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("nayati-live-stroke-backup-copy"),
                });
            for (_, coordinate) in &new_coordinates {
                let Some((origin, extent)) = tile_region(*coordinate, self.document_size) else {
                    continue;
                };
                let origin = wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: source,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.live_backup.texture,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: extent[0],
                        height: extent[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.queue.submit([encoder.finish()]);
        }
        self.live_stroke
            .as_mut()
            .expect("token validated")
            .dirty_tiles
            .extend(new_coordinates.iter().map(|(key, _)| *key));

        let mut submission = {
            let surface = self
                .rasters
                .get(&token.layer)
                .ok_or(LiveStrokeError::UnknownLayer(token.layer))?;
            painter.apply_prepared(&surface.tile, &page_prepared)
        };
        let protected = self.protected_sparse_surfaces();
        let document_size = self.document_size;
        for key in batch_keys
            .iter()
            .copied()
            .filter(|key| tile_requires_sparse(key.coordinate(), document_size))
        {
            self.ensure_sparse_raster(key, &protected)
                .map_err(|()| LiveStrokeError::SparseAtlasExhausted)?;
            if new_coordinates
                .iter()
                .any(|(candidate, _)| *candidate == key)
            {
                self.backup_sparse_live_tile(key, &protected)
                    .map_err(|()| LiveStrokeError::SparseAtlasExhausted)?;
            }
            let translated = translated_dabs(key, dabs);
            let sparse_submission = {
                let surface = self
                    .sparse_atlas
                    .surface(SparseSurfaceKey::Raster(key))
                    .expect("sparse raster ensured above");
                let prepared = GpuRoundDabPainter::prepare_live(&surface.tile, &translated);
                let (x, y) = key.pixel_origin();
                painter.apply_prepared_at_origin(&surface.tile, &prepared, [x, y])
            };
            submission.dab_count = submission
                .dab_count
                .saturating_add(sparse_submission.dab_count);
        }
        submission.document_dirty = signed_dirty_for_keys(&batch_keys);
        for key in batch_keys {
            if tile_requires_sparse(key.coordinate(), self.document_size) {
                self.sparse_coordinates.insert(key.coordinate());
            }
            self.invalidate_raster_tile(key.layer, key.coordinate())
                .map_err(|error| match error {
                    LayerUploadError::LayerTree(error) => LiveStrokeError::LayerTree(error),
                    LayerUploadError::UnknownLayer(layer) => LiveStrokeError::UnknownLayer(layer),
                    LayerUploadError::SparseAtlasExhausted => LiveStrokeError::SparseAtlasExhausted,
                    _ => unreachable!("live dirty key is validated"),
                })?;
        }
        Ok(submission)
    }

    /// Commits or cancels the active GPU stroke without any CPU readback.
    ///
    /// # Errors
    ///
    /// Rejects a missing active stroke, stale token, or inconsistent layer
    /// tree.
    ///
    /// # Panics
    ///
    /// Panics only if validated internal active-stroke state disappears or a
    /// previously validated dirty tile leaves the finite document.
    #[allow(clippy::too_many_lines)]
    pub fn finish_live_stroke(
        &mut self,
        token: LiveStrokeToken,
        disposition: LiveStrokeDisposition,
    ) -> Result<LiveStrokeOutcome, LiveStrokeError> {
        self.validate_live_token(token)?;
        let active = self.live_stroke.take().expect("token validated");
        if disposition == LiveStrokeDisposition::Cancel && !active.dirty_tiles.is_empty() {
            let destination = self
                .rasters
                .get(&token.layer)
                .ok_or(LiveStrokeError::UnknownLayer(token.layer))?
                .tile
                .texture
                .clone();
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("nayati-live-stroke-cancel-restore"),
                });
            for key in &active.dirty_tiles {
                let Some((origin, extent)) = tile_region(key.coordinate(), self.document_size)
                else {
                    continue;
                };
                let origin = wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.live_backup.texture,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &destination,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: extent[0],
                        height: extent[1],
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.queue.submit([encoder.finish()]);
            for key in &active.dirty_tiles {
                if tile_requires_sparse(key.coordinate(), self.document_size) {
                    self.restore_sparse_live_tile(*key)
                        .map_err(|()| LiveStrokeError::SparseAtlasExhausted)?;
                    if !self.sparse_closed_tiles.contains_key(key)
                        && !self
                            .sparse_closed_tiles
                            .keys()
                            .any(|candidate| candidate.coordinate() == key.coordinate())
                        && !self
                            .sparse_preview_tiles
                            .iter()
                            .any(|candidate| candidate.coordinate() == key.coordinate())
                    {
                        self.sparse_coordinates.remove(&key.coordinate());
                    }
                }
                self.invalidate_raster_tile(key.layer, key.coordinate())
                    .map_err(|error| match error {
                        LayerUploadError::LayerTree(error) => LiveStrokeError::LayerTree(error),
                        LayerUploadError::UnknownLayer(layer) => {
                            LiveStrokeError::UnknownLayer(layer)
                        }
                        LayerUploadError::SparseAtlasExhausted => {
                            LiveStrokeError::SparseAtlasExhausted
                        }
                        _ => unreachable!("live dirty key is validated"),
                    })?;
            }
        } else if disposition == LiveStrokeDisposition::Commit {
            self.sparse_preview_tiles.extend(
                active
                    .dirty_tiles
                    .iter()
                    .copied()
                    .filter(|key| tile_requires_sparse(key.coordinate(), self.document_size)),
            );
        }
        Ok(LiveStrokeOutcome {
            token,
            disposition,
            dirty_tiles: active.dirty_tiles.into_iter().collect(),
        })
    }

    #[must_use]
    pub fn active_live_stroke(&self) -> Option<LiveStrokeToken> {
        self.live_stroke.as_ref().map(|stroke| stroke.token)
    }

    /// Synchronous diagnostic readback of a resident base-mip raster tile.
    /// Includes sparse boundary padding. Never called by the input/render loop.
    /// Missing/evicted raster tiles return None rather than fabricated pixels.
    ///
    /// # Errors
    /// Returns device-copy or mapping failures from the readback operation.
    pub fn readback_resident_raster_tile(
        &self,
        key: TileKey,
    ) -> Result<Option<crate::Rgba8Readback>, crate::GpuReadbackError> {
        if key.mip != 0 {
            return Ok(None);
        }
        if tile_requires_sparse(key.coordinate(), self.document_size) {
            return self
                .sparse_atlas
                .surface(SparseSurfaceKey::Raster(key))
                .map(|surface| surface.tile.readback_rgba8(&self.device, &self.queue))
                .transpose();
        }
        let Some(surface) = self.rasters.get(&key.layer) else {
            return Ok(None);
        };
        let Some((origin, extent)) = tile_region(key.coordinate(), self.document_size) else {
            return Ok(None);
        };
        let tile = GpuWorkingTile {
            texture: surface.tile.texture.clone(),
            view: surface.tile.view.clone(),
            copy_origin: wgpu::Origin3d {
                x: origin[0],
                y: origin[1],
                z: 0,
            },
            width: extent[0],
            height: extent[1],
        };
        tile.readback_rgba8(&self.device, &self.queue).map(Some)
    }

    /// Diagnostic readback of an already rendered group tile, without page
    /// checkerboard or workspace chrome. Boundary/signed coordinates use the
    /// complete sparse product; fully in-page coordinates use the page product.
    /// Never called by the input/render loop or used as durable artwork.
    /// # Errors
    /// Returns device-copy/mapping failures, or None for a nonresident product.
    pub fn readback_resident_group_tile(
        &self,
        group: GroupId,
        coordinate: TileCoordinate,
    ) -> Result<Option<crate::Rgba8Readback>, crate::GpuReadbackError> {
        if coordinate.mip != 0 {
            return Ok(None);
        }
        if tile_requires_sparse(coordinate, self.document_size) {
            return self
                .sparse_atlas
                .surface(SparseSurfaceKey::Group(CompositeTileKey {
                    group,
                    tile: coordinate,
                }))
                .map(|surface| surface.tile.readback_rgba8(&self.device, &self.queue))
                .transpose();
        }
        let Some(surface) = self.groups.get(&group) else {
            return Ok(None);
        };
        let Some((origin, extent)) = tile_region(coordinate, self.document_size) else {
            return Ok(None);
        };
        GpuWorkingTile {
            texture: surface.tile.texture.clone(),
            view: surface.tile.view.clone(),
            copy_origin: wgpu::Origin3d {
                x: origin[0],
                y: origin[1],
                z: 0,
            },
            width: extent[0],
            height: extent[1],
        }
        .readback_rgba8(&self.device, &self.queue)
        .map(Some)
    }

    fn validate_live_token(&self, token: LiveStrokeToken) -> Result<(), LiveStrokeError> {
        let Some(active) = &self.live_stroke else {
            return Err(LiveStrokeError::NoActiveStroke);
        };
        if active.token != token {
            return Err(LiveStrokeError::StaleToken {
                expected: active.token,
                actual: token,
            });
        }
        Ok(())
    }

    /// Uploads one CPU-authoritative closed tile and invalidates only its
    /// ancestor composites at the same coordinate.
    ///
    /// # Errors
    ///
    /// Rejects an unknown layer, non-base mip, malformed byte length, or a
    /// layer-tree invalidation failure. Signed off-page tiles are retained for
    /// the editor workspace rather than rejected.
    ///
    /// # Panics
    ///
    /// Panics only if a clipped 128px tile offset cannot fit `usize`/`u32` on
    /// a WGPU-supported target.
    #[allow(clippy::too_many_lines)]
    pub fn upload_closed_tile(
        &mut self,
        key: TileKey,
        pixels: &[u8],
    ) -> Result<(), LayerUploadError> {
        if self
            .live_stroke
            .as_ref()
            .is_some_and(|stroke| stroke.token.layer == key.layer)
        {
            return Err(LayerUploadError::LiveStrokeActive(
                self.live_stroke.as_ref().expect("checked above").token,
            ));
        }
        if key.mip != 0 {
            return Err(LayerUploadError::UnsupportedMip(key.mip));
        }
        if pixels.len() != TILE_BYTE_LEN {
            return Err(LayerUploadError::InvalidByteLength {
                expected: TILE_BYTE_LEN,
                actual: pixels.len(),
            });
        }
        let page_texture = self
            .rasters
            .get(&key.layer)
            .ok_or(LayerUploadError::UnknownLayer(key.layer))?
            .tile
            .texture
            .clone();
        let (origin_x, origin_y) = key.pixel_origin();
        let doc_width = i64::from(self.document_size[0]);
        let doc_height = i64::from(self.document_size[1]);
        let destination_x = origin_x.max(0);
        let destination_y = origin_y.max(0);
        let end_x = (origin_x + i64::from(TILE_EDGE)).min(doc_width);
        let end_y = (origin_y + i64::from(TILE_EDGE)).min(doc_height);

        let has_workspace_pixels = origin_x < 0
            || origin_y < 0
            || origin_x + i64::from(TILE_EDGE) > doc_width
            || origin_y + i64::from(TILE_EDGE) > doc_height;
        if has_workspace_pixels {
            self.sparse_preview_tiles.remove(&key);
            if pixels.iter().all(|byte| *byte == 0) {
                self.sparse_closed_tiles.remove(&key);
                if !self
                    .sparse_closed_tiles
                    .keys()
                    .any(|candidate| candidate.coordinate() == key.coordinate())
                    && !self
                        .sparse_preview_tiles
                        .iter()
                        .any(|candidate| candidate.coordinate() == key.coordinate())
                    && !self.live_stroke.as_ref().is_some_and(|stroke| {
                        stroke
                            .dirty_tiles
                            .iter()
                            .any(|candidate| candidate.coordinate() == key.coordinate())
                    })
                {
                    self.sparse_coordinates.remove(&key.coordinate());
                }
            } else {
                self.sparse_closed_tiles
                    .insert(key, pixels.to_vec().into_boxed_slice());
                self.sparse_coordinates.insert(key.coordinate());
            }
            if let Some(surface) = self.sparse_atlas.surface(SparseSurfaceKey::Raster(key)) {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &surface.tile.texture,
                        mip_level: 0,
                        origin: surface.tile.copy_origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(TILE_EDGE * RGBA8_BYTES_PER_PIXEL),
                        rows_per_image: Some(TILE_EDGE),
                    },
                    wgpu::Extent3d {
                        width: TILE_EDGE,
                        height: TILE_EDGE,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        if destination_x < end_x && destination_y < end_y {
            let source_x = usize::try_from(destination_x - origin_x).expect("clipped source x");
            let source_y = usize::try_from(destination_y - origin_y).expect("clipped source y");
            let source_offset = (source_y * usize::try_from(TILE_EDGE).expect("tile edge")
                + source_x)
                * usize::try_from(RGBA8_BYTES_PER_PIXEL).expect("rgba stride");
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &page_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: u32::try_from(destination_x).expect("non-negative destination x"),
                        y: u32::try_from(destination_y).expect("non-negative destination y"),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels[source_offset..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(TILE_EDGE * RGBA8_BYTES_PER_PIXEL),
                    rows_per_image: Some(TILE_EDGE),
                },
                wgpu::Extent3d {
                    width: u32::try_from(end_x - destination_x).expect("clipped width"),
                    height: u32::try_from(end_y - destination_y).expect("clipped height"),
                    depth_or_array_layers: 1,
                },
            );
        }
        self.invalidate_raster_tile(key.layer, key.coordinate())
    }

    /// Uploads a tightly packed full-document raster snapshot.
    ///
    /// # Errors
    ///
    /// Rejects an unknown raster, a byte-length mismatch, or a layer-tree
    /// invalidation failure.
    ///
    /// # Panics
    ///
    /// Panics only if a device-supported document dimension cannot be reduced
    /// to the layer model's signed tile coordinate.
    pub fn upload_layer_rgba8(
        &mut self,
        layer: LayerId,
        pixels: &[u8],
    ) -> Result<(), LayerUploadError> {
        if self
            .live_stroke
            .as_ref()
            .is_some_and(|stroke| stroke.token.layer == layer)
        {
            return Err(LayerUploadError::LiveStrokeActive(
                self.live_stroke.as_ref().expect("checked above").token,
            ));
        }
        let expected =
            rgba8_len(self.document_size).ok_or(LayerUploadError::InvalidByteLength {
                expected: usize::MAX,
                actual: pixels.len(),
            })?;
        if pixels.len() != expected {
            return Err(LayerUploadError::InvalidByteLength {
                expected,
                actual: pixels.len(),
            });
        }
        let surface = self
            .rasters
            .get(&layer)
            .ok_or(LayerUploadError::UnknownLayer(layer))?;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surface.tile.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.document_size[0] * RGBA8_BYTES_PER_PIXEL),
                rows_per_image: Some(self.document_size[1]),
            },
            wgpu::Extent3d {
                width: self.document_size[0],
                height: self.document_size[1],
                depth_or_array_layers: 1,
            },
        );
        let tiles_x = self.document_size[0].div_ceil(TILE_EDGE);
        let tiles_y = self.document_size[1].div_ceil(TILE_EDGE);
        for y in 0..tiles_y {
            for x in 0..tiles_x {
                self.invalidate_raster_tile(
                    layer,
                    TileCoordinate {
                        mip: 0,
                        x: i32::try_from(x).expect("document tile x fits i32"),
                        y: i32::try_from(y).expect("document tile y fits i32"),
                    },
                )?;
            }
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns the document model's validation error for an invalid node.
    pub fn set_visibility(
        &mut self,
        node: LayerTreeNodeId,
        visible: bool,
    ) -> Result<(), LayerTreeError> {
        let invalidation = self.tree.set_visibility(node, visible)?;
        self.page_cache.apply_invalidation(&invalidation);
        self.sparse_cache.apply_invalidation(&invalidation);
        Ok(())
    }

    /// # Errors
    ///
    /// Returns the document model's validation error for an invalid node.
    pub fn set_opacity(
        &mut self,
        node: LayerTreeNodeId,
        opacity_u16: u16,
    ) -> Result<(), LayerTreeError> {
        let invalidation = self.tree.set_opacity(node, opacity_u16)?;
        self.page_cache.apply_invalidation(&invalidation);
        self.sparse_cache.apply_invalidation(&invalidation);
        Ok(())
    }

    /// # Errors
    ///
    /// Returns the document model's validation error for an invalid node/name.
    pub fn rename(&mut self, node: LayerTreeNodeId, name: &str) -> Result<(), LayerTreeError> {
        self.tree.rename(node, name)?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns the document model's failure-atomic reorder validation error.
    pub fn reorder(
        &mut self,
        node: LayerTreeNodeId,
        new_parent: GroupId,
        index: usize,
    ) -> Result<(), LayerTreeError> {
        let invalidation = self.tree.reorder(node, new_parent, index)?;
        self.page_cache.apply_invalidation(&invalidation);
        self.sparse_cache.apply_invalidation(&invalidation);
        Ok(())
    }

    /// Inserts an empty raster/group and allocates its disposable GPU surface.
    /// The document tree remains authoritative; the newly allocated texture is
    /// transparent and can be reconstructed from durable CPU tiles.
    ///
    /// # Errors
    ///
    /// Returns a tree validation error or rejects a page-surface allocation
    /// that would exceed the temporary Sprint 3 scene budget.
    pub fn insert_empty_node(
        &mut self,
        parent: GroupId,
        index: usize,
        node: LayerTreeNode,
    ) -> Result<(), CompositeRenderError> {
        self.validate_empty_node_insert(parent, index, &node)?;
        let node_id = node.id();
        let invalidation = self
            .tree
            .insert(parent, index, node)
            .map_err(CompositeRenderError::LayerTree)?;
        let tile = create_document_texture(
            &self.device,
            &self.queue,
            self.document_size,
            match node_id {
                LayerTreeNodeId::Raster(_) => "nayati-added-raster-layer",
                LayerTreeNodeId::Group(_) => "nayati-added-group-composite",
            },
        );
        let surface = CompositeSurface::new(tile, &self.device, &self.source_layout);
        match node_id {
            LayerTreeNodeId::Raster(id) => {
                self.rasters.insert(id, surface);
            }
            LayerTreeNodeId::Group(id) => {
                self.groups.insert(id, surface);
            }
        }
        self.page_cache.apply_invalidation(&invalidation);
        self.sparse_cache.apply_invalidation(&invalidation);
        Ok(())
    }

    /// Performs the failure checks for [`Self::insert_empty_node`] without
    /// changing renderer or document state.
    ///
    /// # Errors
    ///
    /// Returns the same deterministic validation and allocation errors as the
    /// corresponding insertion.
    pub fn validate_empty_node_insert(
        &self,
        parent: GroupId,
        index: usize,
        node: &LayerTreeNode,
    ) -> Result<(), CompositeRenderError> {
        let surface_count = self
            .rasters
            .len()
            .saturating_add(self.groups.len())
            .saturating_add(2);
        let required_bytes = u64::from(self.document_size[0])
            .checked_mul(u64::from(self.document_size[1]))
            .and_then(|pixels| pixels.checked_mul(u64::from(RGBA8_BYTES_PER_PIXEL)))
            .and_then(|surface_bytes| surface_bytes.checked_mul(u64::try_from(surface_count).ok()?))
            .unwrap_or(u64::MAX)
            .saturating_add(COMPOSITE_SCRATCH_BYTES);
        if required_bytes > MAX_PERSISTENT_SCENE_BYTES {
            return Err(CompositeRenderError::AllocationBudgetExceeded {
                required_bytes,
                max_bytes: MAX_PERSISTENT_SCENE_BYTES,
                surfaces: surface_count,
            });
        }
        let mut candidate = self.tree.clone();
        candidate
            .insert(parent, index, node.clone())
            .map_err(CompositeRenderError::LayerTree)?;
        Ok(())
    }

    fn protected_sparse_surfaces(&self) -> BTreeSet<SparseSurfaceKey> {
        let mut protected: BTreeSet<_> = self
            .sparse_preview_tiles
            .iter()
            .copied()
            .map(SparseSurfaceKey::Raster)
            .collect();
        if let Some(active) = &self.live_stroke {
            protected.extend(
                active
                    .dirty_tiles
                    .iter()
                    .copied()
                    .map(SparseSurfaceKey::Raster),
            );
            protected.extend(
                active
                    .dirty_tiles
                    .iter()
                    .copied()
                    .map(SparseSurfaceKey::LiveBackup),
            );
        }
        protected
    }

    fn handle_sparse_eviction(&mut self, evicted: Option<SparseSurfaceKey>) -> Result<(), ()> {
        match evicted {
            Some(SparseSurfaceKey::Group(key)) => {
                self.sparse_cache.remove(key);
            }
            Some(SparseSurfaceKey::Raster(key)) => {
                let invalidation = self
                    .tree
                    .invalidate_raster_tile(key.layer, key.coordinate())
                    .map_err(|_| ())?;
                // Atlas residency loss does not invalidate an independently
                // completed page texture at the same document coordinate.
                self.sparse_cache.apply_invalidation(&invalidation);
            }
            Some(SparseSurfaceKey::LiveBackup(_)) | None => {}
        }
        Ok(())
    }

    fn backup_sparse_live_tile(
        &mut self,
        key: TileKey,
        protected: &BTreeSet<SparseSurfaceKey>,
    ) -> Result<(), ()> {
        let (_, evicted) = self.sparse_atlas.allocate(
            &self.device,
            &self.source_layout,
            SparseSurfaceKey::LiveBackup(key),
            protected,
        )?;
        self.handle_sparse_eviction(evicted)?;
        let source = self
            .sparse_atlas
            .surface(SparseSurfaceKey::Raster(key))
            .ok_or(())?;
        let source_texture = source.tile.texture.clone();
        let source_origin = source.tile.copy_origin;
        let backup = self
            .sparse_atlas
            .surface(SparseSurfaceKey::LiveBackup(key))
            .ok_or(())?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nayati-sparse-live-stroke-backup-copy"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: source_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &backup.tile.texture,
                mip_level: 0,
                origin: backup.tile.copy_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: TILE_EDGE,
                height: TILE_EDGE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn restore_sparse_live_tile(&self, key: TileKey) -> Result<(), ()> {
        let backup = self
            .sparse_atlas
            .surface(SparseSurfaceKey::LiveBackup(key))
            .ok_or(())?;
        let backup_texture = backup.tile.texture.clone();
        let backup_origin = backup.tile.copy_origin;
        let destination = self
            .sparse_atlas
            .surface(SparseSurfaceKey::Raster(key))
            .ok_or(())?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nayati-sparse-live-stroke-cancel-restore"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &backup_texture,
                mip_level: 0,
                origin: backup_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &destination.tile.texture,
                mip_level: 0,
                origin: destination.tile.copy_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: TILE_EDGE,
                height: TILE_EDGE,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn ensure_sparse_raster(
        &mut self,
        key: TileKey,
        protected: &BTreeSet<SparseSurfaceKey>,
    ) -> Result<(), ()> {
        if self
            .sparse_atlas
            .surface_mut(SparseSurfaceKey::Raster(key))
            .is_some()
        {
            return Ok(());
        }
        let (_, evicted) = self.sparse_atlas.allocate(
            &self.device,
            &self.source_layout,
            SparseSurfaceKey::Raster(key),
            protected,
        )?;
        self.handle_sparse_eviction(evicted)?;
        let pixels = self
            .sparse_closed_tiles
            .get(&key)
            .map_or(self.zero_tile.as_slice(), Box::as_ref);
        let surface = self
            .sparse_atlas
            .surface(SparseSurfaceKey::Raster(key))
            .expect("allocated sparse raster is resident");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &surface.tile.texture,
                mip_level: 0,
                origin: surface.tile.copy_origin,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(TILE_EDGE * RGBA8_BYTES_PER_PIXEL),
                rows_per_image: Some(TILE_EDGE),
            },
            wgpu::Extent3d {
                width: TILE_EDGE,
                height: TILE_EDGE,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    fn child_visible(&self, child: &LayerTreeNode) -> bool {
        self.solo_scope.as_ref().map_or_else(
            || match child {
                LayerTreeNode::Raster(layer) => layer.visible,
                LayerTreeNode::Group(group) => group.visible,
            },
            |scope| scope.contains(&child.id()),
        )
    }
    fn sparse_group_has_content(&self, group: &GroupNode, coordinate: TileCoordinate) -> bool {
        layer_stacks(group).any(|stack| {
            self.child_visible(stack.base)
                && node_opacity(stack.base) != 0
                && self.sparse_node_has_content(stack.base, coordinate)
        })
    }
    fn sparse_node_has_content(&self, node: &LayerTreeNode, coordinate: TileCoordinate) -> bool {
        match node {
            LayerTreeNode::Raster(layer) => {
                let key = TileKey {
                    layer: layer.id,
                    mip: 0,
                    x: coordinate.x,
                    y: coordinate.y,
                };
                self.sparse_closed_tiles.contains_key(&key)
                    || self.sparse_preview_tiles.contains(&key)
                    || self
                        .live_stroke
                        .as_ref()
                        .is_some_and(|stroke| stroke.dirty_tiles.contains(&key))
            }
            LayerTreeNode::Group(group) => self.sparse_group_has_content(group, coordinate),
        }
    }
    fn ensure_sparse_group_tile(
        &mut self,
        group_id: GroupId,
        coordinate: TileCoordinate,
        protected: &BTreeSet<SparseSurfaceKey>,
    ) -> Result<u64, CompositeRenderError> {
        let key = CompositeTileKey {
            group: group_id,
            tile: coordinate,
        };
        if self.sparse_cache.get(key).is_some()
            && self
                .sparse_atlas
                .surface_mut(SparseSurfaceKey::Group(key))
                .is_some()
        {
            return Ok(0);
        }
        let group = find_group(self.tree.root(), group_id)
            .cloned()
            .ok_or(CompositeRenderError::UnknownGroup(group_id))?;
        let mut rebuilt = 1_u64;
        // Finish children before reusing the scratch targets for this parent.
        for stack in layer_stacks(&group) {
            if !self.child_visible(stack.base)
                || node_opacity(stack.base) == 0
                || !self.sparse_node_has_content(stack.base, coordinate)
            {
                continue;
            }
            for child in std::iter::once(stack.base).chain(stack.clips.iter()) {
                if !self.child_visible(child) || !self.sparse_node_has_content(child, coordinate) {
                    continue;
                }
                match child {
                    LayerTreeNode::Raster(layer) => self
                        .ensure_sparse_raster(
                            TileKey {
                                layer: layer.id,
                                mip: 0,
                                x: coordinate.x,
                                y: coordinate.y,
                            },
                            protected,
                        )
                        .map_err(|()| CompositeRenderError::SparseAtlasExhausted)?,
                    LayerTreeNode::Group(group) => {
                        rebuilt = rebuilt.saturating_add(
                            self.ensure_sparse_group_tile(group.id, coordinate, protected)?,
                        );
                    }
                }
            }
        }
        let (_, evicted) = self
            .sparse_atlas
            .allocate(
                &self.device,
                &self.source_layout,
                SparseSurfaceKey::Group(key),
                protected,
            )
            .map_err(|()| CompositeRenderError::SparseAtlasExhausted)?;
        self.handle_sparse_eviction(evicted)
            .map_err(|()| CompositeRenderError::SparseAtlasExhausted)?;
        self.composite_stack_tile(&group, coordinate, true)?;
        self.sparse_cache.insert(key, ());
        Ok(rebuilt)
    }

    /// Rebuilds only missing visible group/tile cache entries, then samples the
    /// root composite through the supplied affine into the display texture.
    ///
    /// # Errors
    ///
    /// Rejects an invalid/mismatched viewport or an inconsistent renderer
    /// scene. The previous document textures and cache remain owned.
    ///
    /// # Panics
    ///
    /// Panics only if the just-created display texture is unexpectedly absent.
    #[allow(clippy::too_many_lines)]
    pub fn render_viewport(
        &mut self,
        viewport: ViewportTransform,
    ) -> Result<CompositeRenderStats, CompositeRenderError> {
        viewport
            .validate()
            .map_err(CompositeRenderError::InvalidViewport)?;
        self.ensure_display(viewport.physical_size);

        self.solo_scope = self
            .solo_node
            .map(|node| composition_scope_nodes(self.tree.root(), CompositionScope::Solo(node)));
        let mut stats = CompositeRenderStats::default();
        for coordinate in visible_tile_coordinates(viewport, self.document_size) {
            stats.group_tiles_rebuilt = stats
                .group_tiles_rebuilt
                .saturating_add(self.ensure_group_tile(self.tree.root_id(), coordinate)?);
        }
        let sparse_coordinates =
            visible_sparse_coordinates(viewport, self.sparse_coordinates.iter().copied());
        let mut protected = self.protected_sparse_surfaces();
        let mut raster_ids = Vec::new();
        let mut group_ids = vec![self.tree.root_id()];
        collect_node_ids(self.tree.root(), &mut raster_ids, &mut group_ids);
        for coordinate in &sparse_coordinates {
            protected.extend(group_ids.iter().copied().map(|group| {
                SparseSurfaceKey::Group(CompositeTileKey {
                    group,
                    tile: *coordinate,
                })
            }));
            for layer in &raster_ids {
                let key = TileKey {
                    layer: *layer,
                    mip: 0,
                    x: coordinate.x,
                    y: coordinate.y,
                };
                if self.sparse_closed_tiles.contains_key(&key)
                    || self.sparse_preview_tiles.contains(&key)
                    || self
                        .live_stroke
                        .as_ref()
                        .is_some_and(|stroke| stroke.dirty_tiles.contains(&key))
                {
                    protected.insert(SparseSurfaceKey::Raster(key));
                }
            }
        }
        for coordinate in &sparse_coordinates {
            stats.group_tiles_rebuilt =
                stats
                    .group_tiles_rebuilt
                    .saturating_add(self.ensure_sparse_group_tile(
                        self.tree.root_id(),
                        *coordinate,
                        &protected,
                    )?);
        }
        self.write_viewport_uniform(viewport)?;
        let affine = viewport_uniform_values(viewport, self.document_size)?;
        self.workspace_params
            .ensure_capacity(&self.device, sparse_coordinates.len());
        self.workspace_params
            .write(&self.queue, affine, &sparse_coordinates);

        let display = self.display.as_ref().expect("display ensured above");
        let root = self
            .groups
            .get(&self.tree.root_id())
            .ok_or(CompositeRenderError::UnknownGroup(self.tree.root_id()))?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nayati-viewport-display"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nayati-viewport-display-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &display.view,
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
            pass.set_pipeline(&self.viewport_pipeline);
            pass.set_bind_group(0, &root.source_bind_group, &[]);
            pass.set_bind_group(1, &self.viewport_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        if !sparse_coordinates.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nayati-workspace-tile-display-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &display.view,
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
            pass.set_pipeline(&self.workspace_pipeline);
            for (index, coordinate) in sparse_coordinates.iter().enumerate() {
                let root = self
                    .sparse_atlas
                    .surface(SparseSurfaceKey::Group(CompositeTileKey {
                        group: self.tree.root_id(),
                        tile: *coordinate,
                    }))
                    .ok_or(CompositeRenderError::SparseAtlasExhausted)?;
                pass.set_bind_group(0, &root.source_bind_group, &[]);
                pass.set_bind_group(
                    1,
                    &self.workspace_params.bind_group,
                    &[self.workspace_params.dynamic_offset(index)?],
                );
                pass.draw(0..3, 0..1);
            }
            stats.display_passes = stats.display_passes.saturating_add(1);
        }
        if self.selection_overlay.dimensions().is_some() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nyatidraw-selection-overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &display.view,
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
            pass.set_pipeline(&self.selection_pipeline);
            pass.set_bind_group(0, &self.selection_overlay.group, &[]);
            pass.set_bind_group(1, &self.viewport_bind_group, &[]);
            pass.draw(0..3, 0..1);
            stats.display_passes = stats.display_passes.saturating_add(1);
        }
        if self
            .gesture_preview
            .render(&self.queue, &mut encoder, &display.view, viewport)
        {
            stats.display_passes = stats.display_passes.saturating_add(1);
        }
        self.queue.submit([encoder.finish()]);
        stats.display_passes = stats.display_passes.saturating_add(1);
        stats.cache_entries = self
            .page_cache
            .len()
            .saturating_add(self.sparse_cache.len());
        Ok(stats)
    }

    fn invalidate_raster_tile(
        &mut self,
        layer: LayerId,
        coordinate: TileCoordinate,
    ) -> Result<(), LayerUploadError> {
        let invalidation = self
            .tree
            .invalidate_raster_tile(layer, coordinate)
            .map_err(LayerUploadError::LayerTree)?;
        self.page_cache.apply_invalidation(&invalidation);
        self.sparse_cache.apply_invalidation(&invalidation);
        Ok(())
    }

    fn ensure_display(&mut self, size: [u32; 2]) {
        let changed = self
            .display
            .as_ref()
            .is_none_or(|display| display.width() != size[0] || display.height() != size[1]);
        if changed {
            self.display = Some(create_document_texture(
                &self.device,
                &self.queue,
                size,
                "nayati-viewport-display",
            ));
        }
    }

    fn ensure_group_tile(
        &mut self,
        group_id: GroupId,
        coordinate: TileCoordinate,
    ) -> Result<u64, CompositeRenderError> {
        let key = CompositeTileKey {
            group: group_id,
            tile: coordinate,
        };
        if self.page_cache.get(key).is_some() {
            return Ok(0);
        }
        let group = find_group(self.tree.root(), group_id)
            .cloned()
            .ok_or(CompositeRenderError::UnknownGroup(group_id))?;
        let mut rebuilt = 1_u64;
        for stack in layer_stacks(&group) {
            if !self.child_visible(stack.base) || node_opacity(stack.base) == 0 {
                continue;
            }
            for child in std::iter::once(stack.base).chain(stack.clips.iter()) {
                if self.child_visible(child)
                    && let LayerTreeNode::Group(group) = child
                {
                    rebuilt = rebuilt.saturating_add(self.ensure_group_tile(group.id, coordinate)?);
                }
            }
        }
        self.composite_stack_tile(&group, coordinate, false)?;
        self.page_cache.insert(key, ());
        Ok(rebuilt)
    }
    fn stack_input(
        &self,
        node: &LayerTreeNode,
        coordinate: TileCoordinate,
        sparse: bool,
    ) -> StackInput {
        if !sparse {
            return StackInput::Page(node.id());
        }
        let key = match node {
            LayerTreeNode::Raster(layer) => SparseSurfaceKey::Raster(TileKey {
                layer: layer.id,
                mip: 0,
                x: coordinate.x,
                y: coordinate.y,
            }),
            LayerTreeNode::Group(group) => SparseSurfaceKey::Group(CompositeTileKey {
                group: group.id,
                tile: coordinate,
            }),
        };
        if self.sparse_atlas.surface(key).is_some()
            && self.sparse_node_has_content(node, coordinate)
        {
            StackInput::Sparse(key)
        } else {
            StackInput::Transparent
        }
    }
    fn stack_source(&self, input: StackInput) -> Result<&wgpu::BindGroup, CompositeRenderError> {
        match input {
            StackInput::Page(LayerTreeNodeId::Raster(id)) => self
                .rasters
                .get(&id)
                .map(|surface| &surface.source_bind_group)
                .ok_or(CompositeRenderError::MissingRaster(id)),
            StackInput::Page(LayerTreeNodeId::Group(id)) => self
                .groups
                .get(&id)
                .map(|surface| &surface.source_bind_group)
                .ok_or(CompositeRenderError::UnknownGroup(id)),
            StackInput::Sparse(key) => self
                .sparse_atlas
                .surface(key)
                .map(|surface| &surface.source_bind_group)
                .ok_or(CompositeRenderError::SparseAtlasExhausted),
            StackInput::Scratch(index) => Ok(&self.composite_scratch[index].source_bind_group),
            StackInput::Transparent => Ok(&self.composite_scratch[0].source_bind_group),
        }
    }
    #[allow(clippy::too_many_lines)]
    fn composite_stack_tile(
        &mut self,
        group: &GroupNode,
        coordinate: TileCoordinate,
        sparse: bool,
    ) -> Result<(), CompositeRenderError> {
        let origin = if sparse {
            [0, 0]
        } else {
            tile_region(coordinate, self.document_size)
                .ok_or(CompositeRenderError::InvalidDocumentSize)?
                .0
        };
        let mut passes = Vec::new();
        let mut accumulated = 0;
        // Membership is resolved on original siblings before visibility.
        for stack in layer_stacks(group) {
            if !self.child_visible(stack.base) || node_opacity(stack.base) == 0 {
                continue;
            }
            let input = self.stack_input(stack.base, coordinate, sparse);
            if matches!(input, StackInput::Transparent) {
                continue;
            }
            passes.push(StackPass {
                input,
                backdrop: 3,
                target: 2,
                params: [65535, 4, origin[0], origin[1]],
            });
            let mut current_stack = 2;
            for clip in stack.clips {
                if !self.child_visible(clip) || node_opacity(clip) == 0 {
                    continue;
                }
                let input = self.stack_input(clip, coordinate, sparse);
                if matches!(input, StackInput::Transparent) {
                    continue;
                }
                let next = 5 - current_stack;
                passes.push(StackPass {
                    input,
                    backdrop: current_stack,
                    target: next,
                    params: [
                        u32::from(node_opacity(clip)),
                        blend_operation(clip) + 2,
                        origin[0],
                        origin[1],
                    ],
                });
                current_stack = next;
            }
            let next = 1 - accumulated;
            passes.push(StackPass {
                input: StackInput::Scratch(current_stack),
                backdrop: accumulated,
                target: next,
                params: [
                    u32::from(node_opacity(stack.base)),
                    blend_operation(stack.base),
                    0,
                    0,
                ],
            });
            accumulated = next;
        }
        self.opacity
            .ensure_capacity(&self.device, &self.opacity_layout, passes.len());
        self.opacity.write(
            &self.queue,
            &passes.iter().map(|pass| pass.params).collect::<Vec<_>>(),
        );
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nyatidraw-isolated-clipping-stack-tile"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nyatidraw-stack-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.composite_scratch[0].tile.view,
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
        for (index, operation) in passes.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nyatidraw-stack-integer-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.composite_scratch[operation.target].tile.view,
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
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, self.stack_source(operation.input)?, &[]);
            pass.set_bind_group(
                1,
                &self.composite_scratch[operation.backdrop].source_bind_group,
                &[],
            );
            pass.set_bind_group(
                2,
                &self.opacity.bind_group,
                &[self.opacity.dynamic_offset(index)?],
            );
            pass.draw(0..3, 0..1);
        }
        let (target, destination, extent) = if sparse {
            let target = self
                .sparse_atlas
                .surface(SparseSurfaceKey::Group(CompositeTileKey {
                    group: group.id,
                    tile: coordinate,
                }))
                .ok_or(CompositeRenderError::SparseAtlasExhausted)?;
            (
                &target.tile.texture,
                target.tile.copy_origin,
                [TILE_EDGE; 2],
            )
        } else {
            let surface = self
                .groups
                .get(&group.id)
                .ok_or(CompositeRenderError::UnknownGroup(group.id))?;
            let (origin, extent) = tile_region(coordinate, self.document_size)
                .ok_or(CompositeRenderError::InvalidDocumentSize)?;
            (
                &surface.tile.texture,
                wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                extent,
            )
        };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.composite_scratch[accumulated].tile.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: destination,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: extent[0],
                height: extent[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn write_viewport_uniform(
        &self,
        viewport: ViewportTransform,
    ) -> Result<(), CompositeRenderError> {
        if viewport.physical_size
            != self
                .display
                .as_ref()
                .map_or([0, 0], |display| [display.width(), display.height()])
        {
            return Err(CompositeRenderError::ViewportSizeMismatch);
        }
        let values = viewport_uniform_values(viewport, self.document_size)?;
        self.queue
            .write_buffer(&self.viewport_uniform, 0, &encode_f32s(&values));
        Ok(())
    }
}

fn create_source_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nayati-composite-source-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

fn create_opacity_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nayati-composite-opacity-layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: NonZeroU64::new(UNIFORM_VALUE_BYTES as u64),
            },
            count: None,
        }],
    })
}

fn create_composite_pipeline(
    device: &wgpu::Device,
    source_layout: &wgpu::BindGroupLayout,
    opacity_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("nayati-composite-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("stack_composite.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("nayati-composite-pipeline-layout"),
        bind_group_layouts: &[source_layout, source_layout, opacity_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("nayati-composite-pipeline"),
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
    })
}

fn create_viewport_binding(
    device: &wgpu::Device,
    uniform: &wgpu::Buffer,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nayati-viewport-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(48),
                },
                count: None,
            },
        ],
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("nayati-viewport-sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("nayati-viewport-bind-group"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    (layout, bind_group)
}

fn create_viewport_pipeline(
    device: &wgpu::Device,
    source_layout: &wgpu::BindGroupLayout,
    viewport_layout: &wgpu::BindGroupLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("nayati-viewport-shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("nayati-viewport-pipeline-layout"),
        bind_group_layouts: &[source_layout, viewport_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("nayati-viewport-pipeline"),
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
    })
}

fn create_workspace_pipeline(
    device: &wgpu::Device,
    source_layout: &wgpu::BindGroupLayout,
    workspace_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("nayati-workspace-tile-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("workspace_tiles.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("nayati-workspace-tile-pipeline-layout"),
        bind_group_layouts: &[source_layout, workspace_layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("nayati-workspace-tile-pipeline"),
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
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn create_document_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [u32; 2],
    label: &'static str,
) -> GpuWorkingTile {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
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
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("nayati-document-texture-init"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nayati-document-texture-clear"),
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
    queue.submit([encoder.finish()]);
    GpuWorkingTile {
        texture,
        view,
        copy_origin: wgpu::Origin3d::ZERO,
        width: size[0],
        height: size[1],
    }
}

fn create_uniform_buffer(
    device: &wgpu::Device,
    stride: usize,
    capacity: usize,
    label: &'static str,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: u64::try_from(stride.saturating_mul(capacity)).expect("uniform buffer size fits u64"),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(UNIFORM_VALUE_BYTES as u64),
            }),
        }],
    })
}

fn create_workspace_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    label: &'static str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: NonZeroU64::new(WORKSPACE_TILE_UNIFORM_BYTES as u64),
            }),
        }],
    })
}

fn collect_node_ids(group: &GroupNode, rasters: &mut Vec<LayerId>, groups: &mut Vec<GroupId>) {
    for child in &group.children {
        match child {
            LayerTreeNode::Raster(layer) => rasters.push(layer.id),
            LayerTreeNode::Group(child_group) => {
                groups.push(child_group.id);
                collect_node_ids(child_group, rasters, groups);
            }
        }
    }
}

fn find_group(group: &GroupNode, id: GroupId) -> Option<&GroupNode> {
    if group.id == id {
        return Some(group);
    }
    group.children.iter().find_map(|child| match child {
        LayerTreeNode::Raster(_) => None,
        LayerTreeNode::Group(child_group) => find_group(child_group, id),
    })
}

fn tile_region(
    coordinate: TileCoordinate,
    document_size: [u32; 2],
) -> Option<([u32; 2], [u32; 2])> {
    if coordinate.mip != 0 || coordinate.x < 0 || coordinate.y < 0 {
        return None;
    }
    let x = u32::try_from(coordinate.x).ok()?.checked_mul(TILE_EDGE)?;
    let y = u32::try_from(coordinate.y).ok()?.checked_mul(TILE_EDGE)?;
    if x >= document_size[0] || y >= document_size[1] {
        return None;
    }
    Some((
        [x, y],
        [
            TILE_EDGE.min(document_size[0] - x),
            TILE_EDGE.min(document_size[1] - y),
        ],
    ))
}

fn visible_tile_coordinates(
    viewport: ViewportTransform,
    document_size: [u32; 2],
) -> Vec<TileCoordinate> {
    let origin = viewport.window_origin_physical;
    let width = f64::from(viewport.physical_size[0]);
    let height = f64::from(viewport.physical_size[1]);
    let corners = [
        origin,
        Point {
            x: origin.x + width,
            y: origin.y,
        },
        Point {
            x: origin.x,
            y: origin.y + height,
        },
        Point {
            x: origin.x + width,
            y: origin.y + height,
        },
    ];
    let mapped: Vec<_> = corners
        .into_iter()
        .filter_map(|point| viewport.window_to_document(point))
        .collect();
    let min_x = mapped
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min)
        .max(0.0);
    let min_y = mapped
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min)
        .max(0.0);
    let max_x = mapped
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max)
        .min(f64::from(document_size[0]));
    let max_y = mapped
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max)
        .min(f64::from(document_size[1]));
    if min_x >= max_x || min_y >= max_y {
        return Vec::new();
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bounds = (
        (min_x.floor() as u32) / TILE_EDGE,
        (min_y.floor() as u32) / TILE_EDGE,
        ((max_x.ceil() as u32).saturating_sub(1)) / TILE_EDGE,
        ((max_y.ceil() as u32).saturating_sub(1)) / TILE_EDGE,
    );
    let mut coordinates = Vec::new();
    for y in bounds.1..=bounds.3 {
        for x in bounds.0..=bounds.2 {
            coordinates.push(TileCoordinate {
                mip: 0,
                x: i32::try_from(x).expect("document tile x fits i32"),
                y: i32::try_from(y).expect("document tile y fits i32"),
            });
        }
    }
    coordinates
}

fn visible_sparse_coordinates(
    viewport: ViewportTransform,
    coordinates: impl IntoIterator<Item = TileCoordinate>,
) -> Vec<TileCoordinate> {
    let origin = viewport.window_origin_physical;
    let width = f64::from(viewport.physical_size[0]);
    let height = f64::from(viewport.physical_size[1]);
    let corners = [
        origin,
        Point {
            x: origin.x + width,
            y: origin.y,
        },
        Point {
            x: origin.x,
            y: origin.y + height,
        },
        Point {
            x: origin.x + width,
            y: origin.y + height,
        },
    ];
    let mapped: Vec<_> = corners
        .into_iter()
        .filter_map(|point| viewport.window_to_document(point))
        .collect();
    let min_x = mapped
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = mapped
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = mapped
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = mapped
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_tile_x = tile_coordinate_from_document(min_x);
    let min_tile_y = tile_coordinate_from_document(min_y);
    let max_tile_x = tile_coordinate_from_document(max_x - f64::EPSILON);
    let max_tile_y = tile_coordinate_from_document(max_y - f64::EPSILON);
    coordinates
        .into_iter()
        .filter(|coordinate| {
            coordinate.mip == 0
                && (min_tile_x..=max_tile_x).contains(&coordinate.x)
                && (min_tile_y..=max_tile_y).contains(&coordinate.y)
        })
        .collect()
}

fn signed_dab_keys(layer: LayerId, dabs: &[BrushDab]) -> Vec<TileKey> {
    let mut keys = BTreeSet::new();
    for dab in dabs {
        let radius = f64::from(dab.radius_px);
        let opacity = (dab.opacity.clamp(0.0, 1.0) * dab.flow.clamp(0.0, 1.0)).clamp(0.0, 1.0);
        if !dab.center.x.is_finite()
            || !dab.center.y.is_finite()
            || !radius.is_finite()
            || radius <= 0.0
            || opacity <= 0.0
        {
            continue;
        }
        let min_x = tile_coordinate_from_document((dab.center.x - radius).floor());
        let min_y = tile_coordinate_from_document((dab.center.y - radius).floor());
        let max_x = tile_coordinate_from_document((dab.center.x + radius).ceil() - 1.0);
        let max_y = tile_coordinate_from_document((dab.center.y + radius).ceil() - 1.0);
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                keys.insert(TileKey {
                    layer,
                    mip: 0,
                    x,
                    y,
                });
            }
        }
    }
    keys.into_iter().collect()
}

#[allow(clippy::cast_possible_truncation)]
fn tile_coordinate_from_document(value: f64) -> i32 {
    (value / f64::from(TILE_EDGE))
        .floor()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

#[allow(clippy::cast_precision_loss)]
fn translated_dabs(key: TileKey, dabs: &[BrushDab]) -> Vec<BrushDab> {
    let (origin_x, origin_y) = key.pixel_origin();
    dabs.iter()
        .copied()
        .map(|mut dab| {
            dab.center.x -= origin_x as f64;
            dab.center.y -= origin_y as f64;
            dab
        })
        .collect()
}

fn signed_dirty_for_keys(keys: &[TileKey]) -> Option<crate::SignedDirtyRect> {
    keys.iter().copied().fold(None, |bounds, key| {
        let (min_x, min_y) = key.pixel_origin();
        let tile = crate::SignedDirtyRect {
            min_x,
            min_y,
            max_x: min_x + i64::from(TILE_EDGE),
            max_y: min_y + i64::from(TILE_EDGE),
        };
        Some(bounds.map_or(tile, |bounds: crate::SignedDirtyRect| bounds.union(tile)))
    })
}

fn tile_requires_sparse(coordinate: TileCoordinate, document_size: [u32; 2]) -> bool {
    let origin_x = i64::from(coordinate.x) * i64::from(TILE_EDGE);
    let origin_y = i64::from(coordinate.y) * i64::from(TILE_EDGE);
    origin_x < 0
        || origin_y < 0
        || origin_x + i64::from(TILE_EDGE) > i64::from(document_size[0])
        || origin_y + i64::from(TILE_EDGE) > i64::from(document_size[1])
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn viewport_uniform_values(
    viewport: ViewportTransform,
    document_size: [u32; 2],
) -> Result<[f32; 12], CompositeRenderError> {
    let origin = viewport.window_origin_physical;
    let p0 = viewport
        .window_to_document(origin)
        .ok_or(CompositeRenderError::InvalidViewport(
            ViewportValidationError::NonFinite,
        ))?;
    let px = viewport
        .window_to_document(Point {
            x: origin.x + 1.0,
            y: origin.y,
        })
        .ok_or(CompositeRenderError::InvalidViewport(
            ViewportValidationError::NonFinite,
        ))?;
    let py = viewport
        .window_to_document(Point {
            x: origin.x,
            y: origin.y + 1.0,
        })
        .ok_or(CompositeRenderError::InvalidViewport(
            ViewportValidationError::NonFinite,
        ))?;
    Ok([
        (px.x - p0.x) as f32,
        (py.x - p0.x) as f32,
        p0.x as f32,
        0.0,
        (px.y - p0.y) as f32,
        (py.y - p0.y) as f32,
        p0.y as f32,
        0.0,
        document_size[0] as f32,
        document_size[1] as f32,
        0.0,
        0.0,
    ])
}

fn rgba8_len(size: [u32; 2]) -> Option<usize> {
    usize::try_from(size[0])
        .ok()?
        .checked_mul(usize::try_from(size[1]).ok()?)?
        .checked_mul(usize::try_from(RGBA8_BYTES_PER_PIXEL).ok()?)
}

fn encode_f32s(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len().saturating_mul(size_of::<f32>()));
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

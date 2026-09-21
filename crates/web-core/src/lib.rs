#![forbid(unsafe_code)]

//! Bounded, CPU-authoritative drawing session shared by browser hosts.
//! No filesystem, browser, UI, GPU, or desktop project-store dependencies.

use std::collections::{BTreeSet, VecDeque};

pub use nyatidraw_api::{CanvasSpec, LayerBlendMode, LayerId};
use nyatidraw_api::{ContentRootId, GroupId, LayerTreeNodeId};
pub use nyatidraw_brush::BrushPreset;
use nyatidraw_brush::{
    BrushPresetId, PencilKind, ROUND_BRUSH_ENGINE_VERSION, ROUND_BRUSH_PRESET_SCHEMA_VERSION,
    RoundBrushEvaluator, pencil_preset,
};
use nyatidraw_document::{GroupNode, LayerTreeError};
pub use nyatidraw_document::{LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_paint_cpu::flatten_layer_tree_rgba8;
use nyatidraw_tiles::color::{linear_premultiplied_to_srgb8, srgb8_to_linear_premultiplied};
pub use nyatidraw_tiles::{TILE_BYTE_LEN, TILE_EDGE, TileKey, TileSnapshot};

mod codec;
mod drawing;
mod interchange;
pub use interchange::WebPreferences;
#[cfg(test)]
mod tests;

pub const MAX_CANVAS_EDGE: u32 = 4096;
pub const MAX_RESIDENT_TILES: usize = 1024;
pub const MAX_HISTORY_ENTRIES: usize = 128;
pub const MAX_HISTORY_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_PORTABLE_BYTES: usize = 68 * 1024 * 1024;
pub const MAX_LAYERS: usize = 32;
pub const PORTABLE_MIME: &str = "application/vnd.nyatidraw.web-document";
const MAX_COORDINATE: f64 = 32768.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum WebTool {
    #[default]
    Pencil2H,
    Pencil2B,
    Pen,
    SoftBrush,
    Eraser,
}

impl WebTool {
    pub const ALL: [Self; 5] = [
        Self::Pencil2H,
        Self::Pencil2B,
        Self::Pen,
        Self::SoftBrush,
        Self::Eraser,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pencil2H => "2H 샤프",
            Self::Pencil2B => "2B 연필",
            Self::Pen => "펜",
            Self::SoftBrush => "소프트 브러시",
            Self::Eraser => "지우개",
        }
    }

    fn preset(self) -> BrushPreset {
        match self {
            Self::Pencil2H => pencil_preset(PencilKind::Mechanical2H),
            Self::Pencil2B => pencil_preset(PencilKind::Graphite2B),
            _ => BrushPreset {
                id: BrushPresetId(16 + self as u128),
                schema_version: ROUND_BRUSH_PRESET_SCHEMA_VERSION,
                engine_version: ROUND_BRUSH_ENGINE_VERSION,
                size_px: if self == Self::SoftBrush { 24.0 } else { 5.0 },
                opacity: 1.0,
                flow: if self == Self::SoftBrush { 0.15 } else { 1.0 },
                spacing_ratio: 0.15,
                size_pressure: true,
                opacity_pressure: false,
                size_min_ratio: 0.1,
                opacity_min_ratio: 0.0,
                hardness: if self == Self::SoftBrush { 0.0 } else { 1.0 },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushSettings {
    pub size_px: f32,
    pub opacity: f32,
    pub hardness: f32,
    pub size_pressure: bool,
    pub opacity_pressure: bool,
    pub size_minimum_u16: u16,
    pub opacity_minimum_u16: u16,
    pub smoothing: u8,
}

impl BrushSettings {
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn for_tool(tool: WebTool) -> Self {
        let preset = tool.preset();
        Self {
            size_px: preset.size_px,
            opacity: preset.opacity,
            hardness: preset.hardness,
            size_pressure: preset.size_pressure,
            opacity_pressure: preset.opacity_pressure,
            size_minimum_u16: (preset.size_min_ratio * f32::from(u16::MAX)).round() as u16,
            opacity_minimum_u16: (preset.opacity_min_ratio * f32::from(u16::MAX)).round() as u16,
            smoothing: 0,
        }
    }

    fn validate(self) -> Result<Self, WebError> {
        if !self.size_px.is_finite()
            || !(0.1..=200.0).contains(&self.size_px)
            || !self.opacity.is_finite()
            || !(0.0..=1.0).contains(&self.opacity)
            || !self.hardness.is_finite()
            || !(0.0..=1.0).contains(&self.hardness)
            || self.smoothing > 100
        {
            return Err(WebError::InvalidBrush);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokePoint {
    pub x: f64,
    pub y: f64,
    pub pressure: f32,
    pub time_ms: f64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CanvasUpdate {
    /// Includes removed keys: a missing snapshot entry means erase that GPU tile.
    pub dirty_tiles: Vec<TileKey>,
    pub changed: bool,
    /// A completed artwork edit, not a transient brush preview or tool setting.
    pub committed: bool,
    /// Layer structure/properties or page dimensions changed; rebuild composition.
    pub structure_changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebError {
    InvalidCanvas,
    InvalidInput,
    InvalidBrush,
    StrokeInProgress,
    NoStroke,
    LayerUnavailable,
    InvalidLayer,
    LimitExceeded,
    InvalidPixels,
    InvalidPortable,
    UnsupportedVersion,
}

impl std::fmt::Display for WebError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCanvas => "Canvas dimensions must be between 1 and 4096 pixels",
            Self::InvalidInput => "Invalid or out-of-range drawing input; stroke cancelled",
            Self::InvalidBrush => "Invalid brush setting",
            Self::StrokeInProgress => "Finish or cancel the current stroke first",
            Self::NoStroke => "No active stroke",
            Self::LayerUnavailable => "The active layer is hidden or locked",
            Self::InvalidLayer => "Invalid layer operation",
            Self::LimitExceeded => "Browser drawing resource limit reached",
            Self::InvalidPixels => "Invalid image pixels",
            Self::InvalidPortable => "Invalid or damaged browser project",
            Self::UnsupportedVersion => "Unsupported browser project version",
        })
    }
}

impl std::error::Error for WebError {}

impl From<LayerTreeError> for WebError {
    fn from(_: LayerTreeError) -> Self {
        Self::InvalidLayer
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Artwork {
    canvas: CanvasSpec,
    tiles: TileSnapshot,
    layers: LayerTree,
    active: LayerId,
}

/// Tool preferences are outside artwork history. Undo never changes brush/color.
pub struct WebDocument {
    artwork: Artwork,
    undo: VecDeque<Artwork>,
    redo: Vec<Artwork>,
    tool: WebTool,
    settings: [BrushSettings; 5],
    foreground: [u8; 4],
    background: [u8; 4],
    evaluator: RoundBrushEvaluator,
    stroke: Option<drawing::ActiveStroke>,
    next_layer_id: u128,
}

impl Default for WebDocument {
    fn default() -> Self {
        Self::new(CanvasSpec {
            width_px: 1280,
            height_px: 720,
            pixels_per_inch: 96,
        })
        .expect("default web canvas is valid")
    }
}

impl WebDocument {
    /// Creates one transparent raster layer. No background or sample artwork.
    /// # Errors
    /// Rejects zero/oversized dimensions or invalid resolution.
    pub fn new(canvas: CanvasSpec) -> Result<Self, WebError> {
        validate_canvas(canvas)?;
        Ok(Self {
            artwork: Artwork {
                canvas,
                tiles: TileSnapshot::empty(),
                layers: LayerTree::new(GroupNode {
                    id: GroupId(0),
                    name: "Root".into(),
                    visible: true,
                    opacity_u16: u16::MAX,
                    clip_to_below: false,
                    blend_mode: LayerBlendMode::Normal,
                    children: vec![LayerTreeNode::Raster(new_layer(LayerId(1), "Layer 1"))],
                })?,
                active: LayerId(1),
            },
            undo: VecDeque::new(),
            redo: Vec::new(),
            tool: WebTool::default(),
            settings: WebTool::ALL.map(BrushSettings::for_tool),
            foreground: [0, 0, 0, 255],
            background: [255; 4],
            evaluator: RoundBrushEvaluator::default(),
            stroke: None,
            next_layer_id: 2,
        })
    }

    #[must_use]
    pub const fn canvas(&self) -> CanvasSpec {
        self.artwork.canvas
    }
    #[must_use]
    pub const fn snapshot(&self) -> &TileSnapshot {
        &self.artwork.tiles
    }
    #[must_use]
    pub const fn layers(&self) -> &LayerTree {
        &self.artwork.layers
    }
    #[must_use]
    pub const fn active_layer(&self) -> LayerId {
        self.artwork.active
    }
    #[must_use]
    pub const fn tool(&self) -> WebTool {
        self.tool
    }
    #[must_use]
    pub const fn foreground(&self) -> [u8; 4] {
        self.foreground
    }
    #[must_use]
    pub const fn background(&self) -> [u8; 4] {
        self.background
    }
    #[must_use]
    pub const fn brush_settings(&self) -> BrushSettings {
        self.settings[self.tool as usize]
    }
    #[must_use]
    pub fn is_drawing(&self) -> bool {
        self.stroke.is_some()
    }
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.stroke.is_none() && !self.undo.is_empty()
    }
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.stroke.is_none() && !self.redo.is_empty()
    }
    #[must_use]
    pub fn history_len(&self) -> usize {
        self.undo.len() + self.redo.len()
    }

    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// # Errors
    /// Tool changes during a stroke are rejected; temporary host tools stay host-owned.
    pub fn select_tool(&mut self, tool: WebTool) -> Result<(), WebError> {
        self.require_idle()?;
        self.tool = tool;
        Ok(())
    }

    /// # Errors
    /// Rejects invalid settings or changing the captured preset mid-stroke.
    pub fn set_brush_settings(&mut self, mut settings: BrushSettings) -> Result<(), WebError> {
        self.require_idle()?;
        settings.validate()?;
        if matches!(self.tool, WebTool::Pencil2H | WebTool::Pencil2B) {
            settings.hardness = 1.0;
        }
        self.settings[self.tool as usize] = settings;
        Ok(())
    }

    pub fn set_foreground(&mut self, color: [u8; 4]) {
        self.foreground = color;
    }
    pub fn set_background(&mut self, color: [u8; 4]) {
        self.background = color;
    }

    /// # Errors
    /// Rejects unknown layers or changing the drawing target mid-stroke.
    pub fn set_active_layer(&mut self, layer: LayerId) -> Result<(), WebError> {
        self.require_idle()?;
        self.artwork
            .layers
            .raster(layer)
            .ok_or(WebError::InvalidLayer)?;
        self.artwork.active = layer;
        Ok(())
    }

    /// Adds a raster above the current stack and selects it.
    /// # Errors
    /// Rejects invalid names, a live stroke, or more than 32 layers.
    pub fn add_layer(&mut self, name: &str) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        if name.trim().is_empty() || name.chars().count() > 128 {
            return Err(WebError::InvalidLayer);
        }
        if self.layers().root().children.len() >= MAX_LAYERS {
            return Err(WebError::LimitExceeded);
        }
        let id = LayerId(self.next_layer_id);
        let next = self
            .next_layer_id
            .checked_add(1)
            .ok_or(WebError::LimitExceeded)?;
        let mut artwork = self.artwork.clone();
        artwork.layers.insert(
            artwork.layers.root_id(),
            artwork.layers.root().children.len(),
            LayerTreeNode::Raster(new_layer(id, name.trim())),
        )?;
        artwork.active = id;
        let update = self.commit_artwork(artwork);
        self.next_layer_id = next;
        Ok(update)
    }

    /// # Errors
    /// Rejects deleting the final layer, a live stroke, or unknown layer IDs.
    pub fn delete_layer(&mut self, layer: LayerId) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        if self.layers().root().children.len() <= 1 {
            return Err(WebError::InvalidLayer);
        }
        let mut artwork = self.artwork.clone();
        artwork.layers.remove(LayerTreeNodeId::Raster(layer))?;
        artwork.tiles = TileSnapshot::from_objects(
            artwork
                .tiles
                .iter()
                .filter(|(key, _)| key.layer != layer)
                .map(|(key, tile)| (key, tile.clone())),
        )
        .map_err(|_| WebError::InvalidPixels)?;
        if artwork.active == layer {
            let Some(LayerTreeNode::Raster(first)) = artwork.layers.root().children.last() else {
                return Err(WebError::InvalidLayer);
            };
            artwork.active = first.id;
        }
        Ok(self.commit_artwork(artwork))
    }

    /// # Errors
    /// Rejects invalid layer IDs or edits during an active stroke.
    pub fn set_layer_visible(
        &mut self,
        layer: LayerId,
        visible: bool,
    ) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| {
            tree.set_visibility(LayerTreeNodeId::Raster(layer), visible)
                .map(|_| ())
        })
    }

    /// # Errors
    /// Rejects opacity outside 0..=1, invalid layer IDs, or an active stroke.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn set_layer_opacity(
        &mut self,
        layer: LayerId,
        opacity: f32,
    ) -> Result<CanvasUpdate, WebError> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            return Err(WebError::InvalidLayer);
        }
        self.edit_layers(|tree| {
            tree.set_opacity(
                LayerTreeNodeId::Raster(layer),
                (opacity * f32::from(u16::MAX)).round() as u16,
            )
            .map(|_| ())
        })
    }

    /// # Errors
    /// Rejects invalid layer IDs or an active stroke.
    pub fn set_layer_locked(
        &mut self,
        layer: LayerId,
        locked: bool,
    ) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| tree.set_locked(layer, locked))
    }

    /// # Errors
    /// Rejects invalid layer IDs or an active stroke.
    pub fn set_layer_alpha_locked(
        &mut self,
        layer: LayerId,
        locked: bool,
    ) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| tree.set_alpha_locked(layer, locked))
    }

    /// # Errors
    /// Rejects invalid names/layer IDs or an active stroke.
    pub fn rename_layer(&mut self, layer: LayerId, name: &str) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| {
            tree.rename(LayerTreeNodeId::Raster(layer), name)
                .map(|_| ())
        })
    }

    /// # Errors
    /// Rejects invalid layers, positions, or a live stroke.
    pub fn reorder_layer(
        &mut self,
        layer: LayerId,
        index: usize,
    ) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| {
            tree.reorder(LayerTreeNodeId::Raster(layer), tree.root_id(), index)
                .map(|_| ())
        })
    }

    /// # Errors
    /// Rejects invalid layer IDs or an active stroke.
    pub fn set_layer_blend_mode(
        &mut self,
        layer: LayerId,
        mode: LayerBlendMode,
    ) -> Result<CanvasUpdate, WebError> {
        self.edit_layers(|tree| {
            tree.set_blend_mode(LayerTreeNodeId::Raster(layer), mode)
                .map(|_| ())
        })
    }

    /// # Errors
    /// Rejects locked/alpha-locked layers or a live stroke. Empty clears make no history.
    pub fn clear_active_layer(&mut self) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let layer = self
            .layers()
            .raster(self.active_layer())
            .ok_or(WebError::InvalidLayer)?;
        if layer.locked || layer.alpha_locked {
            return Err(WebError::LayerUnavailable);
        }
        let mut artwork = self.artwork.clone();
        artwork.tiles = TileSnapshot::from_objects(
            artwork
                .tiles
                .iter()
                .filter(|(key, _)| key.layer != artwork.active)
                .map(|(key, tile)| (key, tile.clone())),
        )
        .map_err(|_| WebError::InvalidPixels)?;
        Ok(self.commit_artwork(artwork))
    }

    /// # Errors
    /// Undo is unavailable while a stroke is live. Empty history is a no-op.
    pub fn undo(&mut self) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let Some(previous) = self.undo.pop_back() else {
            return Ok(CanvasUpdate::default());
        };
        let update = difference(&self.artwork, &previous, true);
        self.redo
            .push(std::mem::replace(&mut self.artwork, previous));
        Ok(update)
    }

    /// # Errors
    /// Redo is unavailable while a stroke is live. Empty history is a no-op.
    pub fn redo(&mut self) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let Some(next) = self.redo.pop() else {
            return Ok(CanvasUpdate::default());
        };
        let update = difference(&self.artwork, &next, true);
        self.undo
            .push_back(std::mem::replace(&mut self.artwork, next));
        Ok(update)
    }

    /// Changes the export page without deleting outside pixels.
    /// # Errors
    /// Rejects invalid/oversized canvas metadata or a live stroke.
    pub fn set_canvas(&mut self, canvas: CanvasSpec) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        validate_canvas(canvas)?;
        let mut artwork = self.artwork.clone();
        artwork.canvas = canvas;
        Ok(self.commit_artwork(artwork))
    }

    /// Flattens the finite page to straight sRGB8; outside pixels are not exported.
    /// # Errors
    /// Rejects an active stroke or invalid composite data.
    pub fn export_rgba8(&self) -> Result<Vec<u8>, WebError> {
        self.require_idle()?;
        let mut image = flatten_layer_tree_rgba8(self.snapshot(), self.layers(), self.canvas())
            .map_err(|_| WebError::InvalidPixels)?
            .pixels;
        for pixel in image.chunks_exact_mut(4) {
            let rgba = [pixel[0], pixel[1], pixel[2], pixel[3]];
            pixel.copy_from_slice(&linear_premultiplied_to_srgb8(rgba).unwrap_or([0; 4]));
        }
        Ok(image)
    }

    /// Creates a document from straight sRGB8. Linear RGBA8 quantization is intentional.
    /// # Errors
    /// Checks dimensions/byte count before allocating tiles. Does not mutate another document.
    pub fn import_rgba8(width: u32, height: u32, pixels: &[u8]) -> Result<Self, WebError> {
        let mut document = Self::new(CanvasSpec {
            width_px: width,
            height_px: height,
            pixels_per_inch: 96,
        })?;
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(WebError::InvalidPixels);
        }
        let mut tiles = Vec::new();
        for ty in 0..height.div_ceil(TILE_EDGE) {
            for tx in 0..width.div_ceil(TILE_EDGE) {
                let mut tile = vec![0; TILE_BYTE_LEN];
                for y in 0..TILE_EDGE.min(height - ty * TILE_EDGE) {
                    for x in 0..TILE_EDGE.min(width - tx * TILE_EDGE) {
                        let input =
                            (((ty * TILE_EDGE + y) * width + tx * TILE_EDGE + x) * 4) as usize;
                        let output = ((y * TILE_EDGE + x) * 4) as usize;
                        tile[output..output + 4].copy_from_slice(&srgb8_to_linear_premultiplied([
                            pixels[input],
                            pixels[input + 1],
                            pixels[input + 2],
                            pixels[input + 3],
                        ]));
                    }
                }
                tiles.push((
                    TileKey {
                        layer: LayerId(1),
                        mip: 0,
                        x: i32::try_from(tx).map_err(|_| WebError::InvalidPixels)?,
                        y: i32::try_from(ty).map_err(|_| WebError::InvalidPixels)?,
                    },
                    tile,
                ));
            }
        }
        document.artwork.tiles =
            TileSnapshot::from_tiles(tiles).map_err(|_| WebError::InvalidPixels)?;
        Ok(document)
    }

    fn edit_layers(
        &mut self,
        edit: impl FnOnce(&mut LayerTree) -> Result<(), LayerTreeError>,
    ) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let mut artwork = self.artwork.clone();
        edit(&mut artwork.layers)?;
        Ok(self.commit_artwork(artwork))
    }

    fn require_idle(&self) -> Result<(), WebError> {
        if self.stroke.is_some() {
            Err(WebError::StrokeInProgress)
        } else {
            Ok(())
        }
    }

    fn commit_artwork(&mut self, next: Artwork) -> CanvasUpdate {
        let update = difference(&self.artwork, &next, true);
        if update.changed {
            self.redo.clear();
            self.undo
                .push_back(std::mem::replace(&mut self.artwork, next));
            self.trim_history();
        }
        update
    }

    fn trim_history(&mut self) {
        while self.undo.len() + self.redo.len() > MAX_HISTORY_ENTRIES
            || self.resident_bytes() > MAX_HISTORY_BYTES
        {
            if self.undo.pop_front().is_none() {
                if self.redo.is_empty() {
                    break;
                }
                self.redo.remove(0);
            }
        }
    }

    fn resident_bytes(&self) -> usize {
        let mut pixels = BTreeSet::new();
        let mut metadata = 0;
        for state in std::iter::once(&self.artwork)
            .chain(self.undo.iter())
            .chain(self.redo.iter())
            .chain(self.stroke.iter().map(|stroke| &stroke.before))
        {
            metadata += state.tiles.len() * 256 + state.layers.root().children.len() * 1024;
            for (_, tile) in state.tiles.iter() {
                pixels.insert(tile.pixels().as_ptr() as usize);
            }
        }
        pixels.len() * TILE_BYTE_LEN + metadata
    }
}

fn validate_canvas(canvas: CanvasSpec) -> Result<(), WebError> {
    if canvas.validate().is_err()
        || canvas.width_px > MAX_CANVAS_EDGE
        || canvas.height_px > MAX_CANVAS_EDGE
        || canvas.pixels_per_inch > 9600
    {
        return Err(WebError::InvalidCanvas);
    }
    Ok(())
}

fn new_layer(id: LayerId, name: &str) -> LayerNode {
    LayerNode {
        id,
        name: name.to_owned(),
        visible: true,
        locked: false,
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: LayerBlendMode::Normal,
        reference: false,
        opacity_u16: u16::MAX,
        content_root: ContentRootId(0),
    }
}

fn difference(before: &Artwork, after: &Artwork, committed: bool) -> CanvasUpdate {
    let mut keys: BTreeSet<_> = before
        .tiles
        .iter()
        .chain(after.tiles.iter())
        .map(|(key, _)| key)
        .collect();
    let layout_changed = before.layers != after.layers || before.canvas != after.canvas;
    if !layout_changed {
        keys.retain(|key| {
            before
                .tiles
                .get(*key)
                .map(nyatidraw_tiles::TileObject::hash)
                != after.tiles.get(*key).map(nyatidraw_tiles::TileObject::hash)
        });
    }
    let changed = !keys.is_empty() || layout_changed || before.active != after.active;
    CanvasUpdate {
        dirty_tiles: keys.into_iter().collect(),
        changed,
        committed: committed && changed,
        structure_changed: layout_changed,
    }
}

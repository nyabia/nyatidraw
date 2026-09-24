use std::sync::Arc;

use nyatidraw_api::{EditCommand, LayerCommand, LayerTreeNodeId};
use nyatidraw_editor::{layer_edit, pixel_edit::execute_pixel_edit};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask};

use crate::{
    Artwork, CanvasUpdate, MAX_LAYERS, MAX_RESIDENT_TILES, TileSnapshot, WebDocument, WebError,
    validate_canvas,
};

impl WebDocument {
    #[must_use]
    pub fn selection(&self) -> Option<&Arc<SelectionMask>> {
        self.transform_preview
            .as_ref()
            .map_or(self.selection.as_ref(), |preview| {
                preview.selection.as_ref()
            })
    }

    #[must_use]
    pub const fn solo_node(&self) -> Option<LayerTreeNodeId> {
        self.solo
    }

    /// # Errors
    /// Rejects invalid edits and browser resource limits before replacing artwork.
    pub fn apply_edit(&mut self, command: EditCommand) -> Result<CanvasUpdate, WebError> {
        if command == EditCommand::PasteSelection {
            return self.apply_transform(nyatidraw_api::TransformCommand::Paste);
        }
        if let EditCommand::FreeTransform(command) = command {
            return self.apply_transform(command);
        }
        self.require_idle()?;
        let mut clipboard =
            nyatidraw_editor::pixel_edit::MemoryArtworkClipboard(self.clipboard.0.clone());
        let edit = execute_pixel_edit(
            command,
            self.active_layer(),
            self.canvas(),
            self.layers(),
            &self.selection,
            self.snapshot(),
            self.next_layer_id,
            &mut clipboard,
            EditLimits::default(),
        );
        let edit = match edit {
            Ok(edit) => edit,
            Err(error) => return Err(WebError::EditRejected(error.0)),
        };
        let mut next = self.artwork.clone();
        if let Some(tiles) = edit.tiles {
            next.tiles = tiles;
        }
        if let Some(tree) = edit.tree {
            next.layers = tree;
        }
        if let Some(canvas) = edit.canvas {
            next.canvas = canvas;
        }
        if let Some(layer) = edit.active_layer {
            next.active = layer;
        }
        validate_artwork(&next)?;
        let next_id =
            layer_edit::next_layer_node_id(&next.layers).map_err(WebError::EditRejected)?;
        let update = self.commit_artwork(next);
        self.next_layer_id = self.next_layer_id.max(next_id);
        self.selection = edit.selection;
        self.clipboard = clipboard;
        if let Some(color) = edit.sampled_color {
            self.foreground = color;
        }
        Ok(update)
    }

    /// # Errors
    /// Validates the entire hierarchy and tile budget before committing a layer change.
    pub fn apply_layer_command(&mut self, command: LayerCommand) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        match command {
            LayerCommand::SetActive(layer) => {
                self.set_active_layer(layer)?;
                return Ok(CanvasUpdate::default());
            }
            LayerCommand::ToggleSolo(node) => {
                if self.layers().parent_and_index(node).is_none() {
                    return Err(WebError::InvalidLayer);
                }
                self.solo = if self.solo == Some(node) {
                    None
                } else {
                    Some(node)
                };
                return Ok(CanvasUpdate {
                    structure_changed: true,
                    ..CanvasUpdate::default()
                });
            }
            _ => {}
        }
        if command == LayerCommand::AddWhiteBackground {
            let canvas = self.canvas();
            let count = canvas.width_px.div_ceil(crate::TILE_EDGE)
                * canvas.height_px.div_ceil(crate::TILE_EDGE);
            if self.snapshot().len() + count as usize > MAX_RESIDENT_TILES {
                return Err(WebError::LimitExceeded);
            }
        }
        let prepared = layer_edit::prepare_layer_edit(
            self.layers(),
            self.active_layer(),
            self.next_layer_id,
            command,
        )
        .map_err(|reason| WebError::EditRejected(format!("{reason:?}")))?;
        let mut next = self.artwork.clone();
        next.layers = prepared.tree;
        next.active = prepared.active;
        if let Some(pixels) = prepared.pixels {
            next.tiles = pixels
                .apply(self.snapshot(), self.canvas())
                .map_err(WebError::EditRejected)?;
        }
        next.tiles = TileSnapshot::from_objects(
            next.tiles
                .iter()
                .filter(|(key, _)| next.layers.raster(key.layer).is_some())
                .map(|(key, tile)| (key, tile.clone())),
        )
        .map_err(|_| WebError::InvalidPixels)?;
        validate_artwork(&next)?;
        let next_id =
            layer_edit::next_layer_node_id(&next.layers).map_err(WebError::EditRejected)?;
        if self
            .solo
            .is_some_and(|node| next.layers.parent_and_index(node).is_none())
        {
            self.solo = None;
        }
        let update = self.commit_artwork(next);
        self.next_layer_id = self.next_layer_id.max(next_id);
        Ok(update)
    }
}

pub(super) fn node_count(tree: &crate::LayerTree) -> usize {
    fn count(group: &nyatidraw_document::GroupNode) -> usize {
        group
            .children
            .iter()
            .map(|node| match node {
                crate::LayerTreeNode::Raster(_) => 1,
                crate::LayerTreeNode::Group(group) => 1 + count(group),
            })
            .sum()
    }
    count(tree.root())
}

pub(super) fn validate_artwork(artwork: &Artwork) -> Result<(), WebError> {
    validate_canvas(artwork.canvas)?;
    if artwork.tiles.len() > MAX_RESIDENT_TILES || node_count(&artwork.layers) > MAX_LAYERS {
        return Err(WebError::LimitExceeded);
    }
    if artwork.layers.raster(artwork.active).is_none() {
        return Err(WebError::InvalidLayer);
    }
    if artwork.tiles.iter().any(|(key, _)| {
        key.mip != 0
            || key.x.unsigned_abs() > 257
            || key.y.unsigned_abs() > 257
            || artwork.layers.raster(key.layer).is_none()
    }) {
        return Err(WebError::LimitExceeded);
    }
    Ok(())
}

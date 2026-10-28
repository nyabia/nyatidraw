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
        if let Some(mask_command) =
            nyatidraw_editor::masks::mask_edit_command(&command, self.raster_target())
        {
            return self.apply_mask_command(mask_command);
        }
        if matches!(
            self.raster_target(),
            nyatidraw_api::RasterEditTarget::Mask { .. }
        ) && !nyatidraw_editor::masks::edit_allowed_on_mask(&command)
        {
            return Err(WebError::EditRejected(
                "이 작업은 이미지 대상을 선택한 뒤 사용할 수 있습니다.".into(),
            ));
        }
        if let EditCommand::ColorAdjustment(command) = command {
            return self.apply_color_adjustment(command);
        }
        if command == EditCommand::PasteSelection {
            return self.apply_transform(nyatidraw_api::TransformCommand::Paste);
        }
        if let EditCommand::FreeTransform(command) = command {
            return self.apply_transform(command);
        }
        self.require_idle()?;
        let mut clipboard =
            nyatidraw_editor::pixel_edit::MemoryArtworkClipboard(self.clipboard.0.clone());
        let edit_tree = self.edit_tree()?;
        let edit = execute_pixel_edit(
            command,
            self.active_layer(),
            self.canvas(),
            &edit_tree,
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
            next.layers = self.merge_edit_tree(&tree)?;
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
                if !self.active_node(node) {
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
            if self.snapshot().total_len() + count as usize > MAX_RESIDENT_TILES {
                return Err(WebError::LimitExceeded);
            }
        }
        let edit_tree = self.edit_tree()?;
        let prepared = layer_edit::prepare_layer_edit(
            &edit_tree,
            self.active_layer(),
            self.next_layer_id,
            command,
        )
        .map_err(|reason| WebError::EditRejected(format!("{reason:?}")))?;
        let mut next = self.artwork.clone();
        next.layers = self.merge_edit_tree(&prepared.tree)?;
        next.active = prepared.active;
        if let Some(pixels) = prepared.pixels {
            next.tiles = pixels
                .apply_to_tree(
                    self.snapshot(),
                    self.canvas(),
                    &self.artwork.layers,
                    &next.layers,
                )
                .map_err(WebError::EditRejected)?;
        }
        next.tiles = TileSnapshot::from_planes(
            next.tiles
                .iter()
                .filter(|(key, _)| next.layers.raster(key.layer).is_some())
                .map(|(key, tile)| (key, tile.clone())),
            next.tiles
                .coverage_iter()
                .filter(|(key, _)| next.layers.mask_ids().contains(&key.mask))
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
    if artwork.tiles.total_len() > MAX_RESIDENT_TILES {
        return Err(WebError::LimitExceeded);
    }
    if let Some(timeline) = &artwork.timeline {
        timeline
            .validate_against(&artwork.layers)
            .map_err(|error| WebError::EditRejected(format!("Animation: {error:?}")))?;
        if node_count(&artwork.layers) > 4096
            || timeline.frames().iter().any(|frame| {
                timeline
                    .resolve_frame_tree(&artwork.layers, frame.id)
                    .map_or(true, |tree| node_count(&tree) > MAX_LAYERS + 1)
            })
        {
            return Err(WebError::LimitExceeded);
        }
    } else if node_count(&artwork.layers) > MAX_LAYERS {
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
    let masks = artwork.layers.mask_ids();
    if artwork.tiles.coverage_iter().any(|(key, _)| {
        key.mip != 0
            || key.x.unsigned_abs() > 257
            || key.y.unsigned_abs() > 257
            || !masks.contains(&key.mask)
    }) {
        return Err(WebError::InvalidPixels);
    }
    Ok(())
}

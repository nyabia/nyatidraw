use nyatidraw_api::{AnimationCommand, FrameId, LayerTreeNodeId};
use nyatidraw_document::{AnimationTimeline, LayerTree};

use crate::{CanvasUpdate, WebDocument, WebError};

impl WebDocument {
    #[must_use]
    pub fn animation_timeline(&self) -> Option<&AnimationTimeline> {
        self.artwork.timeline.as_ref()
    }

    #[must_use]
    pub const fn active_frame(&self) -> Option<FrameId> {
        self.active_frame
    }

    #[must_use]
    pub fn render_layers(&self) -> &LayerTree {
        self.render_tree.as_ref().unwrap_or(self.layers())
    }

    pub(super) fn refresh_frame_projection(&mut self) {
        self.render_preview_tree = None;
        self.playback_frame = None;
        self.playback_tree = None;
        self.animation_display = None;
        let Some(timeline) = &self.artwork.timeline else {
            self.active_frame = None;
            self.render_tree = None;
            self.raster_target = Some(self.raster_target());
            return;
        };
        let frame = self
            .active_frame
            .filter(|frame| timeline.cel_for_frame(*frame).is_some())
            .or_else(|| {
                timeline
                    .frames()
                    .iter()
                    .find(|frame| {
                        timeline
                            .resolve_frame_tree(self.layers(), frame.id)
                            .is_ok_and(|tree| tree.raster(self.active_layer()).is_some())
                    })
                    .map(|frame| frame.id)
            })
            .unwrap_or(timeline.frames()[0].id);
        let tree = timeline
            .resolve_frame_tree(self.layers(), frame)
            .expect("validated animation owns every frame cel");
        self.artwork.active =
            nyatidraw_editor::layer_edit::valid_active_layer(&tree, self.active_layer());
        self.active_frame = Some(frame);
        self.render_tree = Some(tree);
        self.raster_target = Some(self.raster_target());
    }

    /// # Errors
    /// Returns only the selected cel's editable layers, including a live transform preview.
    pub fn layer_panel_tree(&self) -> Result<LayerTree, WebError> {
        let tree = self
            .transform_preview
            .as_ref()
            .map_or(self.layers(), |preview| &preview.tree);
        nyatidraw_editor::animation::active_edit_tree(
            tree,
            self.animation_timeline(),
            self.active_frame,
        )
        .map_err(WebError::EditRejected)
    }

    pub(super) fn edit_tree(&self) -> Result<LayerTree, WebError> {
        nyatidraw_editor::animation::active_edit_tree(
            self.layers(),
            self.animation_timeline(),
            self.active_frame,
        )
        .map_err(WebError::EditRejected)
    }

    pub(super) fn merge_edit_tree(&self, edited: &LayerTree) -> Result<LayerTree, WebError> {
        nyatidraw_editor::animation::merge_active_edit_tree(
            self.layers(),
            self.animation_timeline(),
            self.active_frame,
            edited,
        )
        .map_err(WebError::EditRejected)
    }

    pub(super) fn refresh_preview_projection(&mut self) -> Result<(), WebError> {
        self.render_preview_tree = match (
            &self.transform_preview,
            self.animation_timeline(),
            self.active_frame,
        ) {
            (Some(preview), Some(timeline), Some(frame)) => Some(
                timeline
                    .resolve_frame_tree(&preview.tree, frame)
                    .map_err(|error| {
                        WebError::EditRejected(format!("Animation preview: {error:?}"))
                    })?,
            ),
            _ => None,
        };
        Ok(())
    }

    /// # Errors
    /// Rejects live drafts and invalid edits before replacing any cel or its metadata.
    pub fn apply_animation_command(
        &mut self,
        command: AnimationCommand,
    ) -> Result<CanvasUpdate, WebError> {
        self.apply_animation_command_checked(command, |_, _| Ok(()))
    }

    /// # Errors
    /// Resource rejection leaves the authoritative artwork and history untouched.
    pub fn apply_animation_command_checked(
        &mut self,
        command: AnimationCommand,
        preflight: impl FnOnce(nyatidraw_api::CanvasSpec, &LayerTree) -> Result<(), String>,
    ) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let prepared = nyatidraw_editor::animation::prepare_animation_edit(
            command,
            self.layers(),
            self.snapshot(),
            self.animation_timeline(),
            self.active_frame,
            self.active_layer(),
            self.canvas(),
            self.next_layer_id,
        )
        .map_err(WebError::EditRejected)?;
        let mut next = self.artwork.clone();
        next.layers = prepared.tree;
        next.tiles = prepared.tiles;
        next.timeline = prepared.timeline;
        next.active = prepared.active_layer;
        crate::editing::validate_artwork(&next)?;
        let render = match (&next.timeline, prepared.active_frame) {
            (Some(timeline), Some(frame)) => timeline
                .resolve_frame_tree(&next.layers, frame)
                .map_err(|error| WebError::EditRejected(format!("Animation: {error:?}")))?,
            _ => next.layers.clone(),
        };
        preflight(next.canvas, &render).map_err(WebError::EditRejected)?;
        let next_id = nyatidraw_editor::layer_edit::next_layer_node_id(&next.layers)
            .map_err(WebError::EditRejected)?;
        self.selection = None;
        self.solo = None;
        self.active_frame = prepared.active_frame;
        let mut update = if prepared.artwork_changed {
            self.commit_artwork(next)
        } else {
            self.artwork.active = next.active;
            CanvasUpdate::default()
        };
        self.next_layer_id = self.next_layer_id.max(next_id);
        self.refresh_frame_projection();
        update.structure_changed = true;
        Ok(update)
    }

    #[must_use]
    pub const fn has_animation_display(&self) -> bool {
        self.animation_display.is_some()
    }

    pub fn set_animation_display(
        &mut self,
        display: Option<nyatidraw_editor::animation_display::FrameDisplay>,
    ) {
        self.animation_display = display;
    }

    pub fn clear_animation_overlays(&mut self) {
        self.selection = None;
        self.solo = None;
    }

    #[must_use]
    pub const fn display_frame(&self) -> Option<FrameId> {
        match self.playback_frame {
            Some(frame) => Some(frame),
            None => self.active_frame,
        }
    }

    /// # Errors
    /// Rejects invalid frames and live edits without changing the editing target.
    pub fn set_playback_frame(&mut self, frame: Option<FrameId>) -> Result<(), WebError> {
        self.require_idle()?;
        let frame_to_render = frame.or(self.active_frame);
        let tree = match (self.animation_timeline(), frame_to_render) {
            (Some(timeline), Some(frame)) => Some(
                timeline
                    .resolve_frame_tree(self.layers(), frame)
                    .map_err(|error| {
                        WebError::EditRejected(format!("Animation playback: {error:?}"))
                    })?,
            ),
            (None, None) => None,
            _ => return Err(WebError::InvalidInput),
        };
        self.playback_tree = if frame.is_some() { tree } else { None };
        self.animation_display = None;
        self.playback_frame = frame;
        Ok(())
    }

    pub(super) fn active_node(&self, node: LayerTreeNodeId) -> bool {
        self.render_layers().ancestors(node).is_some()
    }
}

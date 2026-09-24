use crate::{CanvasUpdate, WebDocument, WebError, difference};
use nyatidraw_api::{LayerId, SnapshotId, TransformCommand, TransformProjection};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::transform::{TransformError, TransformStore};
use nyatidraw_tiles::TileSnapshot;

impl WebDocument {
    /// # Errors
    /// Keeps the draft and current tool intact if its last valid preview cannot commit.
    pub fn finish_transform_for_tool_switch(&mut self) -> Result<CanvasUpdate, WebError> {
        let Some(mut command) = self.transform.tool_switch_command() else {
            return Ok(CanvasUpdate::default());
        };
        if matches!(command, TransformCommand::Preview(_)) {
            self.apply_transform(command)?;
            command = self
                .transform
                .tool_switch_command()
                .ok_or(WebError::InvalidInput)?;
        }
        self.apply_transform(command)
    }

    #[must_use]
    pub fn transform_projection(&self) -> Option<&TransformProjection> {
        self.transform.projection()
    }
    #[must_use]
    pub fn display_snapshot(&self) -> &TileSnapshot {
        self.transform_preview
            .as_ref()
            .map_or(self.snapshot(), |preview| &preview.tiles)
    }
    #[must_use]
    pub fn display_layers(&self) -> &LayerTree {
        self.transform_preview
            .as_ref()
            .map_or(self.layers(), |preview| &preview.tree)
    }
    #[must_use]
    pub fn display_active_layer(&self) -> LayerId {
        self.transform_preview
            .as_ref()
            .map_or(self.active_layer(), |preview| preview.active_layer)
    }

    pub(super) fn apply_transform(
        &mut self,
        command: TransformCommand,
    ) -> Result<CanvasUpdate, WebError> {
        if self.is_drawing() {
            return Err(WebError::StrokeInProgress);
        }
        let mut transform = std::mem::take(&mut self.transform);
        let mut clipboard = std::mem::take(&mut self.clipboard);
        let mut selection = self.selection.clone();
        let mut next_id = self.next_layer_id;
        let tree = self.layers().clone();
        let before = self.artwork.clone();
        let target = transform.target().unwrap_or(self.active_layer());
        let result = transform.execute(
            command,
            target,
            self.canvas(),
            &tree,
            &mut selection,
            &mut BrowserTransform {
                document: self,
                active: target,
            },
            &mut next_id,
            &mut clipboard,
        );
        self.transform = transform;
        self.clipboard = clipboard;
        let outcome = result.map_err(|error| match error {
            TransformError::Rejected(message) | TransformError::Fatal(message) => {
                WebError::EditRejected(message)
            }
        })?;
        self.next_layer_id = next_id;
        self.selection = selection;
        let update = if outcome.projection.is_some() {
            self.transform_preview = Some(outcome);
            CanvasUpdate {
                structure_changed: true,
                ..CanvasUpdate::default()
            }
        } else {
            self.transform_preview = None;
            let mut update = difference(&before, &self.artwork, outcome.committed);
            update.structure_changed = true;
            update
        };
        Ok(update)
    }
}

struct BrowserTransform<'a> {
    document: &'a mut WebDocument,
    active: LayerId,
}
impl TransformStore for BrowserTransform<'_> {
    fn current_snapshot(&self) -> SnapshotId {
        SnapshotId(self.document.revision)
    }
    fn tiles(&self) -> &TileSnapshot {
        self.document.snapshot()
    }
    fn validate_preview(&self, tiles: &TileSnapshot, tree: &LayerTree) -> Result<(), String> {
        let artwork = crate::Artwork {
            canvas: self.document.canvas(),
            tiles: tiles.clone(),
            layers: tree.clone(),
            active: self.document.active_layer(),
        };
        crate::editing::validate_artwork(&artwork).map_err(|error| error.to_string())
    }
    fn commit(&mut self, _id: u128, tiles: TileSnapshot, tree: &LayerTree) -> Result<(), String> {
        let mut artwork = self.document.artwork.clone();
        artwork.tiles = tiles;
        artwork.layers = tree.clone();
        artwork.active = self.active;
        crate::editing::validate_artwork(&artwork).map_err(|error| error.to_string())?;
        self.document.commit_artwork(artwork);
        Ok(())
    }
}

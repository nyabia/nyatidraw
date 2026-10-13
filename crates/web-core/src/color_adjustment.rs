use crate::{CanvasUpdate, WebDocument, WebError, transform::BrowserTransform};
use nyatidraw_api::{ColorAdjustmentCommand, ColorAdjustmentProjection};
use nyatidraw_editor::transform::TransformError;

impl WebDocument {
    #[must_use]
    pub fn color_adjustment_projection(&self) -> Option<&ColorAdjustmentProjection> {
        self.color_adjustment.projection()
    }

    pub(super) fn apply_color_adjustment(
        &mut self,
        command: ColorAdjustmentCommand,
    ) -> Result<CanvasUpdate, WebError> {
        if self.is_drawing() || self.transform_projection().is_some() {
            return Err(WebError::StrokeInProgress);
        }
        let mut adjustment = std::mem::take(&mut self.color_adjustment);
        let mut next = self.next_layer_id;
        let tree = self.layers().clone();
        let selection = self.selection.clone();
        let target = self.active_layer();
        let result = adjustment.execute(
            command,
            target,
            &tree,
            &selection,
            &mut BrowserTransform {
                document: self,
                active: target,
            },
            &mut next,
        );
        self.color_adjustment = adjustment;
        let outcome = result.map_err(|error| match error {
            TransformError::Rejected(message) | TransformError::Fatal(message) => {
                WebError::EditRejected(message)
            }
        })?;
        self.next_layer_id = next;
        let committed = outcome.committed;
        self.color_adjustment_preview = if outcome.projection.is_some() {
            Some(outcome)
        } else {
            None
        };
        Ok(CanvasUpdate {
            structure_changed: true,
            changed: committed,
            committed,
            ..CanvasUpdate::default()
        })
    }
}

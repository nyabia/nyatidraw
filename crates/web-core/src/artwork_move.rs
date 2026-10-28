use std::sync::Arc;

use nyatidraw_api::{EditCommand, RasterEditTarget};
use nyatidraw_editor::pixel_edit::{MemoryArtworkClipboard, execute_pixel_edit};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask};

use crate::{Artwork, CanvasUpdate, LayerTree, TileSnapshot, WebDocument, WebError};

/// An immutable drag source. Preview pixels never enter document history or recovery.
pub struct ArtworkMoveDraft {
    original: Artwork,
    tree: LayerTree,
    selection: Option<Arc<SelectionMask>>,
    revision: u128,
}

pub struct ArtworkMovePreview {
    pub tiles: TileSnapshot,
    pub selection: Option<Arc<SelectionMask>>,
}

impl ArtworkMoveDraft {
    /// # Errors
    /// Rejects invalid coordinates, locked targets and bounded-work overflow.
    pub fn preview(&self, offset: [i32; 2]) -> Result<ArtworkMovePreview, WebError> {
        let result = execute_pixel_edit(
            EditCommand::MoveArtwork { offset },
            self.original.active,
            self.original.canvas,
            &self.tree,
            &self.selection,
            &self.original.tiles,
            0,
            &mut MemoryArtworkClipboard::default(),
            EditLimits::default(),
        )
        .map_err(|error| WebError::EditRejected(error.0))?;
        let mut candidate = self.original.clone();
        if let Some(tiles) = result.tiles {
            candidate.tiles = tiles;
        }
        crate::editing::validate_artwork(&candidate)?;
        Ok(ArtworkMovePreview {
            tiles: candidate.tiles,
            selection: result.selection,
        })
    }
}

impl WebDocument {
    /// # Errors
    /// Requires an idle document and an editable image target.
    pub fn begin_artwork_move(&self) -> Result<ArtworkMoveDraft, WebError> {
        self.require_idle()?;
        if matches!(self.raster_target(), RasterEditTarget::Mask { .. }) {
            return Err(WebError::EditRejected(
                "그림 이동은 이미지 대상을 선택한 뒤 사용할 수 있습니다.".into(),
            ));
        }
        let draft = ArtworkMoveDraft {
            original: self.artwork.clone(),
            tree: self.edit_tree()?,
            selection: self.selection.clone(),
            revision: self.revision,
        };
        draft.preview([0, 0])?;
        Ok(draft)
    }

    /// # Errors
    /// Rejects a changed source or invalid final offset without committing a preview.
    pub fn finish_artwork_move(
        &mut self,
        draft: &ArtworkMoveDraft,
        offset: [i32; 2],
    ) -> Result<CanvasUpdate, WebError> {
        if self.revision != draft.revision
            || self.artwork != draft.original
            || self.selection != draft.selection
        {
            return Err(WebError::EditRejected(
                "그림이 변경되어 이동을 취소했습니다.".into(),
            ));
        }
        self.apply_edit(EditCommand::MoveArtwork { offset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{AnimationCommand, LayerCommand, MaskCommand};

    #[test]
    fn drag_preview_preserves_original_masks_inactive_cels_and_one_undo() {
        let mut doc = WebDocument::import_rgba8(4, 2, &[255; 32]).unwrap();
        let first_layer = doc.active_layer();
        doc.apply_edit(EditCommand::SelectLasso {
            vertices: vec![[0, 0], [2, 0], [2, 2], [0, 2]],
        })
        .unwrap();
        doc.apply_mask_command(MaskCommand::AddFromSelection(first_layer))
            .unwrap();
        doc.apply_mask_command(MaskCommand::SelectImage(first_layer))
            .unwrap();
        doc.apply_edit(EditCommand::ClearSelection).unwrap();
        doc.apply_animation_command(AnimationCommand::Enable)
            .unwrap();
        let first_frame = doc.active_frame().unwrap();
        doc.apply_animation_command(AnimationCommand::Duplicate(first_frame))
            .unwrap();
        let target = doc.active_layer();
        let target_mask = doc.layers().raster(target).unwrap().mask.unwrap().id;
        doc.apply_edit(EditCommand::SelectLasso {
            vertices: vec![[0, 0], [3, 0], [3, 2], [0, 2]],
        })
        .unwrap();
        let source = doc.snapshot().clone();
        let source_selection = doc.selection().cloned();
        let history = doc.undo_len();
        let draft = doc.begin_artwork_move().unwrap();
        let mut last = None;
        for offset in [[3, -1], [-2, 4], [0, 0], [3, -1]] {
            let preview = draft.preview(offset).unwrap();
            assert_eq!(doc.snapshot(), &source);
            assert_eq!(doc.selection(), source_selection.as_ref());
            assert_eq!(doc.undo_len(), history);
            assert_eq!(
                preview
                    .tiles
                    .iter()
                    .filter(|(key, _)| key.layer != target)
                    .collect::<Vec<_>>(),
                source
                    .iter()
                    .filter(|(key, _)| key.layer != target)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                preview
                    .tiles
                    .coverage_iter()
                    .filter(|(key, _)| key.mask != target_mask)
                    .collect::<Vec<_>>(),
                source
                    .coverage_iter()
                    .filter(|(key, _)| key.mask != target_mask)
                    .collect::<Vec<_>>()
            );
            if offset == [0, 0] {
                assert_eq!(preview.tiles, source);
            }
            last = Some(preview);
        }
        let preview = last.unwrap();
        let update = doc.finish_artwork_move(&draft, [3, -1]).unwrap();
        assert!(update.committed && update.changed);
        assert_eq!(doc.snapshot(), &preview.tiles);
        assert_eq!(doc.selection(), preview.selection.as_ref());
        assert_eq!(doc.undo_len(), history + 1);
        doc.undo().unwrap();
        assert_eq!(doc.snapshot(), &source);
        doc.redo().unwrap();
        assert_eq!(doc.snapshot(), &preview.tiles);
    }

    #[test]
    fn discarded_invalid_or_stale_move_drafts_cannot_commit_preview_pixels() {
        let mut doc = WebDocument::import_rgba8(2, 1, &[255; 8]).unwrap();
        let original = doc.encode_portable().unwrap();
        let draft = doc.begin_artwork_move().unwrap();
        draft.preview([4, -3]).unwrap();
        assert!(draft.preview([i32::MAX, 0]).is_err());
        drop(draft);
        assert_eq!(doc.encode_portable().unwrap(), original);
        let draft = doc.begin_artwork_move().unwrap();
        doc.apply_layer_command(LayerCommand::AddRaster).unwrap();
        let changed = doc.encode_portable().unwrap();
        assert!(doc.finish_artwork_move(&draft, [2, 0]).is_err());
        assert_eq!(doc.encode_portable().unwrap(), changed);
        doc.undo().unwrap();
        let draft = doc.begin_artwork_move().unwrap();
        let preview = draft.preview([2, -1]).unwrap();
        doc.finish_artwork_move(&draft, [2, -1]).unwrap();
        let reopened = WebDocument::decode_portable(&doc.encode_portable().unwrap()).unwrap();
        assert_eq!(reopened.snapshot(), &preview.tiles);
    }
}

use crate::{CanvasUpdate, WebDocument, WebError};
use nyatidraw_api::{MaskCommand, RasterEditTarget};

impl WebDocument {
    #[must_use]
    pub fn raster_target(&self) -> RasterEditTarget {
        nyatidraw_editor::masks::valid_raster_target(
            &self.artwork.layers,
            self.active_layer(),
            self.raster_target,
        )
    }

    /// # Errors
    /// Rejects stale owners, locked layers and resource limits before mutation.
    pub fn apply_mask_command(&mut self, command: MaskCommand) -> Result<CanvasUpdate, WebError> {
        self.require_idle()?;
        let tree = self.edit_tree()?;
        let prepared = nyatidraw_editor::masks::prepare_mask_edit(
            command,
            &tree,
            self.snapshot(),
            self.selection.as_deref(),
            self.next_layer_id,
        )
        .map_err(WebError::EditRejected)?;
        let mut next = self.artwork.clone();
        next.layers = self.merge_edit_tree(&prepared.tree)?;
        next.tiles = prepared.tiles;
        next.active = prepared.target.layer();
        super::editing::validate_artwork(&next)?;
        let next_id = nyatidraw_editor::layer_edit::next_layer_node_id(&next.layers)
            .map_err(WebError::EditRejected)?;
        let update = if prepared.changed {
            self.commit_artwork(next)
        } else {
            self.artwork.active = next.active;
            CanvasUpdate::default()
        };
        self.raster_target = Some(prepared.target);
        self.next_layer_id = self.next_layer_id.max(next_id);
        Ok(update)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StrokePoint, WebTool};
    use nyatidraw_api::{EditCommand, LayerCommand};
    use nyatidraw_paint_cpu::SelectionMask;

    #[test]
    fn history_keeps_only_valid_mask_targets_before_the_next_artwork_stroke() {
        for scenario in 0..3 {
            let mut doc = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
            let owner = doc.active_layer();
            doc.apply_mask_command(MaskCommand::AddRevealAll(owner))
                .unwrap();
            let mask = doc.layers().raster(owner).unwrap().mask.unwrap();
            match scenario {
                0 => {
                    doc.selection = Some(std::sync::Arc::new(
                        SelectionMask::from_coverage_at([0, 0], [1, 1], &[255]).unwrap(),
                    ));
                    doc.apply_edit(EditCommand::FillSelection {
                        color: [0, 0, 0, 255],
                    })
                    .unwrap();
                    doc.undo().unwrap();
                    doc.redo().unwrap();
                    assert_eq!(
                        doc.raster_target(),
                        RasterEditTarget::Mask {
                            layer: owner,
                            mask: mask.id
                        }
                    );
                    doc.undo().unwrap();
                }
                1 => {
                    doc.undo().unwrap();
                    assert_eq!(doc.raster_target(), RasterEditTarget::Image(owner));
                    doc.redo().unwrap();
                }
                _ => {
                    doc.apply_layer_command(LayerCommand::AddRaster).unwrap();
                    doc.undo().unwrap();
                }
            }
            let remains_mask = scenario == 0;
            assert_eq!(
                doc.raster_target(),
                if remains_mask {
                    RasterEditTarget::Mask {
                        layer: owner,
                        mask: mask.id,
                    }
                } else {
                    RasterEditTarget::Image(owner)
                }
            );
            let before = doc.snapshot().clone();
            doc.select_tool(WebTool::Pen).unwrap();
            doc.set_foreground([0, 0, 0, 255]);
            let point = StrokePoint {
                x: 8.0,
                y: 8.0,
                pressure: 1.0,
                time_ms: 1.0,
            };
            doc.begin_stroke(point).unwrap();
            doc.end_stroke(StrokePoint {
                time_ms: 2.0,
                ..point
            })
            .unwrap();
            assert_eq!(
                doc.snapshot().iter().collect::<Vec<_>>() == before.iter().collect::<Vec<_>>(),
                remains_mask,
                "the next stroke must modify only the explicitly retained editing plane",
            );
            assert_eq!(
                doc.snapshot().coverage_at(mask.id, 255, [8, 8]) < 255,
                remains_mask
            );
        }
    }

    #[test]
    fn mask_fill_is_one_history_step_and_cannot_change_inactive_cel_or_image() {
        use nyatidraw_api::AnimationCommand as A;
        let mut doc = WebDocument::import_rgba8(2, 1, &[255; 8]).unwrap();
        let first_owner = doc.active_layer();
        doc.apply_mask_command(MaskCommand::AddRevealAll(first_owner))
            .unwrap();
        let first_mask = doc.layers().raster(first_owner).unwrap().mask.unwrap();
        doc.apply_animation_command(A::Enable).unwrap();
        let first_frame = doc.active_frame().unwrap();
        doc.apply_animation_command(A::Duplicate(first_frame))
            .unwrap();
        let owner = doc.active_layer();
        doc.apply_mask_command(MaskCommand::SelectMask(owner))
            .unwrap();
        let mask = doc.layers().raster(owner).unwrap().mask.unwrap();
        let fill = EditCommand::FillSelection {
            color: [0, 0, 0, 255],
        };
        assert!(
            doc.apply_edit(fill.clone()).is_err(),
            "no selection must not fill an infinite mask"
        );
        doc.selection = Some(std::sync::Arc::new(
            SelectionMask::from_coverage_at([0, 0], [2, 1], &[64, 255]).unwrap(),
        ));
        for (protect, restore) in [
            (
                LayerCommand::SetLocked {
                    layer: owner,
                    locked: true,
                },
                LayerCommand::SetLocked {
                    layer: owner,
                    locked: false,
                },
            ),
            (
                LayerCommand::SetVisibility {
                    node: nyatidraw_api::LayerTreeNodeId::Raster(owner),
                    visible: false,
                },
                LayerCommand::SetVisibility {
                    node: nyatidraw_api::LayerTreeNodeId::Raster(owner),
                    visible: true,
                },
            ),
        ] {
            doc.apply_layer_command(protect).unwrap();
            doc.apply_mask_command(MaskCommand::SelectMask(owner))
                .unwrap();
            let protected = doc.snapshot().clone();
            let history = doc.undo_len();
            assert!(doc.apply_edit(fill.clone()).is_err());
            assert_eq!(doc.snapshot(), &protected);
            assert_eq!(doc.undo_len(), history);
            doc.apply_layer_command(restore).unwrap();
            doc.apply_mask_command(MaskCommand::SelectMask(owner))
                .unwrap();
        }
        let before = doc.snapshot().clone();
        let tree = doc.layers().clone();
        let selection = doc.selection.clone();
        let history = doc.undo_len();
        let update = doc.apply_edit(fill).unwrap();
        assert!(update.committed);
        assert_eq!(doc.undo_len(), history + 1);
        assert_eq!(doc.layers(), &tree);
        assert_eq!(
            doc.selection, selection,
            "mask fills retain fractional selection for subsequent edits"
        );
        assert_eq!(doc.snapshot().coverage_at(mask.id, 255, [0, 0]), 191);
        assert_eq!(doc.snapshot().coverage_at(mask.id, 255, [1, 0]), 0);
        assert_eq!(doc.snapshot().coverage_at(first_mask.id, 255, [1, 0]), 255);
        assert_eq!(
            doc.snapshot().iter().collect::<Vec<_>>(),
            before.iter().collect::<Vec<_>>()
        );
        let filled = doc.snapshot().clone();
        doc.undo().unwrap();
        assert_eq!(doc.snapshot(), &before);
        doc.redo().unwrap();
        assert_eq!(doc.snapshot(), &filled);
        let mut adopted = WebDocument::from_animation_snapshot(
            doc.canvas(),
            filled,
            doc.layers().clone(),
            owner,
            doc.animation_timeline().cloned(),
            doc.active_frame(),
        )
        .unwrap();
        assert_eq!(adopted.sample_rgba8(1, 0).unwrap()[3], 0);
        adopted
            .apply_animation_command(A::Select(first_frame))
            .unwrap();
        assert_eq!(adopted.sample_rgba8(1, 0).unwrap(), [255; 4]);
    }

    #[test]
    fn mask_stroke_cancel_undo_duplicate_and_adoption_preserve_original_image() {
        let mut doc = WebDocument::import_rgba8(16, 16, &[255; 16 * 16 * 4]).unwrap();
        let source = doc
            .snapshot()
            .iter()
            .map(|(key, tile)| (key, tile.clone()))
            .collect::<Vec<_>>();
        let owner = doc.active_layer();
        doc.apply_mask_command(MaskCommand::AddRevealAll(owner))
            .unwrap();
        let mask = doc.artwork.layers.raster(owner).unwrap().mask.unwrap();
        let before = doc.snapshot().clone();
        doc.select_tool(WebTool::Pen).unwrap();
        doc.set_foreground([0, 0, 0, 255]);
        let point = StrokePoint {
            x: 8.0,
            y: 8.0,
            pressure: 1.0,
            time_ms: 1.0,
        };
        doc.begin_stroke(point).unwrap();
        assert!(
            doc.apply_mask_command(MaskCommand::SelectImage(owner))
                .is_err()
        );
        doc.cancel_stroke();
        assert_eq!(doc.snapshot(), &before);
        doc.begin_stroke(point).unwrap();
        let update = doc
            .end_stroke(StrokePoint {
                time_ms: 2.0,
                ..point
            })
            .unwrap();
        assert!(update.committed && !update.dirty_masks.is_empty());
        assert!(doc.snapshot().coverage_at(mask.id, 255, [8, 8]) < 255);
        assert_eq!(
            doc.snapshot()
                .iter()
                .map(|(key, tile)| (key, tile.clone()))
                .collect::<Vec<_>>(),
            source
        );
        let painted = doc.snapshot().clone();
        doc.undo().unwrap();
        assert_eq!(doc.snapshot(), &before);
        doc.redo().unwrap();
        assert_eq!(doc.snapshot(), &painted);
        assert!(doc.apply_edit(EditCommand::ClearActiveLayer).is_err());
        let reopened = WebDocument::from_snapshot(
            doc.canvas(),
            doc.snapshot().clone(),
            doc.layers().clone(),
            owner,
        )
        .unwrap();
        assert_eq!(reopened.snapshot(), &painted);
        assert_eq!(
            reopened.sample_rgba8(8, 8).unwrap(),
            doc.sample_rgba8(8, 8).unwrap()
        );
        doc.apply_mask_command(MaskCommand::SelectImage(owner))
            .unwrap();
        doc.apply_layer_command(LayerCommand::DuplicateRaster(owner))
            .unwrap();
        let duplicate = doc
            .artwork
            .layers
            .raster(doc.active_layer())
            .unwrap()
            .mask
            .unwrap();
        assert_ne!(mask.id, duplicate.id);
        assert_eq!(
            doc.snapshot().coverage_at(mask.id, 255, [8, 8]),
            doc.snapshot().coverage_at(duplicate.id, 255, [8, 8])
        );
    }

    #[test]
    fn fractional_selection_mask_changes_only_coverage_and_disable_reveals_original() {
        let mut doc = WebDocument::import_rgba8(2, 1, &[255; 8]).unwrap();
        doc.selection = Some(std::sync::Arc::new(
            SelectionMask::from_coverage_at([0, 0], [2, 1], &[64, 192]).unwrap(),
        ));
        let owner = doc.active_layer();
        doc.apply_mask_command(MaskCommand::AddFromSelection(owner))
            .unwrap();
        assert_eq!(doc.sample_rgba8(0, 0).unwrap()[3], 64);
        assert_eq!(doc.sample_rgba8(1, 0).unwrap()[3], 192);
        doc.apply_mask_command(MaskCommand::SetEnabled {
            layer: owner,
            enabled: false,
        })
        .unwrap();
        assert_eq!(doc.sample_rgba8(0, 0).unwrap(), [255; 4]);
        doc.apply_mask_command(MaskCommand::Remove(owner)).unwrap();
        assert_eq!(doc.snapshot().coverage_len(), 0);
        assert_eq!(doc.snapshot().len(), 1);
    }
    #[test]
    fn linked_affine_moves_coverage_with_image_and_cancel_keeps_both_planes() {
        use nyatidraw_api::{AffineTransform, TransformCommand as T};
        let mut doc = WebDocument::import_rgba8(2, 1, &[255; 8]).unwrap();
        let owner = doc.active_layer();
        doc.selection = Some(std::sync::Arc::new(
            SelectionMask::from_coverage_at([0, 0], [2, 1], &[64, 192]).unwrap(),
        ));
        doc.apply_mask_command(MaskCommand::AddFromSelection(owner))
            .unwrap();
        let mask = doc.layers().raster(owner).unwrap().mask.unwrap();
        doc.apply_mask_command(MaskCommand::SelectImage(owner))
            .unwrap();
        doc.apply_edit(EditCommand::ClearSelection).unwrap();
        let original = doc.snapshot().clone();
        for commit in [false, true] {
            doc.apply_edit(EditCommand::FreeTransform(T::Begin))
                .unwrap();
            doc.apply_edit(EditCommand::FreeTransform(T::Preview(AffineTransform {
                offset_milli: [-1000, 2000],
                ..Default::default()
            })))
            .unwrap();
            assert_eq!(doc.snapshot(), &original);
            assert_eq!(doc.display_snapshot().coverage_at(mask.id, 0, [-1, 2]), 64);
            assert_eq!(doc.display_snapshot().coverage_at(mask.id, 0, [0, 2]), 192);
            if commit {
                let generation = doc.transform_projection().unwrap().generation;
                doc.apply_edit(EditCommand::FreeTransform(T::Commit { generation }))
                    .unwrap();
                assert_eq!(doc.snapshot().coverage_at(mask.id, 0, [-1, 2]), 64);
                doc.undo().unwrap();
            } else {
                doc.apply_edit(EditCommand::FreeTransform(T::Cancel))
                    .unwrap();
            }
            assert_eq!(doc.snapshot(), &original);
        }
    }
    #[test]
    fn whole_layer_affine_keeps_off_image_coverage_and_mask_only_history() {
        use nyatidraw_api::{AffineTransform, TransformCommand as T};
        for image in [false, true] {
            let mut doc =
                WebDocument::import_rgba8(1, 1, if image { &[255; 4] } else { &[0; 4] }).unwrap();
            let owner = doc.active_layer();
            doc.apply_mask_command(MaskCommand::AddRevealAll(owner))
                .unwrap();
            let mask = doc.layers().raster(owner).unwrap().mask.unwrap();
            let key = nyatidraw_tiles::CoverageTileKey::from_pixel(mask.id, 128, -10, 0);
            let mut values = vec![255; nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN];
            values[118] = 64;
            doc.artwork.tiles = doc
                .snapshot()
                .with_coverage_replacements([(key, values)])
                .unwrap();
            doc.apply_mask_command(MaskCommand::SelectImage(owner))
                .unwrap();
            let before = doc.snapshot().clone();
            doc.apply_edit(EditCommand::FreeTransform(T::Begin))
                .unwrap();
            doc.apply_edit(EditCommand::FreeTransform(T::Preview(AffineTransform {
                offset_milli: [2000, 0],
                ..Default::default()
            })))
            .unwrap();
            assert_eq!(
                doc.display_snapshot().coverage_at(mask.id, 255, [-8, 0]),
                64
            );
            assert_eq!(
                doc.display_snapshot().coverage_at(mask.id, 255, [-10, 0]),
                255
            );
            let generation = doc.transform_projection().unwrap().generation;
            assert!(
                doc.apply_edit(EditCommand::FreeTransform(T::Commit { generation }))
                    .unwrap()
                    .committed
            );
            doc.undo().unwrap();
            assert_eq!(doc.snapshot(), &before);
        }
    }
}

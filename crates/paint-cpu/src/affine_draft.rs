//! One bounded, disposable preview of an immutable transform source.
//! The caller persists a prepared result before retiring the draft. Previewing
//! never changes the durable document, so dropping a draft is an exact cancel.
use crate::{
    AffineResult, EditError, EditLimits, SelectionMask, SelectionPaintResult,
    transform_selection_affine,
};
use nyatidraw_api::{AffineTransform, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileSnapshot};
use std::collections::BTreeSet;

pub struct AffineDraft {
    snapshot_id: SnapshotId,
    source: TileSnapshot,
    tree: LayerTree,
    layer: LayerId,
    layers: Vec<LayerId>,
    whole_layer_masks: bool,
    selection: SelectionMask,
    latest_generation: u64,
    candidate_generation: u64,
    candidate: Option<AffineResult>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AffineDraftError {
    InvalidSelection,
    StaleGeneration,
    SourceChanged,
    NoPreview,
    Edit(EditError),
}

impl AffineDraft {
    /// Captures one source root and selection. All later previews use this
    /// source, never the previous preview's resampled pixels.
    ///
    /// # Errors
    /// Rejects unknown/locked targets and empty selection without changing it.
    pub fn new(
        snapshot_id: SnapshotId,
        source: &TileSnapshot,
        tree: &LayerTree,
        layer: LayerId,
        selection: &SelectionMask,
    ) -> Result<Self, AffineDraftError> {
        Self::new_layers(snapshot_id, source, tree, &[layer], selection)
    }

    /// Pins independent rasters to one source selection and transform pivot.
    /// # Errors
    /// Rejects empty, duplicate, unknown or protected targets, including hidden rasters.
    pub fn new_layers(
        snapshot_id: SnapshotId,
        source: &TileSnapshot,
        tree: &LayerTree,
        layers: &[LayerId],
        selection: &SelectionMask,
    ) -> Result<Self, AffineDraftError> {
        let layer = *layers
            .first()
            .ok_or(AffineDraftError::Edit(EditError::UnknownLayer))?;
        let mut unique = BTreeSet::new();
        for &id in layers {
            if !unique.insert(id) {
                return Err(AffineDraftError::Edit(EditError::InvalidTransform));
            }
            let target = tree
                .raster(id)
                .ok_or(AffineDraftError::Edit(EditError::UnknownLayer))?;
            if target.locked {
                return Err(AffineDraftError::Edit(EditError::LockedLayer));
            }
            if target.alpha_locked {
                return Err(AffineDraftError::Edit(EditError::AlphaLockedLayer));
            }
        }
        if source
            .iter()
            .any(|(key, _)| unique.contains(&key.layer) && key.mip != 0)
        {
            return Err(AffineDraftError::Edit(EditError::InvalidTransform));
        }
        if selection.bounds_signed().is_none() {
            return Err(AffineDraftError::InvalidSelection);
        }
        Ok(Self {
            snapshot_id,
            source: source.clone(),
            tree: tree.clone(),
            layer,
            layers: layers.to_vec(),
            whole_layer_masks: false,
            selection: selection.clone(),
            latest_generation: 0,
            candidate_generation: 0,
            candidate: None,
        })
    }

    pub fn include_whole_layer_masks(&mut self) {
        self.whole_layer_masks = true;
    }

    /// Replaces a single candidate only after successful bounded calculation.
    /// Failed geometry leaves the previous candidate available to show/cancel,
    /// but it cannot be committed under the newer generation.
    ///
    /// # Errors
    /// Rejects stale generations or invalid/over-budget geometry atomically.
    pub fn preview(
        &mut self,
        generation: u64,
        transform: &AffineTransform,
        limits: EditLimits,
    ) -> Result<&AffineResult, AffineDraftError> {
        if generation <= self.latest_generation {
            return Err(AffineDraftError::StaleGeneration);
        }
        self.latest_generation = generation;
        // A previous successful preview remains alive while its replacement
        // computes. Count its changed tile payload and mask against the budget.
        let retained = self.candidate.as_ref().map_or(0, |candidate| {
            let [w, h] = candidate.selection.dimensions();
            (candidate.paint.changed_tiles.len() as u64)
                .saturating_mul(TILE_BYTE_LEN as u64)
                .saturating_add(u64::from(w) * u64::from(h))
                .saturating_add(
                    candidate
                        .paint
                        .after
                        .coverage_iter()
                        .filter(|(key, tile)| {
                            self.source
                                .get_coverage(*key)
                                .is_none_or(|original| original.hash() != tile.hash())
                        })
                        .count() as u64
                        * nyatidraw_tiles::COVERAGE_TILE_BYTE_LEN as u64,
                )
        });
        let mut budget = limits.bounded();
        budget.max_workspace_bytes = budget
            .max_workspace_bytes
            .checked_sub(retained)
            .ok_or(AffineDraftError::Edit(EditError::LimitExceeded))?;
        let result = transform_layers_affine(
            &self.source,
            &self.tree,
            &self.layers,
            &self.selection,
            self.whole_layer_masks,
            transform,
            budget,
        )
        .map_err(AffineDraftError::Edit)?;
        self.candidate = Some(result);
        self.candidate_generation = generation;
        self.candidate.as_ref().ok_or(AffineDraftError::NoPreview)
    }

    /// Returns the candidate only when its exact source and generation remain
    /// current. Persist the returned pixels before changing the editor head.
    ///
    /// # Errors
    /// Rejects stale previews and changes to history, pixels, tree or selection.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_commit(
        &self,
        generation: u64,
        snapshot_id: SnapshotId,
        source: &TileSnapshot,
        tree: &LayerTree,
        layer: LayerId,
        selection: &SelectionMask,
    ) -> Result<&AffineResult, AffineDraftError> {
        if generation != self.latest_generation || generation != self.candidate_generation {
            return Err(AffineDraftError::StaleGeneration);
        }
        if layer != self.layer
            || snapshot_id != self.snapshot_id
            || source.root() != self.source.root()
            || tree != &self.tree
            || selection != &self.selection
        {
            return Err(AffineDraftError::SourceChanged);
        }
        self.candidate.as_ref().ok_or(AffineDraftError::NoPreview)
    }
}

fn transform_layers_affine(
    source: &TileSnapshot,
    tree: &LayerTree,
    layers: &[LayerId],
    selection: &SelectionMask,
    whole_layer_masks: bool,
    transform: &AffineTransform,
    limits: EditLimits,
) -> Result<AffineResult, EditError> {
    if layers.len() == 1 || *transform == AffineTransform::default() {
        let mut result =
            transform_selection_affine(source, tree, layers[0], selection, transform, limits)?;
        let retained = result.paint.changed_tiles.len() as u64 * (TILE_BYTE_LEN as u64 + 256);
        let mut mask_limits = limits;
        mask_limits.max_workspace_bytes = mask_limits
            .max_workspace_bytes
            .checked_sub(retained)
            .ok_or(EditError::LimitExceeded)?;
        result.paint.after = crate::masks_affine::transform_linked_masks_scoped(
            source,
            &result.paint.after,
            tree,
            layers,
            selection,
            whole_layer_masks,
            transform,
            mask_limits,
        )?;
        return Ok(result);
    }
    let mut replacements = Vec::new();
    let mut geometry = None;
    let mut retained_bytes = (source.total_len() as u64).saturating_mul(256);
    let mut per_layer = limits;
    per_layer.max_polygon_work /= layers.len() as u64;
    per_layer.max_pixels /= layers.len() as u64;
    for &layer in layers {
        per_layer.max_workspace_bytes = limits
            .max_workspace_bytes
            .checked_sub(retained_bytes)
            .ok_or(EditError::LimitExceeded)?;
        let result =
            transform_selection_affine(source, tree, layer, selection, transform, per_layer)?;
        if geometry.is_none() {
            let [w, h] = result.selection.dimensions();
            retained_bytes = retained_bytes.saturating_add(u64::from(w) * u64::from(h));
            geometry = Some((result.selection, result.corners));
        }
        for key in &result.paint.changed_tiles {
            let bytes = result
                .paint
                .after
                .get(*key)
                .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
            replacements.push((*key, bytes));
            // Pending bytes and their final immutable objects coexist while merging.
            retained_bytes = retained_bytes.saturating_add(TILE_BYTE_LEN as u64 * 2 + 256);
        }
        if retained_bytes > limits.max_workspace_bytes {
            return Err(EditError::LimitExceeded);
        }
    }
    let changed_tiles = replacements.iter().map(|(key, _)| *key).collect();
    let after = source
        .with_replacements(replacements)
        .map_err(EditError::Tiles)?;
    let mut mask_limits = limits;
    mask_limits.max_workspace_bytes = mask_limits
        .max_workspace_bytes
        .checked_sub(retained_bytes)
        .ok_or(EditError::LimitExceeded)?;
    let after = crate::masks_affine::transform_linked_masks_scoped(
        source,
        &after,
        tree,
        layers,
        selection,
        whole_layer_masks,
        transform,
        mask_limits,
    )?;
    let (selection, corners) = geometry.ok_or(EditError::UnknownLayer)?;
    Ok(AffineResult {
        paint: SelectionPaintResult {
            after,
            changed_tiles,
        },
        selection,
        corners,
    })
}

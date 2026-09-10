//! One bounded, disposable preview of an immutable transform source.
//! The caller persists a prepared result before retiring the draft. Previewing
//! never changes the durable document, so dropping a draft is an exact cancel.
use crate::{AffineResult, EditError, EditLimits, SelectionMask, transform_selection_affine};
use nyatidraw_api::{AffineTransform, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileSnapshot};

pub struct AffineDraft {
    snapshot_id: SnapshotId,
    source: TileSnapshot,
    tree: LayerTree,
    layer: LayerId,
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
        let target = tree
            .raster(layer)
            .ok_or(AffineDraftError::Edit(EditError::UnknownLayer))?;
        if target.locked {
            return Err(AffineDraftError::Edit(EditError::LockedLayer));
        }
        if target.alpha_locked {
            return Err(AffineDraftError::Edit(EditError::AlphaLockedLayer));
        }
        if selection.bounds_signed().is_none() {
            return Err(AffineDraftError::InvalidSelection);
        }
        Ok(Self {
            snapshot_id,
            source: source.clone(),
            tree: tree.clone(),
            layer,
            selection: selection.clone(),
            latest_generation: 0,
            candidate_generation: 0,
            candidate: None,
        })
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
        });
        let mut budget = limits.bounded();
        budget.max_workspace_bytes = budget
            .max_workspace_bytes
            .checked_sub(retained)
            .ok_or(AffineDraftError::Edit(EditError::LimitExceeded))?;
        let result = transform_selection_affine(
            &self.source,
            &self.tree,
            self.layer,
            &self.selection,
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

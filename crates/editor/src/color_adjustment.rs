//! Disposable color correction previews; only explicit confirmation persists.
use crate::transform::{TransformError, TransformStore};
use nyatidraw_api::{ColorAdjustmentCommand, ColorAdjustmentProjection, LayerId, SnapshotId};
use nyatidraw_document::LayerTree;
use nyatidraw_paint_cpu::{ColorAdjustment, EditLimits, SelectionMask, adjust_raster_colors};
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

#[derive(Default)]
pub struct ColorAdjustmentSession {
    draft: Option<Draft>,
    generation: u64,
}

struct Draft {
    snapshot: SnapshotId,
    original: TileSnapshot,
    tree: LayerTree,
    target: LayerId,
    selection: Option<Arc<SelectionMask>>,
    displayed: TileSnapshot,
    projection: ColorAdjustmentProjection,
}

pub struct ColorAdjustmentOutcome {
    pub tiles: TileSnapshot,
    pub projection: Option<ColorAdjustmentProjection>,
    pub committed: bool,
}

fn rejected(error: impl std::fmt::Debug) -> TransformError {
    TransformError::Rejected(format!("Color correction: {error:?}"))
}

impl ColorAdjustmentSession {
    #[must_use]
    pub fn projection(&self) -> Option<&ColorAdjustmentProjection> {
        self.draft.as_ref().map(|draft| &draft.projection)
    }

    #[must_use]
    pub fn target(&self) -> Option<LayerId> {
        self.draft.as_ref().map(|draft| draft.target)
    }

    /// # Errors
    /// Rejects stale sources/generations, failed previews, or failed durable commits.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::needless_pass_by_value
    )]
    pub fn execute(
        &mut self,
        command: ColorAdjustmentCommand,
        target: LayerId,
        tree: &LayerTree,
        selection: &Option<Arc<SelectionMask>>,
        store: &mut dyn TransformStore,
        next_id: &mut u128,
    ) -> Result<ColorAdjustmentOutcome, TransformError> {
        match command {
            ColorAdjustmentCommand::Begin => {
                if self.draft.is_some() {
                    return Err(rejected("AlreadyAdjusting"));
                }
                let original = store.tiles().clone();
                let result = adjust_raster_colors(
                    &original,
                    tree,
                    target,
                    selection.as_deref(),
                    ColorAdjustment::default(),
                    EditLimits::default(),
                )
                .map_err(rejected)?;
                store
                    .validate_preview(&result.after, tree)
                    .map_err(TransformError::Rejected)?;
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| rejected("GenerationExhausted"))?;
                self.draft = Some(Draft {
                    snapshot: store.current_snapshot(),
                    original,
                    tree: tree.clone(),
                    target,
                    selection: selection.clone(),
                    displayed: result.after,
                    projection: ColorAdjustmentProjection {
                        generation: self.generation,
                        adjustment: ColorAdjustment::default(),
                        can_commit: true,
                    },
                });
            }
            ColorAdjustmentCommand::Preview(adjustment) => {
                let draft = self
                    .draft
                    .as_mut()
                    .ok_or_else(|| rejected("NoAdjustment"))?;
                draft.projection.can_commit = false;
                draft.guard(target, tree, selection, store)?;
                let result = adjust_raster_colors(
                    &draft.original,
                    tree,
                    target,
                    selection.as_deref(),
                    adjustment,
                    EditLimits::default(),
                )
                .map_err(rejected)?;
                store
                    .validate_preview(&result.after, tree)
                    .map_err(TransformError::Rejected)?;
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| rejected("GenerationExhausted"))?;
                draft.displayed = result.after;
                draft.projection = ColorAdjustmentProjection {
                    generation: self.generation,
                    adjustment,
                    can_commit: true,
                };
            }
            ColorAdjustmentCommand::Commit { generation } => {
                let draft = self
                    .draft
                    .as_ref()
                    .ok_or_else(|| rejected("NoAdjustment"))?;
                draft.guard(target, tree, selection, store)?;
                if !draft.projection.can_commit || generation != draft.projection.generation {
                    return Err(rejected("StalePreview"));
                }
                let changed = draft.original.root() != draft.displayed.root();
                if changed {
                    let next = next_id
                        .checked_add(1)
                        .ok_or_else(|| rejected("IdentifierExhausted"))?;
                    store
                        .commit(*next_id, draft.displayed.clone(), tree)
                        .map_err(TransformError::Fatal)?;
                    *next_id = next;
                }
                let outcome = ColorAdjustmentOutcome {
                    tiles: draft.displayed.clone(),
                    projection: None,
                    committed: changed,
                };
                self.draft = None;
                return Ok(outcome);
            }
            ColorAdjustmentCommand::Cancel => {
                let draft = self
                    .draft
                    .as_ref()
                    .ok_or_else(|| rejected("NoAdjustment"))?;
                draft.guard(target, tree, selection, store)?;
                let outcome = ColorAdjustmentOutcome {
                    tiles: draft.original.clone(),
                    projection: None,
                    committed: false,
                };
                self.draft = None;
                return Ok(outcome);
            }
        }
        let draft = self
            .draft
            .as_ref()
            .ok_or_else(|| rejected("NoAdjustment"))?;
        Ok(ColorAdjustmentOutcome {
            tiles: draft.displayed.clone(),
            projection: Some(draft.projection.clone()),
            committed: false,
        })
    }
}

impl Draft {
    #[allow(clippy::ref_option)]
    fn guard(
        &self,
        target: LayerId,
        tree: &LayerTree,
        selection: &Option<Arc<SelectionMask>>,
        store: &dyn TransformStore,
    ) -> Result<(), TransformError> {
        if target != self.target
            || tree != &self.tree
            || selection != &self.selection
            || store.current_snapshot() != self.snapshot
            || store.tiles().root() != self.original.root()
        {
            return Err(rejected("SourceChanged"));
        }
        Ok(())
    }
}

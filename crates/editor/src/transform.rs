//! Disposable transform state owned by the project worker, never by the GPU.
use crate::pixel_edit::ArtworkClipboard;

use nyatidraw_api::{
    AffineTransform, CanvasSpec, ContentRootId, LayerId, LayerTreeNodeId, SnapshotId,
    TransformCommand, TransformProjection,
};
use nyatidraw_document::{LayerNode, LayerTree, LayerTreeNode};

use nyatidraw_paint_cpu::{AffineDraft, EditLimits, SelectionMask, paste_fragment, selection_all};

use nyatidraw_tiles::{TILE_BYTE_LEN, TileSnapshot};
use std::sync::Arc;

#[derive(Debug)]
pub enum TransformError {
    Rejected(String),
    Fatal(String),
}

pub trait TransformStore {
    fn current_snapshot(&self) -> SnapshotId;
    fn tiles(&self) -> &TileSnapshot;
    /// # Errors
    /// Rejects previews that cannot be displayed within the host's resource limits.
    fn validate_preview(&self, _tiles: &TileSnapshot, _tree: &LayerTree) -> Result<(), String> {
        Ok(())
    }
    /// # Errors
    /// Returns validation or persistence failure without accepting partial artwork.
    fn commit(&mut self, id: u128, tiles: TileSnapshot, tree: &LayerTree) -> Result<(), String>;
}

#[derive(Default)]
pub struct TransformSession {
    draft: Option<Transaction>,
    generation: u64,
}

struct Transaction {
    pasted: bool,
    snapshot: SnapshotId,
    original: TileSnapshot,
    original_tree: LayerTree,
    original_selection: Option<Arc<SelectionMask>>,
    original_layer: LayerId,
    canvas: CanvasSpec,
    source: TileSnapshot,
    tree: LayerTree,
    layer: LayerId,
    selection: Arc<SelectionMask>,
    affine: AffineDraft,
    projection: TransformProjection,
    displayed: TileSnapshot,
    displayed_selection: Arc<SelectionMask>,
    retained_bytes: u64,
}

pub struct TransformOutcome {
    pub tiles: TileSnapshot,
    pub tree: LayerTree,
    pub active_layer: LayerId,
    pub selection: Option<Arc<SelectionMask>>,
    pub projection: Option<TransformProjection>,
    pub committed: bool,
}

fn rejected(error: impl std::fmt::Debug) -> TransformError {
    TransformError::Rejected(format!("Transform: {error:?}"))
}

#[must_use]
pub fn tool_switch_command(projection: &TransformProjection, pasted: bool) -> TransformCommand {
    if !pasted && projection.transform == AffineTransform::default() {
        TransformCommand::Cancel
    } else if projection.can_commit {
        TransformCommand::Commit {
            generation: projection.generation,
        }
    } else {
        TransformCommand::Preview(projection.transform)
    }
}

impl TransformSession {
    #[must_use]
    pub fn tool_switch_command(&self) -> Option<TransformCommand> {
        self.draft
            .as_ref()
            .map(|draft| tool_switch_command(&draft.projection, draft.pasted))
    }
    #[must_use]
    pub fn projection(&self) -> Option<&TransformProjection> {
        self.draft.as_ref().map(|tx| &tx.projection)
    }
    #[must_use]
    pub fn target(&self) -> Option<LayerId> {
        self.draft.as_ref().map(|tx| tx.layer)
    }

    /// # Errors
    /// Rejects invalid drafts, stale previews and failed artwork commits.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::needless_pass_by_value
    )]
    pub fn execute(
        &mut self,
        command: TransformCommand,
        target: LayerId,
        canvas: CanvasSpec,
        tree: &LayerTree,
        selection: &mut Option<Arc<SelectionMask>>,
        session: &mut dyn TransformStore,
        next_id: &mut u128,
        clipboard: &mut dyn ArtworkClipboard,
    ) -> Result<TransformOutcome, TransformError> {
        match command {
            TransformCommand::Begin | TransformCommand::Paste => {
                if command == TransformCommand::Begin
                    && tree.raster(target).is_some_and(|layer| layer.alpha_locked)
                {
                    return Err(rejected(
                        "투명도 잠금 중에는 변형할 수 없습니다. 잠금을 해제하세요.",
                    ));
                }
                if self.draft.is_some() {
                    return Err(rejected("먼저 현재 변형을 확정하거나 취소하세요."));
                }
                let generation = self.next_generation()?;
                let mut source = session.tiles().clone();
                let mut source_tree = tree.clone();
                let mut layer = target;
                let mut retained_bytes = 0;
                let mask = if command == TransformCommand::Paste {
                    let fragment = clipboard.read().map_err(TransformError::Rejected)?;
                    let id = crate::layer_edit::next_layer_node_id(tree)
                        .map_err(TransformError::Rejected)?
                        .max(*next_id);
                    id.checked_add(1)
                        .ok_or_else(|| rejected("IdentifierExhausted"))?;
                    layer = LayerId(id);
                    let (parent, index) = tree
                        .parent_and_index(LayerTreeNodeId::Raster(target))
                        .ok_or_else(|| rejected("붙여넣기 위치 레이어가 없습니다."))?;
                    source_tree
                        .insert(
                            parent,
                            index + 1,
                            LayerTreeNode::Raster(LayerNode {
                                alpha_locked: false,
                                clip_to_below: false,
                                blend_mode: nyatidraw_api::LayerBlendMode::Normal,
                                id: layer,
                                name: "붙여넣기".into(),
                                visible: true,
                                locked: false,
                                reference: false,
                                opacity_u16: u16::MAX,
                                content_root: ContentRootId(0),
                            }),
                        )
                        .map_err(rejected)?;
                    let pasted = paste_fragment(
                        session.tiles(),
                        &source_tree,
                        layer,
                        &fragment,
                        EditLimits::default(),
                    )
                    .map_err(rejected)?;
                    retained_bytes =
                        (pasted.changed_tiles.len() as u64).saturating_mul(TILE_BYTE_LEN as u64);
                    source = pasted.after;
                    Arc::new(
                        selection_all(fragment.origin(), fragment.size(), EditLimits::default())
                            .map_err(rejected)?,
                    )
                } else {
                    selection
                        .clone()
                        .ok_or_else(|| rejected("먼저 변형할 영역을 선택하세요."))?
                };
                let (origin, size) = mask
                    .bounds_signed()
                    .ok_or_else(|| rejected("EmptySelection"))?;
                // The draft owns another binary source mask. Include retained
                // source masks/provisional paste payload in the preview budget.
                retained_bytes = retained_bytes.saturating_add(
                    u64::from(mask.dimensions()[0]) * u64::from(mask.dimensions()[1]) * 2,
                );
                let mut affine = AffineDraft::new(
                    session.current_snapshot(),
                    &source,
                    &source_tree,
                    layer,
                    &mask,
                )
                .map_err(rejected)?;
                let transform = AffineTransform::default();
                let result = affine
                    .preview(generation, &transform, preview_limits(retained_bytes)?)
                    .map_err(rejected)?;
                session
                    .validate_preview(&result.paint.after, &source_tree)
                    .map_err(TransformError::Rejected)?;
                let projection = TransformProjection {
                    generation,
                    transform,
                    origin,
                    size,
                    corners_milli: corners_milli(result.corners),
                    can_commit: true,
                };
                let displayed = result.paint.after.clone();
                let displayed_selection = Arc::new(result.selection.clone());
                self.draft = Some(Transaction {
                    pasted: command == TransformCommand::Paste,
                    snapshot: session.current_snapshot(),
                    original: session.tiles().clone(),
                    original_tree: tree.clone(),
                    original_selection: selection.clone(),
                    original_layer: target,
                    canvas,
                    source,
                    tree: source_tree,
                    layer,
                    selection: mask,
                    affine,
                    projection,
                    displayed,
                    displayed_selection,
                    retained_bytes,
                });
            }
            TransformCommand::Preview(transform) => {
                let generation = self.next_generation()?;
                let tx = self.draft.as_mut().ok_or_else(|| rejected("NoTransform"))?;
                tx.projection.can_commit = false;
                tx.guard(target, canvas, tree, selection, session)?;
                let [width, height] = tx.displayed_selection.dimensions();
                let retained = tx
                    .retained_bytes
                    .saturating_add(u64::from(width) * u64::from(height));
                let result = tx
                    .affine
                    .preview(generation, &transform, preview_limits(retained)?)
                    .map_err(rejected)?;
                session
                    .validate_preview(&result.paint.after, &tx.tree)
                    .map_err(TransformError::Rejected)?;
                tx.projection.generation = generation;
                tx.projection.transform = transform;
                tx.projection.corners_milli = corners_milli(result.corners);
                tx.projection.can_commit = true;
                tx.displayed = result.paint.after.clone();
                tx.displayed_selection = Arc::new(result.selection.clone());
            }
            TransformCommand::Commit { generation } => {
                let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
                if !tx.projection.can_commit {
                    return Err(rejected("미리보기를 다시 갱신한 뒤 확정하세요."));
                }
                tx.guard(target, canvas, tree, selection, session)?;
                let result = tx
                    .affine
                    .prepare_commit(
                        generation,
                        tx.snapshot,
                        &tx.source,
                        &tx.tree,
                        tx.layer,
                        &tx.selection,
                    )
                    .map_err(rejected)?;
                let after = result.paint.after.clone();
                let retained_selection = Arc::new(result.selection.clone());
                let changed = after.root() != tx.original.root() || tx.tree != tx.original_tree;
                if changed {
                    // Reserve both provisional layer and operation identifiers
                    // only at successful commit; Cancel consumes no document id.
                    let id = (*next_id).max(tx.layer.0.saturating_add(1));
                    let next = id
                        .checked_add(1)
                        .ok_or_else(|| rejected("IdentifierExhausted"))?;
                    session
                        .commit(id, after.clone(), &tx.tree)
                        .map_err(TransformError::Fatal)?;
                    *next_id = next;
                }
                *selection = Some(retained_selection);
                let outcome = TransformOutcome {
                    tiles: after,
                    tree: tx.tree.clone(),
                    active_layer: tx.layer,
                    selection: selection.clone(),
                    projection: None,
                    committed: changed,
                };
                self.draft = None;
                return Ok(outcome);
            }
            TransformCommand::Cancel => {
                let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
                // A stale source may not be visually rolled back over newer
                // artwork. Normal actor routing prevents edits during drafts.
                tx.guard(target, canvas, tree, selection, session)?;
                let outcome = TransformOutcome {
                    tiles: tx.original.clone(),
                    tree: tx.original_tree.clone(),
                    active_layer: tx.original_layer,
                    selection: tx.original_selection.clone(),
                    projection: None,
                    committed: false,
                };
                self.draft = None;
                return Ok(outcome);
            }
        }
        let tx = self.draft.as_ref().ok_or_else(|| rejected("NoTransform"))?;
        Ok(TransformOutcome {
            tiles: tx.displayed.clone(),
            tree: tx.tree.clone(),
            active_layer: tx.layer,
            selection: Some(tx.displayed_selection.clone()),
            projection: Some(tx.projection.clone()),
            committed: false,
        })
    }

    fn next_generation(&mut self) -> Result<u64, TransformError> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| rejected("GenerationExhausted"))?;
        Ok(self.generation)
    }
}

impl Transaction {
    #[allow(clippy::ref_option)] // Equality against the pinned optional Arc is the source guard.
    fn guard(
        &self,
        target: LayerId,
        canvas: CanvasSpec,
        tree: &LayerTree,
        selection: &Option<Arc<SelectionMask>>,
        session: &dyn TransformStore,
    ) -> Result<(), TransformError> {
        if target != self.layer
            || canvas != self.canvas
            || tree != &self.original_tree
            || selection != &self.original_selection
            || session.current_snapshot() != self.snapshot
            || session.tiles().root() != self.original.root()
        {
            return Err(rejected("SourceChanged"));
        }
        Ok(())
    }
}

fn preview_limits(retained_bytes: u64) -> Result<EditLimits, TransformError> {
    let mut limits = EditLimits::default();
    limits.max_workspace_bytes = limits
        .max_workspace_bytes
        .checked_sub(retained_bytes)
        .ok_or_else(|| rejected("LimitExceeded"))?;
    Ok(limits)
}

#[allow(clippy::cast_possible_truncation)]
fn corners_milli(corners: [[f64; 2]; 4]) -> [[i64; 2]; 4] {
    // Affine preflight has already bounded finite geometry to signed pixels.
    corners.map(|point| point.map(|value| (value * 1_000.0).round() as i64))
}

use super::{
    ActiveCanvas, Arc, ArtworkOutcome, BrushDab, ClosedStrokeCompletion, ClosedStrokeRequest,
    CommandRejectReason, EditCommand, EditFailure, HeadlessStrokeSession, HistoryMove,
    HistoryNodeId, LayerTree, LiveStrokeDisposition, ProjectDb, SnapshotId, StrokeColor,
    TileSnapshot, WorkerRequest, animation, next_layer_node_id, sync_channel, system_timestamp_ns,
};
use nyatidraw_api::{MaskCommand, RasterEditTarget};
use nyatidraw_document::LayerMask;
use nyatidraw_paint_cpu::{EditLimits, PremultipliedRgba8, SelectionMask};

pub(super) struct CanvasState {
    pub(super) authoritative: TileSnapshot,
    display: TileSnapshot,
    generation: u64,
    pub(super) preview: Option<Preview>,
}

pub(super) struct Preview {
    pub(super) generation: u64,
    mask: LayerMask,
    color: StrokeColor,
    erase: bool,
    selection: Option<Arc<SelectionMask>>,
    before: TileSnapshot,
}

fn coverage(snapshot: &TileSnapshot) -> TileSnapshot {
    TileSnapshot::from_planes(
        std::iter::empty(),
        snapshot
            .coverage_iter()
            .map(|(key, tile)| (key, tile.clone())),
    )
    .expect("validated immutable coverage")
}

impl CanvasState {
    pub(super) fn trace_display(&self) -> &TileSnapshot {
        &self.display
    }

    pub(super) fn new(snapshot: &TileSnapshot) -> Self {
        let authoritative = coverage(snapshot);
        Self {
            display: authoritative.clone(),
            authoritative,
            generation: 0,
            preview: None,
        }
    }
}

impl ActiveCanvas {
    pub(super) fn synchronize_mask_target(&mut self) {
        let target = nyatidraw_editor::masks::valid_raster_target(
            &self.authoritative_tree,
            self.active_layer,
            self.projection.current().raster_target,
        );
        self.stroke.selected_mask = match target {
            RasterEditTarget::Mask { layer, .. } => self
                .authoritative_tree
                .raster(layer)
                .and_then(|node| node.mask),
            RasterEditTarget::Image(_) => None,
        };
    }

    pub(super) fn guard_mask_edit(&self, command: &EditCommand) -> Result<(), CommandRejectReason> {
        if matches!(
            self.projection.current().raster_target,
            Some(RasterEditTarget::Mask { .. })
        ) && !nyatidraw_editor::masks::edit_allowed_on_mask(command)
        {
            self.stroke.live_ink.publish_activation_notice(
                "이 편집은 레이어의 이미지 대상을 선택한 뒤 사용하세요.".into(),
            );
            return Err(CommandRejectReason::InvalidLayerMove);
        }
        Ok(())
    }

    pub(super) fn enqueue_mask(
        &mut self,
        command: MaskCommand,
    ) -> Result<bool, CommandRejectReason> {
        self.ensure_artwork_command_idle()?;
        let editable = self.animation.state.edit_tree(&self.authoritative_tree);
        if editable.raster(command.layer()).is_none() {
            return Err(CommandRejectReason::UnknownLayer);
        }
        if !self.stroke.live_ink.try_pause_for_edit() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let (reply, received) = sync_channel(1);
        self.enqueue_artwork(
            WorkerRequest::Mask { command, reply },
            received,
            Some(command.layer()),
        )
    }

    pub(super) fn adopt_mask_snapshot(
        &mut self,
        snapshot: &TileSnapshot,
    ) -> Result<(), CommandRejectReason> {
        self.scene
            .replace_mask_tiles(snapshot)
            .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        self.mask_state = CanvasState::new(snapshot);
        Ok(())
    }

    pub(super) fn adopt_mask_completion(
        &mut self,
        snapshot: &TileSnapshot,
        generation: u64,
    ) -> Result<(), String> {
        self.mask_state.authoritative = coverage(snapshot);
        if self.mask_state.generation <= generation && self.mask_state.preview.is_none() {
            self.mask_state
                .display
                .clone_from(&self.mask_state.authoritative);
            self.scene
                .replace_mask_tiles(&self.mask_state.display)
                .map_err(|error| format!("Mask adoption: {error:?}"))?;
        }
        self.animation.refresh_onion = self.animation.onion_skin;
        Ok(())
    }

    pub(super) fn begin_mask_preview(
        &mut self,
        generation: u64,
        mask: LayerMask,
        color: StrokeColor,
        erase: bool,
        selection: Option<Arc<SelectionMask>>,
    ) {
        self.mask_state.generation = generation;
        self.mask_state.preview = Some(Preview {
            generation,
            mask,
            color,
            erase,
            selection,
            before: self.mask_state.display.clone(),
        });
    }

    pub(super) fn apply_mask_preview(&mut self, dabs: &[BrushDab]) -> Result<(), String> {
        let preview = self
            .mask_state
            .preview
            .as_ref()
            .ok_or("Missing mask preview")?;
        let after = nyatidraw_paint_cpu::paint_mask_dabs(
            &self.mask_state.display,
            preview.mask,
            dabs,
            PremultipliedRgba8(preview.color.0),
            preview.erase,
            preview.selection.as_deref(),
            EditLimits::default(),
        )
        .map_err(|error| format!("Mask preview: {error:?}"))?;
        self.scene
            .replace_mask_tiles(&after)
            .map_err(|error| format!("Mask preview upload: {error:?}"))?;
        self.mask_state.display = after;
        Ok(())
    }

    pub(super) fn finish_mask_preview(
        &mut self,
        generation: u64,
        disposition: LiveStrokeDisposition,
    ) -> bool {
        if self
            .mask_state
            .preview
            .as_ref()
            .is_none_or(|preview| preview.generation != generation)
        {
            return false;
        }
        let preview = self.mask_state.preview.take().expect("matched preview");
        if disposition == LiveStrokeDisposition::Cancel {
            self.mask_state.display = preview.before;
            if let Err(error) = self.scene.replace_mask_tiles(&self.mask_state.display) {
                self.stroke
                    .live_ink
                    .publish_workspace_error(format!("Mask cancellation: {error:?}"));
            }
        }
        true
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    command: MaskCommand,
    tree: &mut LayerTree,
    animation: &animation::State,
    session: &mut HeadlessStrokeSession,
    selection: Option<&SelectionMask>,
    next_id: &mut u128,
    db: &ProjectDb,
) -> Result<ArtworkOutcome, EditFailure> {
    if animation.edit_tree(tree).raster(command.layer()).is_none() {
        return Err(EditFailure::Rejected(
            "Mask owner is outside the current frame".into(),
        ));
    }
    let mask_id = next_layer_node_id(tree).map_err(EditFailure::Rejected)?;
    let prepared = nyatidraw_editor::masks::prepare_mask_edit(
        command,
        tree,
        session.tiles(),
        selection,
        mask_id,
    )
    .map_err(EditFailure::Rejected)?;
    if prepared.changed {
        commit(
            session,
            next_id,
            db,
            &prepared.tree,
            prepared.tiles.clone(),
            system_timestamp_ns(),
        )
        .map_err(EditFailure::Fatal)?;
        tree.clone_from(&prepared.tree);
    }
    Ok(ArtworkOutcome::Mask {
        moved: HistoryMove {
            tiles: prepared.changed.then_some(prepared.tiles),
            tree: prepared.tree_changed.then_some(prepared.tree),
            canvas: None,
            history: session.history_projection(),
            worker_timing: None,
        },
        target: prepared.target,
        changed: prepared.changed,
    })
}

fn commit(
    session: &mut HeadlessStrokeSession,
    next_id: &mut u128,
    db: &ProjectDb,
    tree: &LayerTree,
    after: TileSnapshot,
    timestamp: u64,
) -> Result<(), String> {
    let next = next_id
        .checked_add(1)
        .ok_or("project identifier exhausted")?;
    let batch = session
        .prepare_structural_change(
            SnapshotId(*next_id),
            HistoryNodeId(*next_id),
            timestamp,
            after,
        )
        .map_err(|error| format!("Mask prepare: {error:?}"))?;
    db.commit_structural_with_layer_tree(&batch, tree)
        .map_err(|error| format!("Mask durable commit: {error}"))?;
    session
        .accept_structural_change(&batch)
        .map_err(|error| format!("Mask accept: {error:?}"))?;
    *next_id = next;
    Ok(())
}

pub(super) fn commit_stroke(
    request: &ClosedStrokeRequest,
    tree: &LayerTree,
    editable: &LayerTree,
    session: &mut HeadlessStrokeSession,
    next_id: &mut u128,
    db: &ProjectDb,
) -> Result<ClosedStrokeCompletion, String> {
    let mask = request.mask.ok_or("Missing captured mask")?;
    let owner = editable
        .raster(request.layer)
        .ok_or("Mask owner is outside the current frame")?;
    if owner.locked || owner.mask != Some(mask) {
        return Err("Captured mask target changed before commit".into());
    }
    let mut dabs = Vec::new();
    nyatidraw_brush::replay_round_stroke(
        &request.brush,
        &request.recorded,
        &request.samples,
        &mut dabs,
    )
    .map_err(|error| format!("Mask replay: {error:?}"))?;
    let erase = request.samples.first().is_some_and(|sample| sample.eraser);
    let after = nyatidraw_paint_cpu::paint_mask_dabs(
        session.tiles(),
        mask,
        &dabs,
        PremultipliedRgba8(request.color.0),
        erase,
        request.selection.as_deref(),
        EditLimits::default(),
    )
    .map_err(|error| format!("Mask paint: {error:?}"))?;
    let changed = after.root() != session.tiles().root();
    if changed {
        commit(
            session,
            next_id,
            db,
            tree,
            after.clone(),
            request.end_timestamp_ns,
        )?;
    }
    Ok(ClosedStrokeCompletion {
        ordinal: request.ordinal,
        stroke_generation: request.stroke_generation,
        tiles: Vec::new(),
        coverage: Some(coverage(&after)),
        history: changed.then(|| session.history_projection()),
    })
}

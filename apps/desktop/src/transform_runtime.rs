//! Disposable GPU preview adoption and native transform interaction.
use super::{
    ActiveCanvas, BTreeSet, CommandRejectReason, CpuSnapshotAdoption, DrawingTool, EditCommand,
    LayerId, LayerTreeNode, PointerPhase, StrokePipeline, StylusSample, TRANSPARENT_HISTORY_TILE,
    TileKey, WorkerRequest, empty_raster, next_layer_node_id, sync_channel,
};
use nyatidraw_api::TransformCommand;

impl ActiveCanvas {
    pub(super) fn enqueue_free_transform(
        &mut self,
        command: TransformCommand,
    ) -> Result<bool, CommandRejectReason> {
        let active = self.stroke.transform_projection.is_some();
        if command == TransformCommand::Cancel && active {
            self.stroke.transform_gesture = None;
            self.stroke.pending_transform = None;
            if self.artwork_job.is_some() {
                self.stroke.pending_transform_cancel = true;
                return Ok(false);
            }
        }
        if matches!(command, TransformCommand::Commit { .. })
            && (self.stroke.transform_gesture.is_some()
                || self.stroke.pending_transform.is_some()
                || self.stroke.pending_transform_cancel
                || self
                    .stroke
                    .transform_projection
                    .as_ref()
                    .is_none_or(|p| !p.can_commit))
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        self.ensure_artwork_command_idle()?;
        if matches!(command, TransformCommand::Begin | TransformCommand::Paste) {
            if active {
                return Err(CommandRejectReason::CommandQueueBusy);
            }
            if command == TransformCommand::Paste {
                let mut candidate = self.scene.tree().clone();
                let (parent, index) = candidate
                    .parent_and_index(nyatidraw_api::LayerTreeNodeId::Raster(self.active_layer))
                    .ok_or(CommandRejectReason::UnknownLayer)?;
                let id = next_layer_node_id(&candidate)
                    .map_err(|_| CommandRejectReason::RevisionExhausted)?;
                candidate
                    .insert(
                        parent,
                        index + 1,
                        LayerTreeNode::Raster(empty_raster(LayerId(id), "붙여넣기".into())),
                    )
                    .map_err(|_| CommandRejectReason::InvalidLayerMove)?;
                self.scene
                    .validate_tree_replacement(&candidate)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
            }
        } else if !active {
            return Err(CommandRejectReason::UnsupportedCommand);
        }
        // An active transform admits pointer moves while a CPU preview is in
        // flight. All those samples are routed exclusively to transform input.
        // Begin/Commit still need a clean native phase boundary.
        if (!active || matches!(command, TransformCommand::Commit { .. }))
            && !self.stroke.live_ink.try_pause_for_edit()
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let (reply, received) = sync_channel(1);
        self.enqueue_artwork(
            WorkerRequest::Edit {
                command: EditCommand::FreeTransform(command),
                target: self.active_layer,
                reply,
            },
            received,
            None,
        )
    }

    pub(super) fn flush_transform_preview(&mut self) {
        if self.artwork_job.is_some() {
            return;
        }
        let command = if self.stroke.pending_transform_cancel {
            TransformCommand::Cancel
        } else if let Some(transform) = self.stroke.pending_transform {
            TransformCommand::Preview(transform)
        } else {
            return;
        };
        match self.enqueue_free_transform(command) {
            Ok(_) => {
                self.stroke.pending_transform = None;
                self.stroke.pending_transform_cancel = false;
            }
            Err(
                CommandRejectReason::CommandQueueBusy | CommandRejectReason::StaleProjection { .. },
            ) => {
                self.stroke.live_ink.request_redraw();
            }
            Err(reason) => {
                self.stroke.pending_transform = None;
                self.stroke.pending_transform_cancel = false;
                self.stroke
                    .live_ink
                    .publish_activation_notice(format!("변형을 갱신하지 못했습니다: {reason:?}"));
            }
        }
    }

    pub(super) fn install_transform_outcome(
        &mut self,
        outcome: crate::transform_worker::TransformOutcome,
    ) -> Result<(), String> {
        // Neither preview nor Cancel goes through durable snapshot adoption:
        // cpu_tiles remains the last committed state throughout the draft.
        let mut keys: BTreeSet<TileKey> = self.cpu_tiles.keys().copied().collect();
        if let Some(previous) = &self.transform_display {
            keys.extend(previous.iter().map(|(key, _)| key));
        }
        keys.extend(outcome.tiles.iter().map(|(key, _)| key));
        if self.scene.tree() != &outcome.tree {
            self.scene
                .replace_tree(outcome.tree.clone())
                .map_err(|e| format!("Transform display tree: {e:?}"))?;
        }
        for key in keys {
            if self.scene.tree().raster(key.layer).is_none() {
                continue;
            }
            let pixels = outcome.tiles.get(key).map_or(
                &TRANSPARENT_HISTORY_TILE[..],
                nyatidraw_tiles::TileObject::pixels,
            );
            self.scene
                .upload_closed_tile(key, pixels)
                .map_err(|e| format!("Transform display pixels: {e:?}"))?;
        }
        self.active_layer = outcome.active_layer;
        self.stroke.live_ink.set_admission_layer(self.active_layer);
        self.install_selection(outcome.selection)?;
        self.projection.stage_history(outcome.history);
        if outcome.committed {
            CpuSnapshotAdoption::new(&mut self.cpu_tiles, &outcome.tiles).apply();
            let mut current = self.projection.current().clone();
            current.dirty = true;
            self.projection.install_authoritative(current);
        }
        let active = outcome.projection.is_some();
        self.transform_display = active.then_some(outcome.tiles);
        self.stroke.transform_projection = outcome.projection;
        if active {
            self.drawing.select_tool(DrawingTool::MoveSelection);
            self.stroke.live_ink.set_navigation_tool(false);
        } else {
            self.stroke.transform_gesture = None;
            self.stroke.pending_transform = None;
            self.stroke.pending_transform_cancel = false;
            // Exit transform mode explicitly; another MoveSelection activation
            // starts a fresh draft with the retained selection, never the old
            // immediate integer-transform gesture.
            self.drawing.select_tool(DrawingTool::Lasso);
            self.stroke.live_ink.set_navigation_tool(false);
        }
        self.projection.stage_drawing_controls(
            self.drawing.tool,
            self.drawing.size_tenths,
            self.drawing.opacity_u16,
            self.drawing.color,
            self.drawing.background_color,
        );
        self.latest_preview_generation.clear();
        Ok(())
    }
}

impl StrokePipeline {
    pub(super) fn observe_transform_sample(&mut self, sample: StylusSample) {
        let point = [sample.position_document.x, sample.position_document.y];
        match sample.phase {
            PointerPhase::Begin => {
                let Some(projection) = &self.transform_projection else {
                    return;
                };
                // A second drag cannot start from a stale preview still in flight.
                if self.pending_transform.is_some() || self.live_ink.protocol_snapshot().0.edit.busy
                {
                    return;
                }
                let zoom = self.live_ink.canvas_viewport_snapshot().zoom;
                self.transform_gesture = crate::transform_gesture::TransformGesture::begin(
                    projection,
                    point,
                    7.0 / zoom,
                )
                .map(|gesture| (gesture, sample.viewport_revision, projection.transform));
            }
            PointerPhase::Move | PointerPhase::End => {
                if let Some((gesture, revision, original)) = &self.transform_gesture {
                    if *revision != sample.viewport_revision {
                        self.pending_transform = Some(*original);
                        self.transform_gesture = None;
                        return;
                    }
                    match gesture.update(point) {
                        Ok(transform) => self.pending_transform = Some(transform),
                        Err(reason) => {
                            self.pending_transform = Some(*original);
                            self.transform_gesture = None;
                            self.live_ink.publish_activation_notice(reason);
                        }
                    }
                }
                if sample.phase == PointerPhase::End {
                    self.transform_gesture = None;
                }
            }
            PointerPhase::Cancel => {
                if let Some((_, _, original)) = self.transform_gesture.take() {
                    self.pending_transform = Some(original);
                }
            }
        }
    }
}

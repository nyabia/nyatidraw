//! Disposable GPU preview adoption and native transform interaction.
use super::{
    ActiveCanvas, BTreeSet, CommandRejectReason, CpuSnapshotAdoption, DrawingTool, EditCommand,
    LayerId, LayerTreeNode, PointerPhase, StrokePipeline, StylusSample, TRANSPARENT_HISTORY_TILE,
    TileKey, WorkerRequest, empty_raster, next_layer_node_id, sync_channel,
};
use nyatidraw_api::{AffineTransform, ToolCommand, TransformCommand, TransformProjection};
use nyatidraw_brush::BrushEvaluator;

pub(super) fn selected_tool(current: DrawingTool, command: ToolCommand) -> Option<DrawingTool> {
    Some(match command {
        ToolCommand::Select(tool) => tool,
        ToolCommand::SelectPencilTemplate(_) => DrawingTool::Pencil,
        ToolCommand::CycleBrushFamily => match current {
            DrawingTool::Pencil => DrawingTool::Pen,
            DrawingTool::Pen => DrawingTool::Brush,
            _ => DrawingTool::Pencil,
        },
        ToolCommand::CycleSelectionFamily => match current {
            DrawingTool::Move => DrawingTool::MoveSelection,
            DrawingTool::MoveSelection => DrawingTool::Wand,
            DrawingTool::Wand => DrawingTool::Lasso,
            DrawingTool::Lasso => DrawingTool::RectangleSelection,
            _ => DrawingTool::Move,
        },
        ToolCommand::CycleFillFamily => {
            if current == DrawingTool::Fill {
                DrawingTool::Gradient
            } else {
                DrawingTool::Fill
            }
        }
        _ => return None,
    })
}

impl ActiveCanvas {
    pub(super) fn transform_active(&self) -> bool {
        self.stroke.transform_starting
            || self.stroke.transform_projection.is_some()
            || self
                .artwork_job
                .as_ref()
                .is_some_and(|job| job.transform.is_some())
    }

    pub(super) fn stage_cancel_availability(&mut self) {
        let mut edit = self.projection.current().edit.clone();
        edit.can_cancel = if let Some(job) = &self.artwork_job {
            job.picker_token
                .is_some_and(|token| self.stroke.picker_token_valid(token))
                || matches!(
                    job.transform,
                    Some(
                        TransformCommand::Begin
                            | TransformCommand::Paste
                            | TransformCommand::Preview(_)
                    )
                ) && !self.stroke.pending_transform_cancel
        } else {
            !self.stroke.pending_transform_cancel
                && (self.stroke.transform_projection.is_some()
                    || self.stroke.transform_starting
                    || self.stroke.edit_gesture.is_some()
                    || self.stroke.picker.gesture.is_some()
                    || self.stroke.active.is_some())
        };
        self.projection.stage_edit(edit);
    }

    pub(super) fn switch_transform_tool(
        &mut self,
        tool: DrawingTool,
        pencil_template: Option<nyatidraw_api::PencilTemplate>,
    ) {
        if tool == self.drawing.tool && self.stroke.pending_tool.is_none() {
            return;
        }
        // Switching intent stays bounded while the worker owns the draft. Do
        // not mutate the selected tool until its commit/cancel is adopted.
        self.stroke.pending_tool = Some(tool);
        self.stroke.pending_pencil_template = pencil_template;
        self.stroke.transform_gesture = None;
        self.flush_transform_preview();
        self.stage_cancel_availability();
        self.stroke.live_ink.request_redraw();
    }

    pub(super) fn enqueue_free_transform(
        &mut self,
        command: TransformCommand,
    ) -> Result<bool, CommandRejectReason> {
        let active = self.stroke.transform_projection.is_some();
        if command == TransformCommand::Cancel && self.transform_active() {
            // A dispatched commit is already durable work; Escape cannot turn
            // it into a cancellation or clear the selection behind its back.
            if self.artwork_job.as_ref().is_some_and(|job| {
                matches!(
                    job.transform,
                    Some(TransformCommand::Commit { .. } | TransformCommand::Cancel)
                )
            }) {
                return Ok(false);
            }
            self.stroke.transform_gesture = None;
            self.stroke.pending_transform = None;
            self.stroke.pending_tool = None;
            self.stroke.pending_pencil_template = None;
            if self.artwork_job.is_some() {
                self.stroke.pending_transform_cancel = true;
                self.stage_cancel_availability();
                self.stroke.live_ink.request_redraw();
                return Ok(false);
            }
            if self.stroke.transform_starting && !active {
                self.stroke.transform_starting = false;
                self.stroke.pending_transform_cancel = false;
                self.stage_cancel_availability();
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
            if active || (self.stroke.transform_starting && command == TransformCommand::Paste) {
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
        // Explicit Begin/Commit need a clean native phase boundary. A native
        // Begin has already claimed this contact exclusively for transform.
        if ((!active && !self.stroke.transform_starting)
            || matches!(command, TransformCommand::Commit { .. }))
            && !self.stroke.live_ink.try_pause_for_edit()
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let (reply, received) = sync_channel(1);
        let starting = matches!(command, TransformCommand::Begin | TransformCommand::Paste);
        let pasting = command == TransformCommand::Paste;
        let result = self.enqueue_artwork(
            WorkerRequest::Edit {
                command: EditCommand::FreeTransform(command),
                target: self.active_layer,
                reply,
            },
            received,
            None,
        );
        if result.is_ok() && starting {
            self.stroke.transform_starting = true;
            self.stroke.transform_paste = pasting;
            // Explicit Begin is one semantic action: select its tool only once
            // the worker accepts the draft, without a second UI revision race.
            self.drawing.select_tool(DrawingTool::MoveSelection);
            self.stroke.live_ink.set_navigation_tool(false);
            self.projection.stage_drawing_controls(
                self.drawing.tool,
                self.drawing.size_tenths,
                self.drawing.opacity_u16,
                self.drawing.color,
                self.drawing.background_color,
            );
        }
        result
    }

    pub(super) fn flush_transform_preview(&mut self) {
        if self.artwork_job.is_some() {
            return;
        }
        let command = if self.stroke.pending_transform_cancel {
            TransformCommand::Cancel
        } else if self.stroke.transform_starting {
            TransformCommand::Begin
        } else if let Some(transform) = self.stroke.pending_transform {
            TransformCommand::Preview(transform)
        } else if self.stroke.pending_tool.is_some() {
            let Some(projection) = &self.stroke.transform_projection else {
                self.finish_transform_tool();
                return;
            };
            if !self.stroke.transform_paste && projection.transform == AffineTransform::default() {
                TransformCommand::Cancel
            } else if projection.can_commit {
                TransformCommand::Commit {
                    generation: projection.generation,
                }
            } else {
                // A rejected latest request invalidates commit authorization,
                // so explicitly regenerate the last valid displayed transform.
                self.stroke.revalidating_transform = true;
                TransformCommand::Preview(projection.transform)
            }
        } else {
            return;
        };
        let pending_tool = self.stroke.pending_tool;
        let pending_pencil_template = self.stroke.pending_pencil_template;
        match self.enqueue_free_transform(command.clone()) {
            Ok(_) => {
                // Internal Cancel must retain a tool transition; explicit Esc
                // clears it before reaching this flush.
                self.stroke.pending_tool = pending_tool;
                self.stroke.pending_pencil_template = pending_pencil_template;
                if !matches!(command, TransformCommand::Begin | TransformCommand::Paste) {
                    self.stroke.pending_transform = None;
                    self.stroke.pending_transform_cancel = false;
                }
                self.async_publication_base_revision
                    .get_or_insert(self.projection.current().revision);
                self.publish_current_projection();
            }
            Err(
                CommandRejectReason::CommandQueueBusy | CommandRejectReason::StaleProjection { .. },
            ) => {
                self.stroke.pending_tool = pending_tool;
                self.stroke.pending_pencil_template = pending_pencil_template;
                if command == TransformCommand::Cancel {
                    self.stroke.pending_transform_cancel = true;
                }
                self.stroke.live_ink.request_redraw();
            }
            Err(reason) => {
                self.stroke.pending_transform = None;
                self.stroke.pending_transform_cancel = false;
                self.stroke.pending_tool = None;
                self.stroke.pending_pencil_template = None;
                self.stroke.transform_starting = false;
                self.stroke
                    .live_ink
                    .publish_activation_notice(format!("변형을 갱신하지 못했습니다: {reason:?}"));
            }
        }
    }

    pub(super) fn reject_transform_outcome(&mut self, command: Option<&TransformCommand>) {
        if matches!(
            command,
            Some(TransformCommand::Begin | TransformCommand::Paste)
        ) {
            self.stroke.transform_starting = false;
            self.stroke.transform_gesture = None;
            self.stroke.pending_transform = None;
            self.stroke.pending_transform_cancel = false;
            self.stroke.transform_paste = false;
            if self.stroke.pending_tool.is_some() {
                self.finish_transform_tool();
            }
        } else if self.stroke.revalidating_transform {
            // A failed recovery leaves the original draft available for Cancel;
            // never loop forever or switch tools after an unsuccessful commit.
            self.stroke.pending_tool = None;
            self.stroke.pending_pencil_template = None;
        }
        self.stroke.revalidating_transform = false;
        if matches!(
            command,
            Some(TransformCommand::Commit { .. } | TransformCommand::Cancel)
        ) {
            self.stroke.pending_tool = None;
            self.stroke.pending_pencil_template = None;
        }
    }

    fn finish_transform_tool(&mut self) {
        let tool = self
            .stroke
            .pending_tool
            .take()
            .unwrap_or(DrawingTool::Lasso);
        if let Some(template) = self.stroke.pending_pencil_template.take()
            && tool == DrawingTool::Pencil
        {
            self.drawing.remember_current();
            self.drawing.pencil_template = template;
        }
        self.drawing.select_tool(tool);
        self.projection
            .stage_pencil_template(self.drawing.pencil_template);
        self.stroke
            .live_ink
            .set_navigation_tool(tool == DrawingTool::Move);
        self.projection
            .stage_brush_settings(self.drawing.brush_settings);
        self.projection.stage_drawing_controls(
            self.drawing.tool,
            self.drawing.size_tenths,
            self.drawing.opacity_u16,
            self.drawing.color,
            self.drawing.background_color,
        );
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
        self.stroke.transform_starting = false;
        self.stroke.revalidating_transform = false;
        self.transform_display = active.then_some(outcome.tiles);
        self.stroke.transform_projection = outcome.projection;
        if active {
            self.drawing.select_tool(DrawingTool::MoveSelection);
            self.stroke.live_ink.set_navigation_tool(false);
        } else {
            self.stroke.transform_gesture = None;
            self.stroke.pending_transform = None;
            self.stroke.pending_transform_cancel = false;
            self.stroke.transform_paste = false;
            self.finish_transform_tool();
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
    pub(super) fn cancel_native_gesture(&mut self) -> bool {
        let cancelled = self.edit_gesture.take().is_some()
            | self.completed_edit.take().is_some()
            | self.cancel_picker()
            | self.active.is_some();
        if !cancelled {
            return false;
        }
        if let Some(sequence) = self.completed_temporary_picker.take() {
            self.live_ink.complete_temporary_pick(sequence);
        }
        if let Some(active) = self.active.take() {
            self.scratch_dabs.clear();
            let _ = self.evaluator.end(active.token, &mut self.scratch_dabs);
            self.scratch_dabs.clear();
            self.gpu_ops.push(super::GpuStrokeOp::Finish {
                generation: active.generation,
                disposition: super::LiveStrokeDisposition::Cancel,
            });
        }
        // A late Move/End from the cancelled contact can neither complete an
        // edit nor accidentally start paint after the next tool command.
        self.awaiting_clean_begin = true;
        true
    }

    pub(super) fn begin_native_transform(&mut self, sample: StylusSample) {
        if sample.phase != PointerPhase::Begin {
            return;
        }
        let Some((origin, size)) = self
            .selection
            .as_ref()
            .and_then(|mask| mask.bounds_signed())
        else {
            return;
        };
        let x = i64::from(origin[0]) * 1000;
        let y = i64::from(origin[1]) * 1000;
        let right = x + i64::from(size[0]) * 1000;
        let bottom = y + i64::from(size[1]) * 1000;
        // Geometry is native session state only. The worker must still admit
        // the immutable artwork draft before any preview can be submitted.
        let projection = TransformProjection {
            generation: 0,
            transform: AffineTransform::default(),
            origin,
            size,
            corners_milli: [[x, y], [right, y], [right, bottom], [x, bottom]],
            can_commit: false,
        };
        let point = [sample.position_document.x, sample.position_document.y];
        let zoom = self.live_ink.canvas_viewport_snapshot().zoom;
        if let Some(gesture) =
            crate::transform_gesture::TransformGesture::begin(&projection, point, 7.0 / zoom)
        {
            self.transform_gesture =
                Some((gesture, sample.viewport_revision, projection.transform));
            self.transform_starting = true;
        }
    }

    pub(super) fn observe_transform_sample(&mut self, sample: StylusSample) {
        if self.pending_tool.is_some() || self.pending_transform_cancel {
            return;
        }
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

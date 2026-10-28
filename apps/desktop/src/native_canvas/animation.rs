use super::{
    ActiveCanvas, CommandRejectReason, EditFailure, HistoryMove, LayerId, LayerTree, ProjectDb,
    WorkerRequest, sync_channel,
};
use nyatidraw_api::{AnimationCommand, FrameId, HistoryNodeId, SnapshotId};
use nyatidraw_document::AnimationTimeline;
use nyatidraw_editor::{
    HeadlessStrokeSession,
    animation::{active_edit_tree, animation_projection, playback_frame, prepare_animation_edit},
};
use nyatidraw_tiles::TileSnapshot;
use std::time::Instant;

#[derive(Clone, Default)]
pub(super) struct State {
    pub(super) timeline: Option<AnimationTimeline>,
    pub(super) editing_frame: Option<FrameId>,
}

impl State {
    pub(super) fn restored(
        timeline: Option<AnimationTimeline>,
        tree: &LayerTree,
        layer: LayerId,
    ) -> Self {
        let editing_frame = timeline.as_ref().and_then(|timeline| {
            timeline
                .frames()
                .iter()
                .find(|frame| {
                    active_edit_tree(tree, Some(timeline), Some(frame.id))
                        .is_ok_and(|tree| tree.raster(layer).is_some())
                })
                .or_else(|| timeline.frames().first())
                .map(|frame| frame.id)
        });
        Self {
            timeline,
            editing_frame,
        }
    }

    pub(super) fn edit_tree(&self, tree: &LayerTree) -> LayerTree {
        active_edit_tree(tree, self.timeline.as_ref(), self.editing_frame)
            .expect("validated current animation frame")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum OnionScene {
    Clean,
    WithNeighbors,
}

pub(super) struct Runtime {
    pub(super) state: State,
    pub(super) display_frame: Option<FrameId>,
    pub(super) playing: Option<(FrameId, Instant)>,
    looping: bool,
    pub(super) onion_skin: bool,
    pub(super) refresh_onion: bool,
    onion_scene: OnionScene,
    clock: Option<(
        std::sync::mpsc::SyncSender<()>,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    )>,
}

impl Runtime {
    pub(super) fn new(state: State) -> Self {
        Self {
            display_frame: state.editing_frame,
            state,
            playing: None,
            looping: true,
            onion_skin: false,
            refresh_onion: false,
            onion_scene: OnionScene::Clean,
            clock: None,
        }
    }

    pub(super) fn render_tree(&self, tree: &LayerTree) -> Result<LayerTree, String> {
        match (
            &self.state.timeline,
            self.display_frame.or(self.state.editing_frame),
        ) {
            (Some(timeline), Some(frame)) => timeline
                .resolve_frame_tree(tree, frame)
                .map_err(|error| format!("frame render: {error:?}")),
            (None, _) => Ok(tree.clone()),
            _ => Err("애니메이션 프레임이 없습니다.".into()),
        }
    }
}

pub(super) struct Outcome {
    pub(super) state: State,
    pub(super) moved: HistoryMove,
    pub(super) active_layer: LayerId,
    pub(super) artwork_changed: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    command: AnimationCommand,
    state: &mut State,
    tree: &mut LayerTree,
    active: LayerId,
    canvas: nyatidraw_api::CanvasSpec,
    session: &mut HeadlessStrokeSession,
    next_id: &mut u128,
    db: &ProjectDb,
) -> Result<Outcome, EditFailure> {
    let prepared = prepare_animation_edit(
        command,
        tree,
        session.tiles(),
        state.timeline.as_ref(),
        state.editing_frame,
        active,
        canvas,
        *next_id,
    )
    .map_err(EditFailure::Rejected)?;
    if prepared.artwork_changed {
        let next = next_id
            .checked_add(1)
            .ok_or_else(|| EditFailure::Rejected("IdentifierExhausted".into()))?;
        let batch = session
            .prepare_structural_change(
                SnapshotId(*next_id),
                HistoryNodeId(*next_id),
                super::system_timestamp_ns(),
                prepared.tiles.clone(),
            )
            .map_err(|error| EditFailure::Fatal(format!("animation prepare: {error:?}")))?;
        db.commit_structural_with_document_metadata(
            &batch,
            &prepared.tree,
            canvas,
            prepared.timeline.as_ref(),
        )
        .map_err(|error| EditFailure::Fatal(format!("animation commit: {error}")))?;
        session
            .accept_structural_change(&batch)
            .map_err(|error| EditFailure::Fatal(format!("animation accept: {error:?}")))?;
        *next_id = next.max(super::next_layer_node_id(&prepared.tree).map_err(EditFailure::Fatal)?);
    }
    *tree = prepared.tree.clone();
    *state = State {
        timeline: prepared.timeline,
        editing_frame: prepared.active_frame,
    };
    Ok(Outcome {
        state: state.clone(),
        active_layer: prepared.active_layer,
        artwork_changed: prepared.artwork_changed,
        moved: HistoryMove {
            tiles: Some(prepared.tiles),
            tree: Some(prepared.tree),
            canvas: None,
            history: session.history_projection(),
            worker_timing: None,
        },
    })
}

impl ActiveCanvas {
    pub(super) fn stage_animation(&mut self) {
        self.projection.stage_animation(animation_projection(
            self.animation.state.timeline.as_ref(),
            if self.animation.playing.is_some() {
                self.animation
                    .display_frame
                    .or(self.animation.state.editing_frame)
            } else {
                self.animation.state.editing_frame
            },
            self.animation.playing.is_some(),
            self.animation.looping,
            self.animation.onion_skin,
        ));
    }

    pub(super) fn apply_animation_command(
        &mut self,
        command: AnimationCommand,
    ) -> Result<bool, CommandRejectReason> {
        match command {
            AnimationCommand::Stop => {
                self.stop_animation()?;
                return Ok(false);
            }
            AnimationCommand::SetLoop(value) => {
                self.animation.looping = value;
                return Ok(false);
            }
            AnimationCommand::SetOnionSkin(value) => {
                self.ensure_artwork_command_idle()?;
                if self.animation.playing.is_some()
                    || self.transform_active()
                    || self.color_adjustment_active()
                    || self.animation.state.timeline.is_none()
                {
                    return Err(CommandRejectReason::CommandQueueBusy);
                }
                if value {
                    self.refresh_onion_display()?;
                    self.animation.onion_skin = true;
                } else {
                    if !self.stroke.live_ink.try_pause_for_edit() {
                        return Err(CommandRejectReason::CommandQueueBusy);
                    }
                    let result = self.display_animation_frame(self.animation.state.editing_frame);
                    self.stroke.live_ink.resume_after_edit();
                    result?;
                    self.animation.onion_skin = false;
                    self.animation.refresh_onion = false;
                }
                return Ok(false);
            }
            _ => {}
        }
        if self.animation.playing.is_some() {
            self.stop_animation()?;
        }
        self.ensure_artwork_command_idle()?;
        if self.transform_active()
            || self.color_adjustment_active()
            || !self.stroke.live_ink.try_pause_for_edit()
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        if command == AnimationCommand::Play {
            return self.begin_animation_playback();
        }
        let preflight = prepare_animation_edit(
            command,
            &self.authoritative_tree,
            &TileSnapshot::empty(),
            self.animation.state.timeline.as_ref(),
            self.animation.state.editing_frame,
            self.active_layer,
            self.canvas_spec,
            self.next_layer_node_id,
        )
        .map_err(|reason| {
            self.stroke.live_ink.publish_activation_notice(reason);
            CommandRejectReason::UnsupportedCommand
        });
        let preflight = preflight.and_then(|candidate| {
            let runtime = Runtime::new(State {
                timeline: candidate.timeline,
                editing_frame: candidate.active_frame,
            });
            let tree = runtime
                .render_tree(&candidate.tree)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
            self.scene
                .validate_tree_replacement(&tree)
                .map_err(|_| CommandRejectReason::WorkspaceFailed)
        });
        if let Err(error) = preflight {
            self.stroke.live_ink.resume_after_edit();
            return Err(error);
        }
        let (reply, received) = sync_channel(1);
        self.enqueue_artwork(
            WorkerRequest::Animation {
                command,
                active: self.active_layer,
                reply,
            },
            received,
            None,
        )
    }

    fn begin_animation_playback(&mut self) -> Result<bool, CommandRejectReason> {
        let Some(frame) = self.animation.state.editing_frame else {
            self.stroke.live_ink.resume_after_edit();
            return Err(CommandRejectReason::UnsupportedCommand);
        };
        let preflight = self
            .animation
            .state
            .timeline
            .as_ref()
            .ok_or(CommandRejectReason::UnknownLayer)
            .and_then(|timeline| {
                for entry in timeline.frames() {
                    let tree = timeline
                        .resolve_frame_tree(&self.authoritative_tree, entry.id)
                        .map_err(|_| CommandRejectReason::UnknownLayer)?;
                    self.scene
                        .validate_tree_replacement(&tree)
                        .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
                }
                Ok(())
            });
        if let Err(error) = preflight {
            self.stroke.live_ink.resume_after_edit();
            return Err(error);
        }
        if let Err(error) = self
            .display_animation_frame(Some(frame))
            .and_then(|()| self.start_animation_clock())
        {
            self.stroke.live_ink.resume_after_edit();
            return Err(error);
        }
        self.animation.playing = Some((frame, Instant::now()));
        self.stroke.live_ink.request_redraw();
        Ok(false)
    }

    pub(super) fn install_animation(
        &mut self,
        outcome: Outcome,
    ) -> Result<(), CommandRejectReason> {
        self.animation.state = outcome.state;
        self.animation.display_frame = self.animation.state.editing_frame;
        self.animation.playing = None;
        if self.animation.state.timeline.is_none() {
            self.animation.onion_skin = false;
        }
        self.active_layer = outcome.active_layer;
        self.projection.stage_solo(None);
        self.install_history_move(outcome.moved)?;
        self.scene
            .set_solo(None)
            .map_err(|_| CommandRejectReason::UnknownLayer)?;
        if outcome.artwork_changed {
            let mut current = self.projection.current().clone();
            current.dirty = true;
            self.projection.install_authoritative(current);
        }
        Ok(())
    }

    fn display_animation_frame(
        &mut self,
        frame: Option<FrameId>,
    ) -> Result<(), CommandRejectReason> {
        self.animation.onion_scene = OnionScene::Clean;
        self.animation.display_frame = frame;
        let tree = self
            .animation
            .render_tree(&self.authoritative_tree)
            .map_err(|_| CommandRejectReason::UnknownLayer)?;
        self.scene
            .replace_tree(tree)
            .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        self.scene
            .replace_mask_tiles(&self.mask_state.authoritative)
            .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
        for (key, pixels) in &self.cpu_tiles {
            if self.scene.tree().raster(key.layer).is_some() {
                self.scene
                    .upload_closed_tile(*key, pixels)
                    .map_err(|_| CommandRejectReason::WorkspaceFailed)?;
            }
        }
        self.scene
            .set_solo(None)
            .map_err(|_| CommandRejectReason::UnknownLayer)?;
        Ok(())
    }

    fn stop_animation(&mut self) -> Result<(), CommandRejectReason> {
        if self.animation.playing.take().is_some() {
            if let Some((clock, running)) = &self.animation.clock {
                running.store(false, std::sync::atomic::Ordering::Release);
                let _ = clock.try_send(());
            }
            if let Err(error) = self.display_animation_frame(self.animation.state.editing_frame) {
                self.stroke.live_ink.publish_workspace_error(format!(
                    "Animation stopped but the editing frame could not be restored: {error:?}"
                ));
                return Err(error);
            }
            self.animation.refresh_onion = self.animation.onion_skin;
            self.stroke.live_ink.resume_after_edit();
        }
        Ok(())
    }

    fn start_animation_clock(&mut self) -> Result<(), CommandRejectReason> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::RecvTimeoutError,
        };
        if self.animation.clock.is_none() {
            let (sender, receiver) = sync_channel(1);
            let running = Arc::new(AtomicBool::new(false));
            let worker_running = Arc::clone(&running);
            let live_ink = self.stroke.live_ink.clone();
            std::thread::Builder::new()
                .name("nyatidraw-animation-clock".into())
                .spawn(move || {
                    loop {
                        let message = if worker_running.load(Ordering::Acquire) {
                            receiver.recv_timeout(std::time::Duration::from_millis(16))
                        } else {
                            receiver.recv().map_err(|_| RecvTimeoutError::Disconnected)
                        };
                        match message {
                            Err(RecvTimeoutError::Timeout)
                                if worker_running.load(Ordering::Acquire) =>
                            {
                                live_ink.request_redraw();
                            }
                            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                })
                .map_err(|_| CommandRejectReason::CommandQueueBusy)?;
            self.animation.clock = Some((sender, running));
        }
        let (sender, running) = self.animation.clock.as_ref().unwrap();
        running.store(true, Ordering::Release);
        match sender.try_send(()) {
            Ok(()) | Err(std::sync::mpsc::TrySendError::Full(())) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(())) => {
                Err(CommandRejectReason::CommandQueueBusy)
            }
        }
    }

    fn refresh_onion_display(&mut self) -> Result<(), CommandRejectReason> {
        self.ensure_artwork_command_idle()?;
        if !self.stroke.live_ink.try_pause_for_edit() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let (reply, received) = sync_channel(1);
        let result =
            self.enqueue_artwork(WorkerRequest::AnimationDisplay { reply }, received, None);
        if result.is_ok() {
            self.animation.refresh_onion = false;
        }
        result.map(|_| ())
    }

    pub(super) fn discard_onion_display(&mut self) -> Result<(), CommandRejectReason> {
        if self.animation.onion_scene == OnionScene::WithNeighbors {
            self.display_animation_frame(self.animation.state.editing_frame)?;
            self.scene
                .set_solo(self.projection.current().solo_node)
                .map_err(|_| CommandRejectReason::UnknownLayer)?;
            self.animation.refresh_onion = self.animation.onion_skin;
        }
        Ok(())
    }

    pub(super) fn install_onion_display(
        &mut self,
        display: nyatidraw_editor::animation_display::FrameDisplay,
    ) -> Result<(), String> {
        self.scene
            .validate_tree_replacement(&display.tree)
            .map_err(|error| format!("Onion display budget: {error:?}"))?;
        self.scene
            .replace_tree(display.tree)
            .map_err(|error| format!("Onion display: {error:?}"))?;
        self.scene
            .replace_mask_tiles(&display.tiles)
            .map_err(|error| format!("Onion masks: {error:?}"))?;
        for (key, tile) in display.tiles.iter() {
            self.scene
                .upload_closed_tile(key, tile.pixels())
                .map_err(|error| format!("Onion tile: {error:?}"))?;
        }
        self.scene
            .set_solo(self.projection.current().solo_node)
            .map_err(|error| format!("Onion solo: {error:?}"))?;
        self.animation.onion_scene = OnionScene::WithNeighbors;
        Ok(())
    }

    pub(super) fn tick_animation(&mut self) -> Result<(), CommandRejectReason> {
        let Some((start, since)) = self.animation.playing else {
            if self.animation.refresh_onion
                && self.artwork_job.is_none()
                && !self.transform_active()
                && !self.color_adjustment_active()
            {
                // A live stroke may temporarily prevent this disposable refresh.
                let _ = self.refresh_onion_display();
            }
            return Ok(());
        };
        let timeline = self
            .animation
            .state
            .timeline
            .as_ref()
            .ok_or(CommandRejectReason::UnknownLayer)?;
        let elapsed = u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (frame, ended) = playback_frame(timeline, start, elapsed, self.animation.looping)
            .map_err(|_| CommandRejectReason::UnknownLayer)?;
        if ended {
            self.stop_animation()?;
            self.publish_current_projection();
        } else if self.animation.display_frame != Some(frame) {
            self.display_animation_frame(Some(frame))?;
            self.publish_current_projection();
        }
        Ok(())
    }
}

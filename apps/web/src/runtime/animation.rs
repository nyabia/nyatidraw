use dioxus::prelude::ReadableExt;
use nyatidraw_api::{AnimationCommand, AnimationProjection, FrameId};
use nyatidraw_web_core::CanvasUpdate;

use super::{Editor, Runtime};

pub(super) struct Playback {
    start: FrameId,
    began_ms: f64,
}

impl Editor {
    pub fn animation_command(self, command: AnimationCommand) -> Result<(), String> {
        let cell = self.runtime.peek().clone();
        let persist = {
            let mut borrow = cell.borrow_mut();
            let runtime = borrow.as_mut().ok_or("그림을 여는 중입니다.")?;
            if runtime.gpu_failed {
                return Err("화면이 중단되었습니다. 먼저 작업을 저장하세요.".into());
            }
            if runtime.document.is_drawing()
                || runtime.gesture.is_some()
                || runtime.transform_drag.is_some()
                || runtime.document.transform_projection().is_some()
                || runtime.document.color_adjustment_projection().is_some()
            {
                return Err("현재 획이나 편집을 먼저 마쳐주세요.".into());
            }
            match command {
                AnimationCommand::Play => {
                    let frame = runtime
                        .document
                        .active_frame()
                        .ok_or("먼저 애니메이션을 시작하세요.")?;
                    let timeline = runtime
                        .document
                        .animation_timeline()
                        .ok_or("Missing timeline")?;
                    for cel in timeline.cels() {
                        let frame = timeline
                            .frames()
                            .iter()
                            .find(|frame| frame.cel == cel.id)
                            .ok_or("Missing cel")?;
                        let tree = timeline
                            .resolve_frame_tree(runtime.document.layers(), frame.id)
                            .map_err(|error| format!("{error:?}"))?;
                        runtime
                            .renderer
                            .validate_document(runtime.document.canvas(), &tree)?;
                    }
                    runtime.document.clear_animation_overlays();
                    runtime.document.set_animation_display(None);
                    runtime.sync_animation_scene()?;
                    runtime.animation_playback = Some(Playback {
                        start: frame,
                        began_ms: now_ms(),
                    });
                    false
                }
                AnimationCommand::Stop => {
                    runtime.stop_animation()?;
                    false
                }
                AnimationCommand::SetLoop(value) => {
                    runtime.animation_loop = value;
                    false
                }
                AnimationCommand::SetOnionSkin(value) => {
                    if runtime.animation_playback.is_some() {
                        return Err("먼저 재생을 멈춰주세요.".into());
                    }
                    if value && runtime.document.animation_timeline().is_none() {
                        return Err("먼저 애니메이션을 시작하세요.".into());
                    }
                    runtime.rebuild_onion(value)?;
                    runtime.sync_animation_scene()?;
                    runtime.animation_onion = value;
                    false
                }
                command => {
                    if runtime.animation_playback.is_some() {
                        return Err("먼저 애니메이션 재생을 멈춰주세요.".into());
                    }
                    let renderer = &runtime.renderer;
                    let update = runtime
                        .document
                        .apply_animation_command_checked(command, |canvas, tree| {
                            renderer.validate_document(canvas, tree)
                        })
                        .map_err(|error| error.to_string())?;
                    if let Err(error) = runtime.apply(&update) {
                        runtime.gpu_failed = true;
                        drop(borrow);
                        self.persist();
                        self.refresh();
                        return Err(error);
                    }
                    (update.committed && update.changed)
                        || matches!(command, AnimationCommand::Select(_))
                }
            }
        };
        self.refresh();
        self.redraw();
        if persist {
            self.persist();
        }
        Ok(())
    }
}

impl Runtime {
    pub fn animation_projection(&self) -> AnimationProjection {
        nyatidraw_editor::animation::animation_projection(
            self.document.animation_timeline(),
            self.document.display_frame(),
            self.animation_playback.is_some(),
            self.animation_loop,
            self.animation_onion,
        )
    }

    pub(super) fn rebuild_onion(&mut self, enabled: bool) -> Result<(), String> {
        if !enabled
            || self.animation_playback.is_some()
            || self.document.is_drawing()
            || self.document.transform_projection().is_some()
            || self.document.color_adjustment_projection().is_some()
        {
            self.document.set_animation_display(None);
            return Ok(());
        }
        let display = nyatidraw_editor::animation_display::build_frame_display(
            self.document.layers(),
            self.document.snapshot(),
            self.document.animation_timeline(),
            self.document.active_frame(),
            self.document.canvas(),
            true,
            128 * 1024 * 1024,
        )?;
        self.renderer
            .validate_document(self.document.canvas(), &display.tree)?;
        self.document.set_animation_display(Some(display));
        Ok(())
    }

    fn stop_animation(&mut self) -> Result<(), String> {
        self.animation_playback = None;
        self.document
            .set_playback_frame(None)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.apply(&CanvasUpdate {
            structure_changed: true,
            ..CanvasUpdate::default()
        }) {
            self.gpu_failed = true;
            return Err(error);
        }
        Ok(())
    }

    fn sync_animation_scene(&mut self) -> Result<(), String> {
        if let Err(error) = self.renderer.sync_document(&self.document) {
            self.animation_playback = None;
            self.gpu_failed = true;
            let _ = self.document.set_playback_frame(None);
            return Err(error);
        }
        Ok(())
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) fn advance_animation(&mut self) -> Result<bool, String> {
        let Some(playback) = &self.animation_playback else {
            return Ok(false);
        };
        let timeline = self
            .document
            .animation_timeline()
            .ok_or("Missing playback timeline")?;
        let elapsed = (now_ms() - playback.began_ms).max(0.0) as u64;
        let (frame, stopped) = nyatidraw_editor::animation::playback_frame(
            timeline,
            playback.start,
            elapsed,
            self.animation_loop,
        )?;
        if stopped {
            self.stop_animation()?;
            return Ok(true);
        }
        if self.document.display_frame() == Some(frame) {
            return Ok(false);
        }
        let tree = timeline
            .resolve_frame_tree(self.document.layers(), frame)
            .map_err(|error| format!("{error:?}"))?;
        self.renderer
            .validate_document(self.document.canvas(), &tree)?;
        self.document
            .set_playback_frame(Some(frame))
            .map_err(|error| error.to_string())?;
        self.sync_animation_scene()?;
        Ok(true)
    }
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or_else(js_sys::Date::now, |performance| performance.now())
}

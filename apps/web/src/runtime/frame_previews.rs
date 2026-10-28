use dioxus::prelude::ReadableExt;
use nyatidraw_api::FrameId;
use nyatidraw_editor_ui::preview::{
    FrameThumbnailCache, FrameThumbnailRequest, FrameThumbnailSnapshot, frame_thumbnail_frame,
};
use wasm_bindgen_futures::{JsFuture, spawn_local};

use super::{Editor, Runtime, browser};

#[derive(Default)]
pub(super) struct State {
    pub(super) cache: FrameThumbnailCache,
    running: bool,
}

impl Editor {
    pub fn frame_thumbnail_snapshot(self) -> FrameThumbnailSnapshot {
        self.read(|runtime| runtime.frame_previews.cache.snapshot())
            .unwrap_or_default()
    }

    pub fn request_frame_thumbnails(self, frames: &[FrameId]) {
        let cell = self.runtime.peek().clone();
        if let Some(runtime) = cell.borrow_mut().as_mut() {
            runtime
                .frame_previews
                .cache
                .request(runtime.project_epoch, frames);
        }
        self.queue_frame_previews();
    }

    pub(super) fn queue_frame_previews(self) {
        let cell = self.runtime.peek().clone();
        let request = {
            let mut borrow = cell.borrow_mut();
            let Some(runtime) = borrow.as_mut() else {
                return;
            };
            if runtime.frame_previews.running || runtime.frame_previews_paused() {
                return;
            }
            let Some(request) = runtime.frame_previews.cache.next_request() else {
                return;
            };
            runtime.frame_previews.running = true;
            request
        };
        spawn_local(async move {
            let _ = JsFuture::from(browser::background_turn(16.0)).await;
            let changed = {
                let mut borrow = cell.borrow_mut();
                let Some(runtime) = borrow.as_mut() else {
                    return;
                };
                runtime.frame_previews.running = false;
                if runtime.frame_previews_paused() {
                    return;
                }
                if runtime.frame_previews.cache.is_current(&request) {
                    let result = runtime.render_frame_thumbnail(&request);
                    if let Err(error) = &result {
                        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(error));
                    }
                    runtime.frame_previews.cache.finish(&request, result.ok())
                } else {
                    false
                }
            };
            if changed {
                self.refresh();
            }
            self.queue_frame_previews();
        });
    }
}

impl Runtime {
    fn frame_previews_paused(&self) -> bool {
        self.document.is_drawing()
            || self.animation_playback.is_some()
            || self.gpu_failed
            || self.document.transform_projection().is_some()
            || self.document.color_adjustment_projection().is_some()
    }

    fn render_frame_thumbnail(
        &self,
        request: &FrameThumbnailRequest,
    ) -> Result<
        (
            nyatidraw_tiles::ObjectHash,
            nyatidraw_editor_ui::preview::FrameThumbnailFrame,
        ),
        String,
    > {
        let source = nyatidraw_editor::frame_preview::FramePreviewSource::prepare(
            self.document.layers(),
            self.document.snapshot(),
            self.document
                .animation_timeline()
                .ok_or("No animation timeline")?,
            request.frame,
            self.document.canvas(),
        )?;
        let frame = if let Some(frame) = request.reuse(source.fingerprint) {
            frame
        } else {
            frame_thumbnail_frame(request, &source.render(self.document.snapshot())?)?
        };
        Ok((source.fingerprint, frame))
    }
}

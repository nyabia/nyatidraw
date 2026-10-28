use nyatidraw_api::FrameId;
use nyatidraw_editor_ui::preview::{
    FrameThumbnailFrame, FrameThumbnailRequest, FrameThumbnailSnapshot,
};
use nyatidraw_tiles::ObjectHash;

use super::LiveInkBridge;

impl LiveInkBridge {
    pub(crate) fn frame_thumbnail_snapshot(&self) -> FrameThumbnailSnapshot {
        self.inner
            .frame_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot()
    }

    pub(crate) fn request_frame_thumbnails(&self, frames: &[FrameId]) {
        self.inner
            .frame_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .request(self.project_epoch(), frames);
        self.request_redraw();
    }

    pub(crate) fn invalidate_frame_thumbnails(&self) {
        self.inner
            .frame_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .invalidate();
        self.notify_ui();
        self.request_redraw();
    }

    pub(crate) fn next_frame_thumbnail_request(&self) -> Option<FrameThumbnailRequest> {
        self.inner
            .frame_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_request()
    }

    pub(crate) fn finish_frame_thumbnail(
        &self,
        request: &FrameThumbnailRequest,
        result: Option<(ObjectHash, FrameThumbnailFrame)>,
    ) {
        let changed = self
            .inner
            .frame_thumbnails
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .finish(request, result);
        if changed {
            self.notify_ui();
        }
    }
}

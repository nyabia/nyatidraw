use std::sync::mpsc::{Receiver, TryRecvError};

use nyatidraw_editor_ui::preview::{
    FrameThumbnailFrame, FrameThumbnailRequest, frame_thumbnail_frame,
};
use nyatidraw_tiles::ObjectHash;

use super::{ActiveCanvas, CanvasSpec, LayerTree, TileSnapshot, WorkerRequest, sync_channel};

pub(super) type Result = std::result::Result<(ObjectHash, FrameThumbnailFrame), String>;

pub(super) struct Job {
    request: FrameThumbnailRequest,
    received: Receiver<Result>,
}

impl ActiveCanvas {
    pub(super) fn poll_frame_previews(&mut self) {
        if let Some(job) = self.frame_preview_job.take() {
            match job.received.try_recv() {
                Ok(result) => {
                    if let Err(error) = &result {
                        eprintln!("frame-thumbnail-skipped error={error}");
                    }
                    self.stroke
                        .live_ink
                        .finish_frame_thumbnail(&job.request, result.ok());
                }
                Err(TryRecvError::Empty) => {
                    self.frame_preview_job = Some(job);
                    return;
                }
                Err(TryRecvError::Disconnected) => self
                    .stroke
                    .live_ink
                    .finish_frame_thumbnail(&job.request, None),
            }
        }
        if self.animation.playing.is_some()
            || self.stroke.transform_projection.is_some()
            || self.color_adjustment_active()
            || self.ensure_pixel_state_idle().is_err()
        {
            return;
        }
        let Some(request) = self.stroke.live_ink.next_frame_thumbnail_request() else {
            return;
        };
        let (reply, received) = sync_channel(1);
        let queued = self
            .stroke
            .materializer
            .sender
            .as_ref()
            .is_some_and(|sender| {
                sender
                    .try_send(WorkerRequest::FrameThumbnail {
                        request: request.clone(),
                        reply,
                    })
                    .is_ok()
            });
        if queued {
            self.frame_preview_job = Some(Job { request, received });
        }
    }
}

pub(super) fn render(
    request: &FrameThumbnailRequest,
    tree: &LayerTree,
    tiles: &TileSnapshot,
    timeline: Option<&nyatidraw_document::AnimationTimeline>,
    canvas: CanvasSpec,
) -> Result {
    let source = nyatidraw_editor::frame_preview::FramePreviewSource::prepare(
        tree,
        tiles,
        timeline.ok_or("No animation timeline")?,
        request.frame,
        canvas,
    )?;
    let frame = if let Some(frame) = request.reuse(source.fingerprint) {
        frame
    } else {
        frame_thumbnail_frame(request, &source.render(tiles)?)?
    };
    Ok((source.fingerprint, frame))
}

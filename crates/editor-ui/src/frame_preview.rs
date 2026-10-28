use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use nyatidraw_api::FrameId;
use nyatidraw_tiles::{FlattenedRgba8, ObjectHash};

const MAX_REQUESTED: usize = 24;
const MAX_CACHED: usize = 64;
const MAX_DATA_URI_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameThumbnailFrame {
    pub project_epoch: u64,
    pub generation: u64,
    pub frame: FrameId,
    pub width: u16,
    pub height: u16,
    pub data_uri: Arc<str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrameThumbnailSnapshot {
    pub project_epoch: u64,
    pub generation: u64,
    pub frames: BTreeMap<FrameId, Arc<FrameThumbnailFrame>>,
}

impl FrameThumbnailSnapshot {
    #[must_use]
    pub fn frame(&self, id: FrameId) -> Option<Arc<FrameThumbnailFrame>> {
        self.frames
            .get(&id)
            .filter(|frame| {
                frame.project_epoch == self.project_epoch && frame.generation == self.generation
            })
            .cloned()
    }
}

#[derive(Clone)]
pub struct FrameThumbnailRequest {
    pub project_epoch: u64,
    pub generation: u64,
    pub frame: FrameId,
    pub cached: Option<(ObjectHash, Arc<FrameThumbnailFrame>)>,
}

impl FrameThumbnailRequest {
    #[must_use]
    pub fn reuse(&self, fingerprint: ObjectHash) -> Option<FrameThumbnailFrame> {
        self.cached
            .as_ref()
            .filter(|(key, _)| *key == fingerprint)
            .map(|(_, frame)| FrameThumbnailFrame {
                project_epoch: self.project_epoch,
                generation: self.generation,
                frame: self.frame,
                width: frame.width,
                height: frame.height,
                data_uri: Arc::clone(&frame.data_uri),
            })
    }
}

/// Bounded presentation state. Cached entries contain no durable pixel objects.
#[derive(Default)]
pub struct FrameThumbnailCache {
    snapshot: FrameThumbnailSnapshot,
    requested: Vec<FrameId>,
    attempted: BTreeSet<FrameId>,
    entries: BTreeMap<FrameId, (ObjectHash, Arc<FrameThumbnailFrame>)>,
    recency: VecDeque<FrameId>,
}

impl FrameThumbnailCache {
    pub fn request(&mut self, project_epoch: u64, frames: &[FrameId]) {
        if self.snapshot.project_epoch != project_epoch {
            *self = Self::default();
            self.snapshot.project_epoch = project_epoch;
        }
        let mut requested = Vec::new();
        for frame in frames.iter().copied().take(MAX_REQUESTED) {
            if !requested.contains(&frame) {
                requested.push(frame);
            }
        }
        self.snapshot
            .frames
            .retain(|frame, _| requested.contains(frame));
        self.attempted.retain(|frame| requested.contains(frame));
        self.requested = requested;
    }

    pub fn invalidate(&mut self) {
        self.snapshot.generation = self.snapshot.generation.saturating_add(1);
        self.snapshot.frames.clear();
        self.attempted.clear();
    }

    #[must_use]
    pub fn snapshot(&self) -> FrameThumbnailSnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn next_request(&self) -> Option<FrameThumbnailRequest> {
        self.requested
            .iter()
            .find(|id| !self.attempted.contains(*id))
            .map(|frame| FrameThumbnailRequest {
                project_epoch: self.snapshot.project_epoch,
                generation: self.snapshot.generation,
                frame: *frame,
                cached: self.entries.get(frame).cloned(),
            })
    }

    #[must_use]
    pub fn is_current(&self, request: &FrameThumbnailRequest) -> bool {
        request.project_epoch == self.snapshot.project_epoch
            && request.generation == self.snapshot.generation
            && self.requested.contains(&request.frame)
    }

    pub fn finish(
        &mut self,
        request: &FrameThumbnailRequest,
        result: Option<(ObjectHash, FrameThumbnailFrame)>,
    ) -> bool {
        if !self.is_current(request) {
            return false;
        }
        self.attempted.insert(request.frame);
        if let Some((fingerprint, frame)) = result {
            if frame.project_epoch != request.project_epoch
                || frame.generation != request.generation
                || frame.frame != request.frame
                || frame.data_uri.len() > MAX_DATA_URI_BYTES
            {
                return false;
            }
            let frame = Arc::new(frame);
            self.entries
                .insert(request.frame, (fingerprint, Arc::clone(&frame)));
            self.snapshot.frames.insert(request.frame, frame);
            self.recency.retain(|id| *id != request.frame);
            self.recency.push_back(request.frame);
            while self.entries.len() > MAX_CACHED {
                let evict = self
                    .recency
                    .iter()
                    .position(|id| !self.requested.contains(id));
                if let Some(oldest) = evict.and_then(|index| self.recency.remove(index)) {
                    self.entries.remove(&oldest);
                } else {
                    break;
                }
            }
        }
        true
    }
}

/// # Errors
/// Rejects oversized thumbnails or PNG encoding failure without altering artwork.
pub fn frame_thumbnail_frame(
    request: &FrameThumbnailRequest,
    surface: &FlattenedRgba8,
) -> Result<FrameThumbnailFrame, String> {
    if surface.width > 48 || surface.height > 36 {
        return Err("Frame thumbnail size limit".into());
    }
    let png = nyatidraw_png_io::encode_png_bytes(surface).map_err(|error| error.to_string())?;
    Ok(FrameThumbnailFrame {
        project_epoch: request.project_epoch,
        generation: request.generation,
        frame: request.frame,
        width: u16::try_from(surface.width).map_err(|error| error.to_string())?,
        height: u16::try_from(surface.height).map_err(|error| error.to_string())?,
        data_uri: super::png_data_uri(&png, MAX_DATA_URI_BYTES, "frame-thumbnail")?,
    })
}

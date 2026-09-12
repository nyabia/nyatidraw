//! Disposable native picker feedback. Neither candidates nor pixels are editor commands.
use super::{ActiveCanvas, LayerId, LiveInkBridge, StrokePipeline, WorkerRequest};
use crate::edit_worker::{EditFailure, PickerSource, sample_picker_pixel};
use nyatidraw_input::{Point, StylusSample};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender, TryRecvError, sync_channel},
};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(33);
const PATCH_SIZE: u16 = 13;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PickerFrame {
    pub(crate) size: u16,
    pub(crate) data_uri: Arc<str>,
    /// Opaque sRGB, exactly as a successful release would set foreground RGB.
    /// Transparent artwork has no candidate; the PNG still shows its neighbors.
    pub(crate) candidate: Option<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PickerSnapshot {
    /// `WebView` client CSS pixels, including the native canvas layout origin.
    pub(crate) cursor_css: [f64; 2],
    pub(crate) frame: Option<Arc<PickerFrame>>,
    pub(crate) pending: bool,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PickerToken {
    pub(super) device: u64,
    pub(super) begin: u64,
    pub(super) epoch: u64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct PickerRead {
    token: PickerToken,
    point: [i32; 2],
    target: LayerId,
    source: PickerSource,
}

pub(super) struct PickerGesture {
    pub(super) token: PickerToken,
    pub(super) source: PickerSource,
    pub(super) released: bool,
    viewport_revision: u64,
    latest: PickerRead,
    cursor: Point,
    sampled: Option<PickerRead>,
    frame: Option<Arc<PickerFrame>>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct PickerRuntime {
    pub(super) gesture: Option<PickerGesture>,
    job: Option<PickerPreviewJob>,
    submitted: Option<Instant>,
    wake: Option<PickerWake>,
}

struct PickerPreviewJob {
    request: PickerRead,
    received: Receiver<Result<PickerFrame, String>>,
}

/// One lazy timer per canvas, not a thread or a queued UI update per raw sample.
/// A single outstanding deadline only wakes the render actor to read its latest point.
struct PickerWake {
    sender: SyncSender<Duration>,
    pending: Arc<AtomicBool>,
}

impl PickerWake {
    fn new(ink: LiveInkBridge) -> Option<Self> {
        let (sender, receiver) = sync_channel(1);
        let pending = Arc::new(AtomicBool::new(false));
        let waiting = Arc::clone(&pending);
        std::thread::Builder::new()
            .name("picker-preview-wake".into())
            .spawn(move || {
                while let Ok(delay) = receiver.recv() {
                    std::thread::sleep(delay);
                    waiting.store(false, Ordering::Release);
                    ink.request_redraw();
                }
            })
            .ok()?;
        Some(Self { sender, pending })
    }

    fn schedule(&self, delay: Duration) {
        if !self.pending.swap(true, Ordering::AcqRel) && self.sender.try_send(delay).is_err() {
            self.pending.store(false, Ordering::Release);
        }
    }
}

impl StrokePipeline {
    pub(super) fn begin_picker(
        &mut self,
        sample: StylusSample,
        point: [i32; 2],
        epoch: u64,
        source: PickerSource,
    ) {
        let token = PickerToken {
            device: sample.device_id,
            begin: sample.sequence,
            epoch,
        };
        self.picker.gesture = Some(PickerGesture {
            token,
            source,
            released: false,
            viewport_revision: sample.viewport_revision,
            latest: PickerRead {
                token,
                point,
                target: self.selected_layer,
                source,
            },
            cursor: sample.position_document,
            sampled: None,
            frame: None,
            error: None,
        });
    }

    pub(super) fn move_picker(&mut self, sample: StylusSample, point: Option<[i32; 2]>) {
        if let Some(gesture) = &mut self.picker.gesture {
            if let Some(point) = point {
                gesture.latest.point = point;
                gesture.cursor = sample.position_document;
            } else {
                self.cancel_picker();
            }
        }
    }

    pub(super) fn picker_token_valid(&self, token: PickerToken) -> bool {
        self.live_ink.picker_epoch() == token.epoch
            && !self.live_ink.is_closing()
            && !self.live_ink.workspace_failed()
            && self
                .picker
                .gesture
                .as_ref()
                .is_some_and(|gesture| gesture.token == token)
    }

    pub(super) fn cancel_picker(&mut self) -> bool {
        let pending = matches!(
            self.completed_edit,
            Some(Ok(nyatidraw_api::EditCommand::PickColor { .. }
                | nyatidraw_api::EditCommand::PickDisplayColor { .. }))
        );
        // An End retained behind a full writer must lose its ownership along
        // with the gesture, never retry later as an untagged ordinary edit.
        if pending {
            self.completed_edit = None;
            if let Some(sequence) = self.completed_temporary_picker.take() {
                self.live_ink.complete_temporary_pick(sequence);
            }
        }
        let gesture_cancelled = self.picker.gesture.take().is_some();
        if gesture_cancelled {
            self.edit_gesture = None;
        }
        let cancelled = gesture_cancelled || pending;
        self.live_ink.publish_picker_snapshot(None);
        cancelled
    }
}

impl ActiveCanvas {
    pub(super) fn update_picker_preview(&mut self) {
        let viewport = self.stroke.live_ink.canvas_viewport_snapshot();
        if self.stroke.picker.gesture.as_ref().is_some_and(|gesture| {
            !self.stroke.picker_token_valid(gesture.token)
                || gesture.viewport_revision != viewport.revision
        }) {
            self.stroke.cancel_picker();
        }
        if let Some(job) = &self.stroke.picker.job {
            let request = job.request;
            let result = match job.received.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => {
                    Some(Err("색상 미리보기를 읽지 못했습니다.".into()))
                }
            };
            if let Some(result) = result {
                if let Some(gesture) = &mut self.stroke.picker.gesture
                    && !gesture.released
                    && gesture.latest == request
                {
                    gesture.sampled = Some(request);
                    match result {
                        Ok(frame) => {
                            gesture.error = frame.candidate.is_none().then(|| "투명한 픽셀".into());
                            gesture.frame = Some(Arc::new(frame));
                        }
                        Err(error) => {
                            gesture.frame = None;
                            gesture.error = Some(error);
                        }
                    }
                }
                self.stroke.picker.job = None;
            }
        }
        let Some(gesture) = &self.stroke.picker.gesture else {
            return;
        };
        if gesture.released {
            self.stroke.live_ink.publish_picker_snapshot(None);
            return;
        }
        let latest = gesture.latest;
        let pending = gesture.sampled != Some(latest);
        // Never label a patch sampled elsewhere as the cursor's current candidate.
        let frame = (!pending).then(|| gesture.frame.clone()).flatten();
        let error = (!pending).then(|| gesture.error.clone()).flatten();
        if let Some(cursor_css) = self.stroke.live_ink.picker_client_css(gesture.cursor) {
            self.stroke
                .live_ink
                .publish_picker_snapshot(Some(PickerSnapshot {
                    cursor_css,
                    frame,
                    pending,
                    error,
                }));
        }
        if !pending
            || self.stroke.picker.job.is_some()
            || !self.stroke.pending_materializations.is_empty()
            || self.stroke.materializer.pending.load(Ordering::Acquire) != 0
        {
            return;
        }
        let delay = self.stroke.picker.submitted.map_or(Duration::ZERO, |time| {
            INTERVAL.saturating_sub(time.elapsed())
        });
        if !delay.is_zero() {
            if self.stroke.picker.wake.is_none() {
                self.stroke.picker.wake = PickerWake::new(self.stroke.live_ink.clone());
            }
            if let Some(wake) = &self.stroke.picker.wake {
                wake.schedule(delay);
                return;
            }
        }
        let (reply, received) = sync_channel(1);
        if self
            .stroke
            .materializer
            .sender
            .as_ref()
            .is_some_and(|sender| {
                sender
                    .try_send(WorkerRequest::PickerPreview {
                        request: latest,
                        reply,
                    })
                    .is_ok()
            })
        {
            self.stroke.picker.job = Some(PickerPreviewJob {
                request: latest,
                received,
            });
            self.stroke.picker.submitted = Some(Instant::now());
        }
    }
}

pub(super) fn sample_patch(
    request: PickerRead,
    tiles: &nyatidraw_tiles::TileSnapshot,
    tree: &nyatidraw_document::LayerTree,
) -> Result<PickerFrame, String> {
    let half = i32::from(PATCH_SIZE / 2);
    let mut pixels = Vec::with_capacity(usize::from(PATCH_SIZE).pow(2) * 4);
    let mut candidate = None;
    for y in -half..=half {
        for x in -half..=half {
            let point = [
                request.point[0].checked_add(x),
                request.point[1].checked_add(y),
            ];
            let pixel = if let [Some(x), Some(y)] = point {
                sample_picker_pixel(tiles, tree, request.target, [x, y], request.source).map_err(
                    |error| match error {
                        EditFailure::Rejected(error) | EditFailure::Fatal(error) => error,
                    },
                )?
            } else {
                [0; 4]
            };
            if x == 0 && y == 0 {
                candidate = crate::edit_worker::picker_color(pixel);
            }
            pixels.extend_from_slice(&pixel);
        }
    }
    let surface = nyatidraw_tiles::FlattenedRgba8 {
        origin_x: i64::from(request.point[0]) - i64::from(half),
        origin_y: i64::from(request.point[1]) - i64::from(half),
        width: u32::from(PATCH_SIZE),
        height: u32::from(PATCH_SIZE),
        pixels,
    };
    let png = nyatidraw_png_io::encode_png_bytes(&surface).map_err(|error| error.to_string())?;
    Ok(PickerFrame {
        size: PATCH_SIZE,
        candidate,
        data_uri: crate::preview::png_data_uri(&png, 4096, "picker")?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{
        LIVE_LAYER, TILE_BYTE_LEN, TileKey, TileSnapshot, default_layer_tree, empty_raster,
    };
    use super::*;
    use nyatidraw_api::{EditSource, LayerTreeNodeId};
    use nyatidraw_document::LayerTreeNode;

    #[test]
    fn picker_source_cannot_misreport_the_color_used_by_the_next_stroke() {
        // Product risk: a preview sampled from the page, wrong source, or Solo
        // disagrees with release and silently changes the next painted color.
        let mut tree = default_layer_tree();
        let mut tiles = std::collections::BTreeMap::new();
        for (layer, reference, visible, color) in [
            (LIVE_LAYER, false, true, [128, 0, 0, 128]),
            (LayerId(2), true, true, [0, 0, 255, 255]),
            (LayerId(3), false, true, [0, 255, 0, 255]),
            (LayerId(4), false, false, [255, 0, 255, 255]),
        ] {
            if layer != LIVE_LAYER {
                let mut raster = empty_raster(layer, "picker fixture".into());
                raster.reference = reference;
                raster.visible = visible;
                tree.insert(
                    tree.root_id(),
                    tree.root().children.len(),
                    LayerTreeNode::Raster(raster),
                )
                .unwrap();
            }
            let mut pixels = vec![0; TILE_BYTE_LEN];
            pixels[TILE_BYTE_LEN - 4..].copy_from_slice(&color);
            tiles.insert(TileKey::from_pixel(layer, 128, -1, -1), pixels);
        }
        let tiles = TileSnapshot::from_tiles(tiles).unwrap();
        let original_root = tiles.root();
        let original_tree = tree.clone();
        for (source, expected) in [
            (
                PickerSource::Artwork(EditSource::ActiveLayer),
                [255, 0, 0, 255],
            ),
            (
                PickerSource::Artwork(EditSource::ReferenceLayers),
                [0, 0, 255, 255],
            ),
            (
                PickerSource::Artwork(EditSource::AllVisible),
                [0, 255, 0, 255],
            ),
            (PickerSource::Display(None), [0, 255, 0, 255]),
            (
                PickerSource::Display(Some(LayerTreeNodeId::Raster(LIVE_LAYER))),
                [255, 0, 0, 255],
            ),
        ] {
            let request = PickerRead {
                token: PickerToken {
                    device: 1,
                    begin: 1,
                    epoch: 0,
                },
                point: [-1, -1],
                target: LIVE_LAYER,
                source,
            };
            let preview = sample_patch(request, &tiles, &tree).unwrap();
            assert_eq!(preview.candidate, Some(expected));
            let release = sample_picker_pixel(&tiles, &tree, LIVE_LAYER, request.point, source)
                .unwrap_or_else(|_| panic!("valid picker source"));
            assert_eq!(crate::edit_worker::picker_color(release), preview.candidate);
            assert!(preview.data_uri.len() <= 4096);
            for point in [[0, 0], [i32::MIN, i32::MAX - 1]] {
                assert_eq!(
                    sample_patch(PickerRead { point, ..request }, &tiles, &tree)
                        .unwrap()
                        .candidate,
                    None
                );
            }
        }
        assert_eq!(
            tiles.root(),
            original_root,
            "read previews cannot mutate artwork"
        );
        assert_eq!(tree, original_tree);
    }
}

use super::{
    ActiveCanvas, Arc, BTreeSet, CanvasSpec, DrawingTool, EditCommand, LayerId, LayerTree,
    Receiver, SyncSender, TileKey, TileSnapshot, TrySendError, WorkerRequest, sync_channel,
};
use nyatidraw_editor::pixel_edit::{MemoryArtworkClipboard, execute_pixel_edit};
use nyatidraw_paint_cpu::{EditLimits, SelectionMask};

type Token = ((u64, u64, u64), LayerId);

#[derive(Default)]
pub(super) struct State {
    token: Option<Token>,
    source: Option<Arc<Source>>,
    job: Option<Job>,
    displayed: Option<TileSnapshot>,
    touched: BTreeSet<TileKey>,
    masks_touched: bool,
    selection_touched: bool,
    offset: [i32; 2],
}

struct Source {
    tiles: TileSnapshot,
    tree: LayerTree,
    selection: Option<Arc<SelectionMask>>,
    canvas: CanvasSpec,
}

struct Job {
    token: Token,
    offset: [i32; 2],
    received: Receiver<Result<Preview, String>>,
}

pub(super) struct Request {
    target: LayerId,
    offset: [i32; 2],
    source: Option<Arc<Source>>,
    reply: SyncSender<Result<Preview, String>>,
}

struct Preview {
    source: Arc<Source>,
    tiles: TileSnapshot,
    selection: Option<Arc<SelectionMask>>,
}

pub(super) fn execute(
    request: Request,
    tree: &LayerTree,
    tiles: &TileSnapshot,
    selection: Option<&Arc<SelectionMask>>,
    canvas: CanvasSpec,
) {
    let source = request.source.unwrap_or_else(|| {
        Arc::new(Source {
            tiles: tiles.clone(),
            tree: tree.clone(),
            selection: selection.cloned(),
            canvas,
        })
    });
    let result = if source.tiles.root() != tiles.root()
        || source.tree != *tree
        || !same_selection(source.selection.as_ref(), selection)
        || source.canvas != canvas
    {
        Err("그림이 변경되어 이동 미리보기를 취소했습니다.".into())
    } else {
        execute_pixel_edit(
            EditCommand::MoveArtwork {
                offset: request.offset,
            },
            request.target,
            source.canvas,
            &source.tree,
            &source.selection,
            &source.tiles,
            0,
            &mut MemoryArtworkClipboard::default(),
            EditLimits::default(),
        )
        .map(|edited| Preview {
            tiles: edited.tiles.unwrap_or_else(|| source.tiles.clone()),
            selection: edited.selection,
            source,
        })
        .map_err(|error| error.0)
    };
    let _ = request.reply.send(result);
}

fn same_selection(left: Option<&Arc<SelectionMask>>, right: Option<&Arc<SelectionMask>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right) || left == right,
        (None, None) => true,
        _ => false,
    }
}

fn changed_raster_keys(
    before: &TileSnapshot,
    after: &TileSnapshot,
    target: LayerId,
) -> BTreeSet<TileKey> {
    before
        .iter()
        .chain(after.iter())
        .filter_map(|(key, _)| (key.layer == target).then_some(key))
        .filter(|key| {
            before.get(*key).map(nyatidraw_tiles::TileObject::hash)
                != after.get(*key).map(nyatidraw_tiles::TileObject::hash)
        })
        .collect()
}

fn same_coverage(
    before: &TileSnapshot,
    after: &TileSnapshot,
    mask: Option<nyatidraw_api::MaskId>,
) -> bool {
    let Some(mask) = mask else {
        return true;
    };
    before
        .coverage_iter()
        .filter(|(key, _)| key.mask == mask)
        .map(|(key, tile)| (key, tile.hash()))
        .eq(after
            .coverage_iter()
            .filter(|(key, _)| key.mask == mask)
            .map(|(key, tile)| (key, tile.hash())))
}

impl ActiveCanvas {
    pub(super) fn update_artwork_move_preview(&mut self) {
        if let Err(error) = self.update_artwork_move_display() {
            self.stroke.cancel_native_gesture();
            self.artwork_move.token = None;
            self.artwork_move.source = None;
            if let Err(restore) = self.restore_artwork_move_display() {
                self.stroke.live_ink.publish_workspace_error(restore);
            }
            self.stroke.live_ink.publish_activation_notice(error);
        }
    }

    fn artwork_move_gesture(&self) -> Option<(Token, [i32; 2])> {
        if self.drawing.tool != DrawingTool::MoveArtwork
            || self.stroke.temporary_picker_active
            || self.stroke.selected_mask.is_some()
        {
            return None;
        }
        let gesture = self.stroke.edit_gesture.as_ref()?;
        let (points, _) = gesture.preview();
        let [start, end] = points else {
            return None;
        };
        if self.stroke.edit_gesture_identity.2
            != self.stroke.live_ink.canvas_viewport_snapshot().revision
        {
            return None;
        }
        Some((
            (self.stroke.edit_gesture_identity, self.active_layer),
            [end[0].checked_sub(start[0])?, end[1].checked_sub(start[1])?],
        ))
    }

    fn update_artwork_move_display(&mut self) -> Result<(), String> {
        let gesture = self.artwork_move_gesture();
        if gesture.is_none()
            && self.drawing.tool == DrawingTool::MoveArtwork
            && !self.stroke.temporary_picker_active
            && self.stroke.edit_gesture.is_some()
        {
            self.stroke.cancel_native_gesture();
        }
        let token = gesture.map(|(token, _)| token);
        if self.artwork_move.token != token {
            self.restore_artwork_move_display()?;
            self.artwork_move.token = token;
            self.artwork_move.source = None;
            self.artwork_move.offset = [0; 2];
        }
        if let Some(job) = &self.artwork_move.job {
            match job.received.try_recv() {
                Ok(result) => {
                    let job = self.artwork_move.job.take().expect("observed preview job");
                    if Some(job.token) == token {
                        let preview = result?;
                        self.artwork_move.source = Some(preview.source);
                        self.show_artwork_move_tiles(&preview.tiles)?;
                        if preview.selection.is_some() || self.selection.is_some() {
                            self.artwork_move.selection_touched = true;
                            self.show_artwork_move_selection(preview.selection.as_deref())?;
                        }
                        self.artwork_move.displayed = Some(preview.tiles);
                        self.artwork_move.offset = job.offset;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.artwork_move.job = None;
                    return Err("그림 이동 미리보기 작업자가 종료됐습니다.".into());
                }
            }
        }
        let Some((token, offset)) = gesture else {
            return Ok(());
        };
        if offset == self.artwork_move.offset || self.artwork_job.is_some() {
            return Ok(());
        }
        let (reply, received) = sync_channel(1);
        let request = Request {
            target: self.active_layer,
            offset,
            source: self.artwork_move.source.clone(),
            reply,
        };
        let Some(sender) = self.stroke.materializer.sender.as_ref() else {
            return Err("그림 이동 작업자를 사용할 수 없습니다.".into());
        };
        match sender.try_send(WorkerRequest::MovePreview(request)) {
            Ok(()) => {
                self.artwork_move.job = Some(Job {
                    token,
                    offset,
                    received,
                });
            }
            Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                return Err("그림 이동 작업자가 종료됐습니다.".into());
            }
        }
        Ok(())
    }

    fn show_artwork_move_tiles(&mut self, tiles: &TileSnapshot) -> Result<(), String> {
        let source = self
            .artwork_move
            .source
            .as_ref()
            .expect("received preview source");
        let before = self
            .artwork_move
            .displayed
            .as_ref()
            .unwrap_or(&source.tiles);
        let keys = changed_raster_keys(before, tiles, self.active_layer);
        let restore_keys = changed_raster_keys(&source.tiles, tiles, self.active_layer);
        let mask = source
            .tree
            .raster(self.active_layer)
            .and_then(|layer| layer.mask)
            .map(|mask| mask.id);
        let masks_changed = !same_coverage(before, tiles, mask);
        let restore_masks = !same_coverage(&source.tiles, tiles, mask);
        self.artwork_move.touched.extend(keys.iter().copied());
        if masks_changed {
            self.artwork_move.masks_touched = true;
            self.scene
                .replace_mask_tiles(tiles)
                .map_err(|error| format!("Move preview masks: {error:?}"))?;
        }
        for key in keys {
            if self.scene.tree().raster(key.layer).is_some() {
                let pixels = tiles.get(key).map_or(
                    &super::TRANSPARENT_HISTORY_TILE[..],
                    nyatidraw_tiles::TileObject::pixels,
                );
                self.scene
                    .upload_closed_tile(key, pixels)
                    .map_err(|error| format!("Move preview pixels: {error:?}"))?;
            }
        }
        self.artwork_move.touched = restore_keys;
        self.artwork_move.masks_touched = restore_masks;
        Ok(())
    }

    fn restore_artwork_move_display(&mut self) -> Result<(), String> {
        if self.artwork_move.masks_touched {
            self.scene
                .replace_mask_tiles(&self.mask_state.authoritative)
                .map_err(|error| format!("Move restore masks: {error:?}"))?;
        }
        for &key in &self.artwork_move.touched {
            if self.scene.tree().raster(key.layer).is_some() {
                let pixels = self
                    .cpu_tiles
                    .get(&key)
                    .map_or(&super::TRANSPARENT_HISTORY_TILE[..], Vec::as_slice);
                self.scene
                    .upload_closed_tile(key, pixels)
                    .map_err(|error| format!("Move restore pixels: {error:?}"))?;
            }
        }
        if self.artwork_move.selection_touched {
            self.show_artwork_move_selection(self.selection.clone().as_deref())?;
        }
        self.artwork_move.touched.clear();
        self.artwork_move.masks_touched = false;
        self.artwork_move.selection_touched = false;
        self.artwork_move.displayed = None;
        Ok(())
    }

    fn show_artwork_move_selection(
        &mut self,
        selection: Option<&SelectionMask>,
    ) -> Result<(), String> {
        let mask = selection
            .map(|mask| {
                self.painter.create_selection_coverage_at(
                    mask.origin(),
                    mask.dimensions(),
                    mask.coverage_bytes(),
                )
            })
            .transpose()
            .map_err(|error| format!("Move preview selection: {error:?}"))?;
        self.scene.set_selection_overlay(mask.as_ref());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_move_previews_preserve_the_source_and_reject_changed_artwork() {
        let tree = super::super::default_layer_tree();
        let target = super::super::LIVE_LAYER;
        let key = TileKey {
            layer: target,
            mip: 0,
            x: -1,
            y: 0,
        };
        let mut pixels = vec![0; nyatidraw_tiles::TILE_BYTE_LEN];
        pixels[..4].copy_from_slice(&[255, 0, 0, 255]);
        let tiles = TileSnapshot::from_tiles([(key, pixels)]).unwrap();
        let original = tiles.clone();
        let canvas = CanvasSpec {
            width_px: 32,
            height_px: 32,
            pixels_per_inch: 72,
        };
        let mut source = None;
        let mut first = None;
        for offset in [[3, -2], [-5, 4], [0, 0], [3, -2]] {
            let (reply, received) = sync_channel(1);
            execute(
                Request {
                    target,
                    offset,
                    source: source.clone(),
                    reply,
                },
                &tree,
                &tiles,
                None,
                canvas,
            );
            let preview = received.recv().unwrap().unwrap();
            assert_eq!(tiles, original);
            assert_eq!(preview.source.tiles, original);
            if offset == [0, 0] {
                assert_eq!(preview.tiles, original);
                assert!(changed_raster_keys(&original, &preview.tiles, target).is_empty());
            } else {
                assert!(changed_raster_keys(&original, &preview.tiles, target).contains(&key));
            }
            if offset == [3, -2] {
                if let Some(first) = &first {
                    assert_eq!(&preview.tiles, first);
                } else {
                    first = Some(preview.tiles.clone());
                }
            }
            source = Some(preview.source);
        }
        let changed = TileSnapshot::empty();
        let (reply, received) = sync_channel(1);
        execute(
            Request {
                target,
                offset: [1, 0],
                source,
                reply,
            },
            &tree,
            &changed,
            None,
            canvas,
        );
        assert!(received.recv().unwrap().is_err());
        assert_eq!(changed, TileSnapshot::empty());
    }
}

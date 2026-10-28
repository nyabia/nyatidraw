use nyatidraw_api::{DrawingTool, EditCommand};
use nyatidraw_editor::gesture::EditGesture;
use nyatidraw_input::StylusSample;
use nyatidraw_web_core::{ArtworkMoveDraft, CanvasUpdate};

use super::Runtime;

pub(super) struct Drag {
    source: ArtworkMoveDraft,
    pending: Option<[i32; 2]>,
    displayed: [i32; 2],
}

impl Runtime {
    pub(super) fn artwork_move_input(
        &mut self,
        phase: &str,
        sample: StylusSample,
    ) -> Result<CanvasUpdate, String> {
        match phase {
            "begin" => {
                self.cancel_artwork_move()?;
                let source = self
                    .document
                    .begin_artwork_move()
                    .map_err(|e| e.to_string())?;
                self.gesture = Some(EditGesture::begin(
                    DrawingTool::MoveArtwork,
                    self.document.edit_settings,
                    [0; 4],
                    self.document.selection().is_some(),
                    sample,
                ));
                self.artwork_move = Some(Drag {
                    source,
                    pending: None,
                    displayed: [0, 0],
                });
            }
            "move" => {
                if let (Some(gesture), Some(drag)) = (&mut self.gesture, &mut self.artwork_move) {
                    gesture.push(sample);
                    let (points, _) = gesture.preview();
                    let offset = match points {
                        [start, end] => end[0]
                            .checked_sub(start[0])
                            .zip(end[1].checked_sub(start[1])),
                        _ => None,
                    };
                    let Some((x, y)) = offset else {
                        self.cancel_artwork_move()?;
                        return Err("화면이나 좌표가 변경되어 그림 이동을 취소했습니다.".into());
                    };
                    drag.pending = Some([x, y]);
                }
            }
            "end" => {
                if let (Some(drag), Some(gesture)) = (self.artwork_move.take(), self.gesture.take())
                {
                    let result = gesture.finish(sample).and_then(|command| {
                        let EditCommand::MoveArtwork { offset } = command else {
                            return Err("그림 이동 명령이 변경되어 취소했습니다.".into());
                        };
                        self.document
                            .finish_artwork_move(&drag.source, offset)
                            .map_err(|e| e.to_string())
                    });
                    self.restore_move_scene()?;
                    let update = result?;
                    self.apply(&update)?;
                    return Ok(update);
                }
            }
            _ => {}
        }
        Ok(CanvasUpdate::default())
    }

    pub(super) fn flush_artwork_move(&mut self) -> Result<(), String> {
        let Some(drag) = &mut self.artwork_move else {
            return Ok(());
        };
        let Some(offset) = drag.pending.take() else {
            return Ok(());
        };
        if offset == drag.displayed {
            return Ok(());
        }
        let result = drag
            .source
            .preview(offset)
            .map_err(|e| e.to_string())
            .and_then(|preview| self.renderer.preview_artwork_move(&self.document, &preview));
        if let Err(error) = result {
            self.cancel_artwork_move()?;
            return Err(error);
        }
        drag.displayed = offset;
        Ok(())
    }

    pub(crate) fn cancel_artwork_move(&mut self) -> Result<(), String> {
        if self.artwork_move.take().is_some() {
            self.gesture = None;
            self.restore_move_scene()?;
        }
        Ok(())
    }

    fn restore_move_scene(&mut self) -> Result<(), String> {
        self.renderer
            .sync_document(&self.document)
            .inspect_err(|_| self.gpu_failed = true)
    }
}

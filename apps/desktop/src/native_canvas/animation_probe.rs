use super::{ActiveCanvas, EditorCommand, ExportStatus, ProjectLocation};
use nyatidraw_api::{AnimationCommand as Animation, EditCommand, HistoryCommand, ProjectCommand};
use std::{fmt::Write as _, io::Write as _, path::PathBuf, time::Instant};

pub(super) struct Probe {
    path: PathBuf,
    reopen: bool,
    step: u8,
    playback_started: Option<Instant>,
    saw_other_frame: bool,
}

impl Probe {
    pub(super) fn open(
        location: &ProjectLocation,
        empty: bool,
        animated: bool,
    ) -> Result<Option<Self>, String> {
        let Ok(mode) = std::env::var("NAYATI_ANIMATION_PROBE") else {
            return Ok(None);
        };
        let ProjectLocation::Explicit {
            path,
            bootstrap_png: None,
        } = location
        else {
            return Err("animation probe requires explicit scratch project".into());
        };
        if path.file_name().and_then(|name| name.to_str()) != Some("animation-scratch.ntdr")
            || !path
                .parent()
                .is_some_and(|parent| parent.join(".nyatidraw-scratch-animation-probe").is_file())
            || !matches!(mode.as_str(), "write" | "reopen")
        {
            return Err(
                "animation probe requires its exact scratch filename, marker and write/reopen mode"
                    .into(),
            );
        }
        if mode == "write"
            && (!empty || animated || path.with_extension("animation-proof").exists())
        {
            return Err("animation write probe refuses existing artwork or proof".into());
        }
        Ok(Some(Self {
            path: path.clone(),
            reopen: mode == "reopen",
            step: 0,
            playback_started: None,
            saw_other_frame: false,
        }))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn advance(&mut self, canvas: &mut ActiveCanvas) -> Result<bool, String> {
        if canvas.artwork_job.is_some()
            || canvas
                .stroke
                .materializer
                .pending
                .load(std::sync::atomic::Ordering::Acquire)
                != 0
        {
            return Ok(false);
        }
        if let Some(error) = &canvas.projection.current().edit.error {
            return Err(error.clone());
        }
        if self.reopen {
            let proof = Self::proof(canvas)?;
            let expected = std::fs::read_to_string(self.path.with_extension("animation-proof"))
                .map_err(|error| error.to_string())?;
            if proof != expected {
                return Err(
                    "fresh process changed timeline, layers, pixels, or current frame".into(),
                );
            }
            println!(
                "native-animation-probe event=reopen-complete frames=2 all-cels-preserved=true active-scene-isolated=true physical-input-proof=false"
            );
            return Ok(true);
        }
        let first = canvas
            .animation
            .state
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.frames().first())
            .map(|frame| frame.id);
        let command = match self.step {
            0 => EditorCommand::Edit(EditCommand::ResizePage { size: [32, 32] }),
            1 | 5 => EditorCommand::Edit(EditCommand::SelectAll),
            2 => EditorCommand::Edit(EditCommand::FillSelection {
                color: [255, 0, 0, 255],
            }),
            3 => EditorCommand::Animation(Animation::Enable),
            4 => {
                EditorCommand::Animation(Animation::Duplicate(first.ok_or("missing first frame")?))
            }
            6 => EditorCommand::Edit(EditCommand::FillSelection {
                color: [0, 0, 255, 255],
            }),
            7 => EditorCommand::Animation(Animation::Select(first.ok_or("missing first frame")?)),
            8 => EditorCommand::Animation(Animation::SetDuration {
                frame: first.ok_or("missing first frame")?,
                duration_ms: 40,
            }),
            9 => EditorCommand::History(HistoryCommand::Undo),
            10 => {
                if canvas.animation.state.timeline.as_ref().unwrap().frames()[0].duration_ms == 40 {
                    return Err("duration Undo did not restore metadata".into());
                }
                EditorCommand::History(HistoryCommand::Redo)
            }
            11 => {
                if canvas.animation.state.timeline.as_ref().unwrap().frames()[0].duration_ms != 40 {
                    return Err("duration Redo did not restore metadata".into());
                }
                EditorCommand::Animation(Animation::Play)
            }
            12 => {
                let started = *self.playback_started.get_or_insert_with(Instant::now);
                self.saw_other_frame |=
                    canvas.animation.display_frame != canvas.animation.state.editing_frame;
                if started.elapsed().as_millis() < 200 {
                    return Ok(false);
                }
                if !self.saw_other_frame {
                    return Err("playback never displayed the other frame".into());
                }
                EditorCommand::Animation(Animation::Stop)
            }
            13 => EditorCommand::Animation(Animation::Play),
            14 => EditorCommand::Animation(Animation::Stop),
            15 => EditorCommand::Animation(Animation::SetOnionSkin(true)),
            16 => {
                let editable = canvas.animation.state.edit_tree(&canvas.authoritative_tree);
                if super::raster_layer_ids(canvas.scene.tree()).len()
                    <= super::raster_layer_ids(&editable).len()
                {
                    return Err("onion display did not install a disposable neighbor proxy".into());
                }
                EditorCommand::Animation(Animation::SetOnionSkin(false))
            }
            17 => EditorCommand::Project(ProjectCommand::Save),
            _ => {
                match canvas.stroke.live_ink.export_status_snapshot() {
                    ExportStatus::Current { .. } => {}
                    ExportStatus::Failed { .. } => return Err("PNG save failed".into()),
                    _ => return Ok(false),
                }
                let proof = Self::proof(canvas)?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(self.path.with_extension("animation-proof"))
                    .map_err(|error| error.to_string())?;
                file.write_all(proof.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|error| error.to_string())?;
                println!(
                    "native-animation-probe event=write-complete frames=2 duration-undo-redo=true playback-stop-restart=true onion=true saved=true awaiting-fresh-process=true physical-input-proof=false"
                );
                return Ok(true);
            }
        };
        canvas
            .stroke
            .live_ink
            .push_editor_command(canvas.projection.current().revision, command)
            .map_err(|error| format!("probe command queue: {error:?}"))?;
        println!("native-animation-probe event=queued step={}", self.step);
        self.step += 1;
        Ok(false)
    }

    fn proof(canvas: &ActiveCanvas) -> Result<String, String> {
        let timeline = canvas
            .animation
            .state
            .timeline
            .as_ref()
            .ok_or("missing animation")?;
        if timeline.frames().len() != 2 {
            return Err("expected two frames".into());
        }
        let tiles = nyatidraw_tiles::TileSnapshot::from_tiles(
            canvas
                .cpu_tiles
                .iter()
                .map(|(key, pixels)| (*key, pixels.clone())),
        )
        .map_err(|error| format!("probe tiles: {error:?}"))?;
        for (key, tile) in tiles.iter() {
            if key.mip != 0 {
                return Err("unexpected durable mip in animation fixture".into());
            }
            let (left, top) = key.pixel_origin();
            let edge = i64::from(nyatidraw_tiles::TILE_EDGE);
            for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
                let index = i64::try_from(index).map_err(|error| error.to_string())?;
                let (x, y) = (left + index % edge, top + index / edge);
                if (!(0..32).contains(&x) || !(0..32).contains(&y)) && pixel != [0, 0, 0, 0] {
                    return Err(format!(
                        "probe fixture painted outside the 32px page at {x},{y} layer={:?}",
                        key.layer
                    ));
                }
            }
        }
        let mut proof = format!(
            "{:?}\n{:?}\n{:?}\n{:?}\n",
            timeline,
            canvas.authoritative_tree,
            tiles.root(),
            canvas.animation.state.editing_frame
        );
        for (index, frame) in timeline.frames().iter().enumerate() {
            let tree = timeline
                .resolve_frame_tree(&canvas.authoritative_tree, frame.id)
                .map_err(|error| format!("probe frame: {error:?}"))?;
            let page =
                nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&tiles, &tree, canvas.canvas_spec)
                    .map_err(|error| format!("probe render: {error:?}"))?;
            let expected = if index == 0 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            };
            if page.pixels.chunks_exact(4).any(|pixel| pixel != expected) {
                return Err("cross-cel pixel contamination".into());
            }
            writeln!(proof, "{:?}", page.pixels).map_err(|error| error.to_string())?;
        }
        if canvas.scene.tree() != &canvas.animation.render_tree(&canvas.authoritative_tree)? {
            return Err("scene contains inactive cels or stale onion proxies".into());
        }
        Ok(proof)
    }
}

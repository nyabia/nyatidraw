use super::{
    ActiveCanvas, ContentRootId, DrawingTool, EditCommand, EditorCommand, ExportStatus, Ordering,
    PathBuf, Point, PointerPhase, ProjectCommand, ProjectLocation, TileSnapshot,
    flatten_layer_tree_rgba8, input_probe_sample,
};
use nyatidraw_api::{AnimationCommand as Animation, HistoryCommand, MaskCommand, ToolCommand};
use std::io::Write as _;

pub(super) struct Trace {
    pub(super) remaining: u8,
    pub(super) present: bool,
}

pub(super) struct Probe {
    path: PathBuf,
    reopen: bool,
    step: u8,
    source: Option<ContentRootId>,
    painted: Option<ContentRootId>,
    wake_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Probe {
    pub(super) fn open(location: &ProjectLocation, empty: bool) -> Result<Option<Self>, String> {
        let Ok(mode) = std::env::var("NAYATI_MASK_PROBE") else {
            return Ok(None);
        };
        let ProjectLocation::Explicit {
            path,
            bootstrap_png: None,
        } = location
        else {
            return Err("Mask probe requires explicit scratch project".into());
        };
        if path.file_name().and_then(|name| name.to_str()) != Some("mask-scratch.ntdr")
            || !path
                .parent()
                .is_some_and(|parent| parent.join(".nyatidraw-scratch-mask-probe").is_file())
            || !matches!(mode.as_str(), "write" | "reopen")
        {
            return Err(
                "Mask probe requires exact scratch filename, marker, and write/reopen mode".into(),
            );
        }
        if mode == "write" && (!empty || path.with_extension("mask-proof").exists()) {
            return Err("Mask probe refuses existing artwork or proof".into());
        }
        Ok(Some(Self {
            path: path.clone(),
            reopen: mode == "reopen",
            step: 0,
            source: None,
            painted: None,
            wake_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }))
    }

    fn image_root(canvas: &ActiveCanvas) -> Result<ContentRootId, String> {
        TileSnapshot::from_tiles(
            canvas
                .cpu_tiles
                .iter()
                .map(|(key, pixels)| (*key, pixels.clone())),
        )
        .map(|tiles| tiles.root().id)
        .map_err(|error| format!("Probe source: {error:?}"))
    }

    fn snapshot(canvas: &ActiveCanvas) -> Result<TileSnapshot, String> {
        let image = TileSnapshot::from_tiles(
            canvas
                .cpu_tiles
                .iter()
                .map(|(key, pixels)| (*key, pixels.clone())),
        )
        .map_err(|error| format!("Probe source: {error:?}"))?;
        TileSnapshot::from_planes(
            image.iter().map(|(key, tile)| (key, tile.clone())),
            canvas
                .mask_state
                .authoritative
                .coverage_iter()
                .map(|(key, tile)| (key, tile.clone())),
        )
        .map_err(|error| format!("Probe planes: {error:?}"))
    }

    fn proof(canvas: &ActiveCanvas) -> Result<String, String> {
        let timeline = canvas
            .animation
            .state
            .timeline
            .as_ref()
            .ok_or("Probe animation missing")?;
        if timeline.frames().len() != 2 {
            return Err("Probe needs both cels".into());
        }
        let snapshot = Self::snapshot(canvas)?;
        let mask = canvas
            .authoritative_tree
            .raster(canvas.active_layer)
            .and_then(|layer| layer.mask)
            .ok_or("Probe mask missing")?;
        if !mask.enabled
            || snapshot.coverage_len() == 0
            || snapshot.coverage_at(mask.id, mask.default_coverage, [8, 8]) == 255
        {
            return Err("Painted enabled coverage missing".into());
        }
        let inactive = timeline
            .resolve_frame_tree(&canvas.authoritative_tree, timeline.frames()[1].id)
            .map_err(|error| format!("Probe cel: {error:?}"))?;
        let page = flatten_layer_tree_rgba8(&snapshot, &inactive, canvas.canvas_spec)
            .map_err(|error| format!("Probe inactive flatten: {error:?}"))?;
        if page
            .pixels
            .chunks_exact(4)
            .any(|pixel| pixel != [0, 0, 255, 255])
        {
            return Err("Inactive cel image was damaged by mask editing".into());
        }
        let active = timeline
            .resolve_frame_tree(&canvas.authoritative_tree, timeline.frames()[0].id)
            .map_err(|error| format!("Probe active cel: {error:?}"))?;
        let active_page = flatten_layer_tree_rgba8(&snapshot, &active, canvas.canvas_spec)
            .map_err(|error| format!("Probe active flatten: {error:?}"))?;
        let offset = (8 * 32 + 8) * 4;
        if active_page.pixels[offset..offset + 4] == [255, 0, 0, 255] {
            return Err("Mask coverage did not change the rendered active cel".into());
        }
        Ok(format!(
            "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n",
            timeline,
            canvas.authoritative_tree,
            snapshot.root(),
            Self::image_root(canvas)?,
            active_page.pixels
        ))
    }

    fn schedule_wake(&self, canvas: &ActiveCanvas) -> Result<(), String> {
        if self.wake_pending.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let pending = std::sync::Arc::clone(&self.wake_pending);
        let live_ink = canvas.stroke.live_ink.clone();
        std::thread::Builder::new()
            .name("nyatidraw-mask-probe-wake".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                pending.store(false, Ordering::Release);
                live_ink.request_redraw();
            })
            .map_err(|error| format!("Probe wake: {error}"))?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn advance(&mut self, canvas: &mut ActiveCanvas) -> Result<bool, String> {
        self.schedule_wake(canvas)?;
        if canvas.artwork_job.is_some()
            || canvas.stroke.active.is_some()
            || !canvas.stroke.pending_materializations.is_empty()
            || canvas.stroke.materializer.pending.load(Ordering::Acquire) != 0
        {
            return Ok(false);
        }
        if let Some(error) = &canvas.projection.current().edit.error {
            return Err(error.clone());
        }
        if self.reopen {
            let expected = std::fs::read_to_string(self.path.with_extension("mask-proof"))
                .map_err(|error| error.to_string())?;
            if Self::proof(canvas)? != expected {
                return Err(
                    "Fresh process changed mask planes, source pixels, metadata, or inactive cel"
                        .into(),
                );
            }
            println!(
                "native-mask-probe event=reopen-complete source-exact=true coverage-exact=true inactive-cel=true physical-input-proof=false"
            );
            return Ok(true);
        }
        if let Some(source) = self.source
            && Self::image_root(canvas)? != source
        {
            return Err("Mask action changed source RGBA planes".into());
        }
        let first = canvas
            .animation
            .state
            .timeline
            .as_ref()
            .and_then(|timeline| timeline.frames().first())
            .map(|frame| frame.id);
        let layer = canvas.active_layer;
        let command = match self.step {
            0 => EditorCommand::Edit(EditCommand::ResizePage { size: [32, 32] }),
            1 | 5 => EditorCommand::Edit(EditCommand::SelectAll),
            2 => EditorCommand::Edit(EditCommand::FillSelection {
                color: [255, 0, 0, 255],
            }),
            3 => EditorCommand::Animation(Animation::Enable),
            4 => {
                EditorCommand::Animation(Animation::Duplicate(first.ok_or("Missing first frame")?))
            }
            6 => EditorCommand::Edit(EditCommand::FillSelection {
                color: [0, 0, 255, 255],
            }),
            7 => EditorCommand::Animation(Animation::Select(first.ok_or("Missing first frame")?)),
            8 => {
                self.source = Some(Self::image_root(canvas)?);
                EditorCommand::Mask(MaskCommand::AddRevealAll(layer))
            }
            9 => EditorCommand::Tool(ToolCommand::Select(DrawingTool::Pen)),
            10 => EditorCommand::Tool(ToolCommand::SetColor([0, 0, 0, 255])),
            11 => EditorCommand::Tool(ToolCommand::SetSizeTenths(120)),
            12 => EditorCommand::Tool(ToolCommand::SetOpacityU16(u16::MAX)),
            13 => {
                for (sequence, phase) in [
                    (10_001, PointerPhase::Begin),
                    (10_002, PointerPhase::Move),
                    (10_003, PointerPhase::End),
                ] {
                    let mut sample = input_probe_sample(sequence, phase);
                    sample.position_document = Point { x: 8.0, y: 8.0 };
                    sample.pressure = 1.0;
                    canvas
                        .stroke
                        .live_ink
                        .push(sample)
                        .map_err(|error| format!("Probe input: {error:?}"))?;
                }
                self.step += 1;
                return Ok(false);
            }
            14 => {
                if canvas.mask_state.authoritative.coverage_len() == 0 {
                    return Ok(false);
                }
                self.painted = Some(canvas.mask_state.authoritative.root().id);
                EditorCommand::Mask(MaskCommand::SetEnabled {
                    layer,
                    enabled: false,
                })
            }
            15 => {
                if canvas
                    .authoritative_tree
                    .raster(layer)
                    .and_then(|node| node.mask)
                    .is_none_or(|mask| mask.enabled)
                {
                    return Err("Disable mask failed".into());
                }
                let snapshot = Self::snapshot(canvas)?;
                let tree = canvas.animation.state.edit_tree(&canvas.authoritative_tree);
                let page = flatten_layer_tree_rgba8(&snapshot, &tree, canvas.canvas_spec)
                    .map_err(|error| format!("Disabled probe render: {error:?}"))?;
                if page
                    .pixels
                    .chunks_exact(4)
                    .any(|pixel| pixel != [255, 0, 0, 255])
                {
                    return Err("Disabled mask did not restore exact source appearance".into());
                }
                EditorCommand::Mask(MaskCommand::SetEnabled {
                    layer,
                    enabled: true,
                })
            }
            16 => EditorCommand::Mask(MaskCommand::Remove(layer)),
            17 => {
                if canvas
                    .authoritative_tree
                    .raster(layer)
                    .is_none_or(|node| node.mask.is_some())
                    || canvas.mask_state.authoritative.coverage_len() != 0
                {
                    return Err("Remove mask retained its coverage".into());
                }
                EditorCommand::History(HistoryCommand::Undo)
            }
            18 => {
                if Some(canvas.mask_state.authoritative.root().id) != self.painted {
                    return Err("Mask delete Undo did not restore exact coverage".into());
                }
                Self::proof(canvas)?;
                EditorCommand::Project(ProjectCommand::Save)
            }
            _ => {
                match canvas.stroke.live_ink.export_status_snapshot() {
                    ExportStatus::Current { .. } => {}
                    ExportStatus::Failed { .. } => return Err("Probe PNG export failed".into()),
                    _ => return Ok(false),
                }
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(self.path.with_extension("mask-proof"))
                    .map_err(|error| error.to_string())?;
                file.write_all(Self::proof(canvas)?.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|error| error.to_string())?;
                println!(
                    "native-mask-probe event=write-complete mask-stroke=true disable-enable=true delete-undo=true source-exact=true inactive-cel=true awaiting-fresh-process=true physical-input-proof=false"
                );
                return Ok(true);
            }
        };
        canvas
            .stroke
            .live_ink
            .push_editor_command(canvas.projection.current().revision, command)
            .map_err(|error| format!("Probe command: {error:?}"))?;
        println!("native-mask-probe event=queued step={}", self.step);
        self.step += 1;
        Ok(false)
    }
}

impl ActiveCanvas {
    pub(super) fn advance_mask_probe(&mut self) {
        if let Some(mut probe) = self.mask_probe.take() {
            match probe.advance(self) {
                Ok(false) => self.mask_probe = Some(probe),
                Ok(true) => {}
                Err(error) => {
                    self.stroke
                        .live_ink
                        .publish_workspace_error(format!("Mask scratch probe: {error}"));
                    eprintln!("native-mask-probe failed={error}");
                }
            }
        }
    }
}

impl ActiveCanvas {
    pub(super) fn trace_mask_render(&mut self, viewport: nyatidraw_input::ViewportTransform) {
        if self.mask_trace.remaining == 0 {
            return;
        }
        self.mask_trace.remaining -= 1;
        let frame = 32 - self.mask_trace.remaining;
        match self.mask_render_sample(viewport) {
            Ok(sample) => println!("native-mask-render-trace frame={frame} {sample}"),
            Err(error) => eprintln!("native-mask-render-trace frame={frame} error={error}"),
        }
    }

    fn mask_render_sample(
        &self,
        viewport: nyatidraw_input::ViewportTransform,
    ) -> Result<String, String> {
        let snapshot = Probe::snapshot(self)?;
        let scene_mask = self
            .scene
            .tree()
            .raster(self.active_layer)
            .and_then(|node| node.mask);
        let coverage =
            scene_mask.map(|mask| snapshot.coverage_at(mask.id, mask.default_coverage, [8, 8]));
        let preview_active = self.mask_state.preview.is_some();
        let display_coverage = scene_mask.map(|mask| {
            self.mask_state
                .trace_display()
                .coverage_at(mask.id, mask.default_coverage, [8, 8])
        });
        let expected = nyatidraw_paint_cpu::sample_display_pixel(
            &snapshot,
            self.scene.tree(),
            [8, 8],
            self.projection.current().solo_node,
        )
        .map_err(|error| format!("CPU trace: {error:?}"))?;
        let root = self
            .scene
            .readback_resident_group_tile(
                self.scene.tree().root_id(),
                nyatidraw_api::TileCoordinate { mip: 0, x: 0, y: 0 },
            )
            .map_err(|error| format!("Root trace: {error:?}"))?;
        let root_pixel = root.as_ref().and_then(|pixels| trace_pixel(pixels, [8, 8]));
        let window = viewport
            .document_to_window(Point { x: 8.5, y: 8.5 })
            .ok_or("Trace viewport mapping failed")?;
        let display = self
            .scene
            .readback_display()
            .map_err(|error| format!("Display trace: {error:?}"))?;
        let display_pixel = display
            .as_ref()
            .and_then(|pixels| trace_window_pixel(pixels, window));
        Ok(format!(
            "scene_root={:?} source_root={:?} coverage_root={:?} coverage_tiles={} mask={scene_mask:?} coverage={coverage:?} preview_active={preview_active} display_coverage={display_coverage:?} cpu={expected:?} root_gpu={root_pixel:?} display_gpu={display_pixel:?} window={window:?} viewport={viewport:?}",
            self.scene.tree().root_id(),
            snapshot.root(),
            self.mask_state.authoritative.root(),
            snapshot.coverage_len()
        ))
    }
}

fn trace_pixel(pixels: &nyatidraw_paint_gpu::Rgba8Readback, point: [u32; 2]) -> Option<[u8; 4]> {
    if point[0] >= pixels.width || point[1] >= pixels.height {
        return None;
    }
    let offset = usize::try_from(point[1])
        .ok()?
        .checked_mul(usize::try_from(pixels.width).ok()?)?
        .checked_add(usize::try_from(point[0]).ok()?)?
        .checked_mul(4)?;
    pixels.pixels.get(offset..offset + 4)?.try_into().ok()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Finite coordinates are checked inside the u32 image bounds before conversion"
)]
fn trace_window_pixel(
    pixels: &nyatidraw_paint_gpu::Rgba8Readback,
    point: Point,
) -> Option<[u8; 4]> {
    if !point.x.is_finite()
        || !point.y.is_finite()
        || point.x < 0.0
        || point.y < 0.0
        || point.x >= f64::from(pixels.width)
        || point.y >= f64::from(pixels.height)
    {
        return None;
    }
    trace_pixel(pixels, [point.x.floor() as u32, point.y.floor() as u32])
}

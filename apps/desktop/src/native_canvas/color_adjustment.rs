use super::{
    ActiveCanvas, BTreeSet, CommandRejectReason, CpuSnapshotAdoption, EditCommand, EditorCommand,
    LayerId, TRANSPARENT_HISTORY_TILE, TileKey, ToolCommand, WorkerRequest, sync_channel,
};
use crate::{edit_worker::EditFailure, transform_worker::DurableTransform};
use nyatidraw_api::{
    ColorAdjustment, ColorAdjustmentCommand, ColorAdjustmentProjection, HistoryProjection,
};
use nyatidraw_document::LayerTree;
use nyatidraw_editor::{
    HeadlessStrokeSession,
    color_adjustment::ColorAdjustmentSession,
    transform::{TransformError, TransformStore},
};
use nyatidraw_paint_cpu::SelectionMask;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Worker(ColorAdjustmentSession);

pub(super) struct Outcome {
    pub(super) tiles: TileSnapshot,
    pub(super) projection: Option<ColorAdjustmentProjection>,
    pub(super) committed: bool,
    pub(super) history: HistoryProjection,
}

struct ColorStore<'a>(DurableTransform<'a>);
impl TransformStore for ColorStore<'_> {
    fn current_snapshot(&self) -> nyatidraw_api::SnapshotId {
        self.0.current_snapshot()
    }
    fn tiles(&self) -> &TileSnapshot {
        self.0.tiles()
    }
    fn validate_preview(&self, tiles: &TileSnapshot, _tree: &LayerTree) -> Result<(), String> {
        // Color correction must fit the already admitted surfaces; alpha and extent stay pinned.
        if tiles
            .iter()
            .any(|(key, _)| self.0.tiles().get(key).is_none())
        {
            return Err("색 보정이 기존 타일 범위를 넘었습니다.".into());
        }
        Ok(())
    }
    fn commit(&mut self, id: u128, tiles: TileSnapshot, tree: &LayerTree) -> Result<(), String> {
        self.0.commit(id, tiles, tree)
    }
}

impl Worker {
    pub(super) fn active(&self) -> bool {
        self.0.target().is_some()
    }

    #[allow(clippy::too_many_arguments, clippy::ref_option)]
    pub(super) fn execute(
        &mut self,
        command: ColorAdjustmentCommand,
        target: LayerId,
        tree: &LayerTree,
        selection: &Option<Arc<SelectionMask>>,
        session: &mut HeadlessStrokeSession,
        next_id: &mut u128,
        db: &ProjectDb,
    ) -> Result<Outcome, EditFailure> {
        let result = self
            .0
            .execute(
                command,
                target,
                tree,
                selection,
                &mut ColorStore(DurableTransform { session, db }),
                next_id,
            )
            .map_err(|e| match e {
                TransformError::Rejected(e) => EditFailure::Rejected(e),
                TransformError::Fatal(e) => EditFailure::Fatal(e),
            })?;
        Ok(Outcome {
            tiles: result.tiles,
            projection: result.projection,
            committed: result.committed,
            history: session.history_projection(),
        })
    }
}

#[derive(Default)]
pub(super) struct Runtime {
    pub(super) projection: Option<ColorAdjustmentProjection>,
    starting: bool,
    preview: Option<ColorAdjustment>,
    cancel: bool,
    tool: Option<ToolCommand>,
}

impl ActiveCanvas {
    pub(super) fn color_adjustment_active(&self) -> bool {
        self.color_adjustment.starting
            || self.color_adjustment.projection.is_some()
            || self
                .artwork_job
                .as_ref()
                .is_some_and(|job| job.color_adjustment.is_some())
    }

    pub(super) fn stage_color_adjustment(&mut self) {
        let mut edit = self.projection.current().edit.clone();
        edit.color_adjustment
            .clone_from(&self.color_adjustment.projection);
        if let Some(projection) = &mut edit.color_adjustment {
            projection.can_commit &= self.artwork_job.is_none()
                && self.color_adjustment.preview.is_none()
                && !self.color_adjustment.cancel;
        }
        edit.can_cancel |= self.color_adjustment_active()
            && !self.color_adjustment.cancel
            && !self.artwork_job.as_ref().is_some_and(|job| {
                matches!(
                    job.color_adjustment,
                    Some(ColorAdjustmentCommand::Commit { .. } | ColorAdjustmentCommand::Cancel)
                )
            });
        self.projection.stage_edit(edit);
    }

    pub(super) fn cancel_color_for_tool(
        &mut self,
        command: ToolCommand,
    ) -> Result<bool, CommandRejectReason> {
        if self.artwork_job.as_ref().is_some_and(|job| {
            matches!(
                job.color_adjustment,
                Some(ColorAdjustmentCommand::Commit { .. })
            )
        }) {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        self.color_adjustment.tool = Some(command);
        self.enqueue_color_adjustment(ColorAdjustmentCommand::Cancel)
    }

    pub(super) fn enqueue_color_adjustment(
        &mut self,
        command: ColorAdjustmentCommand,
    ) -> Result<bool, CommandRejectReason> {
        self.guard_mask_edit(&nyatidraw_api::EditCommand::ColorAdjustment(
            command.clone(),
        ))?;
        if self.transform_active() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        if self.artwork_job.is_some() {
            match command {
                ColorAdjustmentCommand::Preview(value)
                    if self.color_adjustment_active()
                        && !self.color_adjustment.cancel
                        && !self.artwork_job.as_ref().is_some_and(|job| {
                            matches!(
                                job.color_adjustment,
                                Some(
                                    ColorAdjustmentCommand::Commit { .. }
                                        | ColorAdjustmentCommand::Cancel
                                )
                            )
                        }) =>
                {
                    self.color_adjustment.preview = Some(value);
                }
                ColorAdjustmentCommand::Cancel
                    if self.color_adjustment_active()
                        && !self.artwork_job.as_ref().is_some_and(|job| {
                            matches!(
                                job.color_adjustment,
                                Some(ColorAdjustmentCommand::Commit { .. })
                            )
                        }) =>
                {
                    self.color_adjustment.preview = None;
                    self.color_adjustment.cancel = true;
                }
                _ => return Err(CommandRejectReason::CommandQueueBusy),
            }
            self.stage_color_adjustment();
            return Ok(false);
        }
        if command == ColorAdjustmentCommand::Begin {
            if self.color_adjustment_active() {
                return Err(CommandRejectReason::CommandQueueBusy);
            }
        } else if !self.color_adjustment_active() {
            return Err(CommandRejectReason::UnsupportedCommand);
        }
        if let ColorAdjustmentCommand::Commit { generation } = command
            && (self.color_adjustment.preview.is_some()
                || self.color_adjustment.cancel
                || self
                    .color_adjustment
                    .projection
                    .as_ref()
                    .is_none_or(|p| !p.can_commit || p.generation != generation))
        {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        self.ensure_artwork_command_idle()?;
        if command == ColorAdjustmentCommand::Cancel {
            self.color_adjustment.preview = None;
        }
        if !self.stroke.live_ink.try_pause_for_edit() {
            return Err(CommandRejectReason::CommandQueueBusy);
        }
        let starting = command == ColorAdjustmentCommand::Begin;
        let (reply, received) = sync_channel(1);
        let result = self.enqueue_artwork(
            WorkerRequest::Edit {
                command: EditCommand::ColorAdjustment(command),
                target: self.active_layer,
                reply,
            },
            received,
            None,
        );
        if result.is_ok() {
            self.color_adjustment.starting |= starting;
            self.stage_color_adjustment();
        }
        result
    }

    pub(super) fn flush_color_adjustment(&mut self) {
        if self.artwork_job.is_some() {
            return;
        }
        let command = if self.color_adjustment.cancel {
            Some(ColorAdjustmentCommand::Cancel)
        } else {
            self.color_adjustment
                .preview
                .map(ColorAdjustmentCommand::Preview)
        };
        if let Some(command) = command {
            if let Err(
                CommandRejectReason::CommandQueueBusy | CommandRejectReason::StaleProjection { .. },
            ) = self.enqueue_color_adjustment(command)
            {
                self.stroke.live_ink.request_redraw();
                return;
            }
            self.color_adjustment.preview = None;
            self.color_adjustment.cancel = false;
            self.stage_color_adjustment();
            self.publish_current_projection();
        } else if !self.color_adjustment_active() {
            self.finish_color_adjustment_tool();
        }
    }

    pub(super) fn finish_color_adjustment_tool(&mut self) {
        if !self.color_adjustment_active()
            && let Some(command) = self.color_adjustment.tool.take()
        {
            if let Err(error) = self.apply_editor_command(EditorCommand::Tool(command), 0, 0, 1.0) {
                self.stroke
                    .live_ink
                    .publish_activation_notice(format!("도구를 전환하지 못했습니다: {error:?}"));
            }
            self.publish_current_projection();
        }
    }

    pub(super) fn reject_color_adjustment(&mut self, command: Option<&ColorAdjustmentCommand>) {
        if matches!(command, Some(ColorAdjustmentCommand::Begin)) {
            self.color_adjustment.starting = false;
            self.color_adjustment.preview = None;
            self.color_adjustment.cancel = false;
        }
        if let Some(projection) = &mut self.color_adjustment.projection {
            projection.can_commit = false;
        }
        if matches!(
            command,
            Some(ColorAdjustmentCommand::Commit { .. } | ColorAdjustmentCommand::Cancel)
        ) {
            self.color_adjustment.tool = None;
        }
    }

    pub(super) fn install_color_adjustment(&mut self, outcome: Outcome) -> Result<(), String> {
        let mut keys: BTreeSet<TileKey> = self.cpu_tiles.keys().copied().collect();
        if let Some(previous) = &self.transform_display {
            keys.extend(previous.iter().map(|(key, _)| key));
        }
        keys.extend(outcome.tiles.iter().map(|(key, _)| key));
        self.scene
            .replace_mask_tiles(&outcome.tiles)
            .map_err(|error| format!("Color adjustment masks: {error:?}"))?;
        for key in keys {
            if self.scene.tree().raster(key.layer).is_none() {
                continue;
            }
            let pixels = outcome.tiles.get(key).map_or(
                &TRANSPARENT_HISTORY_TILE[..],
                nyatidraw_tiles::TileObject::pixels,
            );
            self.scene
                .upload_closed_tile(key, pixels)
                .map_err(|e| format!("색 보정 표시 실패: {e:?}"))?;
        }
        if outcome.committed {
            self.adopt_mask_snapshot(&outcome.tiles)
                .map_err(|error| format!("Color mask adoption: {error:?}"))?;
            CpuSnapshotAdoption::new(&mut self.cpu_tiles, &outcome.tiles).apply();
            self.projection.stage_history(outcome.history);
            let mut current = self.projection.current().clone();
            current.dirty = true;
            self.projection.install_authoritative(current);
        }
        self.transform_display = outcome.projection.as_ref().map(|_| outcome.tiles);
        self.color_adjustment.starting = false;
        self.color_adjustment.projection = outcome.projection;
        if self.color_adjustment.projection.is_none() {
            self.color_adjustment.preview = None;
            self.color_adjustment.cancel = false;
        }
        self.latest_preview_generation.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::{CanvasSpec, HistoryNodeId, SnapshotId};
    use nyatidraw_document::LayerTreeNode;
    use nyatidraw_paint_cpu::{
        EditLimits, SelectionCombine, combine_selection, flatten_layer_tree_rgba8, selection_all,
    };
    use nyatidraw_tiles::TILE_BYTE_LEN;

    #[test]
    #[ignore = "requires a real WGPU adapter; run explicitly with --ignored --exact --nocapture"]
    #[allow(clippy::too_many_lines)]
    fn native_color_preview_cancel_and_commit_match_resident_gpu_and_display() {
        use crate::native_canvas::{
            ProjectLocation,
            flat_fill_gpu_acceptance::{actual_pixels, compare_gpu, display_pixel, settle},
        };
        use nyatidraw_api::{HistoryCommand, ProjectCommand};
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("explicit color acceptance requires a GPU adapter");
        let info = adapter.get_info();
        println!(
            "color-correction-native-gpu adapter={} backend={:?} driver={} profile={} physical-pen-proof=false present-proof=false",
            info.name,
            info.backend,
            info.driver,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nyatidraw-color-native-acceptance"),
            ..Default::default()
        }))
        .unwrap();
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-color-native-gpu-{}-{}.ntdr",
            std::process::id(),
            crate::native_canvas::system_timestamp_ns()
        ));
        let location = ProjectLocation::Explicit {
            path: path.clone(),
            bootstrap_png: None,
        };
        let tree = crate::native_canvas::default_layer_tree();
        let page = CanvasSpec {
            width_px: 32,
            height_px: 32,
            pixels_per_inch: 96,
        };
        let key = TileKey {
            layer: LayerId(1),
            mip: 0,
            x: 0,
            y: 0,
        };
        let signed = TileKey { x: -1, ..key };
        let mut pixels = vec![0; TILE_BYTE_LEN];
        for x in [2, 35] {
            let offset = (2 * 128 + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        let mut offpage = vec![0; TILE_BYTE_LEN];
        let offset = (5 * 128 + 125) * 4;
        offpage[offset..offset + 4].copy_from_slice(&[0, 128, 0, 128]);
        let before = TileSnapshot::from_tiles([(key, pixels), (signed, offpage)]).unwrap();
        {
            let db = ProjectDb::open(&path).unwrap();
            db.persist_canvas_spec(page).unwrap();
            let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::default());
            let seed = session
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, before.clone())
                .unwrap();
            db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
            session.accept_structural_change(&seed).unwrap();
        }
        let ink = crate::native_canvas::LiveInkBridge::with_capacity(64, LayerId(1));
        let mut canvas = ActiveCanvas::new(&device, &queue, &ink, &location).unwrap();
        settle(&mut canvas);
        actual_pixels(&mut canvas);
        let history = canvas.projection.current().history.clone();
        let dirty = canvas.projection.current().dirty;
        let checked = before.iter().map(|(key, _)| key).collect();
        compare_gpu(&canvas, &before, &checked);
        assert_eq!(display_pixel(&canvas, [2, 2]), [255, 0, 0, 255]);
        assert_eq!(display_pixel(&canvas, [35, 2]), [255, 0, 0, 255]);
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Begin)
            .unwrap();
        settle(&mut canvas);
        let adjustment = ColorAdjustment {
            hue_degrees: 120,
            ..Default::default()
        };
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Preview(adjustment))
            .unwrap();
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Preview(ColorAdjustment {
                hue_degrees: -120,
                ..Default::default()
            }))
            .unwrap();
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Preview(adjustment))
            .unwrap();
        settle(&mut canvas);
        let preview = canvas.transform_display.clone().unwrap();
        assert_eq!(
            TileSnapshot::from_tiles(canvas.cpu_tiles.clone()).unwrap(),
            before
        );
        assert_eq!(canvas.projection.current().history, history);
        assert_eq!(canvas.projection.current().dirty, dirty);
        assert_eq!(
            canvas
                .projection
                .current()
                .edit
                .color_adjustment
                .as_ref()
                .unwrap()
                .adjustment,
            adjustment
        );
        compare_gpu(&canvas, &preview, &checked);
        assert_eq!(display_pixel(&canvas, [2, 2]), [0, 255, 0, 255]);
        assert_eq!(display_pixel(&canvas, [35, 2]), [0, 255, 0, 255]);
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Cancel)
            .unwrap();
        settle(&mut canvas);
        assert!(canvas.transform_display.is_none());
        assert!(canvas.projection.current().edit.color_adjustment.is_none());
        assert_eq!(canvas.projection.current().history, history);
        compare_gpu(&canvas, &before, &checked);
        assert_eq!(display_pixel(&canvas, [2, 2]), [255, 0, 0, 255]);
        assert_eq!(display_pixel(&canvas, [35, 2]), [255, 0, 0, 255]);
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Begin)
            .unwrap();
        settle(&mut canvas);
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Preview(adjustment))
            .unwrap();
        settle(&mut canvas);
        let generation = canvas
            .projection
            .current()
            .edit
            .color_adjustment
            .as_ref()
            .unwrap()
            .generation;
        canvas
            .enqueue_color_adjustment(ColorAdjustmentCommand::Commit { generation })
            .unwrap();
        settle(&mut canvas);
        let after = TileSnapshot::from_tiles(canvas.cpu_tiles.clone()).unwrap();
        assert_eq!(after, preview);
        compare_gpu(&canvas, &after, &checked);
        assert!(canvas.projection.current().dirty);
        for (command, expected, color) in [
            (HistoryCommand::Undo, &before, [255, 0, 0, 255]),
            (HistoryCommand::Redo, &after, [0, 255, 0, 255]),
        ] {
            canvas
                .apply_editor_command(EditorCommand::History(command), 640, 480, 1.0)
                .unwrap();
            settle(&mut canvas);
            assert_eq!(
                TileSnapshot::from_tiles(canvas.cpu_tiles.clone()).unwrap(),
                *expected
            );
            compare_gpu(&canvas, expected, &checked);
            assert_eq!(display_pixel(&canvas, [2, 2]), color);
            assert_eq!(display_pixel(&canvas, [35, 2]), color);
        }
        canvas
            .apply_editor_command(EditorCommand::Project(ProjectCommand::Save), 640, 480, 1.0)
            .unwrap();
        settle(&mut canvas);
        drop(canvas);
        let db = ProjectDb::open(&path).unwrap();
        let reopened = db.load_reopened().unwrap().unwrap();
        assert_eq!(reopened.current_tiles(), &after);
        assert_eq!(reopened.history().node_count(), 2);
        let page_pixels = flatten_layer_tree_rgba8(reopened.current_tiles(), &tree, page).unwrap();
        let saved = nyatidraw_png_io::decode_png(&path.with_extension("png"), LayerId(1)).unwrap();
        let exported = nyatidraw_png_io::decode_png_bytes(
            &nyatidraw_png_io::encode_png_bytes(&page_pixels).unwrap(),
            LayerId(1),
            1024,
        )
        .unwrap();
        assert_eq!(saved.tiles, exported.tiles);
        drop(db);
        std::fs::remove_file(path.with_extension("png")).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn correction_previews_cancel_without_history_and_confirmed_selected_pixels_reopen_in_fresh_process()
     {
        const REOPEN: &str = "NYATIDRAW_COLOR_WORKER_REOPEN";
        const ROOT: &str = "NYATIDRAW_COLOR_WORKER_ROOT";
        if let Some(path) = std::env::var_os(REOPEN) {
            let path = std::path::Path::new(&path);
            let db = ProjectDb::open(path).unwrap();
            let reopened = db.load_reopened().unwrap().unwrap();
            let tree = db.load_layer_tree().unwrap().unwrap();
            assert_eq!(reopened.history().node_count(), 2);
            assert_eq!(
                format!("{:?}", reopened.current_tiles().root()),
                std::env::var(ROOT).unwrap()
            );
            let signed = TileKey {
                layer: LayerId(1),
                mip: 0,
                x: -1,
                y: -1,
            };
            assert_eq!(
                &reopened.current_tiles().get(signed).unwrap().pixels()[..4],
                &[0, 255, 0, 255]
            );
            let canvas = db.load_canvas_spec().unwrap();
            let page = flatten_layer_tree_rgba8(reopened.current_tiles(), &tree, canvas).unwrap();
            let saved =
                nyatidraw_png_io::decode_png(&path.with_extension("png"), LayerId(1)).unwrap();
            let exported = nyatidraw_png_io::decode_png_bytes(
                &nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
                LayerId(1),
                256,
            )
            .unwrap();
            assert_eq!(saved.canvas, canvas);
            assert_eq!(saved.tiles, exported.tiles);
            return;
        }
        let path = std::env::temp_dir().join(format!(
            "nyatidraw-color-worker-{}-{}.ntdr",
            std::process::id(),
            crate::native_canvas::system_timestamp_ns()
        ));
        let db = ProjectDb::open(&path).unwrap();
        let mut tree = crate::native_canvas::default_layer_tree();
        tree.insert(
            tree.root().id,
            1,
            LayerTreeNode::Raster(crate::native_canvas::empty_raster(
                LayerId(2),
                "Untouched".into(),
            )),
        )
        .unwrap();
        let canvas = CanvasSpec {
            width_px: 4,
            height_px: 4,
            pixels_per_inch: 96,
        };
        db.persist_canvas_spec(canvas).unwrap();
        let signed = TileKey {
            layer: LayerId(1),
            mip: 0,
            x: -1,
            y: -1,
        };
        let page_key = TileKey {
            x: 0,
            y: 0,
            ..signed
        };
        let other = TileKey {
            layer: LayerId(2),
            ..signed
        };
        let mut bytes = vec![0; TILE_BYTE_LEN];
        bytes[..4].copy_from_slice(&[255, 0, 0, 255]);
        bytes[4..8].copy_from_slice(&[32, 16, 8, 128]);
        let original = TileSnapshot::from_tiles([
            (signed, bytes.clone()),
            (page_key, bytes.clone()),
            (other, bytes),
        ])
        .unwrap();
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::default());
        let seed = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, original.clone())
            .unwrap();
        db.commit_structural_with_layer_tree(&seed, &tree).unwrap();
        session.accept_structural_change(&seed).unwrap();
        let offpage = selection_all([-128, -128], [1, 1], EditLimits::default()).unwrap();
        let onpage = selection_all([0, 0], [1, 1], EditLimits::default()).unwrap();
        let selection = Some(Arc::new(
            combine_selection(
                &offpage,
                &onpage,
                SelectionCombine::Add,
                EditLimits::default(),
            )
            .unwrap(),
        ));
        let mut next = 10;
        let mut worker = Worker::default();
        let run = |worker: &mut Worker, command, session: &mut _, next: &mut _| {
            worker
                .execute(command, LayerId(1), &tree, &selection, session, next, &db)
                .unwrap_or_else(|e| match e {
                    EditFailure::Rejected(e) | EditFailure::Fatal(e) => panic!("{e}"),
                })
        };
        let neutral = run(
            &mut worker,
            ColorAdjustmentCommand::Begin,
            &mut session,
            &mut next,
        );
        let neutral_commit = run(
            &mut worker,
            ColorAdjustmentCommand::Commit {
                generation: neutral.projection.unwrap().generation,
            },
            &mut session,
            &mut next,
        );
        assert!(!neutral_commit.committed);
        assert_eq!(next, 10);
        assert_eq!(session.history().node_count(), 1);
        let first = run(
            &mut worker,
            ColorAdjustmentCommand::Begin,
            &mut session,
            &mut next,
        );
        let old_generation = first.projection.unwrap().generation;
        let adjustment = ColorAdjustment {
            hue_degrees: 120,
            ..Default::default()
        };
        let preview = run(
            &mut worker,
            ColorAdjustmentCommand::Preview(adjustment),
            &mut session,
            &mut next,
        );
        assert_eq!(session.tiles(), &original);
        assert_eq!(
            db.load_reopened().unwrap().unwrap().current_tiles(),
            &original
        );
        assert_eq!(preview.tiles.get(other), original.get(other));
        for key in [signed, page_key] {
            assert_eq!(
                &preview.tiles.get(key).unwrap().pixels()[..4],
                &[0, 255, 0, 255]
            );
            assert!(
                preview.tiles.get(key).unwrap().pixels()[4..]
                    == original.get(key).unwrap().pixels()[4..],
                "unselected tile pixels changed"
            );
        }
        assert!(
            worker
                .execute(
                    ColorAdjustmentCommand::Commit {
                        generation: old_generation
                    },
                    LayerId(1),
                    &tree,
                    &selection,
                    &mut session,
                    &mut next,
                    &db
                )
                .is_err()
        );
        let cancel = run(
            &mut worker,
            ColorAdjustmentCommand::Cancel,
            &mut session,
            &mut next,
        );
        assert_eq!(cancel.tiles, original);
        assert_eq!(next, 10);
        assert_eq!(session.history().node_count(), 1);
        run(
            &mut worker,
            ColorAdjustmentCommand::Begin,
            &mut session,
            &mut next,
        );
        let latest = run(
            &mut worker,
            ColorAdjustmentCommand::Preview(adjustment),
            &mut session,
            &mut next,
        );
        let generation = latest.projection.unwrap().generation;
        let committed = run(
            &mut worker,
            ColorAdjustmentCommand::Commit { generation },
            &mut session,
            &mut next,
        );
        assert!(committed.committed);
        assert_eq!(session.tiles(), &committed.tiles);
        assert_eq!(session.history().node_count(), 2);
        assert_eq!(next, 11);
        assert!(
            worker
                .execute(
                    ColorAdjustmentCommand::Commit { generation },
                    LayerId(1),
                    &tree,
                    &selection,
                    &mut session,
                    &mut next,
                    &db
                )
                .is_err()
        );
        let undo = session.prepare_undo_cursor().unwrap();
        let restored = db.load_cursor_tiles(undo.target()).unwrap();
        assert_eq!(restored, original);
        db.persist_history_cursor(undo.target()).unwrap();
        session.accept_history_cursor_move(undo, restored).unwrap();
        let redo = session.prepare_redo_cursor().unwrap();
        let restored = db.load_cursor_tiles(redo.target()).unwrap();
        assert_eq!(restored, committed.tiles);
        db.persist_history_cursor(redo.target()).unwrap();
        session.accept_history_cursor_move(redo, restored).unwrap();
        let page = flatten_layer_tree_rgba8(session.tiles(), &tree, canvas).unwrap();
        std::fs::write(
            path.with_extension("png"),
            nyatidraw_png_io::encode_png_bytes(&page).unwrap(),
        )
        .unwrap();
        let expected_root = format!("{:?}", session.tiles().root());
        drop(db);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_canvas::color_adjustment::tests::correction_previews_cancel_without_history_and_confirmed_selected_pixels_reopen_in_fresh_process", "--nocapture"])
            .env(REOPEN, &path).env(ROOT, expected_root).status().unwrap();
        assert!(status.success());
        std::fs::remove_file(path.with_extension("png")).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

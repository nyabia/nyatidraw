use std::{ffi::OsString, path::Path};

use nyatidraw_api::{CanvasSpec, LayerId, LayerTreeNodeId};
use nyatidraw_document::{AnimationTimeline, LayerTree};
use nyatidraw_editor::animation_export::{export_sprite_sheet, sprite_sheet_manifest};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::TileSnapshot;

pub(super) struct ExportSource {
    pub tiles: TileSnapshot,
    pub tree: LayerTree,
    pub canvas: CanvasSpec,
    pub timeline: Option<AnimationTimeline>,
    active_layer: Option<LayerId>,
}

impl ExportSource {
    pub(super) fn still_tree(&self) -> Result<LayerTree, String> {
        let Some(timeline) = &self.timeline else {
            return Ok(self.tree.clone());
        };
        let ancestors = self
            .active_layer
            .and_then(|layer| self.tree.ancestors(LayerTreeNodeId::Raster(layer)))
            .unwrap_or_default();
        let frame = timeline
            .frames()
            .iter()
            .find(|frame| {
                timeline
                    .cel_for_frame(frame.id)
                    .is_some_and(|cel| ancestors.contains(&cel.root_group))
            })
            .or_else(|| timeline.frames().first())
            .ok_or("animation has no frames")?;
        timeline
            .resolve_frame_tree(&self.tree, frame.id)
            .map_err(|error| error.to_string())
    }
}

pub(super) fn load_source(path: &Path) -> Result<ExportSource, String> {
    let database = ProjectDb::open_read_only(path).map_err(|error| error.to_string())?;
    let active_layer = database.load_active_layer().ok().flatten();
    let reopened = database
        .load_reopened()
        .map_err(|error| error.to_string())?;
    let (tiles, canvas, tree, timeline) = if let Some(reopened) = reopened {
        let parts = reopened.into_parts();
        (
            parts.current_tiles,
            database
                .load_cursor_canvas_spec(parts.current_cursor)
                .map_err(|error| error.to_string())?,
            database
                .load_cursor_layer_tree(parts.current_cursor)
                .map_err(|error| error.to_string())?,
            database
                .load_cursor_animation_timeline(parts.current_cursor)
                .map_err(|error| error.to_string())?,
        )
    } else {
        (
            TileSnapshot::empty(),
            database
                .load_canvas_spec()
                .map_err(|error| error.to_string())?,
            database
                .load_layer_tree()
                .map_err(|error| error.to_string())?,
            database
                .load_animation_timeline()
                .map_err(|error| error.to_string())?,
        )
    };
    let tree =
        tree.ok_or("project has no layer metadata; reopen and save it in NyatiDraw first")?;
    Ok(ExportSource {
        tiles,
        tree,
        canvas,
        timeline,
        active_layer,
    })
}

pub(super) fn run(args: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let project = super::required_path(args, "project path")?;
    let destination = super::required_path(args, "new spritesheet directory")?;
    let columns = args
        .next()
        .map(|arg| {
            arg.into_string()
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
                .ok_or_else(|| "columns must be a positive integer".to_owned())
        })
        .transpose()?;
    super::no_extra_args(args)?;
    if destination.exists() {
        return Err("spritesheet output directory must not already exist".into());
    }
    let source = load_source(&project)?;
    let timeline = source
        .timeline
        .as_ref()
        .ok_or("project has no animation timeline")?;
    let sheet = export_sprite_sheet(
        &source.tiles,
        &source.tree,
        timeline,
        source.canvas,
        columns,
    )
    .map_err(|error| error.to_string())?;
    nyatidraw_png_io::publish_sprite_sheet_new(
        &destination,
        &sheet.surface,
        sprite_sheet_manifest(&sheet).as_bytes(),
    )?;
    println!(
        "{{\"event\":\"export-spritesheet\",\"frames\":{},\"width\":{},\"height\":{},\"source_unchanged\":true}}",
        sheet.frames.len(),
        sheet.surface.width,
        sheet.surface.height
    );
    Ok(())
}

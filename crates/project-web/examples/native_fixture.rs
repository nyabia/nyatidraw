use std::{error::Error, path::Path};

use nyatidraw_api::{CanvasSpec, HistoryNodeId, SnapshotId};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_project_web::WebProject;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use nyatidraw_web_core::WebDocument;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    let mode = arguments
        .get(1)
        .ok_or("usage: native_fixture create|inspect PATH")?;
    let path = Path::new(arguments.get(2).ok_or("missing scratch path")?);
    if mode == "create" {
        create(path)?;
    } else if mode != "inspect" {
        return Err("mode must be create or inspect".into());
    }
    let bytes = std::fs::read(path)?;
    let project = WebProject::decode_ntdr(&bytes)?;
    let rgba = project.export_rgba8()?;
    let native = ProjectDb::open(path)?;
    let reopened = native
        .load_reopened()?
        .ok_or("native fixture has no history")?
        .into_parts();
    if reopened.current_tiles != *project.snapshot()
        || native
            .load_cursor_layer_tree(reopened.current_cursor)?
            .as_ref()
            != Some(project.layers())
        || native.load_cursor_canvas_spec(reopened.current_cursor)? != project.canvas()
    {
        return Err("native disk reopen and browser decoder disagree".into());
    }
    println!(
        "{{\"width\":{},\"height\":{},\"tiles\":{},\"layers\":{},\"history\":{},\"rgba_blake3\":\"{}\",\"root\":\"{:?}\"}}",
        project.canvas().width_px,
        project.canvas().height_px,
        project.snapshot().len(),
        project.layers().root().children.len(),
        reopened.history.node_count(),
        blake3::hash(&rgba),
        project.snapshot().root().hash
    );
    Ok(())
}

fn create(path: &Path) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Err("fixture destination must not exist".into());
    }
    let mut pixels = vec![0; 64 * 64 * 4];
    for y in 8..56 {
        for x in 8..56 {
            let i = (y * 64 + x) * 4;
            pixels[i..i + 4].copy_from_slice(if x < 32 {
                &[230, 40, 65, 255]
            } else {
                &[30, 180, 210, 180]
            });
        }
    }
    let document = WebDocument::import_rgba8(64, 64, &pixels)?;
    let canvas = CanvasSpec {
        width_px: 64,
        height_px: 64,
        pixels_per_inch: 300,
    };
    let db = ProjectDb::open(path)?;
    db.persist_layer_tree(document.layers())?;
    db.persist_canvas_spec(canvas)?;
    let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
    let batch = session
        .prepare_structural_change(
            SnapshotId(1),
            HistoryNodeId(1),
            0,
            document.snapshot().clone(),
        )
        .map_err(|e| format!("{e:?}"))?;
    db.commit_structural_with_metadata(&batch, document.layers(), canvas)?;
    session
        .accept_structural_change(&batch)
        .map_err(|e| format!("{e:?}"))?;
    let mut outside = vec![0; TILE_BYTE_LEN];
    outside[..4].copy_from_slice(&[128, 0, 0, 128]);
    let tiles = document
        .snapshot()
        .with_replacements([(
            TileKey {
                layer: document.active_layer(),
                mip: 0,
                x: -1,
                y: 0,
            },
            outside,
        )])
        .map_err(|e| format!("{e:?}"))?;
    let batch = session
        .prepare_structural_change(SnapshotId(2), HistoryNodeId(2), 0, tiles)
        .map_err(|e| format!("{e:?}"))?;
    db.commit_structural_with_metadata(&batch, document.layers(), canvas)?;
    Ok(())
}

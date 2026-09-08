//! Scratch-only create/verify around an actual desktop Save As operation.
//! Run `create DIR`, open its source.ntdr in the desktop, use Save As to
//! `사본 그림.ntdr`, close the desktop, then run `verify DIR` in a new process.
//! The verifier never substitutes a filesystem copy for the UI operation.
use std::{fs, path::Path};

use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const SCRATCH_NAME: &str = "nyatidraw-save-as-scratch";
const SOURCE: &str = "source.ntdr";
const TARGET: &str = "사본 그림.ntdr";
const MARKER: &str = ".nyatidraw-save-as-fixture";
const BASELINE: &str = "source.original-bytes";
const EXPECTED_PNG: &str = "expected.png";

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    let mode = args
        .get(1)
        .and_then(|value| value.to_str())
        .ok_or("create/verify required")?;
    let directory = Path::new(args.get(2).ok_or("scratch directory required")?);
    if directory.file_name().and_then(|value| value.to_str()) != Some(SCRATCH_NAME) {
        return Err(format!(
            "directory must end in {SCRATCH_NAME}; existing directories are never initialized"
        )
        .into());
    }
    match mode {
        "create" => create(directory),
        "verify" => verify(directory),
        _ => Err("expected create or verify".into()),
    }
}

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LayerId(1),
            name: "Save As scratch".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })],
    })
    .expect("fixed valid fixture tree")
}

fn pixels(value: u8) -> Result<TileSnapshot> {
    // Keep signed off-page artwork as well as visible pixels: a flattened-only
    // Save As would lose this tile even if its exported PNG looked correct.
    let visible = [value, 30, 80, 255].repeat(TILE_BYTE_LEN / 4);
    let outside = [80, value, 30, 255].repeat(TILE_BYTE_LEN / 4);
    TileSnapshot::from_tiles([(0, visible), (-1, outside)].map(|(x, bytes)| {
        (
            TileKey {
                layer: LayerId(1),
                mip: 0,
                x,
                y: 0,
            },
            bytes,
        )
    }))
    .map_err(|error| format!("fixture tiles: {error:?}").into())
}

fn append(database: &ProjectDb, session: &mut HeadlessStrokeSession, id: u128) -> Result<()> {
    let batch = session
        .prepare_structural_change(
            SnapshotId(id),
            HistoryNodeId(id),
            u64::try_from(id)?,
            pixels(u8::try_from(id * 40)?)?,
        )
        .map_err(|error| format!("prepare fixture: {error:?}"))?;
    database.commit_structural_with_layer_tree(&batch, &tree())?;
    session
        .accept_structural_change(&batch)
        .map_err(|error| format!("accept fixture: {error:?}"))?;
    Ok(())
}

fn create(directory: &Path) -> Result<()> {
    fs::create_dir(directory)?; // Exclusive: never reuse a user's directory.
    fs::write(directory.join(MARKER), b"nyatidraw-save-as-fixture-v1")?;
    let source = directory.join(SOURCE);
    let database = ProjectDb::open(&source)?;
    let canvas = CanvasSpec {
        width_px: 16,
        height_px: 16,
        pixels_per_inch: 96,
    };
    database.persist_canvas_spec(canvas)?;
    let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
    append(&database, &mut session, 1)?;
    append(&database, &mut session, 2)?;
    let undo = session
        .prepare_undo_cursor()
        .map_err(|error| format!("undo: {error:?}"))?;
    let restored = database.load_cursor_tiles(undo.target())?;
    database.persist_history_cursor(undo.target())?;
    session
        .accept_history_cursor_move(undo, restored)
        .map_err(|error| format!("accept undo: {error:?}"))?;
    append(&database, &mut session, 3)?; // Snapshot 2 is an off-current redo branch.
    let reopened = database.load_reopened()?.ok_or("fixture history absent")?;
    let surface =
        nyatidraw_paint_cpu::flatten_layer_tree_rgba8(reopened.current_tiles(), &tree(), canvas)
            .map_err(|error| format!("fixture composite: {error:?}"))?;
    nyatidraw_png_io::encode_png(&directory.join(EXPECTED_PNG), &surface)?;
    drop(database);
    fs::copy(&source, directory.join(BASELINE))?;
    println!(
        "save-as-fixture created source={} target={} nodes=3 off_current_redo=2 signed_tile=true",
        source.display(),
        directory.join(TARGET).display()
    );
    Ok(())
}

fn verify(directory: &Path) -> Result<()> {
    if fs::read(directory.join(MARKER))? != b"nyatidraw-save-as-fixture-v1" {
        return Err("not a Save As scratch fixture".into());
    }
    let source = directory.join(SOURCE);
    let target = directory.join(TARGET);
    if fs::read(&source)? != fs::read(directory.join(BASELINE))? {
        return Err("source bytes changed".into());
    }
    let original = ProjectDb::open(&source)?;
    let copied = ProjectDb::open(&target)?;
    let expected = original
        .load_reopened()?
        .ok_or("source history absent")?
        .into_parts();
    let actual = copied
        .load_reopened()?
        .ok_or("target history absent")?
        .into_parts();
    if actual.current_cursor != expected.current_cursor
        || actual.cursors != expected.cursors
        || actual.current_tiles != expected.current_tiles
        || actual.history.node_count() != 3
    {
        return Err("Save As changed current artwork or lost redo history".into());
    }
    for cursor in expected.cursors.values() {
        if original.load_cursor_tiles(*cursor)? != copied.load_cursor_tiles(*cursor)? {
            return Err("an immutable history snapshot changed".into());
        }
    }
    if original.load_layer_tree()? != copied.load_layer_tree()?
        || original.load_canvas_spec()? != copied.load_canvas_spec()?
    {
        return Err("Save As changed layer/canvas metadata".into());
    }
    if fs::read(target.with_extension("png"))? != fs::read(directory.join(EXPECTED_PNG))? {
        return Err("target PNG does not match expected durable composite".into());
    }
    println!(
        "save-as-fixture status=passed source_bytes=unchanged history_nodes=3 all_cursor_tiles=equal metadata=equal png=byte-equal reopen=independent-process ui_trigger=external physical_pen_proof=false"
    );
    Ok(())
}

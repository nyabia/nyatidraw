//! Separate-process selected brush/eraser, history and legacy-reader acceptance.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_brush::{
    BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, ROUND_BRUSH_ENGINE_VERSION,
    RoundBrushEvaluator, begin_round_stroke,
};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_stroke::{StrokeColor, StrokeSelection};
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{path::Path, process::Command, sync::Arc};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 17,
    height_px: 5,
    pixels_per_inch: 96,
};
const MARKER: &str = ".nyatidraw-selected-stroke-probe";

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LayerId(1),
            name: "Ink".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })],
    })
    .unwrap()
}

fn color(stage: u128, x: usize) -> [u8; 4] {
    if stage == 3 && (4..12).contains(&x) {
        [0; 4]
    } else if stage >= 2 && x < 8 {
        [255, 0, 0, 255]
    } else {
        [40, 40, 40, 255]
    }
}

fn expected(stage: u128) -> Result<TileSnapshot> {
    let mut pixels = [40, 40, 40, 255].repeat(TILE_BYTE_LEN / 4);
    for y in 0..5 {
        for x in 0..17 {
            let offset = (y * 128 + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&color(stage, x));
        }
    }
    TileSnapshot::from_tiles(
        [
            (0, pixels),
            (-1, [99, 99, 99, 255].repeat(TILE_BYTE_LEN / 4)),
        ]
        .map(|(x, pixels)| {
            (
                TileKey {
                    layer: LayerId(1),
                    mip: 0,
                    x,
                    y: 0,
                },
                pixels,
            )
        }),
    )
    .map_err(|error| format!("fixture: {error:?}").into())
}

fn mask(eraser: bool) -> Arc<StrokeSelection> {
    let mut bits = vec![0; (17_usize * 5).div_ceil(8)];
    for y in 0..5 {
        for x in if eraser { 4..12 } else { 0..8 } {
            let index = y * 17 + x;
            bits[index / 8] |= 1 << (index % 8);
        }
    }
    Arc::new(StrokeSelection::from_packed_bits(17, 5, &bits).unwrap())
}

fn edit(project: &Path, operation: &str) -> Result<()> {
    let db = ProjectDb::open(project)?;
    if operation == "base" {
        db.persist_canvas_spec(CANVAS)?;
        let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(1)?)
            .map_err(|error| format!("base: {error:?}"))?;
        db.commit_structural_with_layer_tree(&batch, &tree())?;
        return Ok(());
    }
    let session = HeadlessStrokeSession::from_reopened(db.load_reopened()?.ok_or("missing head")?);
    if operation == "undo" || operation == "redo" {
        let movement = if operation == "undo" {
            session.prepare_undo_cursor()
        } else {
            session.prepare_redo_cursor()
        }
        .map_err(|error| format!("history: {error:?}"))?;
        db.persist_history_cursor(movement.target())?;
        return Ok(());
    }
    let eraser = operation == "erase";
    let id = if eraser { 3 } else { 2 };
    let preset = BrushPreset {
        id: BrushPresetId(1),
        schema_version: 1,
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        size_px: 64.0,
        opacity: 1.0,
        flow: 1.0,
        spacing_ratio: 0.25,
    };
    let samples = [PointerPhase::Begin, PointerPhase::End]
        .into_iter()
        .enumerate()
        .map(|(index, phase)| {
            let sequence = u64::try_from(index).unwrap() + 1;
            StylusSample {
                sequence,
                timestamp_ns: sequence * 1_000,
                device_id: 1,
                phase,
                position_document: Point { x: 8.0, y: 2.0 },
                pressure: 1.0,
                tilt: None,
                twist_radians: None,
                tangential_pressure: None,
                buttons: PenButtons::default(),
                eraser,
                viewport_revision: 1,
            }
        })
        .collect::<Vec<_>>();
    let mut evaluator = RoundBrushEvaluator::new(1);
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
    evaluator.push(&mut token, &samples[1..], &mut dabs);
    let recorded = evaluator.end(token, &mut dabs);
    let batch = session
        .prepare_round_stroke_with_selection(
            SnapshotId(id),
            HistoryNodeId(id),
            10,
            LayerId(1),
            BrushSnapshot { preset },
            recorded,
            StrokeColor([255, 0, 0, 255]),
            samples,
            Some(mask(eraser)),
        )
        .map_err(|error| format!("selected stroke: {error:?}"))?;
    db.commit(&batch)?;
    Ok(())
}

fn verify(project: &Path, label: &str, stage: u128, nodes: usize) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing reopened head")?;
    if reopened.current_snapshot() != SnapshotId(stage)
        || reopened.history().node_count() != nodes
        || reopened.current_tiles() != &expected(stage)?
        || db.load_layer_tree()? != Some(tree())
    {
        return Err(format!("{label}: tiles/tree/history differ").into());
    }
    if stage > 1
        && db
            .load_current()?
            .ok_or("missing stroke")?
            .stroke
            .selection()
            != Some(mask(stage == 3).as_ref())
    {
        return Err("stored replay selection changed".into());
    }
    let actual_pixels =
        nyatidraw_paint_cpu::flatten_layer_tree_rgba8(reopened.current_tiles(), &tree(), CANVAS)
            .map_err(|error| format!("flatten: {error:?}"))?;
    let golden_pixels = FlattenedRgba8 {
        origin_x: 0,
        origin_y: 0,
        width: 17,
        height: 5,
        pixels: (0..5)
            .flat_map(|_| (0..17).flat_map(|x| color(stage, x)))
            .collect(),
    };
    let actual = project.with_file_name(format!("{label}-actual.png"));
    let golden = project.with_file_name(format!("{label}-golden.png"));
    nyatidraw_png_io::encode_png(&actual, &actual_pixels)?;
    nyatidraw_png_io::encode_png(&golden, &golden_pixels)?;
    let actual = nyatidraw_png_io::decode_png(&actual, LayerId(1))?;
    let golden = nyatidraw_png_io::decode_png(&golden, LayerId(1))?;
    if actual.canvas != golden.canvas || actual.tiles != golden.tiles {
        return Err("PNG mismatch".into());
    }
    println!(
        "selected-stroke-reopen stage={label} snapshot={stage} nodes={nodes} pixels=exact mask=exact png=exact"
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if matches!(args.get(1).map(String::as_str), Some("--edit" | "--verify")) {
        let project = Path::new(args.get(2).ok_or("scratch project")?);
        if !project.starts_with(std::env::temp_dir())
            || !project.parent().ok_or("parent")?.join(MARKER).is_file()
        {
            return Err("requires marked scratch directory".into());
        }
        return if args[1] == "--edit" {
            edit(project, &args[3])
        } else {
            verify(project, &args[3], args[4].parse()?, args[5].parse()?)
        };
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("nyatidraw-selected-stroke-{nonce}"));
    std::fs::create_dir(&directory)?;
    std::fs::write(directory.join(MARKER), b"scratch")?;
    let project = directory.join("async-edit-scratch.ntdr");
    for (operation, stage, nodes) in [
        ("base", 1, 1),
        ("brush", 2, 2),
        ("erase", 3, 3),
        ("undo", 2, 3),
        ("redo", 3, 3),
    ] {
        for mode in ["--edit", "--verify"] {
            let status = Command::new(std::env::current_exe()?)
                .arg(mode)
                .arg(&project)
                .arg(operation)
                .arg(stage.to_string())
                .arg(nodes.to_string())
                .status()?;
            if !status.success() {
                return Err(format!("{mode} {operation} child failed").into());
            }
        }
        if stage > 1 && args.get(1).map(String::as_str) == Some("--legacy-reader") {
            let before = std::fs::read(&project)?;
            let output = Command::new(args.get(2).ok_or("legacy reader path")?)
                .arg("verify")
                .arg(&project)
                .args(["2", "2"])
                .output()?;
            let error = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                || !error.contains("InvalidNonEmpty")
                || before != std::fs::read(&project)?
            {
                return Err(format!(
                    "legacy reader did not reject without changing bytes: {error}"
                )
                .into());
            }
            println!(
                "selected-stroke-legacy stage={operation} rejected=true original_bytes=preserved"
            );
        }
    }
    println!(
        "selected-stroke-reopen status=passed backend=cpu physical_pen=false directory={}",
        directory.display()
    );
    Ok(())
}

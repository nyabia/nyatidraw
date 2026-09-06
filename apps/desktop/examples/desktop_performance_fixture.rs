//! Scratch setup and separate-process replay/reopen/export agreement for paced input.
#[path = "../src/performance_workload.rs"]
mod workload;

use nyatidraw_api::{ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_brush::{BrushEvaluator, BrushSnapshot, RoundBrushEvaluator, begin_round_stroke};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_stroke::StrokeColor;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{collections::BTreeMap, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: [2, 1]
            .map(|id| {
                LayerTreeNode::Raster(LayerNode {
                    id: LayerId(id),
                    name: if id == 1 {
                        "Paced ink"
                    } else {
                        "Locked background"
                    }
                    .into(),
                    visible: true,
                    locked: id == 2,
                    reference: false,
                    opacity_u16: u16::MAX,
                    content_root: ContentRootId(0),
                })
            })
            .into(),
    })
    .unwrap()
}

fn initial() -> Result<TileSnapshot> {
    let mut tiles = BTreeMap::new();
    for y in 0..i32::try_from(workload::CANVAS.height_px)? {
        for x in 0..i32::try_from(workload::CANVAS.width_px)? {
            let key = TileKey::from_pixel(LayerId(2), 128, x, y);
            let at = (y.rem_euclid(128) as usize * 128 + x.rem_euclid(128) as usize) * 4;
            tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN])[at..at + 4]
                .copy_from_slice(&[32, 32, 32, 255]);
        }
    }
    TileSnapshot::from_tiles(tiles).map_err(|e| format!("fixture tiles: {e:?}").into())
}

fn expected() -> Result<TileSnapshot> {
    let mut tiles = initial()?;
    let mut evaluator = RoundBrushEvaluator::new(0x4e41_5941_5449);
    for stroke in 0..workload::STROKES {
        let samples: Vec<_> = (0..=workload::LAST_SAMPLE)
            .map(|i| workload::sample(stroke, i))
            .collect();
        let mut dabs = Vec::new();
        let mut token = begin_round_stroke(&mut evaluator, &workload::BRUSH, samples[0], &mut dabs);
        evaluator.push(&mut token, &samples[1..], &mut dabs);
        let recorded = evaluator.end(token, &mut dabs);
        let id = u128::from(stroke) + 2;
        let session = HeadlessStrokeSession::new(SnapshotId(id - 1), tiles);
        let batch = session
            .prepare_round_stroke(
                SnapshotId(id),
                HistoryNodeId(id),
                1,
                LayerId(1),
                BrushSnapshot {
                    preset: workload::BRUSH,
                },
                recorded,
                StrokeColor(nyatidraw_tiles::color::srgb8_to_linear_premultiplied(
                    workload::COLOR,
                )),
                samples,
            )
            .map_err(|e| format!("replay: {e:?}"))?;
        tiles = batch.materialized.after.clone();
    }
    Ok(tiles)
}

fn verify(project: &Path) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing artwork")?;
    let tiles = expected()?;
    if reopened.current_snapshot() != SnapshotId(u128::from(workload::STROKES) + 1)
        || reopened.history().node_count() != workload::STROKES as usize + 1
        || db.load_canvas_spec()? != workload::CANVAS
        || db.load_layer_tree()? != Some(tree())
        || reopened.current_tiles() != &tiles
    {
        return Err("paced input differs from offline replay/page/tree/history".into());
    }
    let flattened =
        nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&tiles, &tree(), workload::CANVAS)
            .map_err(|e| format!("flatten: {e:?}"))?;
    let golden = project.with_extension("expected.png");
    nyatidraw_png_io::encode_png(&golden, &flattened)?;
    if std::fs::read(golden)? != std::fs::read(project.with_extension("png"))? {
        return Err("PNG differs from offline replay composite".into());
    }
    println!(
        "performance-verify canvas=3840x2160 strokes={} history={} tiles=exact png=exact root={:?} oracle=shared-deterministic-replay",
        workload::STROKES,
        reopened.history().node_count(),
        tiles.root()
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("create or verify")?;
    let project = Path::new(args.get(2).ok_or("scratch path")?);
    if project.file_name().and_then(|v| v.to_str()) != Some("performance-scratch.ntdr") {
        return Err("requires exact scratch filename".into());
    }
    match mode.as_str() {
        "create" => {
            if project.exists() {
                return Err("scratch must not exist".into());
            }
            let parent = project.parent().ok_or("scratch parent")?;
            std::fs::create_dir_all(parent)?;
            let db = ProjectDb::open(project)?;
            db.persist_canvas_spec(workload::CANVAS)?;
            let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
            let batch = session
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, initial()?)
                .map_err(|e| format!("fixture: {e:?}"))?;
            db.commit_structural_with_layer_tree(&batch, &tree())?;
            std::fs::write(
                parent.join(".nyatidraw-performance-scratch"),
                "Explicit synthetic performance scratch.\n",
            )?;
            println!("performance-fixture created={}", project.display());
            Ok(())
        }
        "verify" => verify(project),
        "measure-history-storage" => measure_history_storage(project),
        "verify-reference" => verify_reference(
            project,
            Path::new(args.get(3).ok_or("reference project path")?),
        ),
        _ => Err("invalid mode".into()),
    }
}

// Isolate the durable portion of the desktop worker. This deliberately excludes
// worker admission, previews, UI adoption and presentation; it is not Undo latency.
fn measure_history_storage(project: &Path) -> Result<()> {
    let marker = project
        .parent()
        .ok_or("scratch parent")?
        .join(".nyatidraw-performance-scratch");
    if !project.is_file() || !marker.is_file() {
        return Err("history measurements require an existing marked scratch".into());
    }
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing scratch artwork")?;
    let mut session = HeadlessStrokeSession::from_reopened(reopened);
    let original = session.tiles().clone();
    let snapshot = session.current_snapshot();
    println!(
        "history-storage fixture_tiles={} fixture_unique_objects={}",
        original.len(),
        original
            .iter()
            .map(|(_, object)| object.hash())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    for operation in 0..20 {
        let started = std::time::Instant::now();
        let prepared = if operation % 2 == 0 {
            session.prepare_undo_cursor()
        } else {
            session.prepare_redo_cursor()
        }
        .map_err(|error| format!("history prepare: {error:?}"))?;
        let target = prepared.target();
        let load_started = std::time::Instant::now();
        let tiles = db.load_cursor_tiles(target)?;
        let load_us = load_started.elapsed().as_micros();
        let metadata_started = std::time::Instant::now();
        let _tree = db.load_cursor_layer_tree(target)?;
        let _canvas = db.load_cursor_canvas_spec(target)?;
        let metadata_us = metadata_started.elapsed().as_micros();
        let persist_started = std::time::Instant::now();
        db.persist_history_cursor(target)?;
        let persist_us = persist_started.elapsed().as_micros();
        let accept_started = std::time::Instant::now();
        session
            .accept_history_cursor_move(prepared, tiles.clone())
            .map_err(|error| format!("history accept: {error:?}"))?;
        let accept_us = accept_started.elapsed().as_micros();
        let total_us = started.elapsed().as_micros();
        println!(
            "history-storage operation={operation} load_us={load_us} metadata_us={metadata_us} persist_us={persist_us} accept_us={accept_us} total_us={total_us} tiles={}",
            tiles.len()
        );
    }
    if session.current_snapshot() != snapshot || session.tiles() != &original {
        return Err("history measurement did not return to its original artwork".into());
    }
    println!("history-storage completed=20 original_artwork=exact");
    Ok(())
}

// Compare a copied historical scratch without reinterpreting its recorded color
// with today's UI transfer function. The reference project is never exported.
fn verify_reference(project: &Path, reference: &Path) -> Result<()> {
    if !project.is_file() || !reference.is_file() || project == reference {
        return Err("requires two existing separate scratch projects".into());
    }
    let reference_db = ProjectDb::open(reference)?;
    let reference_head = reference_db
        .load_reopened()?
        .ok_or("missing reference head")?;
    let canvas = reference_db.load_canvas_spec()?;
    let tree = reference_db
        .load_layer_tree()?
        .ok_or("missing reference tree")?;
    let db = ProjectDb::open(project)?;
    let head = db.load_reopened()?.ok_or("missing copied head")?;
    if head.current_snapshot() != reference_head.current_snapshot()
        || head.history().node_count() != reference_head.history().node_count()
        || head.current_tiles() != reference_head.current_tiles()
        || db.load_canvas_spec()? != canvas
        || db.load_layer_tree()? != Some(tree.clone())
    {
        return Err(
            "copied project no longer matches its reference artwork/history/page/tree".into(),
        );
    }
    let flattened = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(
        reference_head.current_tiles(),
        &tree,
        canvas,
    )
    .map_err(|e| format!("reference flatten: {e:?}"))?;
    let imported = nyatidraw_png_io::decode_png(&project.with_extension("png"), LayerId(1))?;
    let exported = imported
        .tiles
        .crop_base_layer_rgba8_to_canvas(LayerId(1), imported.canvas)
        .map_err(|e| format!("export crop: {e:?}"))?;
    if imported.canvas != canvas || exported.pixels != flattened.pixels {
        return Err("copied project PNG differs from reference artwork".into());
    }
    println!(
        "history-reference-verify snapshot={} history={} tiles={} all_tiles=exact png=exact root={:?}",
        head.current_snapshot().0,
        head.history().node_count(),
        head.current_tiles().iter().count(),
        head.current_tiles().root()
    );
    Ok(())
}

//! Scratch fixture creation and independent process-reopen verification for UI acceptance.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::path::Path;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 129,
    height_px: 4,
    pixels_per_inch: 96,
};

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: vec![LayerTreeNode::Raster(LayerNode {
            id: LayerId(1),
            name: "Scratch ink".into(),
            visible: true,
            locked: false,
            reference: false,
            opacity_u16: u16::MAX,
            content_root: ContentRootId(0),
        })],
    })
    .unwrap()
}

fn expected(filled: bool) -> Result<TileSnapshot> {
    let mut first = vec![0; TILE_BYTE_LEN];
    let mut second = vec![0; TILE_BYTE_LEN];
    second[4..8].copy_from_slice(&[9, 9, 9, 255]); // Off-page boundary padding.
    if filled {
        for y in 0..4 {
            for x in 0..129 {
                let offset = (y * 128 + x % 128) * 4;
                let tile = if x < 128 { &mut first } else { &mut second };
                tile[offset..offset + 4].copy_from_slice(&[0, 255, 0, 255]);
            }
        }
    }
    TileSnapshot::from_tiles(
        [
            (0, first),
            (1, second),
            (-1, [7, 7, 7, 255].repeat(TILE_BYTE_LEN / 4)),
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
    .map_err(|error| format!("expected: {error:?}").into())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("create or verify required")?;
    let project = Path::new(args.get(2).ok_or("scratch project required")?);
    if project.file_name().and_then(|name| name.to_str()) != Some("async-edit-scratch.ntdr") {
        return Err("requires exact scratch filename".into());
    }
    if mode == "create" || mode == "create-selection" {
        if project.exists() {
            return Err("scratch must not exist".into());
        }
        std::fs::create_dir_all(project.parent().ok_or("scratch parent")?)?;
        std::fs::write(
            project
                .parent()
                .ok_or("scratch parent")?
                .join(".nyatidraw-scratch-edit-probe"),
            b"scratch only",
        )?;
        let db = ProjectDb::open(project)?;
        db.persist_canvas_spec(if mode == "create-selection" {
            CanvasSpec {
                height_px: 65,
                ..CANVAS
            }
        } else {
            CANVAS
        })?;
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(false)?)
            .map_err(|error| format!("prepare: {error:?}"))?;
        db.commit_structural_with_layer_tree(&batch, &tree())?;
        session
            .accept_structural_change(&batch)
            .map_err(|error| format!("accept: {error:?}"))?;
    } else if mode == "verify-native-edit" {
        verify_native_edit(
            project,
            args.get(3).ok_or("snapshot")?.parse()?,
            args.get(4).ok_or("nodes")?.parse()?,
            args.get(5).ok_or("solid/gradient/empty")?,
        )?;
    } else if mode == "verify-selection" {
        verify_selection(
            project,
            args.get(3).ok_or("snapshot")?.parse()?,
            args.get(4).ok_or("nodes")?.parse()?,
        )?;
    } else if mode == "verify" {
        let stage: u128 = args.get(3).ok_or("snapshot required")?.parse()?;
        let nodes: usize = args.get(4).ok_or("node count required")?.parse()?;
        if ![1, 2].contains(&stage) {
            return Err("invalid fixture stage".into());
        }
        let db = ProjectDb::open(project)?;
        let reopened = db.load_reopened()?.ok_or("no durable head")?;
        if reopened.current_snapshot() != SnapshotId(stage)
            || reopened.history().node_count() != nodes
            || reopened.current_tiles() != &expected(stage == 2)?
            || db.load_layer_tree()? != Some(tree())
        {
            return Err("durable tiles/tree/cursor/history differ".into());
        }
        let golden = project.with_extension("expected.png");
        let rgba = if stage == 2 { [0, 255, 0, 255] } else { [0; 4] };
        nyatidraw_png_io::encode_png(
            &golden,
            &FlattenedRgba8 {
                origin_x: 0,
                origin_y: 0,
                width: 129,
                height: 4,
                pixels: rgba.repeat(129 * 4),
            },
        )?;
        let actual = nyatidraw_png_io::decode_png(&project.with_extension("png"), LayerId(1))?;
        let expected = nyatidraw_png_io::decode_png(&golden, LayerId(1))?;
        if actual.canvas != expected.canvas || actual.tiles != expected.tiles {
            return Err("export pixels differ".into());
        }
    } else {
        return Err("invalid mode".into());
    }
    println!(
        "desktop-edit-fixture status=passed mode={mode} project={}",
        project.display()
    );
    Ok(())
}

fn verify_native_edit(project: &Path, stage: u128, nodes: usize, pattern: &str) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing head")?;
    let canvas = CanvasSpec {
        height_px: 65,
        ..CANVAS
    };
    if reopened.current_snapshot() != SnapshotId(stage)
        || reopened.history().node_count() != nodes
        || db.load_layer_tree()? != Some(tree())
        || db.load_canvas_spec()? != canvas
    {
        return Err("native edit metadata mismatch".into());
    }
    let color_at = |x: i64, y: i64| -> Result<[u8; 4]> {
        Ok(match pattern {
            "solid" => [26, 199, 232, 255],
            "rectangle" if (10..100).contains(&x) && (10..55).contains(&y) => [26, 199, 232, 255],
            "rectangle" | "empty" => [0; 4],
            // UI drag document x=10 -> 110. Independent rational interpolation
            // at pixel centers, with the current color fading to transparent.
            "gradient" => {
                let numerator = (220 - (2 * x + 1)).clamp(0, 200);
                [26_i64, 199, 232, 255]
                    .map(|channel| u8::try_from((channel * numerator + 100) / 200).unwrap())
            }
            _ => return Err("unknown native edit pattern".into()),
        })
    };
    let before = expected(false)?;
    let keys: std::collections::BTreeSet<_> = before
        .iter()
        .chain(reopened.current_tiles().iter())
        .map(|(key, _)| key)
        .collect();
    for key in keys {
        let old = before.get(key);
        let actual = reopened.current_tiles().get(key);
        let (origin_x, origin_y) = key.pixel_origin();
        for index in 0..128 * 128 {
            let x = origin_x + i64::try_from(index % 128)?;
            let y = origin_y + i64::try_from(index / 128)?;
            let expected: &[u8] = if (0..129).contains(&x) && (0..65).contains(&y) {
                &color_at(x, y)?
            } else {
                old.map_or(&[0; 4], |tile| &tile.pixels()[index * 4..index * 4 + 4])
            };
            let actual =
                actual.map_or(&[0; 4][..], |tile| &tile.pixels()[index * 4..index * 4 + 4]);
            if actual != expected {
                return Err(format!(
                    "native edit pixels differ ({x},{y}): {actual:?} != {expected:?}"
                )
                .into());
            }
        }
    }
    let mut pixels = Vec::new();
    for y in 0..65 {
        for x in 0..129 {
            pixels.extend_from_slice(&color_at(x, y)?);
        }
    }
    let golden = project.with_extension("expected.png");
    nyatidraw_png_io::encode_png(
        &golden,
        &FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 129,
            height: 65,
            pixels,
        },
    )?;
    let actual = nyatidraw_png_io::decode_png(&project.with_extension("png"), LayerId(1))?;
    let expected = nyatidraw_png_io::decode_png(&golden, LayerId(1))?;
    if actual.canvas != expected.canvas || actual.tiles != expected.tiles {
        return Err("native edit PNG/chrome mismatch".into());
    }
    println!(
        "native-edit-verify snapshot={stage} nodes={nodes} pattern={pattern} all_pixels=exact outside=exact png=exact"
    );
    Ok(())
}

fn verify_selection(project: &Path, stage: u128, nodes: usize) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing head")?;
    let canvas = CanvasSpec {
        height_px: 65,
        ..CANVAS
    };
    if reopened.current_snapshot() != SnapshotId(stage)
        || reopened.history().node_count() != nodes
        || db.load_layer_tree()? != Some(tree())
        || db.load_canvas_spec()? != canvas
    {
        return Err("selection fixture metadata mismatch".into());
    }
    if stage > 1 {
        let batch = db.load_current()?.ok_or("missing selected stroke")?;
        let mask = batch.stroke.selection().ok_or("stroke lost selection")?;
        if mask.dimensions() != [129, 65] || mask.selected_pixels() != 64 * 65 {
            return Err("wrong recorded selection".into());
        }
        for y in 0..65 {
            for x in 0..129 {
                if mask.contains(x, y) != (x < 64) {
                    return Err("recorded mask drift".into());
                }
            }
        }
        let replay = nyatidraw_stroke::materialize_with_strategy(
            nyatidraw_stroke::MaterializationStrategy::CpuReplay,
            &batch.stroke,
            &batch.before,
        )
        .map_err(|error| format!("replay: {error:?}"))?;
        if replay.after != *reopened.current_tiles() {
            return Err("stored stroke replay differs".into());
        }
        let eraser = batch.stroke.samples()[0].eraser;
        let mut changed_alpha = 0;
        for (key, tile) in reopened.current_tiles().iter() {
            let old = batch.before.get(key);
            for (index, pixel) in tile.pixels().chunks_exact(4).enumerate() {
                let old_alpha = old.map_or(0, |tile| tile.pixels()[index * 4 + 3]);
                if (eraser && pixel[3] > old_alpha) || (!eraser && pixel[3] < old_alpha) {
                    return Err("native stroke changed alpha in the wrong direction".into());
                }
                changed_alpha += usize::from(pixel[3] != old_alpha);
            }
        }
        if changed_alpha == 0 {
            return Err("native stroke did not change artwork alpha".into());
        }
        println!("desktop-selection-stroke eraser={eraser} changed_alpha={changed_alpha}");
    }
    let before = expected(false)?;
    let keys: std::collections::BTreeSet<_> = before
        .iter()
        .chain(reopened.current_tiles().iter())
        .map(|(key, _)| key)
        .collect();
    let mut changed_inside = 0;
    for key in keys {
        let old = before
            .get(key)
            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
        let current = reopened
            .current_tiles()
            .get(key)
            .map_or_else(|| vec![0; TILE_BYTE_LEN], |tile| tile.pixels().to_vec());
        let (origin_x, origin_y) = key.pixel_origin();
        for (index, (old, current)) in old.chunks_exact(4).zip(current.chunks_exact(4)).enumerate()
        {
            let x = origin_x + i64::try_from(index % 128)?;
            let y = origin_y + i64::try_from(index / 128)?;
            if (0..64).contains(&x) && (0..65).contains(&y) {
                changed_inside += usize::from(old != current);
            } else if old != current {
                return Err(format!("outside selection changed: {key:?} {x},{y}").into());
            }
        }
    }
    if (stage == 1) != (changed_inside == 0) {
        return Err("unexpected selected artwork change".into());
    }
    let flattened =
        nyatidraw_paint_cpu::flatten_layer_tree_rgba8(reopened.current_tiles(), &tree(), canvas)
            .map_err(|error| format!("flatten: {error:?}"))?;
    let golden = project.with_extension("expected.png");
    nyatidraw_png_io::encode_png(&golden, &flattened)?;
    let actual = nyatidraw_png_io::decode_png(&project.with_extension("png"), LayerId(1))?;
    let expected = nyatidraw_png_io::decode_png(&golden, LayerId(1))?;
    if actual.canvas != expected.canvas || actual.tiles != expected.tiles {
        return Err("selected stroke PNG differs".into());
    }
    println!(
        "desktop-selection-verify snapshot={stage} nodes={nodes} changed_inside={changed_inside} outside=exact replay=exact png=exact"
    );
    Ok(())
}

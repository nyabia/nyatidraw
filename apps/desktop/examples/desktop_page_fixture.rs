//! Independent dimensions/pixels for installed page resize/crop acceptance.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{collections::BTreeMap, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Pixel = (LayerId, [i32; 2], [u8; 4]);

fn canvas(stage: u8) -> Result<CanvasSpec> {
    let [width_px, height_px] = match stage {
        1 => [16, 16],
        2 => [2, 2],
        3 => [16, 12],
        4 => [1, 1],
        _ => return Err("unknown stage".into()),
    };
    Ok(CanvasSpec {
        width_px,
        height_px,
        pixels_per_inch: 96,
    })
}

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: [1, 2, 3]
            .map(|id| {
                LayerTreeNode::Raster(LayerNode {
                    id: LayerId(id),
                    name: match id {
                        1 => "Crop ink",
                        2 => "Locked ink",
                        _ => "Hidden ink",
                    }
                    .into(),
                    visible: id != 3,
                    locked: id != 1,
                    reference: false,
                    opacity_u16: u16::MAX,
                    content_root: ContentRootId(0),
                })
            })
            .into(),
    })
    .unwrap()
}

fn points(stage: u8) -> Vec<Pixel> {
    let mut result = vec![
        (LayerId(1), [-1, 4], [0, 0, 255, 255]),
        (LayerId(1), [10, 8], [255, 0, 0, 255]),
        (LayerId(2), [12, 10], [64, 0, 0, 64]),
        (LayerId(2), [-129, -1], [7, 7, 7, 255]),
        (LayerId(2), [16, 0], [9, 9, 9, 255]),
        (LayerId(3), [4, 4], [255, 0, 255, 255]),
        (LayerId(3), [129, 129], [0, 255, 255, 255]),
    ];
    for y in 4..6 {
        for x in 4..6 {
            result.push((LayerId(1), [x, y], [0, 255, 0, 255]));
        }
    }
    if stage != 1 {
        for (_, point, _) in &mut result {
            point[0] -= 4;
            point[1] -= 4;
        }
    }
    result
}

fn expected(stage: u8) -> Result<TileSnapshot> {
    let mut tiles: BTreeMap<TileKey, Vec<u8>> = BTreeMap::new();
    for (layer, [x, y], color) in points(stage) {
        let key = TileKey::from_pixel(layer, 128, x, y);
        let at = (y.rem_euclid(128) as usize * 128 + x.rem_euclid(128) as usize) * 4;
        tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN])[at..at + 4]
            .copy_from_slice(&color);
    }
    TileSnapshot::from_tiles(tiles).map_err(|e| format!("fixture: {e:?}").into())
}

fn verify(project: &Path, stage: u8, snapshot: u128, nodes: usize) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing artwork")?;
    let page = canvas(stage)?;
    if reopened.current_snapshot() != SnapshotId(snapshot)
        || reopened.history().node_count() != nodes
        || db.load_canvas_spec()? != page
        || db.load_layer_tree()? != Some(tree())
        || reopened.current_tiles() != &expected(stage)?
    {
        return Err("page/pixels/tree/history mismatch".into());
    }
    // Explicit point oracle: no production crop, page flattening or compositor.
    // Visible fixture points do not overlap; hidden layer 3 must never appear.
    let mut pixels = vec![0; (page.width_px * page.height_px * 4) as usize];
    for (layer, [x, y], color) in points(stage) {
        if layer != LayerId(3)
            && x >= 0
            && y >= 0
            && u32::try_from(x)? < page.width_px
            && u32::try_from(y)? < page.height_px
        {
            let at = (u32::try_from(y)? * page.width_px + u32::try_from(x)?) as usize * 4;
            pixels[at..at + 4].copy_from_slice(&color);
        }
    }
    let golden = project.with_extension("expected.png");
    nyatidraw_png_io::encode_png(
        &golden,
        &FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: page.width_px,
            height: page.height_px,
            pixels,
        },
    )?;
    if std::fs::read(golden)? != std::fs::read(project.with_extension("png"))? {
        return Err("PNG differs from independent page pixels/dimensions".into());
    }
    println!(
        "page-verify stage={stage} size={}x{} ppi=96 snapshot={snapshot} history={nodes} tiles=exact signed=exact locked_hidden=exact png=exact",
        page.width_px, page.height_px
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("create or verify")?;
    let project = Path::new(args.get(2).ok_or("scratch path")?);
    if project.file_name().and_then(|v| v.to_str()) != Some("page-scratch.ntdr") {
        return Err("requires exact scratch filename".into());
    }
    match mode.as_str() {
        "create" => {
            if project.exists() {
                return Err("scratch must not exist".into());
            }
            std::fs::create_dir_all(project.parent().ok_or("scratch parent")?)?;
            let db = ProjectDb::open(project)?;
            db.persist_canvas_spec(canvas(1)?)?;
            let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
            let batch = session
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(1)?)
                .map_err(|e| format!("fixture prepare: {e:?}"))?;
            db.commit_structural_with_layer_tree(&batch, &tree())?;
            println!("page-fixture created={}", project.display());
            Ok(())
        }
        "verify" => verify(
            project,
            args.get(3).ok_or("stage")?.parse()?,
            args.get(4).ok_or("snapshot")?.parse()?,
            args.get(5).ok_or("nodes")?.parse()?,
        ),
        "reexport" => {
            if !project.is_file() {
                return Err("requires an existing scratch project".into());
            }
            let db = ProjectDb::open(project)?;
            let reopened = db.load_reopened()?.ok_or("missing artwork")?;
            let layers = db.load_layer_tree()?.ok_or("missing layers")?;
            let surface = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(
                reopened.current_tiles(),
                &layers,
                db.load_canvas_spec()?,
            )
            .map_err(|error| format!("composite: {error:?}"))?;
            nyatidraw_png_io::encode_png(&project.with_extension("png"), &surface)?;
            println!("page-reexport source=existing-scratch cpu_only=true");
            Ok(())
        }
        _ => Err("invalid fixture mode".into()),
    }
}

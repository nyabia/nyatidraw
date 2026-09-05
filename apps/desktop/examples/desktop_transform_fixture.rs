//! Independent pixel coordinates for actual desktop transform acceptance.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{collections::BTreeMap, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 16,
    height_px: 16,
    pixels_per_inch: 96,
};
const COLORS: [[u8; 4]; 6] = [
    [255, 0, 0, 255],
    [0, 255, 0, 255],
    [0, 0, 255, 255],
    [255, 255, 0, 255],
    [255, 0, 255, 255],
    [0, 255, 255, 255],
];

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: [1, 2]
            .map(|id| {
                LayerTreeNode::Raster(LayerNode {
                    id: LayerId(id),
                    name: if id == 1 {
                        "Transform ink"
                    } else {
                        "Untouched"
                    }
                    .into(),
                    visible: true,
                    locked: false,
                    reference: false,
                    opacity_u16: u16::MAX,
                    content_root: ContentRootId(0),
                })
            })
            .into(),
    })
    .unwrap()
}

type Pixel = (LayerId, [i32; 2], [u8; 4]);

fn points(stage: u128) -> Result<Vec<Pixel>> {
    let coordinates = match stage {
        1 | 3 => [[-1, 4], [0, 4], [-1, 5], [0, 5], [-1, 6], [0, 6]],
        // Clockwise 90 degrees, then offset [4,2].
        2 => [[5, 6], [5, 7], [4, 6], [4, 7], [3, 6], [3, 7]],
        // Wand selects only green [0,4], then offset [2,0].
        4 | 5 => [[-1, 4], [2, 4], [-1, 5], [0, 5], [-1, 6], [0, 6]],
        _ => return Err("unknown expected stage".into()),
    };
    let mut result = Vec::new();
    for (point, color) in coordinates.into_iter().zip(COLORS) {
        if stage == 5 {
            // Independent 2x enlargement of the stage-4 4x3 source rectangle.
            for dy in 0..2 {
                for dx in 0..2 {
                    result.push((
                        LayerId(1),
                        [-1 + (point[0] + 1) * 2 + dx, 4 + (point[1] - 4) * 2 + dy],
                        color,
                    ));
                }
            }
        } else {
            result.push((LayerId(1), point, color));
        }
    }
    result.extend([
        (LayerId(2), [10, 10], [64, 0, 0, 64]),
        (LayerId(2), [-129, -1], [7, 7, 7, 255]),
        (LayerId(2), [16, 0], [9, 9, 9, 255]),
    ]);
    Ok(result)
}

fn expected(stage: u128) -> Result<TileSnapshot> {
    let mut tiles: BTreeMap<TileKey, Vec<u8>> = BTreeMap::new();
    for (layer, [x, y], color) in points(stage)? {
        let key = TileKey::from_pixel(layer, 128, x, y);
        let offset = (y.rem_euclid(128) as usize * 128 + x.rem_euclid(128) as usize) * 4;
        tiles.entry(key).or_insert_with(|| vec![0; TILE_BYTE_LEN])[offset..offset + 4]
            .copy_from_slice(&color);
    }
    TileSnapshot::from_tiles(tiles).map_err(|e| format!("fixture: {e:?}").into())
}

fn verify(project: &Path, stage: u128, snapshot: u128, nodes: usize) -> Result<()> {
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing artwork")?;
    if reopened.current_snapshot() != SnapshotId(snapshot)
        || reopened.history().node_count() != nodes
        || db.load_canvas_spec()? != CANVAS
        || db.load_layer_tree()? != Some(tree())
        || reopened.current_tiles() != &expected(stage)?
    {
        return Err("transform pixels/tree/history mismatch".into());
    }
    // No production transform, tile flattener or compositor constructs this oracle.
    let mut pixels = vec![0; 16 * 16 * 4];
    for (_, [x, y], color) in points(stage)? {
        if (0..16).contains(&x) && (0..16).contains(&y) {
            let index = usize::try_from(y * 16 + x)? * 4;
            pixels[index..index + 4].copy_from_slice(&color);
        }
    }
    let golden = project.with_extension("expected.png");
    nyatidraw_png_io::encode_png(
        &golden,
        &FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 16,
            height: 16,
            pixels,
        },
    )?;
    if std::fs::read(golden)? != std::fs::read(project.with_extension("png"))? {
        return Err("transform PNG differs from independent page pixels".into());
    }
    println!(
        "transform-verify stage={stage} snapshot={snapshot} history={nodes} tiles=exact signed=exact other_layers=exact png=exact"
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("create or verify")?;
    let project = Path::new(args.get(2).ok_or("scratch path")?);
    if project.file_name().and_then(|v| v.to_str()) != Some("transform-scratch.ntdr") {
        return Err("requires exact scratch filename".into());
    }
    match mode.as_str() {
        "create" => {
            if project.exists() {
                return Err("scratch must not exist".into());
            }
            std::fs::create_dir_all(project.parent().ok_or("scratch parent")?)?;
            let db = ProjectDb::open(project)?;
            db.persist_canvas_spec(CANVAS)?;
            let session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
            let batch = session
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(1)?)
                .map_err(|e| format!("fixture prepare: {e:?}"))?;
            db.commit_structural_with_layer_tree(&batch, &tree())?;
            println!("transform-fixture created={}", project.display());
            Ok(())
        }
        "verify" => verify(
            project,
            args.get(3).ok_or("stage")?.parse()?,
            args.get(4).ok_or("snapshot")?.parse()?,
            args.get(5).ok_or("nodes")?.parse()?,
        ),
        _ => Err("invalid fixture mode".into()),
    }
}

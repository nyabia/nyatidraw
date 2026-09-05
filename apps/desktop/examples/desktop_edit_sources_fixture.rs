//! Independent scratch-process oracle for native source/tolerance acceptance.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{FlattenedRgba8, TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const CANVAS: CanvasSpec = CanvasSpec {
    width_px: 129,
    height_px: 65,
    pixels_per_inch: 96,
};
const INK: [u8; 4] = [26, 199, 232, 255];

fn tree() -> LayerTree {
    LayerTree::new(GroupNode {
        id: GroupId(100),
        name: "Root".into(),
        visible: true,
        opacity_u16: u16::MAX,
        children: [
            (1, "Ink", false),
            (2, "Reference barrier", true),
            (3, "Visible barrier", false),
        ]
        .map(|(id, name, reference)| {
            LayerTreeNode::Raster(LayerNode {
                id: LayerId(id),
                name: name.into(),
                visible: true,
                locked: false,
                reference,
                opacity_u16: u16::MAX,
                content_root: ContentRootId(0),
            })
        })
        .into(),
    })
    .unwrap()
}

fn expected(limit: usize) -> Result<TileSnapshot> {
    let mut tiles = Vec::new();
    for layer in 1..=3 {
        for tile_x in 0..2 {
            let mut pixels = vec![0; TILE_BYTE_LEN];
            for y in 0..65 {
                for local_x in 0..128 {
                    let x = tile_x * 128 + local_x;
                    let color = match (layer, x) {
                        (1, x) if x < limit => INK,
                        (2, 64) => [32, 0, 0, 64],
                        (3, 32) => [0, 0, 128, 255],
                        _ => [0; 4],
                    };
                    let offset = (y * 128 + local_x) * 4;
                    pixels[offset..offset + 4].copy_from_slice(&color);
                }
            }
            if layer == 1 && tile_x == 1 {
                pixels[4..8].copy_from_slice(&[9, 9, 9, 255]);
            }
            tiles.push((
                TileKey {
                    layer: LayerId(layer),
                    mip: 0,
                    x: i32::try_from(tile_x)?,
                    y: 0,
                },
                pixels,
            ));
        }
    }
    tiles.push((
        TileKey {
            layer: LayerId(1),
            mip: 0,
            x: -1,
            y: 0,
        },
        [7, 7, 7, 255].repeat(TILE_BYTE_LEN / 4),
    ));
    TileSnapshot::from_tiles(tiles).map_err(|error| format!("fixture tiles: {error:?}").into())
}

fn verify(project: &Path, limit: usize, stage: u128, nodes: usize, visible: bool) -> Result<()> {
    if ![32, 64, 129].contains(&limit) {
        return Err("expected filled width must be 32, 64, or 129".into());
    }
    let db = ProjectDb::open(project)?;
    let reopened = db.load_reopened()?.ok_or("missing head")?;
    let mut expected_tree = tree();
    expected_tree
        .set_visibility(nyatidraw_api::LayerTreeNodeId::Raster(LayerId(3)), visible)
        .map_err(|error| format!("fixture tree: {error:?}"))?;
    if reopened.current_snapshot() != SnapshotId(stage)
        || reopened.history().node_count() != nodes
        || db.load_layer_tree()? != Some(expected_tree)
        || db.load_canvas_spec()? != CANVAS
        || reopened.current_tiles() != &expected(limit)?
    {
        return Err("source/tolerance durable artwork, metadata, or history mismatch".into());
    }
    // Explicit piecewise oracle, independent of the production compositor.
    let mut pixels = Vec::new();
    for _ in 0..65 {
        for x in 0..129 {
            pixels.extend_from_slice(&match x {
                32 if visible => [0, 0, 128, 255],
                64 if limit == 129 => [51, 149, 174, 255],
                64 => [32, 0, 0, 64],
                x if x < limit => INK,
                _ => [0; 4],
            });
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
        return Err("source/tolerance exported composite differs".into());
    }
    println!(
        "source-tolerance-verify width={limit} pixels={} tiles=exact other_layers=exact off_page=exact png=exact snapshot={stage} history={nodes} visible={visible}",
        limit * 65
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).ok_or("create or verify")?;
    let project = Path::new(args.get(2).ok_or("scratch path")?);
    if project.file_name().and_then(|name| name.to_str()) != Some("edit-source-scratch.ntdr") {
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
                .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(0)?)
                .map_err(|error| format!("prepare fixture: {error:?}"))?;
            db.commit_structural_with_layer_tree(&batch, &tree())?;
            println!("source-tolerance-fixture created={}", project.display());
            Ok(())
        }
        "verify" | "verify-hidden" | "verify-undo" => {
            let (stage, nodes, visible) = match mode.as_str() {
                "verify-hidden" => (3, 3, false),
                "verify-undo" => (2, 3, true),
                _ => (2, 2, true),
            };
            verify(
                project,
                args.get(3).ok_or("filled width")?.parse()?,
                stage,
                nodes,
                visible,
            )
        }
        _ => Err("invalid mode".into()),
    }
}

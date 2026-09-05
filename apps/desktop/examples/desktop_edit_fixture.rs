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
    if mode == "create" {
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
        db.persist_canvas_spec(CANVAS)?;
        let mut session = HeadlessStrokeSession::new(SnapshotId(0), TileSnapshot::empty());
        let batch = session
            .prepare_structural_change(SnapshotId(1), HistoryNodeId(1), 1, expected(false)?)
            .map_err(|error| format!("prepare: {error:?}"))?;
        db.commit_structural_with_layer_tree(&batch, &tree())?;
        session
            .accept_structural_change(&batch)
            .map_err(|error| format!("accept: {error:?}"))?;
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

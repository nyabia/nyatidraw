//! CPU-only 4K composite benchmark; first warmup excluded, all 20 samples kept.
use nyatidraw_api::{CanvasSpec, ContentRootId, GroupId, LayerId};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_paint_cpu::flatten_layer_tree_rgba8;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use std::{hint::black_box, time::Instant};

fn main() {
    let canvas = CanvasSpec {
        width_px: 3840,
        height_px: 2160,
        pixels_per_inch: 96,
    };
    for (name, layers, dense) in [("sparse8", 8_u8, false), ("dense2", 2, true)] {
        let mut tiles = Vec::new();
        let mut children = Vec::new();
        for layer in 1..=layers {
            children.push(LayerTreeNode::Raster(LayerNode {
                id: LayerId(u128::from(layer)),
                name: format!("Layer{layer}"),
                visible: true,
                locked: false,
                reference: layer == 2,
                opacity_u16: 40_000,
                content_root: ContentRootId(0),
            }));
            for y in 0..17 {
                for x in 0..30 {
                    if dense || (x + y + i32::from(layer)) % 31 == 0 {
                        tiles.push((
                            TileKey {
                                layer: LayerId(u128::from(layer)),
                                mip: 0,
                                x,
                                y,
                            },
                            [layer * 12, 50, 90, 160].repeat(TILE_BYTE_LEN / 4),
                        ));
                    }
                }
            }
        }
        let tree = LayerTree::new(GroupNode {
            id: GroupId(100),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            children: vec![LayerTreeNode::Group(GroupNode {
                id: GroupId(101),
                name: "Isolated".into(),
                visible: true,
                opacity_u16: 50_000,
                children,
            })],
        })
        .unwrap();
        let snapshot = TileSnapshot::from_tiles(tiles).unwrap();
        let expected = flatten_layer_tree_rgba8(&snapshot, &tree, canvas).unwrap();
        let checksum = expected
            .pixels
            .iter()
            .fold(0_u64, |sum, byte| sum.wrapping_add(u64::from(*byte)));
        let mut samples = Vec::with_capacity(20);
        for index in 0..20 {
            let started = Instant::now();
            let actual =
                flatten_layer_tree_rgba8(black_box(&snapshot), black_box(&tree), canvas).unwrap();
            let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(
                actual.pixels, expected.pixels,
                "non-deterministic composite"
            );
            black_box(actual);
            samples.push(milliseconds);
            println!("case={name} sample={} ms={milliseconds:.6}", index + 1);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "case={name} profile={} backend=cpu canvas=3840x2160 samples=20 warmup=1 tiles={} checksum={checksum} p50_ms={:.6} p95_ms={:.6} p99_ms={:.6}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "cargo-release"
            },
            snapshot.len(),
            samples[9],
            samples[18],
            samples[19]
        );
    }
}

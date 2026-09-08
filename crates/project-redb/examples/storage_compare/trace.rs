use std::path::Path;

use nyatidraw_api::{HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_history::{HistoryNode, OperationRecord};
use nyatidraw_project::ProjectStructuralBatch;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use redb::{ReadableDatabase, ReadableTable, TableDefinition, TableHandle};

use super::backend::{Batch, Records, Result, delta};

pub struct Trace {
    pub initial: Records,
    pub batches: Vec<Batch>,
    pub final_records: Records,
}

// Capture actual application wire records and retained-history deletions outside
// timed sections. ProjectDb is the record oracle, not the timed container adapter.
pub fn capture(path: &Path, noisy: bool) -> Result<Trace> {
    drop(ProjectDb::open(path)?);
    let initial = read_project_records(path)?;
    let mut previous = initial.clone();
    let mut batches = Vec::new();
    let mut before = TileSnapshot::empty();
    let mut tiles = vec![vec![0; TILE_BYTE_LEN]; if noisy { 16 } else { 1 }];
    for index in 1_u16..=160 {
        let slot = usize::from(index - 1) % tiles.len();
        let mut random = u32::from(index);
        for (offset, pixel) in tiles[slot].chunks_exact_mut(4).enumerate() {
            if noisy {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                pixel.copy_from_slice(&random.to_le_bytes());
                pixel[3] = 255;
            } else if offset % 128 < usize::from(index % 100 + 1) {
                pixel.copy_from_slice(&[32, 96, 160, 255]);
            } else {
                pixel.fill(0);
            }
        }
        let after = TileSnapshot::from_tiles(tiles.iter().enumerate().map(|(slot, pixels)| {
            let x = i32::try_from(slot).expect("16 tiles") * 128 - 256;
            (
                TileKey::from_pixel(LayerId(1), 128, x, -128),
                pixels.clone(),
            )
        }))
        .expect("valid scratch tiles");
        let node = HistoryNode {
            id: HistoryNodeId(u128::from(index)),
            parent: (index > 1).then(|| HistoryNodeId(u128::from(index - 1))),
            before_root: before.root().id,
            after_root: after.root().id,
            operation: OperationRecord::StructuralChange,
            timestamp_ns: 0,
        };
        let batch = ProjectStructuralBatch::new(
            SnapshotId(u128::from(index + 1)),
            before,
            after.clone(),
            node,
        )
        .expect("valid scratch transition");
        let db = ProjectDb::open(path)?;
        db.commit_structural(&batch)?;
        drop(db);
        let next = read_project_records(path)?;
        batches.push(delta(&previous, &next));
        previous = next;
        before = after;
    }
    let db = ProjectDb::open(path)?;
    let reopened = db.load_reopened()?.expect("saved project");
    assert_eq!(reopened.current_tiles(), &before);
    assert_eq!(reopened.history().node_count(), 128);
    Ok(Trace {
        initial,
        batches,
        final_records: previous,
    })
}

fn read_project_records(path: &Path) -> Result<Records> {
    let db = redb::Database::open(path)?;
    let tx = db.begin_read()?;
    let mut records = Records::new();
    // Enumerate every existing table: unknown tables fail instead of silently
    // dropping some application data from the benchmark.
    for handle in tx.list_tables()? {
        let name = handle.name().to_owned();
        let mut add = |key: &[u8], value: &[u8]| {
            let mut joined = name.as_bytes().to_vec();
            joined.push(0);
            joined.extend_from_slice(key);
            assert!(records.insert(joined, value.to_vec()).is_none());
        };
        match name.as_str() {
            "meta" => {
                for row in tx
                    .open_table(TableDefinition::<&str, u64>::new(&name))?
                    .iter()?
                {
                    let (key, value) = row?;
                    add(key.value().as_bytes(), &value.value().to_le_bytes());
                }
            }
            "state" => {
                for row in tx
                    .open_table(TableDefinition::<&str, &[u8]>::new(&name))?
                    .iter()?
                {
                    let (key, value) = row?;
                    add(key.value().as_bytes(), value.value());
                }
            }
            "tile_root_refs" => {
                for row in tx
                    .open_table(TableDefinition::<&[u8], u64>::new(&name))?
                    .iter()?
                {
                    let (key, value) = row?;
                    add(key.value(), &value.value().to_le_bytes());
                }
            }
            "objects" | "roots" | "history" | "snapshots" | "strokes" | "snapshot_layers"
            | "snapshot_canvas" => {
                for row in tx
                    .open_table(TableDefinition::<&[u8], &[u8]>::new(&name))?
                    .iter()?
                {
                    let (key, value) = row?;
                    add(key.value(), value.value());
                }
            }
            _ => return Err(format!("uncaptured table {name}").into()),
        }
    }
    Ok(records)
}

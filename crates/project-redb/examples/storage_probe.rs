//! Scratch-only storage accounting; never accepts a user project path.
use std::{fs, time::Instant};

use nyatidraw_api::{HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_history::{HistoryNode, OperationRecord};
use nyatidraw_project::ProjectStructuralBatch;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileSnapshot};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch =
        std::env::temp_dir().join(format!("nyatidraw-storage-probe-{}", std::process::id()));
    fs::create_dir(&scratch)?;
    measure_initialization(&scratch)?;
    let path = scratch.join("scratch.ntdr");
    let db = ProjectDb::open(&path)?;
    drop(db);
    report(&path, "empty")?;
    compare_sqlite(&path, &scratch.join("empty-sqlite.ntdr"))?;
    let mut before = TileSnapshot::empty();
    let mut latencies = Vec::new();
    let db = ProjectDb::open(&path)?;
    for index in 1_u16..=160 {
        let mut pixels = vec![0; TILE_BYTE_LEN];
        for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            if i % 128 < usize::from(index % 100 + 1) {
                pixel.copy_from_slice(&[32, 96, 160, 255]);
            }
        }
        let after =
            TileSnapshot::from_tiles([(TileKey::from_pixel(LayerId(1), 128, -128, 0), pixels)])
                .expect("valid scratch pixels");
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
            before.clone(),
            after.clone(),
            node,
        )
        .expect("valid scratch transition");
        let started = Instant::now();
        db.commit_structural(&batch)?;
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        before = after;
    }
    let started = Instant::now();
    drop(db);
    println!("close_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    latencies.sort_by(f64::total_cmp);
    println!(
        "commit_ms p50={:.3} p95={:.3} p99={:.3}",
        latencies[79], latencies[151], latencies[158]
    );
    let started = Instant::now();
    let db = ProjectDb::open(&path)?;
    let reopened = db.load_reopened()?.expect("saved head");
    assert_eq!(reopened.current_tiles(), &before);
    assert_eq!(reopened.history().node_count(), 128);
    println!(
        "reopen_and_load_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    drop(db);
    report(&path, "160_edits_128_retained")?;
    let mut open_times = Vec::new();
    let mut close_times = Vec::new();
    for _ in 0..32 {
        let start = Instant::now();
        let db = ProjectDb::open(&path)?;
        open_times.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        drop(db);
        close_times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    for (name, times) in [
        ("warm_open", &mut open_times),
        ("clean_close", &mut close_times),
    ] {
        times.sort_by(f64::total_cmp);
        println!(
            "{name}_ms n=32 p50={:.3} p95={:.3} p99={:.3}",
            times[15], times[30], times[31]
        );
    }
    compare_sqlite(&path, &scratch.join("populated-sqlite.ntdr"))?;
    let mut raw = redb::Database::open(&path)?;
    raw.compact()?;
    drop(raw);
    report(&path, "compacted")?;
    // Only the exact scratch file created by this process is removed.
    fs::remove_file(&path)?;
    fs::remove_dir(&scratch)?;
    Ok(())
}

fn measure_initialization(scratch: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut initialization_times = Vec::new();
    for index in 0..32 {
        let empty = scratch.join(format!("initialization-{index}.ntdr"));
        let start = Instant::now();
        drop(ProjectDb::open(&empty)?);
        initialization_times.push(start.elapsed().as_secs_f64() * 1000.0);
        fs::remove_file(empty)?;
    }
    initialization_times.sort_by(f64::total_cmp);
    println!(
        "initialize_and_close_ms n=32 p50={:.3} p95={:.3} p99={:.3}",
        initialization_times[15], initialization_times[30], initialization_times[31]
    );
    Ok(())
}

// Container-size/record round-trip probe only: not a production SQLite reader,
// crash-recovery gate, or an end-to-end SQLite latency comparison.
fn compare_sqlite(
    source: &std::path::Path,
    target: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let db = redb::Database::open(source)?;
    let read = db.begin_read()?;
    let mut records = Vec::<(String, Vec<u8>, Vec<u8>)>::new();
    let meta = read.open_table(TableDefinition::<&str, u64>::new("meta"))?;
    for entry in meta.iter()? {
        let (key, value) = entry?;
        records.push((
            "meta".into(),
            key.value().as_bytes().to_vec(),
            value.value().to_le_bytes().to_vec(),
        ));
    }
    if let Ok(state) = read.open_table(TableDefinition::<&str, &[u8]>::new("state")) {
        for entry in state.iter()? {
            let (key, value) = entry?;
            records.push((
                "state".into(),
                key.value().as_bytes().to_vec(),
                value.value().to_vec(),
            ));
        }
    }
    for name in [
        "objects",
        "roots",
        "history",
        "snapshots",
        "strokes",
        "snapshot_layers",
        "snapshot_canvas",
    ] {
        if let Ok(table) = read.open_table(TableDefinition::<&[u8], &[u8]>::new(name)) {
            for entry in table.iter()? {
                let (key, value) = entry?;
                records.push((name.into(), key.value().to_vec(), value.value().to_vec()));
            }
        }
    }
    if let Ok(table) = read.open_table(TableDefinition::<&[u8], u64>::new("tile_root_refs")) {
        for entry in table.iter()? {
            let (key, value) = entry?;
            records.push((
                "tile_root_refs".into(),
                key.value().to_vec(),
                value.value().to_le_bytes().to_vec(),
            ));
        }
    }
    // Fresh, owned scratch path. No arbitrary database is replaced.
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let mut sql = rusqlite::Connection::open(target)?;
    sql.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;
        PRAGMA application_id=1314145362;
        CREATE TABLE records (namespace TEXT NOT NULL, key BLOB NOT NULL, value BLOB NOT NULL,
            PRIMARY KEY(namespace,key)) WITHOUT ROWID;",
    )?;
    let tx = sql.transaction()?;
    {
        let mut insert = tx.prepare("INSERT INTO records VALUES (?1,?2,?3)")?;
        for (name, key, value) in &records {
            insert.execute(rusqlite::params![name, key, value])?;
        }
    }
    tx.commit()?;
    drop(sql);
    let sql = rusqlite::Connection::open(target)?;
    let mut select = sql.prepare("SELECT value FROM records WHERE namespace=?1 AND key=?2")?;
    for (name, key, expected) in &records {
        let actual: Vec<u8> = select.query_row(rusqlite::params![name, key], |row| row.get(0))?;
        assert_eq!(&actual, expected);
    }
    println!(
        "sqlite_equal_records={} file_bytes={}",
        records.len(),
        fs::metadata(target)?.len()
    );
    drop(select);
    drop(sql);
    fs::remove_file(target)?;
    Ok(())
}

fn report(path: &std::path::Path, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("case={name} file_bytes={}", fs::metadata(path)?.len());
    let db = redb::Database::open(path)?;
    let tx = db.begin_read()?;
    for name in ["objects", "roots", "history", "snapshots", "strokes"] {
        let definition: TableDefinition<&[u8], &[u8]> = TableDefinition::new(name);
        if let Ok(table) = tx.open_table(definition) {
            let stats = table.stats()?;
            println!(
                "table={name} entries={} stored={} metadata={} fragmented={}",
                table.len()?,
                stats.stored_bytes(),
                stats.metadata_bytes(),
                stats.fragmented_bytes()
            );
        }
    }
    Ok(())
}

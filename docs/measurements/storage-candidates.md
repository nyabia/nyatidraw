# Current storage candidates: redb and SQLite

> Comparison checkpoint, before the production-adapter upgrade. The follow-up
> [redb 4 native-adapter results](redb4-native-adapter.md) now supersede the
> statements below about the application still using 2.6.3. Re-running the probe
> now captures its wire trace through the current 4.2.0 `ProjectDb`, not 2.6.3.

## Decision and scope

Keep current redb and SQLite as implementation candidates. Persy is research-only;
the user explicitly asked not to spend effort forcing a third implementation.
There is no Persy dependency, benchmark or performance claim.

**Recommend a bounded current-redb production-adapter upgrade spike first.**
Its former multi-megabyte small-file overhead is not intrinsic to current redb.
SQLite is smaller, but replacing the storage engine is no longer justified solely
by the old minimum-size result. This does not establish either backend as optimal.
The installed app and production dependency remain unchanged (redb 2.6.3).

## Reproduce

```powershell
cargo run --release -p nyatidraw-project-redb --example storage_compare --locked
```

Source: `crates/project-redb/examples/storage_compare.rs` and its `backend.rs` /
`trace.rs` support modules. Detailed output: [raw results](storage-candidates-output.txt).

Environment: Windows 11 Pro 10.0.26200; AMD Ryzen 7 5800X3D; local NVMe SSD
SAMSUNG MZVL21T0HCLR-00B00; rustc 1.96.0; x86_64-pc-windows-msvc; Cargo release.
No GPU/backend rendering participates. OS cache and background system load were
not controlled. Measurements are warm and do not establish cold startup latency.

Versions/configuration:

- redb 4.2.0, `Durability::Immediate` per transaction.
- rusqlite 0.40.2 / bundled SQLite 3.53.2, DELETE + synchronous FULL.
- Same SQLite version, WAL + synchronous FULL, auto-checkpoint 1,000 pages.
- Existing redb 2.6.3 produces the application record trace outside all timings.
  No durability option is disabled to improve the results.

## Work and correctness compared

Two deterministic synthetic fixtures, each with 160 actual structural commits
through `ProjectDb` and the existing compression/reference-index/history code:

- `sprite`: one off-page 128x128 tile cycling 100 simple strip patterns. Final
  565 records, 86,027 key/value bytes.
- `noise16`: 16 signed-position 128x128 tiles, modifying one per revision with
  deterministic opaque RGB noise. Final 682 records, 8,270,218 key/value bytes.
  This is a compression stress fixture, not a photograph or a 4K multi-layer app.

Every table is captured; unknown tables fail rather than silently being omitted.
Each candidate replays identical record insert/update/delete batches atomically,
including reference counts and eviction after the 128-operation retention limit.
All original wire values are preserved; keys are `table-name + NUL + original-key`
in one ordered KV table. SQLite uses `WITHOUT ROWID`. Thus this measures a
**normalized candidate record layout**, not a drop-in version upgrade of the
production redb typed-table layout.

Three repetitions per fixture/configuration with rotated execution order.
Each has 160 commit samples and 32 warm open/full-record-read/close samples.
Canonical record comparisons run outside commit timings at retention boundaries.
An independent child process reopens and compares every key/value before and
after explicit compaction. The source `ProjectDb` verifies final pixels and 128
retained history nodes. Candidate backends do not yet decode every historical
state through a production application adapter.

## File sizes

All three repetitions produced the same closed sizes. KiB = 1,024 bytes.
Normal sizes are after clean close, without explicit compaction/VACUUM.

| Fixture | redb normal | redb compacted | SQLite normal, either mode | SQLite VACUUM |
|---|---:|---:|---:|---:|
| Initialized metadata | 40 KiB | not sampled | 8 KiB | not sampled |
| Sprite + 128 history | 272 KiB | 196 KiB | 120 KiB | 96 KiB |
| Noise16 + 128 history | 13,111,296 B | 9,900,032 B | 9,097,216 B | 8,994,816 B |

Active WAL is not free space: immediately before final close, auxiliary WAL/SHM
files totalled 4,020,960 B for sprite and 4,272,280 B for noise16. The sprite main
file was then only 8,192 B: most data was still in WAL. All normal closed cases
had zero auxiliary bytes. Do not copy only the main `.ntdr` of a live WAL database.
These are sampled live sizes, not peaks or proof of a hard WAL-size bound.

Current-redb compaction took about 24-31 ms; SQLite VACUUM about 7-9 ms (sprite)
and 126-153 ms (noise). Neither is included in the normal close values and neither
is proposed as mandatory work on every app close.

## Commit latency

Milliseconds. Each cell is the range of the corresponding per-run percentile
across three runs, **not** a pooled percentile or confidence interval.

| Fixture / configuration | p50 | p95 | p99 |
|---|---:|---:|---:|
| Sprite / redb | 2.337-2.446 | 2.721-3.413 | 4.238-4.383 |
| Sprite / SQLite DELETE | 7.702-7.860 | 8.487-8.851 | 9.501-11.969 |
| Sprite / SQLite WAL | 2.080-2.106 | 2.350-2.638 | 3.464-6.146 |
| Noise16 / redb | 2.653-2.867 | 3.917-4.467 | 5.066-5.426 |
| Noise16 / SQLite DELETE | 8.599-8.778 | 10.020-10.362 | 11.858-34.924 |
| Noise16 / SQLite WAL | 2.614-2.647 | 4.524-4.796 | 15.172-44.231 |

Timings include transaction setup, changed-record writes/deletes and durable
commit; exclude brush evaluation, compression, application history calculation,
export, tracing/model verification and UI. WAL automatic checkpoints remain
enabled and their work is inside commit when triggered. Outliers cannot be
attributed solely to checkpoints because OS/background I/O was not isolated.

## Opening, reading and closing

For the small fixture, per-run percentile ranges in milliseconds (n=32 each run):

| Operation / configuration | p50 | p95 | p99 |
|---|---:|---:|---:|
| Warm open / redb | 2.310-2.371 | 2.493-3.707 | 2.543-6.493 |
| Warm open / SQLite DELETE | 0.326-0.448 | 0.485-0.519 | 0.505-0.677 |
| Warm open / SQLite WAL | 1.672-1.796 | 2.429-2.693 | 2.490-6.197 |
| Read-only close / redb | 6.857-7.143 | 8.158-9.515 | 8.326-10.902 |
| Read-only close / SQLite DELETE | 0.034-0.048 | 0.053-0.056 | 0.053-0.063 |
| Read-only close / SQLite WAL | 0.733-0.848 | 0.868-1.370 | 0.904-4.335 |

Final write-close is sampled once per run, not 32 times: sprite redb 8.479-8.581 ms,
SQLite DELETE 0.106-0.175 ms, SQLite WAL 4.657-7.427 ms. WAL includes final close's
checkpoint work. A read-only close number must not be substituted for this.

Noise16 full-record-read p50 was 3.723-4.375 ms for redb, 9.579-11.338 ms for SQLite
DELETE and 9.696-12.281 ms for SQLite WAL. Full-record scan is not lazy visible-tile
loading. Full percentiles are retained in the raw output. One redb noise run had
warm-open p99 45.059 ms and read-only-close p99 141.766 ms; this unexplained outlier
is retained and prevents claiming a proven tight tail-latency bound.

## Process termination and recovery

Each of three configurations passed two independent subprocess kills:

1. A transaction updates a head record, deletes an old 64KiB blob and inserts a
   new 64KiB blob; kill immediately before commit. Reopen must equal all old records.
2. Kill after durable commit returns, before normal database close. Reopen must
   equal all new records, with no partial old/new combination.

The parent waits for the child's explicit seam signal with a timeout, kills and
reaps that exact child. Another independent process performs canonical equality
checks. All six cases passed. This does not simulate power loss, interrupted
sector writes, disk-full, random in-commit termination or complete editor recovery.

SQLite DELETE left a 12,824-byte non-hot journal after its pre-commit kill even
though read/reopen verified the complete old state. The probe removes only its
owned scratch database/auxiliary files after verification and all handles close;
this is **not** permission for production code to delete live/recovery journals.

## Interpretation and remaining gate

- SQLite retains a size advantage and WAL makes median commit latency comparable.
  WAL adds checkpoint and sidecar lifecycle requirements for a portable project.
- Current redb is a viable single-file candidate; reject the earlier inference
  that redb necessarily leaves every small project at several megabytes.
- Prioritize testing current redb with the actual typed tables/adapter, including
  safe legacy-copy migration. Do not infer that this normalized 40KiB baseline
  is the exact initial size the upgraded application will produce.
- Only then choose production: validate import/save/export, all retained undo/redo
  states, layer/page metadata, concurrent lazy reads, bounded writer backlog,
  invalid-file preservation, migration crash safety and real representative files.
- No user artwork, installed executable, file associations or release were changed.

## Validation performed

- Release `storage_compare`: completed all 18 replay runs, 36 independent
  pre/post-compaction reopen checks and all six commit-boundary kill cases.
- `cargo test -p nyatidraw-project -p nyatidraw-project-redb --locked`: 4 wire
  and 17 production-redb tests passed. These production tests still target 2.6.3.
- `cargo clippy -p nyatidraw-project -p nyatidraw-project-redb --all-targets
  --all-features --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Only generated scratch databases and journals were removed after verification.
  No new UI/mock tests or Persy implementation were added.

## References

- [redb changelog](https://github.com/cberner/redb/blob/master/CHANGELOG.md): 3.0
  file-format/storage improvements and legacy v2-to-v3 upgrade path.
- [SQLite WAL](https://sqlite.org/wal.html): checkpoints, sidecars and copying caveats.
- [SQLite synchronous](https://sqlite.org/pragma.html#pragma_synchronous): durability settings.
- [Persy](https://persy.rs/): research-only single-file alternative, not measured here.

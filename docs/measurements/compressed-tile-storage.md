# Compressed tile storage: bounded evidence

> Historical redb 2.6.3 baseline. The SQLite-only recommendation below is
> superseded by [the current redb/SQLite comparison](storage-candidates.md).
> These old redb sizes are not a minimum for redb 4.2.0.

## Environment and scope

Windows 11, AMD Ryzen 7 5800X3D, x86_64-pc-windows-msvc, Cargo `release` profile,
CPU / local filesystem / redb 2.6.3. No GPU backend participates in this probe.
Background system load and storage device/cache state were not controlled.
No real pen, first-visible-pixel, installed-GUI or cross-platform claim is made.

`cargo run --release -p nyatidraw-project-redb --example storage_probe`

The probe creates its own uniquely named scratch directory, then removes only
its exact generated files. It does not accept an artwork path. It writes 160
structural revisions of one signed off-page tile, cycling through 100 simple
solid-strip patterns, retaining 128 history operations. This is highly compressible
synthetic content, not the user's 17KB PNG or representative painting corpus.

## Observed bytes

| Item | Raw baseline | Compressed / indexed |
|---|---:|---:|
| Empty redb project | 3,686,400 | 3,686,400 |
| File after 160 edits | 18,395,136 | 4,771,840 |
| File after explicit scratch compaction | 18,395,136 | 3,002,368 |
| Tile-table stored key/value bytes | 6,562,000 | 12,815 |
| Tile-table fragmentation | 6,550,252 | 10,669 |
| Root entries | 101 | 100 |
| Retained history nodes | 128 | 128 |

The repeated pattern workload retains all 100 distinct tile objects even after
collection, because retained roots still reference them. The separate unique-tile
retention regression verifies actual object eviction and shared-object survival.
File size includes container allocation; it must not be confused with logical
payload bytes or proof of a fixed minimum on every redb version/workload.

## Container-only SQLite comparison

The probe copies **identical compressed record values** from all relevant tables
into a scratch SQLite `WITHOUT ROWID` key/value table, using DELETE journaling and
FULL synchronous mode. After closing and reopening SQLite, every value is compared
byte-for-byte. No production app/backend path is switched by this probe.

| Same logical records | SQLite bytes |
|---|---:|
| Empty project metadata (6 records) | 8,192 |
| Populated project, roots/objects/history/state/reference index (565 records) | 118,784 |

This comparison does **not** implement the application schema validation,
incremental transactional edit protocol, lazy loading, legacy migration or crash
recovery for SQLite. No SQLite save/startup latency advantage has been established.

## Timing, initial bounded runs

Milliseconds, 160 commit samples per run, nearest-rank p50/p95/p99:

| Variant | Commit p50 | Commit p95 | Commit p99 |
|---|---:|---:|---:|
| Raw baseline | 2.647 | 4.011 | 5.081 |
| Compression + indexed retention | 2.511 | 3.688 | 4.253 |

These are single-run fixture observations, not a statistically established
speedup. Initial close and reopen measurements were single observations, not
percentile evidence. The current probe also emits 32 warm-open and clean-close
samples; it does not evict OS caches or include window/GPU startup.

After adding canonical manifest verification, a subsequent release run reported:

| Operation | Samples | p50 ms | p95 ms | p99 ms |
|---|---:|---:|---:|---:|
| Commit | 160 | 2.550 | 3.971 | 4.381 |
| Warm `ProjectDb::open` | 32 | 3.818 | 4.536 | 4.557 |
| Clean database drop | 32 | 5.294 | 6.009 | 6.320 |

These clean-close values exclude a pending writer backlog, PNG export and any
database compaction. They are not complete application shutdown measurements.

## Correctness gates

- Raw and compressed tile byte equality, checksum identity, incompressible raw
  fallback, bounded decoded size, malformed/trailing frames and bad checksums.
- Legacy raw project open leaves its file bytes unchanged; subsequent edits set
  a capability marker rejected by old readers and still reopen exactly.
- 132 unique artwork revisions, shared tile objects at multiple coordinates,
  negative/off-page tile positions, varying layer names/page dimensions: child
  process reopens, restores all 128 undo states and redo states, plus branching.
  Retained roots/objects are counted; discarded tile versions do not accumulate.
- The existing subprocess kill probe passes before commit and after durable
  commit for both stroke artwork and layer metadata. This is process termination,
  not a power-loss/storage-controller guarantee.

Small-file bloat is **not fixed in the production redb container**. The current
recommendation and outstanding adapter/migration gates are in ADR-0048 and the
linked current comparison, rather than an automatic SQLite migration.

## Validation run

- `cargo fmt --all -- --check`: passed.
- `cargo test --workspace --all-features --locked`: passed; the existing explicit
  scratch-registry acceptance remains ignored. Project wire: 4 tests; redb: 17.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`:
  passed. The existing vendored Wry lifetime warning remains upstream of this change.
- Release `crash_recovery_probe`: passed at before-commit and after-durable-commit
  boundaries for stroke/layer cases.
- No user artwork was converted, no installed app was replaced, and no release,
  tag or push was performed for this storage change.

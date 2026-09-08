# redb 4: actual typed-table adapter

## Scope and reproduction

Follow-up to [candidate microbenchmarks](storage-candidates.md). This run uses the
actual `ProjectDb`, its typed tables, compression/history calculations and new
read-only preflight. It is not a normalized-container replay.

```powershell
cargo run --release -p nyatidraw-project-redb --example storage_probe --locked
cargo test --workspace --all-features --locked
cargo run --release -p nyatidraw-project-redb --features diagnostic --example crash_recovery_probe --locked
```

Windows 11 Pro 10.0.26200, Ryzen 7 5800X3D, local Samsung NVMe SSD
MZVL21T0HCLR-00B00, x86_64-pc-windows-msvc, rustc 1.96.0, release, redb 4.2.0.
No GPU backend. Background load and OS cache were not isolated. No installed-app,
physical pen, first-visible-pixel or cross-platform performance claims.

Fixture: 160 structural changes to one signed off-page 128x128 tile, cycling 100
compressible strip patterns and retaining 128 history operations. This is not the
user's 17KB PNG. Scratch files only; no user file was converted during these runs.

## Size

| Item | Prior redb 2.6.3 adapter | Current redb 4.2.0 adapter |
|---|---:|---:|
| Empty metadata project | 3,686,400 B | 36,864 B (36 KiB) |
| 160 edits / 128 history, normal close | 4,771,840 B | 249,856 B (244 KiB) |
| Explicit scratch compaction | 3,002,368 B | 172,032 B (168 KiB) |

Values include container allocation; the old numbers are a historical baseline,
not a simultaneously controlled speed comparison. Initialization now compacts the
fresh metadata-only database once. Without it, a preliminary run left 1,056,768 B
after the first marker transaction. This one-time cost is measured below; full
compaction is never added to every existing-file open/save/close.

The original container-only conversion preserved old record bytes. Its raw tiles could still
occupy substantial space, and the retained `.bak` deliberately adds a copy of the
old container to total disk use. Do not promise every existing project will shrink
to the synthetic fixture's numbers.

This limitation is now addressed by the one-time lossless tile rebuild described
in [alpha repack acceptance](alpha-storage-repack.md). The timing table below
predates that follow-up and is not a repack-latency measurement.

## Timings

Single bounded release run, milliseconds, nearest-rank percentiles:

| Operation | n | p50 | p95 | p99 |
|---|---:|---:|---:|---:|
| New metadata DB initialize + compact + close | 32 | 52.977 | 59.040 | 862.088 |
| Actual structural commit | 160 | 2.733 | 4.054 | 4.920 |
| Warm `ProjectDb::open`, including preflight | 32 | 4.674 | 5.393 | 6.063 |
| Clean DB drop | 32 | 6.667 | 7.092 | 10.492 |

Final write-close: 8.608 ms; reopen plus an additional `load_reopened`: 6.103 ms
(single observations, not percentiles). Compaction at the end is a scratch-only
size probe and is excluded from normal close. PNG export/writer backlog/window
startup are excluded. The 862 ms initialization outlier is retained; cause is
unattributed, so this does not establish a tight startup latency bound.

## Correctness and remaining work

Actual redb 4 workspace tests and release commit-kill probe pass. Automatic v2
conversion has exact record/backups checks, 128-step child-process undo/redo,
layer/page/off-page correctness, failure/collision handling and real process
termination immediately before/after publication. See
[ADR-0049](../decisions/ADR-0049-redb4-alpha-upgrade.md).

Remaining: real large/multi-layer documents; startup/preflight materialization
cost; user installation/GUI acceptance; power-loss/disk-full migration publication
coverage; non-Windows filesystem behavior. No release/tag/push or installed-app
replacement was performed as part of this storage upgrade.

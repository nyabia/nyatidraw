# ADR-0036: Validate repeated root objects once per load

- Date: 2026-09-06
- Status: Accepted for storage correctness and isolated release improvement; new GUI timing pending

## Problem and decision

The installed history queue-to-present p95 in ADR-0034 was 155.647 ms.
An isolated copy of the same historical 4K fixture showed that loading its
immutable tiles accounted for most of the storage portion: p95 90.378 ms,
versus 0.063 ms for tree/page and 5.487 ms for immediate cursor persistence.

`snapshot_from_manifest` previously read, decoded and hashed an object for
every manifest key, even when many keys referenced the same content hash.
It then constructed each `TileObject`, hashing and allocating pixels again.
The fixture's final 902 tile keys reference only 156 distinct objects.

Keep a local map of validated objects keyed by their content hash during
one root load. Read and validate every distinct stored record in the same
read transaction, construct its immutable `TileObject` once, compare its hash
to the manifest, then share its immutable pixel allocation across keys.
`TileSnapshot::from_objects` assembles these objects with the same signed keys,
duplicate-key rejection and transparent omission as `from_tiles`, and recomputes
the canonical root. The stored expected root must still match.

This cache ends with the root load. A later load reads and verifies the stored
objects again, including after an earlier successful load. It never borrows
unchecked disk content or cached objects from a previous snapshot. Missing
records, bad envelopes, wrong tile lengths, wrong content hashes and wrong roots
remain errors. Cursor, metadata, transaction durability and recovery semantics
are unchanged. No dependency, format or GUI command changes were needed.

The map contains no more entries than the decoded manifest, whose entry count
is checked against its payload length. It retains one small entry per distinct
object while constructing that snapshot; repeated
tiles share immutable pixels instead of separate 64 KiB allocations. Documents
with entirely unique tiles get no deduplication benefit and still pay the map
lookup cost; that workload has not been timed in this campaign.

## Correctness evidence

Two core invariant tests were added:

- Object assembly preserves exact artwork and canonical roots for signed and
  multi-layer keys, independent of input order; duplicate opaque/transparent
  keys are rejected.
- A shared-object project reopens with exact artwork. After a successful load,
  missing, malformed-envelope, wrong-content and wrong-length records are
  rejected on another load/reopen, preserving the corrupted file bytes.

All 19 tests in the two affected crates passed, followed by all 80 workspace
all-feature tests and workspace all-target/all-feature Clippy with warnings
denied. Logs: `target/history-storage/{core-tests-final,workspace-tests,clippy}.log`.
The existing upstream vendored Wry lifetime warning remains outside workspace
diagnostics. No UI/framework mock tests were added.

The measurement helper performs ten Undo/Redo pairs on explicitly marked
scratch copies. Each operation persists its cursor with immediate durability.
After each variant exits, the separate reference verifier reopens its database
and matches all 902 tiles, tree/page, snapshot 33, history count 33 and decoded
PNG to the original. Both passed. This is process restart verification of the
storage path; the optimized loader has not yet completed a new installed GUI
Save/Close/reopen campaign. The existing installed app was left running after
the computer-use foreground PID failure recorded in ADR-0035.

## Release measurement

Windows 11 Home 10.0.26200, Core Ultra 7 155H, redb **2.6.3**, Cargo **release**.
This probe uses CPU/storage; Intel Arc/DX12 is not involved. Fixture: 3840×2160,
two raster layers, 32 strokes, 33 history nodes. Other host activity was
uncontrolled, the installed editor was idle, and no build/test ran concurrently.
Protected downloads were not inspected or modified.

Each variant is one session of 20 alternating Undo/Redo operations, with no
warmup discarded. Values below are nearest-rank quantiles of raw microsecond
samples, converted to milliseconds, not histogram bounds.

| Variant / interval | p50 | p95 | p99 |
|---|---:|---:|---:|
| Before: tile load | 84.992 | 90.378 | 90.902 |
| After: tile load | 10.955 | 12.776 | 14.136 |
| Before: storage portion total | 96.946 | 100.454 | 101.138 |
| After: storage portion total | 13.944 | 16.361 | 217.607 |

The after run's first immediate cursor persistence took **204.677 ms**. This
outlier is retained; it makes the total p99 worse despite the tile-load gain.
The cause of that storage tail has not been established. Do not discard it or
assign it to downloads, antivirus, hardware or the patch without evidence.

The measured total starts before cursor preparation and ends after session
adoption. It excludes worker queueing, navigator/thumbnails, canvas adoption,
composite/present, UI/OS delivery and visible pixels. No whole-Undo 16 ms pass
or replacement of ADR-0034's actual UI measurement is claimed. Next: installed
GUI acceptance, new queue-to-present distribution and persistence-tail analysis.

Reproduce with `desktop_performance_fixture measure-history-storage` on a
separate `performance-scratch.ntdr` whose parent contains
`.nyatidraw-performance-scratch`, then `verify-reference` against the untouched
reference. The helper never exports over the reference. Raw logs and complete
sample rows are linked in
[measurement JSON](../measurements/history-storage-4k-2026-09-06.json).

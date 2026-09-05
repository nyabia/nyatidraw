# ADR-0010: Immutable layer state at history cursors

- Status: Accepted, including desktop layer deletion and metadata Undo
- Date: 2026-09-05

## Problem and decision

Layer metadata was stored only as `state/current_layer_tree`. Moving the history
cursor restored immutable pixels while keeping the latest hierarchy, so layer
deletion and metadata Undo could not safely use that contract.

`commit_structural_with_layer_tree` writes the new hierarchy, pixels, history,
snapshot and current cursor in one immediate redb transaction. `snapshot_layers`
maps snapshot IDs to immutable layer envelopes. Subsequent ordinary stroke or
structural commits inherit the current hierarchy. Cursor moves load the target
record and restore `current_layer_tree` in the cursor transaction. Standalone
`persist_layer_tree` is rejected once layer history has been enabled.

The base project marker stays 1 until the first explicit metadata-history commit.
That transaction promotes it to 2, which older marker-1 writers reject. It also
freezes the existing current tree once and assigns explicit `legacy` references
to the already-existing snapshots and initial cursor. Missing legacy tree means
the same absent tree as before; the desktop can still apply its documented
default. Old files never recorded earlier metadata states, so those states are
not reconstructed or claimed. Merely opening a legacy file does not migrate it.

Reopen validates the mapping against every known snapshot plus the initial
cursor, including branches outside current ancestry. It rejects orphan/missing
records, bad layer envelopes and a current tree that differs from its snapshot.
Existing bounds apply: 100,000 history nodes and 5 MiB maximum layer record.
Migration keeps one shared legacy tree rather than duplicating it for every
old node. Record envelope remains 1. The original layer payload was version 1;
[ADR-0011](ADR-0011-reference-layer-metadata.md) adds backward-readable version 2
for raster Reference membership.

## Evidence and limits

Two core tests cover atomic migration abort, metadata/cursor restoration,
ordinary-stroke inheritance, and table-driven missing/historical-checksum/current
mismatch rejection with exact invalid-file preservation. Workspace tests total
60. All-feature, all-target Clippy passes with `-D warnings`.

The existing scratch crash probe now also kills the exact child process before
and after an immediate combined pixel/metadata commit, reopens the project and
compares both. Release runs passed on Windows 11 Home 10.0.26200 / Core Ultra 7
155H. This is CPU/redb process-kill evidence, not GPU/physical input, power loss,
or filesystem-cache durability proof. Logs: `target/layer-history-release-crash.log`,
`target/layer-history-workspace-tests.log`, `target/layer-history-all-clippy.log`.

Desktop artwork layer commands now commit through this boundary before publishing
the candidate tree. Deletion removes all subtree tiles from the new snapshot;
older snapshots retain them. Undo/Redo restores both tree and pixels, reconciles
GPU surfaces on the existing device, and updates navigator/thumbnail publication.
The active raster falls back to a surviving raster; deleting the last raster
creates an empty one. Solo and active selection remain session state. A failed
GPU adoption after durable acceptance latches a workspace error and prevents
drawing until the saved project is reopened.

The installed DX 0.7.9 release passes a 20-raster, two-level nested-group fixture:
rename, opacity, raster/subtree deletion, last-raster fallback, cross-parent
reorder, branch Undo/Redo, Save and process restart with exact tree/tile/PNG
comparison. Existing installed durability and PNG crash/recovery cases also pass.
Logs: `target/installed-layer-history.log`, `target/installed-layer-durability.log`.
The existing core tree invariant was extended; the test count remains 60.

No dependency was added or changed; redb remains exactly 2.6.3. Idle metadata and
history requests still use synchronous worker replies, so this does not close
the hot-path latency gate. Per-cursor canvas/page state remains subsequent work
in the active Sprint 1–3 goal. Drag reorder UI was subsequently connected as
recorded in ADR-0007. Semantic
probes do not establish direct UI interaction or physical-pen evidence.

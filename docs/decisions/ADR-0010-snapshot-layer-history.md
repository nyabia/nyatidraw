# ADR-0010: Immutable layer state at history cursors

- Status: Storage boundary accepted; desktop integration pending
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
old node. Record envelope and layer payload versions remain 1.

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

No dependency was added or changed; redb remains exactly 2.6.3. This checkpoint
does not enable desktop deletion or restore its GPU tree on Undo yet. Desktop
commands, active-layer fallback, subtree tile removal, Reference metadata and
per-cursor canvas/page state are subsequent work in the active Sprint 1–3 goal.

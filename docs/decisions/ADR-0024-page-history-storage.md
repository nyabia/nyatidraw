# ADR-0024: Page dimensions share the artwork/history transaction

Status: storage boundary accepted by recovery tests. Desktop page resize/crop
commands, rendering/export adoption and installed-app acceptance remain pending.

## Decision

Introduce project schema marker 4 and a `snapshot_canvas` table. Each snapshot,
including the initial Undo cursor, owns a checksum envelope of record kind 9.
Its payload is width, height and pixels-per-inch as three little-endian u32
values. The complete envelope is exactly 64 bytes. Dimensions and resolution
must be nonzero; editor/GPU allocation budgets are a separate admission policy.

`commit_structural_with_canvas` publishes the pixel root, history node/cursor,
page record and current page in one Immediate redb transaction. First use freezes
the last known legacy page for all historical snapshots, including branches and
the initial cursor. It also enables existing immutable layer history. Legacy
files cannot reconstruct page sizes that were never recorded; we explicitly
retain one compatibility value. Opening a legacy file alone does not upgrade it.

Subsequent normal structural commits and all strokes capture the current page.
Selected strokes retain marker 4 instead of overwriting it with marker 3. Existing
v1-v3 readers reject the new marker. Once enabled, out-of-band page persistence
is refused. A validated history cursor move writes its page dimensions together
with its hierarchy and pixel cursor. The new `load_cursor_canvas_spec` exposes
the target page to the desktop worker for subsequent renderer integration.

Open validates record count/keys against every persisted snapshot plus initial
cursor; orphan or missing records fail. Bounded record length, checksum, nonzero
fields, schema marker and current-page agreement are checked. Versioned projects
must contain current canvas metadata even when the intended page equals the
legacy default. Corrupt records on non-current branches also reject open, with
original bytes preserved. No dependency or core-platform boundary changed.

## Evidence

Two core product-risk tests cover:

- First upgrade abort leaves the old marker, page and cursor intact; an error
  returned after durable commit leaves a complete new page/history state.
  Old/new/selected-stroke/branched/initial cursors are then independently closed
  and reopened with exact expected dimensions, resolution, tiles and node count.
  The selected stroke must retain v4; direct page writes must fail after upgrade.
- Seven corrupt scratch variants: missing historical record, bad checksum,
  orphan record, zero dimensions with a valid checksum, wrong current width,
  downgraded schema marker, and missing current canvas version. All fail with
  `Corrupt` and byte-for-byte preservation of the rejected project.

Workspace tests passed with 74 tests in `target/page-history-workspace-tests.log`.
The final added current-marker invariant passed the focused pair again in
`target/page-history-tests-final.log`. Workspace/all-target/all-feature Clippy
with warnings denied passed in `target/page-history-clippy-final.log`.

These tests reopen databases in the same test process; they are not installed
GUI or process-kill evidence. The existing diagnostic transaction mechanism
supplies deterministic before-commit abort and after-commit return failures.
The full page feature still needs command/UI wiring, worker-owned page updates,
GPU scene adoption, latest-page export during Save/Close and independent-process
artwork/PNG acceptance before it is considered complete.

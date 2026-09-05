# ADR-0018: Nonblocking history and layer requests

Status: accepted for writer isolation; large-page adoption latency remains open.

## Decision

Undo/Redo/explicit branch paging and durable layer changes use the same single
pending artwork job as selection and fill. The renderer admits a request with
`try_send` into the existing four-entry writer FIFO and polls a one-entry reply
channel with `try_recv`. The former zero-capacity rendezvous and renderer-side
`recv` calls are removed. The writer requests a redraw after every result,
including branch pages and rejected cursor moves.

Raw admission is atomically paused only when no stroke or queued transition is
in flight. It stays paused until CPU tiles, GPU state, selection and projection
have adopted the result. Other artwork commands are rejected while that job is
pending. Viewport and docking commands, Save and Close remain available. A
newly added layer becomes active only after its commit is adopted. The UI shows
the generic work-in-progress state; admission acknowledgement does not claim
that the durable result has already completed.

Missing Undo/Redo candidates are recoverable rejections. Loading, persisting or
accepting a prepared history move fails closed on error, since the durable
cursor and in-memory state could otherwise disagree. Layer transaction failures
retain the existing fail-stop policy. A failed display adoption also keeps raw
input quarantined until reopen.

Save waits for artwork adoption during normal operation. Close can retire the
display with a result still pending: the buffered reply cannot block the writer,
and the close worker queues the retained export after accepted artwork work.
Export reads the writer-owned layer tree at its FIFO position. Passing the old
renderer tree would export stale visibility when Close precedes adoption.

No dependencies, project format changes or new unit tests were needed. Existing
input transition, durable history and recovery invariant tests remain in force.

## Evidence

Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, installed DX12 release
build (`target/async-artwork-install.log`). A 129×65 scratch copied from the exact
source/tolerance acceptance fixture begins at snapshot 2 with three raster
layers. The computer-use API performed actual toolbar and layer controls.

- A marked scratch-only `NAYATI_ARTWORK_PAUSE=layer` barrier stopped the writer
  before committing the top layer's visibility change. The UI remained at the
  old visible state, displayed work in progress, zoomed from 731% to 913%,
  retained Save, and displayed the normal closing progress dialog.
- Releasing the barrier after Close committed snapshot 3, exported its hidden
  layer composition and joined the writer without a renderer adoption. A
  separate fixture process reopened it and verified exact tiles, off-page
  padding, other layers, tree, three history nodes and the independent PNG
  oracle. The app was then restarted into that hidden state.
- With `NAYATI_ARTWORK_PAUSE=history`, actual Undo again allowed zoom and Save
  while pending. Releasing it while the app remained open restored visibility
  and completed the waiting PNG without another input event. Normal Close and
  a separate reopen verified snapshot 2 / three nodes and exact composition.
- A further process restart and actual Redo restored snapshot 3 / three nodes,
  hid the layer on screen, and passed Save/Close/separate reopen/PNG comparison.

App logs and final Undo/Redo verifier logs are in `target/async-artwork-ui/`.
The fixture's `verify-hidden` and `verify-undo` modes compare against explicit
tree/pixel expectations; PNG expected values do not call the compositor under
test. The barrier requires both the exact scratch filename and a sibling
`.nyatidraw-scratch-artwork-probe` marker, and times out after three minutes.

All 69 existing workspace tests and workspace/all-target/all-feature Clippy
passed (`target/async-artwork-tests.log`, `target/async-artwork-clippy.log`). The
extended fixture additionally passed build and focused Clippy. No physical pen,
first-visible-pixel measurement or p50/p95/p99 latency claim is made by this
controlled writer suspension.

## Remaining work

The renderer still installs returned CPU tiles and uploads GPU state as one
adoption step. Large-page and many-layer adoption, projection generation,
thumbnail costs and display latency require measurements before claiming the
complete hot-path performance gate. Basic transforms/page sizing and remaining
Sprint 3 docking/layout/activation gates are separate work.

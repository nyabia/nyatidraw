# ADR-0025: Non-destructive page resize/crop and worker-owned export dimensions

Status: accepted for the Windows installed-app scenarios below.

## Decision

`ResizePage` changes the output width/height around the existing top-left origin,
without resampling or clipping artwork. PPI stays unchanged. `CropPageToSelection`
uses the selected pixels' complete bounding rectangle, including holes, and
rebases every raster by the negative rectangle origin. Locked, hidden, off-page
and negative-coordinate pixels move with the document and remain durable. This
does not paint into a locked layer; it changes the document coordinate system.
Empty selection rejects crop. Successful page operations clear selection.

The CPU rebase reads one immutable root and builds exact RGBA replacements.
Signed i32 coordinate overflow, unsupported mips and existing 16 Mi-pixel scan /
256 MiB workspace limits reject the whole change. Resize also checks nonzero
dimensions, the output pixel ceiling, device texture limits and the existing
512 MiB persistent scene/atlas budget before durable work. Crop cannot enlarge
the current page. Page-sized GPU surfaces are disposable and rebuilt as needed;
the device, pipelines, native canvas and monotonic stroke identities survive.
The existing view stays in place; Fit uses the new page.

Both operations use the single pending artwork job and bounded writer FIFO.
Page dimensions and artwork/history commit atomically through schema v4
([ADR-0024](ADR-0024-page-history-storage.md)). The writer owns the current page,
updates it after page commits and history moves, and uses that page for subsequent
edits, navigator/thumbnails and PNG export. Export requests carry no renderer
page copy: Save/Close queued before renderer adoption must export the dimensions
at their position in the writer FIFO. Renderer adoption publishes page dimensions
in the semantic UI projection. Raw input never passes through that projection.

The Page toolbar action opens width/height and crop controls in the existing
subtool panel. It shares that space with the numeric Transform panel, supports
Escape/Close, and stops editor shortcuts while entering values. No dependencies
were added. This is not arbitrary canvas-origin editing or image resampling.

## Evidence

One new table-driven core invariant test covers disconnected selection bounds,
all-layer exact translation and inverse across signed tile boundaries, off-page
pixels, bounded work/memory rejection and signed coordinate overflow. Workspace
tests passed with 75 total tests (`target/page-edit-tests.log`). Final workspace,
all-target/all-feature Clippy with warnings denied passed
(`target/page-edit-clippy-final.log`). The pinned DX release build and local
installation passed (`target/page-install.log`). No UI wiring tests were added.

Runtime: Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, DX12 installed
release. `desktop_page_fixture` creates a fresh 16×16 scratch with a four-pixel
green selection, other visible pixels, locked/hidden layers, negative coordinates
and boundary padding. Each verifier is a separate process after the desktop has
joined its writer. Explicit expected coordinates/dimensions construct the PNG
oracle without the production crop, flattener or compositor.

| Actual UI operation | Independent durable expectation | Evidence under `target/page-ui/` |
|---|---|---|
| Wand green; crop; Save; close | 2×2, all layers rebased by [-4,-4], snapshot 2/history 2 | `crop-out.log`, `crop-verify.log` |
| Restart; resize; Fit; Save; close | 16×12, rebased pixels unchanged, snapshot 3/history 3 | `grow-out.log`, `grow-verify.log` |
| Restart; resize to 1×1 while writer paused; Save; Close; release writer | 1×1 latest-page PNG, snapshot 4/history 4; no renderer adoption required | `pending-out.log`, `pending-verify.log` |
| Restart; Undo three times; Save; close | Original 16×16 and every original pixel coordinate, snapshot 1/history 4 | `undo-out.log`, `undo-verify.log` |
| Restart original; Redo crop; Save; close | 2×2 and all rebased pixels, snapshot 2/history 4 | `redo-out.log`, `redo-verify.log` |

Every comparison passed for complete tiles, layer properties, PPI, history and
PNG bytes. Actual restart screens also showed the saved pages. Each session kept
one native canvas/device; normal exits reported `close-ready writer=joined`.
The pending-close scenario deliberately pauses only the writer via the existing
scratch acceptance mechanism: exact `page-scratch.ntdr` name, sibling scratch
marker and `NAYATI_ARTWORK_PAUSE=page` are all required. Actual UI Save and Close
showed the save-in-progress overlay. The log then records commit, latest-page
export and joined close without an artwork-adopted event.

This small fixture proves correctness, not large-page latency or peak GPU-memory
behavior. Large-scene p50/p95/p99, physical pen and visible-pixel evidence remain
separate gates. The paused writer is controlled integration evidence, not a
physical storage stall or crash/power-loss experiment.

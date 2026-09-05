# ADR-0023: Bounded deterministic selection and raster transforms

Status: accepted for numeric basic transforms and the Windows scenarios below.
Free-angle manipulation and canvas transform handles/previews are not provided.

## Semantics

`EditCommand::Transform(RasterTransform)` applies source horizontal/vertical
flips, clockwise quarter-turns, optional nearest pixel-center resampling to an
explicit final size, then integer translation. Its anchor is the original source
bounds' top-left. With no explicit size, rotated dimensions are retained. The
UI offers positive integer width/height together or leaves both automatic.

When selection exists, its coverage bounds define the source; selected active
raster pixels are cut before the transformed pixels are composited source-over.
When selection is absent, bounds include all occupied active-raster pixels,
including negative coordinates and page-boundary padding. Other layers are not
transformed. Transparent sampled pixels leave the destination unchanged. Reads
come exclusively from the immutable original snapshot, so overlaps cannot smear
or double-blend. Premultiplied linear RGBA bytes retain their existing semantics.

The operation does not clip results to the page. Checked signed i32 pixel bounds
reject overflow. Unsupported mips, missing/locked layers, invalid turns/sizes
and resource overflow reject the whole operation. Input tile scanning, source
and destination rectangles are each bounded by the existing 16 Mi-pixel edit
limit. Selection storage, copied root metadata, replacement buffers and immutable
replacement construction share the existing 256 MiB workspace allowance. An
oversized sparse bounding rectangle is rejected instead of allocating by the
unbounded coordinate span. No new dependencies or persisted format were added.

The existing single pending artwork job pauses input admission and executes on
the durable writer. Changed pixels commit one structural history node before
renderer adoption. Success clears the session selection; rejection retains it.
Save/Close continue to use the existing worker FIFO and independent PNG domain.
The parameter panel occupies the existing subtool panel, without overlaying the
native child canvas or recreating its GPU. It stops editor shortcut propagation
while entering values and supports Cancel/Escape before application.

## Evidence

Three core invariant tests cover 9 transform combinations at three offsets,
including negative/cross-tile coordinates, exact inverse sampling, reduction,
expansion, immutable overlap reads, premultiplied source-over, other-layer and
unselected off-page preservation, locked targets and invalid/budget/coordinate
rejection. Workspace tests passed with 72 total tests; all-target/all-feature
Clippy with `-D warnings` passed. Logs are `target/transform-workspace-tests.log`
and `target/transform-clippy.log`. Release DX installation passed in
`target/transform-install.log` using the pinned Dioxus toolchain.

Runtime environment: Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc,
DX12 installed release. `desktop_transform_fixture` created a fresh 16×16 scratch
project with a 2×3 six-color active raster straddling the left page edge. Another
layer contains an on-page translucent pixel, negative artwork and off-page
padding. Every golden expectation uses explicit independent pixel coordinates;
production transform, tile flattening and compositing do not generate the oracle.

Actual computer-use mouse/keyboard actions and separate verification processes:

| Operation | Durable expectation | Evidence under `target/transform-ui/` |
|---|---|---|
| Whole raster right 90° + offset [4,2], Save, Close | snapshot 2/history 2, all six pixels moved inside page | `first-out.log`, `rotate-verify.log` |
| Restart with rotated picture visible, Undo, Save, Close | snapshot 1/history 2, original negative pixels restored | `undo-out.log`, `undo-verify.log` |
| Restart, Wand the green pixel, transform offset [2,0], Save, Close | snapshot 3/history 3, exactly one pixel moved, selection cleared, old branch retained | `selection-out.log`, `selection-verify.log` |
| Restart with moved pixel visible, whole-raster size 8×6, Save, Close | snapshot 4/history 4, exact 2× nearest enlargement including negative pixels | `resize-out.log`, `resize-verify.log` |

All verifiers compared complete tiles, layer tree, page metadata, history counts
and cursor, and byte-exact encoded PNG against independent page pixels. Each
session created its native canvas/GPU once and closed with its writer joined.
The small examples are correctness evidence, not p50/p95/p99 performance or
physical-pen/visible-pixel evidence. Large-scene timing and the separate page
size/crop functionality remain open.

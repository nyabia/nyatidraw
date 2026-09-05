# ADR-0017: Bounded transient edit guides and gesture cancellation

Status: accepted for the initial guide/cancel connection.

## Decision

The in-progress Lasso displays its captured polyline and closing edge; Gradient
displays the axis from Begin to the latest point. These renderer-owned guides
are separate from committed selection coverage and artwork. End replaces the
guide with the completed result; invalidation or cancellation removes it.

The guide renderer accepts at most 4,096 points and allocates a fixed 64 KiB GPU
segment buffer. It projects document coordinates through the current viewport,
including negative window origins, and draws instanced four-physical-pixel quads
with a cyan center and dark edge into the final display texture. No raw sample
or per-move UI projection is introduced. CPU staging is bounded and reused;
there is no guide pass without an active path. This is bounded correctness,
not a measured claim about maximum-size gesture latency.

Esc cancels only an unfinished native edit gesture. The old session selection
and artwork remain intact, and subsequent Move/End cannot finish the cancelled
gesture. Native PointerCancel is ordinary cancellation rather than an edit
failure. Edits already handed to the writer are not cancelled by this command.

No dependencies, project format changes or additional unit tests were needed.

## Evidence

`gpu_selected_stroke --release -- dx12` and the Vulkan run now read the actual
viewport with open/closed guide paths under rotation, DPI 1.5 and a negative
window origin. The guide appears at the projected midpoint; clearing it restores
the exact preceding display bytes. Every resident artwork pixel remains exact.
An oversized path is rejected. Existing selected brush/eraser/Cancel comparisons
also pass. These runs use Windows 11 Home build 26200, Core Ultra 7 155H / Intel
Arc in release profile. Logs: `target/gesture-preview-dx12.log` and
`target/gesture-preview-vulkan.log`.

For installed release acceptance, the marked scratch-only `lasso-guide` probe
feeds Begin, three Move samples and End through the native bounded input lane,
holding before End so the real screen can be inspected. This is synthetic input,
not physical pen or hand-drawn polygon evidence. The computer-use native API
observed the rectangular live guide and its transition to a 4,050-pixel selection.
Actual toolbar/canvas clicks filled it, saved, and closed the app. A separate
process reopened snapshot 2 / two history nodes and compared all pixels with
the independently constructed rectangle x=10..99, y=10..54, plus exact negative
tiles, padding, tree and PNG (`target/gesture-preview-ui-1/verify-lasso.log`).

A second installed scratch run held the same gesture. With keyboard focus in
the editor toolbar, Esc removed the guide; a later synthetic End did not restore
a selection. Save, normal process close and separate reopen retained snapshot 1,
one history node and every baseline/PNG pixel exactly
(`target/gesture-preview-ui-2/verify-cancel.log`). App logs accompany both cases.

All 69 existing workspace/all-target tests, all-feature Clippy and formatting
passed. Logs: `target/gesture-preview-tests.log`, `gesture-preview-clippy-final.log`.
The installed build is recorded in `target/gesture-preview-install-final.log`.

## Remaining gates

Reference/tolerance UI combinations and completed native edit input immediately
before Close still need focused acceptance. Freehand polygon interaction with a
physical pen and visible-pixel timing remain unverified. Synchronous metadata
and history requests on the renderer, large-page adoption measurements and the
remaining Sprint 3 docking/projection gates remain open.

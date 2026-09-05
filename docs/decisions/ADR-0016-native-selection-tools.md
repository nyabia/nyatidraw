# ADR-0016: Native edit gestures and disposable selection boundaries

Status: accepted for the initial desktop connection; in-progress gesture chrome
and the remaining acceptance scenarios below are still open.

## Behavior

Wand (W), Lasso (L), Fill (F) and Gradient (Shift+F) are available from the native
canvas and toolbar. Native samples stay in the bounded input lane; the renderer
captures edit gestures separately from brush strokes and only a completed End
becomes a semantic edit request. Dioxus receives settings and completion/error
projections, never raw input. The existing writer performs durable edits.

Wand and an unselected Fill use Active/Reference/AllVisible source and a 0..255
seed RGBA tolerance. Fill with a selection paints that selection; without one,
it uses a temporary connected mask without changing session selection. Gradient
requires a selection and different endpoints, and fades current color to
transparent. The tool panel states these contracts. Colors use the existing
premultiplied linear tile representation; no dependency or file format changes.

Lasso retains at most 4,096 document pixel vertices. Duplicate consecutive
pixels do not allocate another point. Cancel, insufficient points, invalid
coordinates, viewport changes and overflow never submit partial polygons.
Input discontinuity cancels an active edit gesture and requires a clean Begin.
Only one completed edit is retained per drain: overlapping completions are
explicitly rejected instead of silently dropping a command. Tool changes pause
admission atomically, and active edit gestures prevent target-layer changes.
Native completions rejected because artwork is still busy report a retryable
error. A completed End drained on close is sent to the writer FIFO before export
on the dedicated close worker; unfinished selection gestures are not sealed.

Selection boundaries are a separate final viewport pass sharing the immutable
packed GPU mask. Black/white dashes use physical display coordinates and the
inverse viewport affine. The pass touches neither raster/group textures nor
navigator/export source pixels. Removing selection or moving history removes
the boundary. The boundary is static; no animation timer was introduced.

## Mouse correction found during acceptance

Computer-use drags exposed `GetMouseMovePointsEx` returning a Win32 error when
the dispatched point was absent from global history. Previously that error
discarded even the current WM_MOUSEMOVE point. The decoder now falls back to
the existing CurrentOnly path and re-anchors future history. It never substitutes
unrelated global points. Mouse Up now records its actual mapped position rather
than reusing the last Move, and a changed viewport still cancels the gesture.

## Evidence

One core risk table was added: invalid/cancelled/overflowing/viewport-shifted or
nonfinite Lasso input cannot emit a partial artwork command; valid input can.
Workspace total is 69. Full workspace/all-target tests and all-feature Clippy
passed, as did formatting. Logs: `target/native-edit-tests-final.log` and
`target/native-edit-clippy-final.log`.

The installed release was controlled through the computer-use native mouse API
on Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, DX12. No synthetic
selection bootstrap was enabled for these three 129x65 scratch projects:

- `target/native-edit-ui-1`: click Wand, observe 8,385 selected pixels and the
  boundary, select Fill and click, Save, normal process close, separate reopen
  verifier. All page pixels equal [26,199,232,255]; negative tiles, padding,
  tree/history and exported PNG match the independent expected result.
- `target/native-edit-ui-2`: click Wand, select Gradient, drag from document
  x=10 to x=110. Save/close and independent process verification compare every
  channel against direct rational pixel-center interpolation, including exact
  off-page preservation and PNG. The result matches without tolerance.
- `target/native-edit-ui-3`: choose Fill with no selection, click, Save/close
  and verify the same opaque expected page and exact preserved outside data.
  Session selection remains absent.

Each project ends at snapshot 2 with two history nodes. Verification uses
`desktop_edit_fixture verify-native-edit <scratch> 2 2 solid|gradient`.
The GPU selected-stroke release probe also renders the new boundary with a
rotated viewport and checks all artwork surfaces remain exact on DX12 and
Vulkan. Logs: `target/native-edit-gpu-dx12.log`, `native-edit-gpu-vulkan.log`.
This is correctness evidence on one Windows GPU, not latency or physical pen
proof. Installer evidence is `target/native-edit-install-final.log`.

## Remaining acceptance and UX

In-progress Lasso path and Gradient axis previews are not drawn yet. Lasso has
bounded capture and the CPU selection implementation, but still needs installed
closed-polygon gesture acceptance. Broader reference/tolerance UI scenarios,
native edit End arriving immediately before close, and moving/zooming selection
chrome need focused acceptance. The synchronous history/metadata renderer work,
large-page adoption measurements, physical pen and visible-pixel timing gates
remain open. This checkpoint does not mark the complete basic-edit gate passed.

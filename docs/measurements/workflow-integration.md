# Local drawing workflow integration

Windows, isolated Edge profiles, Dioxus web/Worker debug build,
WebGPU. Native CLI checks used the Windows debug build. This is correctness
acceptance, not a performance benchmark or physical-pen claim.

## Scope and fixes

- Confirmed web color corrections now mark their committed pixel changes so
  navigator and layer thumbnails are regenerated. Preview/cancel/neutral results
  do not invalidate committed-artwork previews.
- Reference-fill guidance follows clipping-base visibility and opacity, including
  group bases. It does not infer whether a raster contains nontransparent pixels.
- Canvas action failures now use the existing transient notice. Transforming an
  alpha-locked layer explains why it was rejected instead of appearing inert.
- CLI validate/export load cursor-aware reopened tiles. Compact base-only artwork
  and Undo-to-initial snapshots are no longer mistaken for an empty stroke log.
  File format and the CLI's existing raw-layer PPM export semantics are unchanged.

## Actual browser path

The scratch image had a closed black contour and a divider, drawn by mouse input.
Its line layer was marked as reference. A separate color layer below it was filled
in two connected compartments using ReferenceOnly. Undo changed the result and
Redo restored identical canvas pixels.

An 80px pen crossed the silhouette with alpha lock enabled; the visible exterior
remained unchanged. A separate Multiply shadow layer clipped to the color base
was painted beyond the silhouette. Disabling clipping revealed the retained raw
overpaint; enabling it restored the exact prior canvas pixels. This confirms the
clipping toggle did not erase the hidden paint.

A hue correction on the color layer updated the canvas, navigator and affected
layer thumbnail. DOM image identities for the untouched line and shadow layers
remained unchanged. During preview the committed thumbnails remained unchanged.
The initial post-build reload used cached old WASM; cache was then disabled in
the isolated profile and the rebuilt WASM response size was verified before
accepting the result.

Marking the reference line as clipped, then hiding its base, showed the missing
effective reference guidance. Restoring the base removed the notice. The test
restored the original layer flags without painting during this probe.

## File and process boundary

The normal Save and PNG export controls produced `workflow-final.ntdr` (282624
bytes) and `workflow-final.png` (39532 bytes). After closing that browser process,
a different fresh profile imported the NTDR through Open and its confirmation.
Line reference, color alpha lock, shadow clipping/Multiply, layer names and the
ReferenceOnly fill setting were restored. The native CLI also reported a valid
nonempty root for the browser-generated file.

Re-exporting PNG from the imported file produced the exact same bytes:

`626c4be0ca542810f822ec88b3fff0e92edf8d6c9fcacf58f0907eb55f3e725d`

On the reopened document, a selected transform was rejected with an explicit
alpha-lock explanation. After unlocking, a 25px horizontal-move preview was
cancelled. Clearing the selection and restoring alpha lock yielded the same PNG
hash again. This checks transform cancellation, not a new transformed-commit
restart case.

Local captures and scratch downloads are under ignored `output/playwright/`.
Canvas screenshot comparisons excluded only the changing bottom 48px save notice;
the test artwork was outside that strip. PNG equality used the complete files.

## Other checks and limits

- Existing W1 preset/tool persistence, temporary eraser and picker checks: 7 passed.
- Correction commit/cache-invalidating core invariant: passed.
- CLI base/cursor regression and existing fresh-process diagnostic smoke: passed.
- Native desktop/shared UI/CLI and WASM web/Worker Clippy with `-D warnings`: passed.
- Web+Worker build, workspace formatting and diff whitespace checks: passed.
- Existing vendor wry lifetime warning remains. Browser warnings/errors showed only
  the missing local favicon and the known WebGPU powerPreference warning.

No user artwork, installed application or real browser profile was used. No
commit, push, tag, release or installation was performed. The native CLI check
establishes file readability, not native-window visual equivalence. Physical pen,
iPad/Safari, desktop window interaction and real illustration W0-W5 gates remain
unverified; a synthetic contour is not an artist workflow sign-off.

## Release candidate correctness pass

Windows, isolated Edge profiles at 1600 by 1000, Dioxus web/Worker
release build, WebGPU. No real artwork or existing browser profile was used.
This pass completes bounded local implementation and correctness checks, not
the physical illustration or installed-release gates.

### Repaired artwork and session failures

- [Paint acceptance](paint-completion.md): alpha-locked fill no longer recolors
  a target's boundary pixels through the backing fringe. The shared UI disables
  expansion and minimum-one-pixel backing while alpha is locked, retaining their
  configured values for unlocking. Tolerance and gap closing remain available.
- [Shape acceptance](shape-completion.md): reject resampling that leaves a
  nonempty selection mask but removes every previously visible selected pixel.
  The rejected preview cannot be committed; source artwork remains intact.
  This is not an area-filter quality improvement for individual small details.
- [Persistence acceptance](persistence-completion.md): unsupported, malformed,
  or oversized optional tool settings do not prevent web artwork from opening
  and being saved. Their original bytes are retained, settings are temporary,
  and the user receives a notice. Invalid required artwork still fails closed.
- Native round input uses explicit live dab limits and whole-stroke cancellation
  on discontinuity. Valid 4K/8K diagonal evaluation and archived round replay keep
  their existing output. The old replay entry point and web's stricter existing
  limits are unchanged; these bounds are not a latency guarantee.

### Release-browser fill, Undo, and reopen

Imported the previous synthetic `workflow-final.png` through Open, making one
raster containing both line and color. Selected current-layer fill, set backing
expansion to 4, and enabled alpha lock. The expansion number, slider, and backing
checkbox became disabled; tolerance and gap controls stayed enabled. Unlocking
restored the editable value 4; relocking and swapping foreground to white allowed
a click inside the upper colored compartment.

Full 1280 by 720 RGBA8 export comparisons, not cropped screenshots:

- 21,665 pixels changed; no alpha changed.
- All 42,805 nontransparent black source pixels remained exact.
- Undo reproduced the original complete RGBA buffer; Redo reproduced the fill.
- Saved `completion-fill.ntdr` (217,088 bytes), closed the browser process,
  opened a different empty profile, imported the file, and re-exported PNG.
- Alpha lock and its disabled fill controls were restored. Every reopened RGBA
  pixel matched the saved result.

Decoded RGBA SHA-256 for fill, Redo, and fresh-profile reopen:

`3b61a93e334bd6b5ac607f011ab75fb8d4a4dc54daa25c613d14523356383f9b`

### Release-browser color correction and reopen

Previewed hue 80 and saturation -20, then cancelled. The exported RGBA buffer
was identical to the pre-preview fill. Reopened the dialog, set hue 40 and
brightness 10 with its number fields, and confirmed. This changed 109,777 pixels
without changing any alpha. Saved both PNG and NTDR, closed the browser process,
and imported the NTDR in a third fresh profile. Its PNG matched every RGBA pixel.

Decoded RGBA SHA-256 for confirmed correction and fresh-profile reopen:

`b326939b32dd4c24de27305bfd2400a609a181815d18a534ed7d3409cafdaf13`

Scratch captures, downloaded projects, and exports are named `completion-*`
under ignored `output/playwright/`. These cases prove synthetic artwork
preservation across browser processes, not artistic usability or physical pen
response. Console diagnostics contained only the missing local favicon,
Chromium's WebGPU power-preference warning, and a non-blocking autofocus notice.

### Build and acceptance boundaries

- `cargo test --workspace --all-features --locked --quiet` passed again after
  the live-input safeguard: brush 6, paint CPU 37, project storage 31, project
  web 12, and desktop 56 passed with 5 existing ignored cases, among the workspace
  suites. Fresh child-process artwork checks passed; ignored cases are not
  counted as verified hardware behavior.
- Strict whole-workspace/all-targets/all-features Clippy and WASM web/Worker
  Clippy passed. Existing vendored Wry/Dioxus warnings are separate from project
  diagnostics; no dependencies were added.
- Final Windows release build passed after the live-input change. A preceding
  isolated release launch loaded a synthetic NTDR and logged GPU submission on
  Dx12/RTX 3080. Windows screenshot/interaction was blocked by capture and cursor
  access errors, so this is not proof of a correctly visible native window.
- Test-process save/reopen evidence is in the three linked completion reports.
  It does not substitute for native-window Save As or installer/update testing.
- The web UI and recovery Worker were rebuilt in release mode after the final
  shared-brush changes. A fourth fresh browser imported `completion-color.ntdr`;
  its complete exported RGBA matched the correction hash above. The native CLI
  independently validated the same browser-produced project as a nonempty root.
- Workspace rustfmt and CRLF-aware diff whitespace checks passed. Build products
  remain local under `target/dx/nyatidraw-desktop/release/windows/app/` and
  `target/dx/nyatidraw-web/release/web/public/`.

Final Windows executable SHA-256:
`e016373fe5ec1b4002486c790d4578c84ced04029476d00fd900b9c0154abe3a`.
The final web UI WASM is `nyatidraw-web_bg-dxhab1f09370f6c853.wasm`, 3,817,779
bytes. These identify the local candidate, not a published or installed version.

### Candidate compatibility and remaining gates

This working tree also includes the earlier named brush presets, shared fill UI,
minimal raster color correction, and non-destructive rough-layer color effect.
Before a future release, its notes must explain:

- Configured layer-color metadata requires the newer reader capability, even
  when disabled or retained only in history. Older readers must reject it rather
  than silently change appearance.
- Named preset libraries belong to the app, not the NTDR document. Supported
  last-tool settings still use the established project/app precedence.
- The shared NTDR format does not remove the web editor's stricter import and
  working-set limits. Undo remains capped at 128 retained operations.
- Bilinear reduction may lose individual fine details; the new guard prevents
  total selected-ink loss, not all downsampling loss.
- W0 reference illustrations, physical pressure/smoothing and pen-tool switching,
  current native-window interactions, and installed/update lifecycle are still
  unverified. This is a local candidate, not all W0-W5 gates passing.

No commit, push, tag, release, installation, or user-original edit was performed.
Layer masks, merge, advanced brushes, patent-sensitive assisted coloring, and
macOS/Linux implementation remain outside this pass.

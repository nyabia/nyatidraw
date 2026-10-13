# Desktop color correction acceptance

Windows, native debug/test profile. Scratch projects only; the installed
application and user artwork/preferences were not changed.

The desktop worker uses the shared pinned color-adjustment session and the existing
durable transform store. The native runtime keeps one in-flight job plus one latest
pending preview; cancellation wins over pending previews. While a draft exists,
native drawing remains paused and artwork, selection, layer and history mutations
are rejected. Tool switching (including named brush preset application) cancels the
draft and applies the tool intent before releasing input. ESC cancels. Save/Save As
require confirmation or cancellation first; closing does not implicitly confirm.

Preview tiles replace only disposable GPU display state. Durable CPU tiles, history,
autosave, Navigator and layer thumbnails retain committed artwork until confirmation.
The adapter rejects previews adding tile keys beyond the pinned, already admitted
surfaces. GPU upload failure fail-stops the workspace rather than enabling editing
over a partially adopted preview. A rejected preview leaves cancellation available
and prevents confirmation until a new valid preview has been adopted.

Passed commands:

- `cargo clippy -p nyatidraw-desktop -- -D warnings`
- `cargo test -p nyatidraw-desktop correction_previews_cancel_without_history -- --nocapture`
  (one writer/session acceptance test, also passed in its spawned fresh process)
- `cargo test -p nyatidraw-paint-cpu -p nyatidraw-web-core --locked color_`
  (three CPU tests and one web-core test)
- `cargo test -p nyatidraw-desktop --locked native_color_preview_cancel_and_commit -- --ignored --nocapture`
  (one actual ActiveCanvas/WGPU acceptance test)
- `cargo test -p nyatidraw-desktop --locked`
  (55 passed, zero failed, five explicit hardware/registry acceptance tests ignored)

The writer/session acceptance checks a union of selected pixels on a negative/offpage tile
and a page tile, preserving unselected pixels, alpha and another layer. Preview
does not enter the project record; neutral confirmation and cancellation consume
no ID/history. Stale-generation and repeated confirmation are rejected. Confirmation
adds one history step; Undo/Redo restore exact snapshots. A saved scratch project is
reopened in a separate test process with the same tile root and offpage pixel, and
its re-exported PNG has the same decoded pixels as the saved PNG.

The explicit native GPU acceptance ran on an NVIDIA GeForce RTX 3080 with the
Vulkan backend and NVIDIA driver, debug profile. It exercises the real ActiveCanvas
worker/adoption path: rapid preview requests retain the latest value, resident GPU
tile bytes and composited page/offpage display match the preview, and cancellation
restores the original display without history or durable changes. Confirmation,
Undo and Redo agree across durable CPU tiles, resident GPU tiles and composite
display. The scratch project is closed and reopened with the same root/history;
decoded saved and reopened PNG exports match. This is headless GPU evidence, not
window presentation or physical input evidence.

Existing vendor wry lifetime warnings remain. The first fixture used a rectangle
which included neighboring signed pixels; the corrected fixture explicitly unions
two single-pixel selections. That fixture failure was not a product regression.

This does not establish physical-pen, desktop window interaction, first-visible-pixel,
high-refresh, cross-platform or real-artwork acceptance. Main-agent browser UI checks
are separate evidence. No benchmark or performance claim is made here.

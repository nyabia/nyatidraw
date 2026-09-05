# ADR-0022: Direct hue, saturation and value controls

Status: accepted for the bounded Windows mouse/keyboard and durable-artwork
scenarios below. Physical-device cancellation and latency evidence remain open.

## Decision

The Color panel supplies a hue ring, central saturation/value square and value
strip, with pointer capture and keyboard adjustment. Local HSV preview remains
in the WebView. Completion sends at most one RGBA tool command bound to the
revision at gesture start through the existing bounded semantic queue. The
picker returns to authoritative RGBA until an accepted command is projected;
current/recent colors use that same projection. No raw samples enter UI state.

Pointer positions clamp saturation/value to their range. Hue and saturation
survive zero value so raising black back to full value recovers its chromatic
color. Direction keys adjust by one unit or ten with Shift; Home/End select the
axis endpoints. Esc, pointer cancellation, capture loss, a hidden document or
actual window blur cancel the local drag. Component remount disposes previous
listeners/observer and resolves the previous evaluation lifetime.

This changes session drawing controls, not the artwork/history format. The
chosen color affects the next drawing/edit operation. Recent colors and drawing
controls remain session state rather than preferences persisted across restart.
No dependencies were added and no core input or brush invariants changed.

## Evidence

Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, DX12 installed release.
The scratch 129×65 project was `target/color-picker-ui/edit-source-scratch.ntdr`,
with an explicit sibling workspace-layout override.

- Actual computer-use hue drag and Home selected red hue. Dragging SV past its
  top-right corner selected `[255,0,0,255]`. Dragging value below the strip
  selected black; End restored exactly `[255,0,0,255]`, retaining saturation.
  Each completion admitted one tool command; current/recent displays agreed.
- An actual Fill shortcut and canvas click filled the active page red. Save and
  PNG completed, followed by normal close and writer join. An independent
  `desktop_edit_sources_fixture verify-red` process compared all 8385 pixels,
  other layers, negative tiles and page-boundary padding, snapshot 3/history 3
  and PNG. The expected red/reference composite uses the explicit independent
  `[223,0,0,255]` value; the visible blue stripe remains `[0,0,128,255]`.
- Restarting the final installed binary restored the red artwork onscreen.
  Moving Color above Navigator remounted its UI. Clicking the moved value strip
  selected `[15,111,130,255]` and updated current/recent. Native canvas and GPU
  creation occurred once for the session. Normal close and independent artwork,
  history and PNG verification passed again without new artwork mutations.

Runtime evidence is in `target/color-picker-ui/{first-out,red-verify,reopen-out,
reopen-verify}.log`. Workspace all-target/all-feature Clippy with warnings denied
passed (`target/color-picker-clippy-final.log`); the final release installation
passed (`target/color-picker-install-final.log`). No unit tests were added for
UI routing; the independent scratch verifier gained the red artwork scenario.

These checks do not establish actual Esc/focus/capture loss during a held color
drag, physical-pen input or first-visible-pixel/latency performance. The computer
use API supplied complete drags, not separately held pointer phases. Those
acceptance items remain explicitly unverified.

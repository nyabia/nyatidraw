# ADR-0021: WebView dock pointer capture and release-time targeting

Status: accepted for the bounded Windows mouse/cancellation scenarios below.
Long-session, physical-pen and live insertion-marker visual acceptance remain open.

## Decision

Dock handles acquire pointer capture on primary left-button pointerdown. A local
four-CSS-pixel movement threshold distinguishes a tab click from a drag. Pointer
coordinates, hit testing and the cyan marker stay in the WebView; they no longer
update a Dioxus signal on each mouse move. Only capture/cancel telemetry and one
completed semantic command cross the bridge. The command retains the revision
at pointerdown and enters the existing bounded editor-command queue.

Release recomputes its target from the current client coordinates. A previously
hovered target is never retained as a fallback. Top toolbar insertion and side
Left/Top/Right targets share this path. The marker uses one pointer-transparent
fixed element with endpoint indicators; it does not recreate or hide the native
canvas, GPU device or project session.

Esc, window blur, hidden document, pointercancel, lost capture, missing source,
and a move with the primary button already released cancel the local operation.
Completion clears local state before releasing capture, so a late capture-loss
event cannot revive or duplicate it. Ordinary element focus changes are excluded
from window-blur cancellation. Listeners and the marker have a disposal path.
No dependencies, artwork transactions or new unit tests were needed.

## Evidence

Windows 11 Home build 26200, Core Ultra 7 155H / Intel Arc, installed DX12 release
build. All work used the 129×65 `edit-source-scratch.ntdr` fixture under
`target/dock-capture-ui/`, with an explicit sibling layout override.

- An actual computer-use drag moved Color from the right across the child canvas
  to above Brush on the left. Capture was held and one dock command was admitted.
- Dragging that panel through a candidate area and releasing in the canvas center
  cancelled as `outside-target`. A toolbar release on the native title bar,
  outside the WebView client area, also cancelled. The settings hash remained
  identical to `before-cancel.layout` across both operations.
- Ordinary History/Layer tab clicks still activated the tabs. Acceptance caught
  an initial bug where element blur events reaching the window capture listener
  cancelled new drags. Filtering for the window itself fixed it; a toolbar drag
  after focusing the Layer tab completed successfully in the final build.
- Four controlled cancellations were injected into actual computer-use drags:
  synthetic Esc, synthetic window blur, synthetic pointercancel, and explicit
  `releasePointerCapture` to cause capture loss. All four cancelled without a
  layout write. Hash comparison against `before-probes.layout` passed. The next
  fresh drag completed a top-toolbar reorder, followed by another successful
  reorder after a normal tab click.
- A normal restart with the probe disabled restored the accepted arrangement.
  Another actual drag released over the native canvas's left target and moved
  Quick colors beside the canvas, demonstrating release delivery over the child
  HWND. The observed sessions each retained one native canvas/GPU creation and
  finished with `close-ready writer=joined`.
- Separate fixture processes after normal close compared all 8385 page pixels,
  other layers, negative tiles, page-boundary padding, snapshot 2/history 2 and
  independent PNG expectations exactly. No stray drawing was introduced by these
  captured mouse gestures or controlled cancellations.

The cancellation probe requires `NAYATI_DOCK_CANCEL_PROBE=sequence`, the exact
scratch filename and sibling `.nyatidraw-scratch-dock-probe` marker. It injects
only four signals, logs them as `synthetic-cancel-probe`, then leaves subsequent
drags unmodified. This is not proof of physical-device capture loss or an actual
OS focus switch during a held drag.

App logs and hash/artwork verification are in `target/dock-capture-ui/`, including
`first-*`, `cancel-verify.log`, `probes-*`, and `final-reopen-*`. Full workspace,
all-target/all-feature Clippy and release installation passed
(`target/dock-capture-clippy-final.log`, `target/dock-capture-install-final.log`).
No unit tests were added for framework event routing. Native input invariants
were not changed. Actual physical-pen continuity, arbitrary desktop/window loss,
long-duration interaction and insertion-marker screenshots during a held drag
are not established by this bounded acceptance.

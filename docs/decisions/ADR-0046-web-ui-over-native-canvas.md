# ADR-0046: Web UI above the native canvas

Status: Windows implementation; focused Release runtime acceptance passed

## Contract and platform boundary

`apps/desktop/src/canvas_host.rs` defines `HostLayout`: canvas bounds, device
scale and bounded overlapping UI input rectangles in webview-client CSS space.
It contains no HWND, COM, GTK or AppKit types. DOM layout updates use this
contract; raw drawing samples never pass through it or through Dioxus signals.

Windows implements this contract in `canvas_host/windows.rs`. The opt-in
vendored Wry composition host attaches the WebView2 visual to the top-level
window's topmost DirectComposition target. The existing WGPU child keeps its
surface and render actor. There is no image readback, texture-to-PNG bridge,
or second rendering of the artwork in the webview.

The WRY child remains an input/focus sink outside the native canvas. Its input
region is the full client minus the canvas, even while a modal is visible.
Overlapping UI input is routed by the Windows host, not by expanding an HWND
over the swapchain. Mouse/contact ownership is fixed for a gesture, so a stroke
cannot turn into a UI click halfway through crossing a panel. Routing decisions
must release Rust state borrows before any reentrant Win32/COM call.
The visual is attached to the parent, independently of this input region.
Canvas ancestors are transparent; menus, panels and dialogs keep their normal
backgrounds, including translucent overlays. Invalid geometry fails closed by
hiding the native canvas and restoring the web input sink.

The original region-union approach passed a partial translucent toast, but
failed a full-screen close backdrop in runtime acceptance. Removing the old
explicit close-time `SW_HIDE` was necessary but insufficient. Removing
`WS_CLIPSIBLINGS` also did not fix the problem and was reverted. These probes
do not establish a general DXGI/driver diagnosis; the host instead avoids a
covering input HWND altogether. `WS_EX_NOREDIRECTIONBITMAP` expresses the WRY
sink's no-pixels ownership, not a proven fix for this failure.

Linux and macOS are **not implemented or validated**. Future GTK/WebKit and
AppKit/WKWebView adapters must satisfy the same geometry, overlap, focus,
capture and lifetime contract using their own compositor/view arrangements.
The Windows HWND-region technique is not a proposed cross-platform solution.
If a platform cannot satisfy the contract without canvas pixel copies, that
requires a separate architecture decision and measurement, not a silent fallback.

## Version and removal boundary

- Dioxus Desktop 0.7.9 (vendored opt-in config forwarding).
- Wry 0.53.5 (vendored composition controller and input adapter).
- Wry Windows bindings 0.61.3, webview2-com 0.38.2; existing app bindings 0.62.2.
- wgpu 26.0.1 unchanged.

Only required DirectComposition, pointer and controls API features were added
to the existing Windows dependencies; no version upgrade is involved.
The public Wry switch defaults off; other Wry
users keep the windowed controller. NyatiDraw enables the new path on Windows.
Removing this vendor patch requires equivalent upstream hosting APIs and repeat
acceptance, not just a version bump. Composition child construction/reparenting
is explicitly unsupported in this initial implementation.

## Related input and sizing changes

Native relative pan/zoom commands use the existing bounded FIFO without a stale
UI-projection precondition. Adjacent pan deltas coalesce using checked addition;
zoom and UI commands retain their ordering. Ordinary UI commands still enforce
revision checks. Capture changes occur after the last mutable canvas-state
access because Windows can synchronously call back on capture loss.

Side stacks store fixed leading-panel heights; their final panel fills remaining
space. Height and width drags display a guide and commit on release, not every
move. Heights use a bounded, versioned `workspace.heights` sidecar and the existing
background preferences writer. This is session layout, not artwork/history.

The native renderer keeps a handle to its last displayed GPU texture during
close and can present it on an explicit host wake without accessing artwork
owned by the close worker. No readback or continuous redraw is introduced.
True size/DPI changes, project activation and reopen discard that cache.
Close acknowledgement must not call `begin_close` a second time on an already
suspended engine, which would republish an acknowledged export failure.

## Acceptance boundary

Build and core tests do not prove visible compositing or physical pen behavior.
Required runtime checks: artwork visible, translucent UI above it, UI clicks do
not paint beneath, native mouse drawing and navigation, vertical resize and
restart persistence, normal close/reopen. Physical pen, mixed-DPI monitors,
IME, capture cancellation and non-Windows hosts must be reported separately.

### Verified Windows run

- Windows, NVIDIA GeForce RTX 3080, DX12, Mailbox presentation, device scale 1.
  This is functional acceptance, not a latency percentile benchmark.
- Release binary SHA-256:
  `439ed6c72d9bc0bd54a4fca7ba411a6899489ad88bab6da40761da69bbbdd8f6`.
- Final fixed-region/router implementation kept both checkerboard and artwork
  visible behind the full-screen translucent export-failure close dialog.
  Clicking its web-hosted reopen button restored the document; native drawing
  remained functional afterward.
- With artwork admission active, a missing-PNG activation produced a toast
  overlapping the canvas. Clicking within that overlap generated no GPU dab,
  closed stroke or materialization; the document stayed at snapshot 2.
- New scratch stroke: snapshot 2, root `8f29cc74d768c8fc8cac57753c727c42`,
  12 tiles. PNG export completed for that same snapshot/root (176,190 bytes).
  Normal close joined the writer, retired the surface, then destroyed the HWND.
  A fresh process reopened the identical root and 12 tiles.
- Navigator separator drag changed the leading panel height; later color/layer
  panels moved and the final panel filled the remaining space. Height survived
  process restart via the 28-byte settings sidecar. Live resize remains deferred.
- Desktop core tests: 28 passed, one explicit scratch-registry acceptance ignored.
  Workspace all-target/all-feature Clippy, rustfmt and both layout JS syntax checks
  passed. DX Release build passed; existing vendor warnings remain.
- Physical pen, actual held-middle-button drag, mixed-DPI and IME remain
  unverified. Move-tool drag exercised the same native pan queue with a visible
  260-by-120-pixel displacement and no stale-command rejection; that is not a
  replacement for physical middle-button acceptance.

Only scratch artwork/settings were used. The installed product, release tags,
remote repository and user artwork were not changed by this acceptance run.

API references:

- [DirectComposition target ordering](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-createtargetforhwnd)
- [WebView2 composition controller](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller)

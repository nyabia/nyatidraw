# ADR-0008: Dioxus Desktop shell with a native WGPU child canvas

- Status: **Accepted for the Windows Sprint 3 shell boundary**
- Date: 2026-09-02
- Supersedes: [ADR-0001](ADR-0001-native-shell.md) and the shell-specific
  layout publication in [ADR-0004](ADR-0004-native-canvas-viewport.md)

## Context

Dioxus Native 0.7/Blitz proved the core separation but remained unstable in the
actual editor. The observed shell crashed on a recoverable WGPU
`SurfaceError::Outdated`, its pointer conversion was incomplete, and the
custom-paint integration made canvas lifetime and layout depend on the
experimental renderer. This meets ADR-0001's reversal conditions.

The replacement must retain native WGPU painting and the direct Windows input
path. Sending high-rate samples or artwork pixels through WebView IPC is not an
acceptable fallback.

## Decision

Use ordinary Dioxus Desktop 0.7.9 for UI chrome. On Windows, create one raw
child HWND from `Config::with_on_window` before Wry creates the WebView. A DOM
`ResizeObserver` sends only the canvas rectangle and device scale through the
Dioxus eval channel. The host positions the child above the WebView in that
rectangle.

The child HWND owns:

- the WGPU instance, surface, adapter, device, and queue;
- the backend-neutral `SharedGpuCanvas` runtime;
- direct `WM_POINTER` and high-resolution mouse-history admission; and
- a small GPU presentation pass from the compositor's RGBA working texture to
  the swapchain format, including the non-document checkerboard.

Pen/mouse samples never enter Dioxus state or WebView IPC. Input is child-local,
so the published input origin is always `(0,0)`; the DOM rectangle is used only
for `SetWindowPos`. Closed strokes remain materialized to CPU/redb on the
existing worker path.

`Outdated`, `Lost`, `Timeout`, and `Other` surface acquisition results are
recoverable. `Outdated` and `Lost` reconfigure the surface and defer the frame;
only out-of-memory latches a workspace failure. The surface is dropped before
the child HWND completes `WM_NCDESTROY`.

## Exact direct dependencies

| Dependency | Version and purpose |
|---|---|
| `dioxus` | `0.7.9`, `desktop`, `launch`, and `lib` features |
| `dioxus-desktop` | `0.7.9`, `tokio_runtime`; Wry/Tao WebView shell |
| `wgpu` | `26.0.1`; native drawing, composition, and presentation |
| `raw-window-handle` | `0.6.2`; child HWND surface target |
| `pollster` | `0.4.0`; bounded adapter/device initialization |
| `windows` | `0.62.2`; child HWND, DPI, messages, and paint lifecycle |

Dioxus Desktop resolves Tao's `raw-window-handle 0.5` while WGPU uses 0.6. The
versions do not cross the boundary: the app constructs WGPU's 0.6 Win32 handle
directly from the child HWND. `dioxus-native`, Blitz, winit, and the previous
Vello patch are no longer in the desktop dependency graph.

## Current evidence

On Windows 11 with an RTX 3080, the exact DX build command succeeded with the
WebView renderer. A debug run selected DX12/BGRA8-sRGB/Mailbox, created the child
host, and logged the first native WGPU present at 1.578 seconds. This is one
startup observation, not a percentile benchmark or present-completion timing.

A dispatched Windows mouse stroke delivered 802 samples, closed once, queued
materialization with zero measured end-path blocking, and committed 23 changed
tiles to the untitled recovery project. Debug CPU replay/redb materialization
took about 751 ms on its worker. This proves separation from the hot input
path, not release performance or physical-pen behavior.

Focused input/queue/recovery tests, desktop Clippy with warnings denied, and
`dx build --windows --renderer webview --package nyatidraw-desktop --locked`
pass. Visual checkerboard and post-migration physical-pen acceptance remain
manual gates.

## Consequences and limits

- UI pen interaction now uses the mature system WebView event path.
- Drawing remains native WGPU and does not pay WebView IPC per sample.
- There is one additional fullscreen GPU presentation pass. Measure it at 4K
  and high refresh before considering a direct-to-surface compositor variant.
- Native canvas content always appears above WebView content in its rectangle;
  future modal/overlay UI must hide or reposition the child while it covers the
  canvas.
- Windows is the only child-surface implementation in this milestone. Other
  desktop platforms can use their own native host later without changing the
  drawing/document crates; macOS remains explicitly deferred.

## Reversal conditions

Replace this host seam if the child HWND cannot maintain correct z-order and
DPI geometry, if the extra presentation pass misses the measured latency
budget, or if Wry cannot support required canvas-overlaid editor interactions.
The drawing runtime, bounded input queue, durable snapshot, and project format
must remain unchanged by another shell migration.

# Native mask presentation investigation

Status: native visible/captured mask presentation remains unresolved. DX12 FIFO,
unchanged-input-region suppression, Vulkan, and the child-window `SWP_NOCOPYBITS`
trial did not prevent the menu-triggered missing-hole capture. The corrected
composition-controller parent HWND also passed a debug build but did not resolve
the actual captured failure. CPU artwork,
project recovery, browser acceptance and sampled GPU output passed their separate
gates. They do not complete native visual acceptance. No latency, physical
display, pen, or other adapter acceptance is implied.

On Windows with an NVIDIA GeForce RTX 3080, DX12 and a Bgra8UnormSrgb Mailbox
surface, the scratch mask project reopened with the correct CPU artwork and
navigator. Windows.Graphics.Capture through the desktop inspection tool
intermittently showed a solid red canvas instead of its transparent mask hole.
The capture is evidence about the window capture path, not direct observation
of physical scanout.

Synchronous scene or pre-present surface readback made the captured hole appear.
The subsequent asynchronous surface probe copied one pixel before Present,
presented without waiting, and collected results with nonblocking polling.
All 32 startup samples contained the expected gray surface pixel
`[221,221,221,255]`; later window capture showed solid red. Those samples were
exhausted before the menu transition, so they do not establish the surface
contents at the failing redraw. The revised diagnostic requests additional
wakes only while one asynchronous readback is pending, preserving its sample
budget for later UI redraws. In the revised `menu-trace.stdout.log` run, the
menu-triggered frame 5 still sampled `[221,221,221,255]` while capture showed
red at the corresponding window coordinate (canvas origin `[313,99]` plus
surface point approximately `[438,248]`). This establishes the discrepancy
after the final surface draw for that sampled point; it does not identify a
particular driver defect or establish physical-display behavior.

The trial selected FIFO on DX12 with AutoVsync fallback. The initial captured
hole was correct, but opening the layer menu reproduced the solid red canvas.
The prior Mailbox preference is restored. Adapter vendor, device, driver and
driver-info logging remains for diagnosis. The same RTX 3080 later reproduced
the issue with Vulkan as described below. No backend-wide workaround is retained.

The narrow host change caches the input HWND's applied outer rectangle and
canvas hole. Menu rectangles always update input routing, but unchanged physical
geometry no longer repeats SetWindowRgn and SetWindowPos. Actual resize and first
layout still update the native region. This removes redundant native window
updates. A normal run still reproduced the solid-red initial capture, so this
optimization is not a fix for the reported discrepancy.

Backend isolation uses the opt-in `NAYATI_GPU_BACKEND=vulkan` or `dx12` diagnostic.
It restricts instance backend enumeration only; no variable preserves the
existing default descriptor. Invalid values fail startup explicitly. There is
no UI preference, automatic fallback, or default-backend decision. The next
Vulkan run used no mask probe/readback flags and reproduced the initial-hole
then menu-triggered solid-red capture. This rules out a DX12-only workaround;
no backend default was changed. The first Vulkan attempt exposed a missing
Win32 raw-window-handle HINSTANCE; the handle now carries the actual child
window's GWLP_HINSTANCE. This fixes backend initialization, not the capture issue.

The presenter uses wgpu-owned texture views and bind groups whose submitted
lifetimes are managed by wgpu. The scene retains the display texture, and the
host retains the last presented texture handle. There is one render actor and
one device/queue. No early raw-resource release was found. Both backend results
now focus investigation on the shared child-HWND/parent-DirectComposition and
window-capture boundary; the DX12 separate-queue mechanism alone does not
explain the reproduction.

The locked `wgpu-hal 26.0.6` package identifies upstream commit
`6f8edda9b180efada6a24b6d40c567c57f59e9ea`. Its
[DX12 queue implementation](https://github.com/gfx-rs/wgpu/blob/6f8edda9b180efada6a24b6d40c567c57f59e9ea/wgpu-hal/src/dx12/mod.rs#L1476-L1509)
submits command lists immediately and signals its fence. It explicitly relies
on D3D runtime synchronization between the main rendering queue and a separate
presentation queue. Mailbox maps to DXGI Present with interval zero, and FIFO
to interval one. That is a relevant timing boundary, not proof that this
upstream mechanism is broken. Microsoft's
[Present documentation](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-present)
also distinguishes interval-zero frame replacement from synchronized
presentation. No dependency or driver change was made for this trial.

Scratch evidence is under the directory named in
`target/native-mask-current.txt`: `trace.stdout.log`, `present.stdout.log`,
`probe-only.stdout.log`, and `async-present.stdout.log`. The root session owns
builds and subsequent runtime acceptance. The guarded diagnostics are capped
and disabled on project activation or Save As.

A later control confirmed the embedded WebView DOM capture is transparent at the
canvas point. Temporarily making the HTML root fully transparent (unchanged
geometry) restored the captured mask hole and exposed an old canvas strip below
the current bounds: the initial 953-pixel-high child had shrunk to 857 pixels.
Restoring HTML opacity kept the hole until another UI change. This is evidence
of stale composed underlay regions, not an HTML artwork image.

The subsequent trial added `SWP_NOCOPYBITS` to the GPU child's SetWindowPos geometry
updates only. Microsoft documents that
[SetWindowPos otherwise preserves and copies old client-area contents after resize or move](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos).
The GPU owns a complete redraw, so old Win32 pixel preservation is unnecessary.
This change has no GPU wait and does not change input-window geometry rules.
The normal trial run (scratch process PID33756, no CDP flags) reproduced the
same initial-hole then menu-triggered disappearance. This trial failed to fix the
issue; copied Win32 client bits are not an established cause. The trial flag was
subsequently reverted. Evidence includes
`no-copy-normal.stdout.log` / `no-copy-normal.stderr.log` and the root session
window captures. The logs alone contain no successful visual proof.


## Actual embedded WebView inspection

The preceding scratch process PID33080 exposed only its Dioxus WebView at loopback
CDP 9333. A separate Playwright CLI session attached to that existing page without
navigation or input. At client point `(744,316)`, the canvas placeholder rectangle
was `(311.5,68,1237.96875,857)`, DPR 1, viewport 1856×1041.

Before and after the root session opened the actual mask popup:

- `#shared-gpu-canvas` was an empty div. Its sixteen covering ancestors had
  transparent computed backgrounds, no filter/transform and no pseudo artwork.
- The popup added only a transparent `.layer-mask-backdrop` at z-index 180 over
  that point. No img/svg/canvas/video/iframe overlapped it.
- The actual WebView-only PNG had RGBA `(0,0,0,0)` at the point in both states.
  Original layer thumbnail, navigator and brush preview images were bounded to
  their side-panel rectangles; none was enlarged over the native canvas.

With root authorization, setting only HTML opacity to 0 temporarily made the OS
capture show the expected hole and exposed old underlay strips. Removing that
inline style restored the exact prior state (style attribute absent, opacity 1).
The canvas rectangle stayed unchanged; the root session's restored capture still
showed the hole until a later UI change. This is a compositor invalidation clue,
not proof of an opaque HTML overlay. Playwright was detached without closing the
native app or unrelated browser sessions.

Evidence: `output/playwright/masks/native-dom-initial-results.txt`,
`native-dom-popup-results.txt`, `native-dom-initial.png`, `native-dom-popup.png`,
`native-opacity-debug.txt`, `native-opacity-restored.txt`. These are actual DOM
and WebView screenshots; they do not include the native child surface.

## Next bounded diagnosis

Keep the saved source and current CPU/GPU checks fixed. Compare one failing
transition across the same document point and geometry epoch: final swapchain
sample, WebView-only transparent pixel, and several timestamped OS capture frames
before/after menu open, close and idle. A single capture immediately after a
transition may be stale; repeated capture with matching epoch/frame evidence is
needed to distinguish persistent composition state from capture timing. The
existing asynchronous frame 5 sample already rules out a startup-only GPU sample
for that reproduction.

The old 953 px child versus current 857 px child strip makes the top-level HWND,
DirectComposition target and child surface geometry/exposure history the next
focus. The application-level source contains no raw-image overlay. Investigate
retained parent/child composition and capture buffers without adding artwork
mutation, blocking GPU waits, continuous redraw or a backend-wide workaround.
Physical scanout remains unverified; another independent capture path or direct
human observation would distinguish window-capture-only behavior from visible
presentation failure. Until that distinction and stable native redraw are
verified, keep the native mask workflow gate open.


## Host-identity correction and failed visual trial

A concrete integration mismatch was corrected: the WebView composition
controller had been created with the input child HWND as its parent, while its
DirectComposition visual targets the top-level HWND. The Windows subclass resize
path also assumed that controller ParentWindow was the input container. The
implementation now aligns the controller parent with the actual visual host and
passes the input container explicitly to the resize/subclass path. Input routing
still uses the dedicated child HWND.

The debug build passed. The actual normal scratch run (PID5768) still reproduced
the missing hole after opening the menu in Windows.Graphics.Capture; normal close
passed. This corrects a host contract but is not a verified fix for the captured
mask problem. Native visual acceptance remains open.

## Capture controls and removed visual diagnostic

An independent Alt+PrintScreen attempt produced no clipboard image and instead
accidentally activated a capture overlay in the preexisting Greenshot instance.
That overlay interfered with some later stale UI captures. The root session used
Greenshot's normal CLI `/exit`, then restarted the same Greenshot application.
Snipping Tool requested Windows privacy permission; that attempt was abandoned
without changing permissions or settings, and the owned helper was closed.
There is still no independent capture or physical-display proof. Do not classify
the issue as capture-only or as physical scanout failure from the current evidence.

The opt-in CompositionVisual trial was subsequently tested and removed, including
its adapter, environment switch, COM guard and Cargo feature additions. The
existing raw HWND swapchain remains the default and current presentation path.
In the final trial run (PID33612), after the Greenshot overlay was already cleared,
the mask hole appeared initially and immediately after a menu interaction, then
was missing again after more than ten seconds of idle and a tooltip. The overlay
therefore does not explain away the remaining reproduction. See the
[removed trial record](../canvas-composition-trial.md) for implementation scope
and retained independent host-contract corrections.

Native visual acceptance remains unpassed. CPU/GPU correctness, project recovery,
completed web acceptance and the now-passing cumulative mask-work regression tests
(`target/mask-budget-tests.log`) are separate evidence, not substitutes for that
gate. No broader renderer rewrite or physical scanout conclusion follows from
this failed diagnostic.

## Parent redirection bitmap isolation

The parent-only trial used `NAYATI_PARENT_NOREDIRECTION=1` before launch. It used
Tao's creation-time `with_no_redirection_bitmap(true)` only on the top-level
window. The raw HWND canvas child, WebView composition target, input routing,
geometry, and GPU presentation remain unchanged. Without that exact value the
previous parent style remains the default. The startup log identifies activation.

Source inspection found Tao 0.34.8 defaults to a redirected top-level window and
does not add `WS_CLIPCHILDREN`; the existing `WS_EX_NOREDIRECTIONBITMAP` applies
only to WRY's input child. Microsoft documents that the
[extended style removes the window's redirection surface](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles).
The topmost WebView visual still composes above child-window content under the
[documented target ordering](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondesktopdevice-createtargetforhwnd).
This tests whether the unused parent bitmap participates in stale underlay
ownership. It does not establish that ordinary redirected parents are invalid.

The trial failed (PID28060): GPU initialization and presentation succeeded using
Vulkan, but the entire canvas was transparent and underlying windows were visible
even after activating NyatiDraw; UI chrome remained correct. This establishes
that the raw canvas child depends on parent redirection in this configuration.
It is not a reproduction of only the missing-mask symptom. The parent-only switch
was removed in favor of the separately controlled topology below.

## Two visual trees on one parent: failed visual trial

`NAYATI_COMPOSITION_HOST=1` selects one complete experimental host topology:
the top-level HWND and canvas input child both omit redirection bitmaps, a DX12
composition swapchain targets the top-level HWND's non-topmost visual tree, and
the existing WebView uses its topmost tree. The raw HWND canvas path remains the
default. An explicitly requested Vulkan backend rejects this opt-in mode.

Unlike the removed child-target trial, both visual trees target the same parent.
The render actor owns the canvas device/target/visual and its COM MTA guard;
the surface drops first. The UI mailbox supplies physical parent-client origin,
size, and visibility. Move-only updates change visual offsets, resizing changes
the local clip and swapchain, and hiding removes the canvas root. Geometry and
swapchain changes commit before presentation. No artwork mutation, frame copy,
GPU readback/wait, continuous redraw, or browser GPU sharing is introduced.

PID5780 initially showed the correct hole, then a mask-menu click produced the
same captured solid-red page while the navigator and mask thumbnail retained the
hole. This trial therefore did not pass visual acceptance. Its opt-in code remains
available only for controlled A/B evidence; the raw path remains the default.

## Correlated final-surface and window-capture evidence

The four-slot asynchronous barcode probe in PID35108 closes the earlier
sample-counter ambiguity. Initial marker serial 5 showed the correct hole; after
opening the menu, captured marker `00000110` identified serial 6 while the page
was red at `[751,346]`. The matching `barcode-pool.stdout.log` readback for serial 6
is `sampled=true`, RGBA `[221,221,221,255]`, geometry epoch 3, surface 1238×857,
child-local point approximately `[438.2265625,247.7265625]`. Flooring the point and
adding the observed canvas origin `[313,99]` gives the same captured pixel.
Its before-present monotonic elapsed time is `56974230` microseconds.
Serial 5 also sampled opaque gray approximately 56.9
seconds earlier. Both records belong to DX12 on the RTX 3080 with Mailbox.

Thus the submitted surface pixel and captured composed-window pixel disagree for
the correlated marked frame. This is stronger than an uncorrelated startup sample;
it still does not identify a faulty component or establish physical-display
behavior. CPU artwork, source masks, navigator, and final-surface samples remain
separate evidence from the failing composed-window capture. Full details and the
earlier unsampled serial-9 ambiguity are retained in
[frame correlation](native-mask-frame-correlation.md).

One remaining bounded isolation would temporarily disconnect only the existing
WebView visual tree, commit, capture the unchanged native frame, then restore the
same visual and commit. The
[RootVisualTarget contract](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2compositioncontroller?view=webview2-1.0.4258.31)
explicitly supports null disconnection and requires an application-device commit.
This is a proposed diagnostic, not implemented or accepted: no new surface,
backend, geometry, artwork, or continuous repaint is needed. Hold the native
serial fixed and compare attached/detached/restored capture states; if detaching
alone reveals the gray hole, WebView visual participation or the resulting
composition invalidation matters. If red persists while chrome disappears, the
WebView tree's presence is not necessary for that stale state. Restoring the tree
must recover normal chrome before the test ends. Either result still leaves
physical scanout and capture-only failure unresolved; it cannot by itself prove
a WebView or driver defect.

# ADR-0028: Coalesce native redraw through WM_PAINT

Status: accepted for Windows redraw scheduling; latency gates remain open.

## Problem and decision

The child window rendered immediately after each mouse/pen/viewport message,
even though raw admission had already posted a renderer wakeup. The posted
`WM_NAYATI_REDRAW` also rendered immediately. Both paths could enter WGPU surface
acquisition while native input messages still awaited dispatch. The installed
4K campaign identified large surface waits, but does not by itself prove which
message path caused an individual wait or the isolated 100 ms hitch.

`WM_NAYATI_REDRAW` now invalidates the child without erasure or synchronous
update. The bridge's existing atomic pending bit stays set until `WM_PAINT`
begins rendering. Input and viewport handlers request the same idempotent wakeup
instead of rendering directly. The bounded raw and semantic queues are unchanged;
this merges paint requests, not stroke transitions. Requests that arrive during
render can still post one subsequent wakeup. Existing resize, activation and
close-recovery lifecycle rendering remains separate.

Microsoft documents that normal message retrieval processes posted/input work
before paint, and multiple invalidations can become one paint. Sources:
[GetMessage](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessage),
[invalidating the client area](https://learn.microsoft.com/en-us/windows/win32/gdi/invalidating-the-client-area),
[paint generation](https://devblogs.microsoft.com/oldnewthing/20111219-00/?p=8863).
We do not use `UpdateWindow`, nested message pumping, a busy render loop or a
new timer to force rendering from input dispatch.

## Limits and validation

Rendering and input still share the Windows UI thread. Surface acquisition
inside `WM_PAINT` can still delay later input, and continuously queued work can
defer paint. This is not full render-thread isolation and does not establish
physical pen latency or 120 Hz cadence. The synthetic paced workload bypasses
Win32 input and therefore tests its renderer wakeup path, not the removed
per-mouse-message render call. Both actual UI acceptance and paced measurements
are needed; neither is promoted beyond its scope.

All-target/all-feature workspace Clippy with warnings denied and pinned-DX
release installation passed (`target/paint-redraw-clippy.log`,
`paint-redraw-install.log`). No new unit tests or dependencies were added for
this OS/framework wiring change. Installed Computer Use exercised a 3px mouse
stroke, Undo, Move-tool pan, Fit, Save and normal Close. The separate-process
page fixture compared every original tile, locked/hidden/signed artwork,
dimensions/PPI, snapshot 1/history 2 and independent PNG exactly. Logs are in
`target/paint-redraw-ui/`; its three input batches are only wiring evidence.

Paced 4K before/after measurements use fresh folders under `target/paint-redraw/`
via `start-desktop-performance.ps1 -Campaign paint-redraw`, preserving the
foreground-before-start protocol and the user-owned background download.
Results and remaining risks are recorded in `docs/performance.md` when complete.

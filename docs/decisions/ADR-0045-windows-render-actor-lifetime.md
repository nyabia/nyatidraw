# ADR-0045: Windows render actor and window retirement

문서 기준 시각: 2026-09-08T21:24:30+09:00

Date: 2026-09-08
Status: Implemented; runtime acceptance and performance campaign pending

## Decision

Move surface creation/configuration/acquisition, GPU scene processing, present,
and renderer-side project transitions to one `nyatidraw-render` thread.
`CanvasWindowState` keeps native input/capture and window placement on the HWND
thread. Its paint and wake handlers only signal the actor. No actor operation
holds its mailbox mutex while executing GPU, filesystem, or HWND work.

The mailbox coalesces one frame wake and the newest geometry. A separate bounded
eight-path activation FIFO retains order and explicitly rejects overflow. Close,
reopen, and Save As have dedicated pending requests. Retirement is sticky even
after a batch is taken. Work arriving during a frame remains for the next batch.

Existing native primary/retry sample lanes and durable writer/export FIFO remain
the authority for artwork. Raw stylus samples never pass through Dioxus state or
the new control mailbox.

## Window ownership and all supported shutdown paths

1. `with_on_window` creates the child HWND but no render worker. Creation uses a
   stack `Option<Box<CanvasWindowState>>`: `WM_NCCREATE` takes it once and
   `WM_NCDESTROY` alone frees an adopted box. Creation error cannot double-free a
   state already adopted and destroyed by Win32.
2. The root's initial-render hook starts the actor after WebView construction
   succeeded, but before browser DOM mount/mutation acknowledgement is proven.
   The actor upgrades a weak Tao window reference to a strong `Arc<Window>` and
   retains it outside its render unwind boundary. Startup GPU errors therefore
   drop any partially created surface before the parent anchor is released.
3. Tao 0.34.8 destroys its native window by posting `DESTROY_MSG_ID` from
   `Window::drop`. The retained parent prevents its child from being destroyed
   while a raw WGPU surface still references it. Explicit child destruction is
   confined to pre-worker creation errors. No new `unsafe impl Send` is used.
4. Only VDOM-owned UI handle clones retain `RenderLifetime`. HWND state and the
   actor do not own that lease. Component/VDOM destruction signals retirement
   without creating a parent/actor ownership cycle.
5. Normal Close closes admission and sends the existing canvas close operation
   to the actor. Writer/export completion and surface retirement are distinct.
   The actor remains alive after writer-ready, allowing update-application
   failure to reopen the previous project. Only after the UI accepts that
   boundary does it request surface retirement and later approve parent Close.
6. Framework retirement permanently latches admission closed, including when a
   previously taken reopen request races retirement or Save As later releases
   its pause. The actor drains through `begin_close` and `suspend`, joining its
   subordinate workers away from the window thread.
7. The actor records `surface_retired` before releasing its parent anchor.
   HWND destruction checks that state, not `JoinHandle::is_finished`: a posted
   parent destruction can run before the final actor instructions finish.
   UI code joins only after the handle reports the OS thread actually finished.

Panic/startup failure is explicit workspace failure. Close against an already
failed actor produces a failed-close state, not an endless saving indicator.
An already-dead actor cannot accept reopen; the UI explains that the application
must be restarted. Hard process termination, aborting panic, power loss, and an
unresponsive graphics driver are not orderly-close guarantees.

## Framework exit gate

The public Dioxus custom-event callback cannot override its launcher's final
ControlFlow assignment. Tao's `run` calls `process::exit` after event-loop exit,
so retaining a window alone cannot guarantee artwork drain or renderer retirement.

A pinned local **dioxus-desktop 0.7.9** patch adds an optional nonblocking exit
guard. Exit/code or application-dispatch panic is latched. Until retirement
finishes, native messages continue pumping at a ten-millisecond wait interval;
the exiting/panicked App dispatcher is not reentered, except ordinary final
`LoopDestroyed` cleanup after retirement. With no guard the upstream normal
dispatch/exit behavior is preserved. The original exit or panic then resumes.
No live render-thread join occurs on the UI thread. Earlier
WebView construction failure has no render worker to await.

Exact published archive provenance, patch scope, and removal criteria are in
[`third_party/dioxus-desktop/NYATIDRAW.md`](../../third_party/dioxus-desktop/NYATIDRAW.md).
The workspace lockfile remains authoritative. No dependency versions or features
were upgraded. The first rebuild recompiles Dioxus Desktop and its application
dependents because the package source is now a local path.

## Geometry and publication

The HWND thread publishes size/DPI changes and invalidates Begin mapping with a
new geometry epoch. The actor renders against a captured epoch. A completed
frame carries its affine and epoch to the surface host; only after successful
`frame.present()` does the host publish that affine. Publication checks the
epoch under the same viewport mutex, so an older frame cannot undo a concurrent
resize/hide invalidation. Project reset also invalidates mapping until the new
document submits its own frame.

This is ordering against the present API, not first-visible-pixel evidence.
Already-admitted samples retain their existing document coordinates and phase
preservation/cancellation policy. A skipped frame does not attach its history
timing to a later frame.

## Evidence at implementation handoff

- Final integration `cargo fmt --all -- --check`, workspace tests, and workspace
  all-target/all-feature Clippy with `-D warnings` passed. Desktop core tests:
  25 passed, 0 failed, 1 explicitly ignored registry acceptance.
- Added three core invariants: stale geometry cannot reopen mapping, retirement
  survives Save As/reopen, and bounded actor batches preserve activation order
  and sticky retirement.
- The final mechanical UI detector reported no findings for `main.rs` and the
  layer-drag probe. This is not runtime or artwork-correctness evidence.
- Runtime markers: `desktop-render event=worker-started`, `event=surface-retired`,
  and `event=canvas-hwnd-destroyed surface-retired=true`.

Runtime evidence is tied to separate DX release executable SHA-256 values, not
retroactively assigned to the latest source:

- `43f09e17e98d868f8de25e3aeec606ff7e9e74ee8acbb98e18f1cec30be43805`:
  strict `desktop_window_lifecycle_smoke --require-render-worker` passed. All
  four children exited normally after WM_CLOSE, with surface retirement before
  HWND destruction. Coverage includes four resize/minimize/restore cycles,
  pending-resize Close, Close before first present, and ordinary restart.
  Independent page/tree/history/signed off-page tiles/PNG verification passed.
  Evidence: `target/window-lifecycle-local-run`.
  Durability acceptance also passed: 32 strokes/eraser, discontinuity, active
  stroke Close/Save and process reopen (`target/render-isolation-durability.log`).
  Invalid nonempty input remained unchanged with an error window retained; its
  exact scratch child was then **aborted**, not counted as graceful shutdown.
  Actual Windows Save As selected a new Korean/space-containing filename,
  switched the title and closed normally. Independent verification confirmed
  unchanged original bytes, three history nodes, branches/off-page tiles,
  metadata and byte-exact PNG. Evidence:
  `target/render-isolation-acceptance/nyatidraw-save-as-scratch/verify.log`.
- `c7d44a70a2106765784a0f8a6d4f123700f10f807f8b63b5baeda2240bba0ace`:
  the scratch drag probe waits for the first presented mapping and matching DOM
  revision, avoiding initial Fit invalidating its drag. All six synthetic drag
  modes passed without weakening stale-drop or artwork assertions. The full
  run later failed during zero-byte-pair restart at unchanged 1x1 geometry;
  this build is **not** a full recovery pass. Evidence:
  `target/render-isolation-export-recovery-final.log`.
- `b2b6d02589c646a096d93fde9f7c2de80b2678389f88cb5fad2e646191a5d742`:
  with optional startup diagnostics, full export/recovery passed twice:
  `target/render-isolation-export-recovery-ack-hold.log` and
  `target/render-isolation-export-recovery-ack-hold-2.log`. Coverage includes
  absent/zero/valid/invalid PNG pairing, same-primary activation, four process
  crash boundaries, superseded export discard and failed Close→reopen/save.
  The optional failure-window hold never triggered in these successful runs;
  original timeouts, failure verdicts and artwork comparisons were retained.

**Intermittent pre-observer startup stalling remains unresolved.** Twelve
restarts of the same zero-byte fixture passed, but another full diagnostic run
stalled with a different layer-history fixture before `observer-started`:
`target/render-isolation-repeated-startup-2.log` and
`target/render-isolation-export-recovery-diagnostic.log`. The later two passes
do not erase these failures or establish that instrumentation fixed anything.
No failed-window activation comparison was obtained. Pinned Dioxus first hides
the Windows builder, then captures `headless=true` for its interpreter; actual
bundled JavaScript applies mutations and acknowledges directly in that branch.
The rAF-only/occluded-window hypothesis therefore does not fit this path.
Diagnostics add mounted-event traffic and a richer geometry message even when
logging is off; they are not claimed to preserve bit-identical timing.

The final DX release build passed (17.76 seconds), with executable SHA-256
`45231e273722b54640e4a433a1a9116c3ccb7cd99e441903de801de29e884134`.
Its matched post-change performance campaign has started with diagnostic logging
disabled; results remain pending. The RTX 3080 baseline had little surface-acquire
waiting; no speedup is inferred merely from thread separation. These results do
not establish installed Windows behavior, physical-pen latency, first-visible
pixels, power-loss survival, 120 Hz, or completion of the startup-stability gate.
The [integration record](../status-performance-2026-09-08.md) retains the full
sequence of successes, failures and build identities.

## Upstream contracts

- [DXGI multithreading guidance](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/dxgi-best-practices#multithreading-and-dxgi)
  explains that DXGI can synchronously message the window thread and that
  blocking its pump while waiting for a render thread can deadlock.
- [DestroyWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-destroywindow)
  documents child destruction and the creating-thread requirement.
- Local, version-pinned Tao 0.34.8 `platform_impl/windows/window.rs` and
  `event_loop.rs`, and Dioxus Desktop 0.7.9 `config.rs`, `launch.rs`, and
  `webview.rs` were inspected for the actual lifetime and startup order.

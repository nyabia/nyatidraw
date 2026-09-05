# ADR-0009: Keep the Windows shell alive through durable Close

Status: accepted for the Windows software slice, 2026-09-05.

## Context

Synchronous teardown preserved closed artwork but blocked the window thread and
destroyed the UI before it could report a failed project commit or PNG export.
Closing an active stroke also required an explicit boundary between admitted
native samples and subsequent input. Export encoding previously relied on PNG
writer Drop for IEND/buffer completion, which can silently ignore write errors.

## Decision

- Intercept parent `WM_CLOSE` using a Windows subclass. Stop raw and semantic
  admission under their existing bounded-lane locks. Repeated Close requests
  cannot bypass saving or dismiss a failure.
- Hide the native child before showing the WebView dialog. Keep the window,
  surface, single-instance guard, and UI message loop alive during saving.
- Transfer the retiring `ActiveCanvas` (which contains no HWND or UI framework
  handles) to one close thread. It seals healthy active input, preserves FIFO
  admission, drains any explicitly requested latest export, and joins the writer.
  The UI observes completion with a 50 ms timer rather than joining a live thread.
- Treat project failure and PNG failure separately. Success permits ordinary
  framework close. Failure keeps a dialog with the recovery path; the user can
  reopen the durable project and retry Save, or explicitly acknowledge the error
  and close. Reopen is a recovery to the last durable state, not reconstruction
  of work that failed to commit.
- Explicitly finish the PNG encoder so IEND/flush failures propagate before the
  sibling temporary file is synced and generation-checked for replacement.
  Interrupted temporary exports are never promoted during project reopen.

## Dependencies and boundaries

No new package or version is introduced. `windows = 0.62.2` gains only its
`Win32_UI_Shell` feature for `SetWindowSubclass`, `RemoveWindowSubclass`, and
`DefSubclassProc`. Dioxus/Desktop remain `0.7.9`, wgpu `26.0.1`, PNG `0.17.16`,
and the existing project backend is unchanged. No OS/UI types enter core crates.

The dialog is desktop-private state, separate from semantic projection revision
and export generation. Raw samples never pass through Dioxus. GPU preview
delivery is retired before shutdown waits; CPU materialization remains required.

## Evidence and limits

`desktop_durability_reopen_smoke` covers 32 paint/erase strokes, active Close,
deferred Save/Close, metadata reopen, discontinuity recovery, and invalid files.
`desktop_export_recovery_smoke` kills its own scratch desktop after encoding,
after sync, and immediately before/after replacement. It compares previous PNG
bytes or latest exact canvas pixels and reopens the durable project in a new
process. It also checks an encoded superseded Save, actual Windows replacement
denial, visible responsive closing windows, dialog mounting, durable reopen,
and retry via the ordinary native Save shortcut.

Automated invariants cover late input rejection without dropping already
admitted work, and final PNG flush failure. Runtime environment and commands
are recorded in [implementation](../implementation.md). These are synthetic
functional and process-kill checks, not power-loss, physical-pen, visual layout,
screen-reader, first-visible-pixel, or latency percentile evidence. Unexpected
OS destruction still uses a blocking final join; ordinary user Close uses the
asynchronous path. Interrupted owned temporary files may remain on disk.

## Reversal conditions

Replace the host-specific close coordinator if subclass lifetime, native capture,
or shell shutdown cannot preserve these guarantees. Preserve bounded admission,
explicit failure, independent PNG/project durability, and last-valid recovery.

# 구현 현황과 검증 경계

## 2026-09-03 현재 구현

Windows-first 수직 절단은 더 이상 Sprint 0의 빈 골격만은 아니다. workspace에는
`Dioxus Desktop 0.7.9` WebView 셸과 DX CLI package boundary, Windows child-HWND `WM_POINTER` hook,
bounded native sample handoff, WGPU compositor, CPU replay 기반 closed-stroke
materialization, `redb 2.6.3` project backend, 그리고 CLI validate/PPM export가
있다. macOS shell 또는 AppKit input은 이 milestone의 범위 밖이다.

현재 편집기 배치와 각 컨트롤의 목표 의미론은
[편집기 UI 목표](editor-ui-target.md)가 정한다. 화면에 존재한다는 사실과 기능이
완료됐다는 사실을 분리하며, 실행 순서는 [스프린트 계획](sprints/README.md)의
Godot 즉석 스케치 → 게임 프로토타이핑 편집기 → 매일 쓰는 개발판을 따른다.

```text
apps/desktop       Dioxus Desktop UI, child-HWND Win32 input, native WGPU canvas
apps/cli           redb validate, bounded PPM(P6) export, owned diagnostic smoke
crates/api         dock/projection and typed command-event contracts
crates/input       platform-neutral samples and viewport transform
crates/input-*     bounded queue and Windows adapter
crates/document    validated raster/group layer tree
crates/paint-*     CPU replay and WGPU round-dab/composite paths
crates/project*    backend-neutral policy and redb durable backend
crates/editor      materialization, reopen, durable cursor orchestration
```

Desktop raw samples bypass Dioxus signals. UI controls enqueue bounded,
metadata-only `CommandEnvelope` values against the authoritative mailbox
revision; the renderer publishes `EditorEvent` plus a monotonic `UiProjection`.
Dock remounts reposition one session-owned child canvas and reuse its WGPU
runtime and recovery repository instead of creating a second authority.

Windows mouse drawing enters that same native bounded lane as a pressure-1
pointer; it is not implemented as Dioxus component state. The pre-dispatch
adapter expands the clicked mouse's bounded 64-point
`GetMouseMovePointsEx` display-point history and preserves its native event
cadence across the virtual desktop. It accepts history only after its last exact anchor; otherwise it
drops the global/stale history and forwards the current message point alone.
Compatibility mouse messages promoted from pen/touch are excluded, while every
message remains inside the child canvas procedure; ordinary UI interaction is
owned by the adjacent system WebView.

The child HWND receives canvas-origin pen messages directly. Pen input outside
that rectangle is handled as normal WebView pointer input, so no synthetic
`Touch`-to-mouse UI bridge is required.

출력 페이지는 1024x768 `CanvasSpec`으로 유한하지만 편집 작업공간은 signed sparse
tile로 페이지 밖까지 이어진다. 페이지 안에만 checker를 표시하고 검은 경계를 두며,
페이지 밖 closed tile도 저장/reopen한다. Windows child canvas에는 wheel vertical pan,
Shift+wheel horizontal pan, middle-button drag와 Ctrl+wheel zoom이 연결되어 있다.
Space/Move-tool pan과 pointer-centered zoom도 Windows child canvas 경로에 연결됐다.

The raw Win32 path expands each in-contact `WM_POINTERUPDATE` with
`GetPointerPenInfoHistory`, reverses Windows' newest-first result, and admits the
coalesced samples oldest-to-newest without duplicating the current entry. A
reusable event-loop-owned scratch buffer is capped at 4,096 samples. Pointer
type and hover are rejected before entering the pen recorder, cancelled flags
become `Cancel`, and updates without a known `Begin` cannot create a stroke.
HIMETRIC device coordinates are converted with the display/device ratio while
retaining fractional client pixels; this avoids the integer-only
`ptPixelLocation` path that made high-rate curves visibly polygonal.

The live renderer coalesces adjacent dab operations for one stroke generation
before WGPU submission and asks winit for a redraw instead of directly invoking
a synthetic `RedrawRequested` callback for every wake. Closed-stroke CPU replay
and immediate redb commit remain on the project-writer thread. A temporarily
full writer channel no longer stops raw input drain and GPU preview; unsent
closed strokes remain in the bounded semantic backlog.

## Evidence that exists

- Physical Windows mouse acceptance on 2026-09-01 confirmed that native
  display-point history removed the visible polyline corners. The accepted
  release run recorded 3,482 samples for one stroke and 594 for another, with
  live GPU drains occurring mostly once per 16-19 ms display interval. The user
  still observed visible cursor-to-ink trailing delay, so this is input-shape
  acceptance, not an input-to-present latency pass.
- The recorded Windows release probes used an RTX 3080/Vulkan device. They
  cover synthetic GPU submission/readback, closed snapshot upload on fresh
  devices, transactional live-stroke cancel/commit, layer/viewport composition,
  and view-only checkerboard output. They do not prove physical pen delivery,
  a first-visible-pixel timestamp, present completion, or latency percentiles.
- The redb scratch evidence covers immediate-transaction reopen, invalid
  non-empty-file preservation, branch-preserving history/cursor reopen,
  versioned LayerTree records, mixed stroke/structural snapshot heads, a white
  background added after ink, process-kill points, and the CLI's non-empty
  validate/export smoke. The project backend now reports an existing writer as
  a distinct locked-open error. The desktop source routes a second process
  through a Windows named mutex plus bounded named pipe, then drains the old
  worker before a UI-thread project switch; installed-release/Explorer
  acceptance remains unverified. It does
  not establish power-loss, arbitrary filesystem-cache, migration, compaction,
  or large-database recovery guarantees.
- Core tests are deliberately limited to artwork/input/recovery risks. Shell,
  Dioxus layout, and Win32 wrapper acceptance use compilation, Clippy, bounded
  runtime probes, and explicit hardware/runtime follow-up instead of mock-heavy
  tests.
- The Windows input core has 14 bounded tests for history ordering/no latest-entry
  duplication, transition non-expansion, hover and orphan-move rejection,
  cancellation, history limits, mouse promotion filtering, and
  pen fractional and mouse negative-display coordinate conversion. Those tests validate
  adapter invariants, not physical mouse/tablet-driver or visible-frame
  behavior.
- The round eraser uses destination-out in both the live GPU painter and CPU
  materializer. The RTX 3080 hardware probe remained inside the existing
  four-channel-byte UNORM tolerance, and the desktop durability smoke reopened
  a 32-stroke project containing eight eraser strokes. The same run exposed and
  fixed a missing `COPY_DST` flag on the live brush-colour uniform and a redraw
  latch that previously suppressed later asynchronous wakeups.

## 2026-09-05 shutdown, deferred Save, and discontinuity recovery

The current-host desktop smoke exposed a shutdown regression: 32 already-closed
scratch strokes entered the native lane, but only five were committed because
the four-entry GPU completion lane filled after its display owner stopped
consuming it. The pipeline now explicitly retires display delivery before its
blocking shutdown admission/drain. CPU replay and immediate project commits
still run for every closed stroke. A full/disconnected completion lane while
the display is live still latches a workspace failure and quarantines input.

The same acceptance run exposed a separate assumption that a semantic stroke
generation equals the GPU token ordinal. Discontinuity recovery can discard a
Begin before GPU submission, so those counters diverge. Preview retirement now
uses semantic generations throughout; GPU tokens remain device-local handles.
The protocol and input-safety probes also now stage their commands after the
startup Fit command has established the current revision.

Close now seals a healthy active stroke with a semantic End that preserves the
last received position, pressure and timestamp. Cancelled/discontinuous strokes
remain discarded; recording-limit and sequence exhaustion explicitly fail.
That End is a shutdown policy action, never a native input sample or physical
pen claim. Project durability remains independent from whether Save was requested.

Save reserves its export generation immediately and waits in one latest-only
slot for the active stroke and older closed backlog. A newer request invalidates
older PNG replacement even while waiting. Normal End admits the latest export
behind the closed stroke; Close seals/drains first and then admits an already
requested pending export before joining the writer. The UI exposes `Waiting`
separately from worker `Queued`/`Running`/`Current`/`Failed`. Export admission is
nonblocking while drawing; only shutdown may wait for a bounded writer slot,
without holding the export replacement lock. Writer dequeue wakes retained work.
New closed strokes now always follow existing retry backlog even when the worker
frees capacity during the same input drain, preserving paint/erase ordering.

Windows 11 Home build 26200, Intel Core Ultra 7 155H / Intel Arc integrated GPU,
DX12, both debug and DX-bundled release profiles: the corrected desktop smoke committed all 32 strokes,
restarted the process, and retained snapshot 32, six tiles and exact root
`a69ad4bb36e3b6392a8ea72f80afacdd`. It also checked layer metadata reopen,
stale-command rejection, clean-Begin recovery after an explicit discontinuity,
and non-empty invalid-file preservation. Three further process-restart cases
cover raw Begin/Move followed by Close, two Saves during a stroke followed by
Close, and two Saves followed by a normal End. They verify exact reopened
samples/history/tiles and compare the saved PNG's full canvas pixels with the
final one-layer fixture. The workspace's 55 automated tests and Clippy for all
targets passed. New regression coverage checks 32-stroke shutdown, healthy vs
cancelled/invalid active close, and ordering across writer backpressure; live
completion saturation still fails closed as required.

Reproduction commands and local logs:

```powershell
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
cargo build -p nyatidraw-desktop --locked
cargo run -p nyatidraw-desktop --example desktop_durability_reopen_smoke --locked
dx build --release --windows --renderer webview --package nyatidraw-desktop --locked
$env:NAYATI_DESKTOP_SMOKE_BINARY = (Resolve-Path target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe).Path
cargo run -p nyatidraw-desktop --example desktop_durability_reopen_smoke --locked
Remove-Item Env:NAYATI_DESKTOP_SMOKE_BINARY
```

The harness uses its adjacent profile executable by default; the explicit
override runs the actual DX bundle with the same acceptance cases. Logs are
under ignored `target/`: `tests-after-recovery.log`, `clippy-after-recovery.log`,
`desktop-durability-before.log`, `desktop-durability-after.log`,
`dx-release-after-recovery.log`, and `desktop-durability-release-after.log`.
DX returned success and emitted a release executable/assets, but installed CLI
0.7.5 reports incompatibility with Dioxus 0.7.9; resolving that tooling mismatch
remains open. No dependencies, file associations or installed app were changed.

This is synthetic-input functional acceptance, not release performance,
physical pen, or first-visible-pixel evidence; p50/p95/p99 were not measured.
The subsequent Close and process-kill work below closes the corresponding
software acceptance items; manual interaction and hardware evidence remain separate.

## 2026-09-05 responsive Close and PNG recovery

The parent Windows subclass now retains the shell when Close starts. Both
bounded admission lanes stop at an explicit boundary; previously admitted input
and Save commands still drain. The native child hides for a WebView progress
dialog. One close thread owns the retiring canvas, seals healthy active input,
processes pending Save, and joins the project writer. The window thread uses a
50 ms completion timer. Repeated Close cannot bypass pending work or dismiss a
failure. Unexpected native destruction retains the blocking final join fallback.

Success closes automatically. A failed project commit or PNG export keeps the
window and recovery path visible, distinguishing whether the project was saved.
The user can reopen the last durable project and retry Save, or acknowledge the
error and close. Reopen does not claim to reconstruct uncommitted failed work.
New file activation during closing is rejected with a notice. Input, painting,
and semantic editor state stay outside the UI's progress mailbox.

PNG encoding now explicitly calls `finish`: previously, Drop could ignore an
IEND or buffered flush failure and report an incomplete export as successful.
Sync and generation-checked replacement now require successful encoder completion.
The 57 workspace tests include regressions for final flush failure and for
preserving admitted work while rejecting late input/commands at Close.

`desktop_export_recovery_smoke` exercises the actual desktop on owned scratch
projects. It pauses after encoding, after sync, and immediately before/after
replacement, then kills only its spawned process. Previous PNG bytes must survive
the first three boundaries; after replacement the latest full canvas pixels must
match the durable stroke. Separate-process reopen retains exactly snapshot 1 and
its tiles/history and never promotes the interrupted temporary file or rewrites
the PNG automatically. A second case holds generation 2 after encoding, accepts
Save generation 3, and verifies the old encoded file is discarded before replacement.

An actual Windows sharing lock also denies PNG replacement. The probe checks
that the closing window stays visible and answers `WM_NULL`, the WebView dialog
mounts, failure retains the window, the project remains durable and the old PNG
is unchanged. It releases the lock, invokes the same child handler as the dialog's
reopen action, and retries through the ordinary native Save keyboard route.
No startup stroke or Save probe is replayed during recovery. The final PNG pixels
and reopened project are exact. This establishes functional wiring and message
responsiveness, not visual layout, screen-reader, real keyboard/pen acceptance,
power-loss, or filesystem-cache guarantees.

Additional reproduction:

On the same Windows 11 / Core Ultra 7 155H / Intel Arc DX12 host, both debug and
DX-bundled release passed the export recovery probe. The final release bundle
also passed the full durability/reopen probe. All 57 tests and all-target Clippy
with `-D warnings` passed; no latency percentiles were measured.

```powershell
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build -p nyatidraw-desktop --locked
cargo run -p nyatidraw-desktop --example desktop_export_recovery_smoke --locked
dx build --release --windows --renderer webview --package nyatidraw-desktop --locked
$env:NAYATI_DESKTOP_SMOKE_BINARY = (Resolve-Path target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe).Path
cargo run -p nyatidraw-desktop --example desktop_durability_reopen_smoke --locked
cargo run -p nyatidraw-desktop --example desktop_export_recovery_smoke --locked
Remove-Item Env:NAYATI_DESKTOP_SMOKE_BINARY
```

The scratch pause requires both opt-in environment values and a sentinel in the
matching temporary directory; it times out after 30 seconds if not killed/released.
Interrupted temporary files can remain, but reopen does not treat them as artwork.
Local logs: `target/close-tests.log`, `close-clippy.log`, `close-durability.log`,
`export-recovery-debug.log`, `close-release-build.log`, `close-durability-release.log`,
and `export-recovery-release.log`. The DX 0.7.5 / Dioxus 0.7.9 mismatch remains a
tooling gate despite the emitted bundle. See [ADR-0009](decisions/ADR-0009-close-and-export-recovery.md)
for ownership and exact dependency details.

## Still-open release gates

- Physical Windows pen acceptance through the live canvas mapping, including
  begin/end/cancel under panel interaction and real DPI/resize changes.
- Physical acceptance of pen interaction in the Dioxus Desktop WebView and
  raw pen delivery in the child canvas remains open. Native mouse ink reaches
  the new HWND path, but cursor-to-ink trailing delay remains a performance gate.
- First-visible-pixel and input-to-present p50/p95/p99 measurement, including
  high-refresh display evidence and 4K/8K stress.
- WebView pointer drag/drop, child-canvas focus, resize/DPI geometry, and modal
  z-order need post-migration manual acceptance. The earlier Dioxus Native
  acceptance remains historical evidence for the semantic command protocol,
  not for the current shell.
- Manual Close dialog layout, focus/accessibility, real capture release, and
  installed-shell acceptance remain open. The scratch process-kill/retry checks
  above do not establish power-loss or arbitrary filesystem-cache recovery.
- A failed PNG export exposes a retry action that queues Save again. Save queues a
  CPU layer-tree PNG export behind the latest durable project work, writes a
  sibling temporary file, syncs it, and performs a Windows atomic replacement.
  UI observes separate `Idle`/`Waiting`/`Queued`/`Running`/`Current`/`Failed` completion
  status; a generation gate rejects queued stale work and rechecks an encoded
  stale job immediately before replacement.

## 2026-09-05 explicit desktop history branches

The history panel now sends `RedoTo(HistoryNodeId)` for a direct child of the
current cursor. The writer validates the branch, loads its immutable tiles,
persists the cursor, then accepts it in the live session. Redo siblings remain
in the database. `ShowRedoBranches` projects 64 candidates at a time from a
stable BTree range; paging does not change artwork or the durable cursor.
Ancestry rows also stay within their 64-entry bound, including the initial row.

History commands reject while active/retained/pending strokes or PNG jobs are
outstanding. Completed stroke payloads are drained before moving the cursor;
if that changes the semantic revision the command is rejected as stale. This
prevents a cursor move from overtaking the closed-stroke backlog or a late
completion from repainting the old cursor. The existing synchronous request/reply
for idle history operations remains; this is not proof of the Sprint 3 hot-path
isolation/latency gate.

Fresh sessions now retain their initial cursor, fixing first-edit Undo before
reopen. One core regression test covers initial Undo and retained redo artwork;
workspace tests total **58**, with all-target Clippy `-D warnings` passing.
DX 0.7.9 release was installed and `desktop_export_recovery_smoke` passed using
the installed binary on Windows 11 Home 10.0.26200 / Core Ultra 7 155H / Intel Arc
integrated / DX12. The scratch fixture retains 66 siblings, pages beyond 64,
selects branches 67 then 2 across restarts, preserves invalid/ambiguous selections,
and compares saved pixels/export/cursor/node count. A fresh PNG import is undone
before restart and explicitly redone after restart. Existing PNG/crash/export
recovery cases also pass. Evidence: `target/installed-history-final.log`,
`target/history-final-tests.log`, `target/history-final-clippy.log`.

These are semantic command and process-restart results; branch-button visual
interaction, physical pen, visible-pixel latency, and per-snapshot layer metadata
restoration are not established by this probe.

## Test policy

Automated tests target only core regressions that can corrupt artwork, lose
stroke transitions, break deterministic brush/history behavior, or violate
project recovery. Small table-driven invariants are preferred. UI layout,
simple constructors, framework wiring, and OS pass-through wrappers are not
test targets; compilation, Clippy, focused runtime acceptance, and measurements
are the appropriate evidence for those seams.

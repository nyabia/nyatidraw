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
- Close-progress UI and crash-point export recovery remain open. A failed PNG
  export now exposes a compact retry action that queues Save again. Save queues a
  CPU layer-tree PNG export behind the latest durable project work, writes a
  sibling temporary file, syncs it, and performs a Windows atomic replacement.
  UI observes separate `Idle`/`Queued`/`Running`/`Current`/`Failed` completion
  status; a generation gate rejects queued stale work and rechecks an encoded
  stale job immediately before replacement.

## Test policy

Automated tests target only core regressions that can corrupt artwork, lose
stroke transitions, break deterministic brush/history behavior, or violate
project recovery. Small table-driven invariants are preferred. UI layout,
simple constructors, framework wiring, and OS pass-through wrappers are not
test targets; compilation, Clippy, focused runtime acceptance, and measurements
are the appropriate evidence for those seams.

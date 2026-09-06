# Sprint 3 — 매일 쓰는 개발판

## 2026-09-06 판정

**장시간 사용과 hot-path 지연 gate는 미완료**다. Toolbar/side stack·layout 복원,
bounded mouse capture/cancel, 비동기 history/layer와 초기 포커스 실패 처리를
검증했다. 그러나 UI thread가 여전히 surface acquire/present를 수행한다.
저장소 국소 개선을 전체 Undo나 physical pen 통과로 계산하지 않는다.
[현재 구현·gate·실행 순서](../status-plan-2026-09-06.md)를 따른다.

## 사용자 결과

Sprint 1~2의 편집기를 오래 켜 두고 반복 사용해도 입력, 창, docking, 저장 상태가
무너지지 않는다. 실패하면 그림을 잃지 않고 원인과 복구 행동을 보여주며, 다음날
PNG `Open with` 또는 `.ntdr` 더블클릭으로 같은 작업을 계속할 수 있다.

## 실행 순서

1. top/left/right docking, vertical stack/fill, cyan insertion marker와 layout 복원을
   완성한다. bottom docking은 제외한다.
2. fixed action과 dockable toolbar를 분리하고 panel 이동 중에도 child HWND, GPU device,
   project session과 active stroke authority를 유지한다.
3. 도구 group의 `G`/`B`/`E`/`F` 반복 순환, active/disabled 표시와 세부 도구
   projection을 완성한다. 장식 control을 남기지 않는다.
4. 실제 navigator, branch history와 layer thumbnail을 장시간 변경에도 낮은 빈도로
   갱신하고 hot input path에서 분리한다.
5. 같은 `.ntdr`의 두 번째 writer는 file lock으로 이미 거부된다. Windows primary는
   named mutex/pipe로 second process의 activation을 받아 writer 하나만 유지한다.
6. PNG `Open with`와 `.ntdr` activation에서 이미 열린 문서를 다시 요청하면 기존 instance로
   routing하고 best-effort restore/foreground를 요청한다. 다른 project는 old writer/export
   FIFO를 drain/join한 뒤 UI-thread에서 정확한 pair rule로 교체한다.
7. maximize-on-pointer-monitor, minimize/restore, resize/DPI, modal z-order와 종료 진행
   상태를 Windows에서 안정화한다.
8. project saved/export pending/export failed/recovered 상태를 구분하고 retry를 제공한다.
9. startup, raw-input-to-present, undo/reopen, 4K export가 drawing에 주는 영향을 release
   profile로 측정하고 병목만 최적화한다.

## 완료 gate

- Windows release 앱을 장시간 사용하며 panel 이동, 창 resize/minimize, pen drawing,
  Save를 반복해도 crash·입력 단절·project authority 재생성이 없다.
- 비정상 종료 뒤 마지막 durable snapshot을 열고 export 실패가 project를 손상시키지
  않는다.
- 사용자별 development install 갱신과 제거가 `.ntdr`과 PNG를 삭제하지 않는다.
- paired PNG나 같은 `.ntdr`을 다시 열어도 두 writer가 생기지 않고 기존 창이 활성화된다.
- tool shortcut cycle, navigator, history, layer thumbnail 중 어느 것도 raw input cadence를
  막지 않는다.
- 측정값은 hardware, OS, backend, profile, p50/p95/p99와 함께 기록한다.

## 이미 확보한 편집기 기반

### 현재 상태

**Windows-first 엔진 기반과 제한된 설치판 시나리오는 검증됐으며 전체 Sprint 완료를
뜻하지 않는다.** UI 셸은 Dioxus Desktop이다. `ViewportTransform`은 physical window, logical
canvas, document 좌표의 pan/zoom/rotation/DPI/revision을 검증한다. Windows recorder는
Begin mapping을 고정하고 native admission은 renderer-owned canvas geometry와 같은
affine snapshot을 사용한다.

`LayerTree`는 raster/group, visibility, opacity, reorder와 cycle validation을
제공한다. Raster tile 변경은 같은 coordinate의 ancestor group/root cache만
무효화하고 sibling/other-coordinate cache를 보존한다. `DockTree`는 Canvas/Tools/
Brush/Navigator/Color/Layers/History와 세 toolbar entry를 각각 한 번만 포함하는
bounded split/tab 및 top-list 모델이며 decode 또는 validation failure는 partial salvage 없이 complete safe
default로 교체한다. Safe default는 compact action bar 아래에 tool/brush, canvas,
navigator/color/layer columns를 배치한다.

`UiProjection`과 typed command/event contract는 backend-neutral core에 있다. Native
input samples, raster pixels, GPU handles는 그 protocol에 표현할 수 없다. Desktop
shell은 실행 시점의 authoritative revision으로 bounded `CommandEnvelope`를 보내고,
renderer는 `EditorEvent`와 monotonic projection을 publish한다. LayerTree는 versioned
wire record로 redb에 즉시 저장되며 visibility/opacity/cross-parent reorder가
process restart/reopen 뒤 exact equality를 유지한다. Corrupt checksum/tag/trailing
payload는 파일을 고치지 않고 scoped `Corrupt`로 거부한다.

### 역사적 GPU compositor/viewport evidence

초기 Native slice의 `GpuCompositeScene`은 당시 Dioxus가 제공한 하나의
`Device`/`Queue`를 사용했다. 아래 기록은 1024x768 document-space raster/group/root textures와 128x128
coordinate `CompositeCache`를 사용한다. Cache miss만 bottom-up recomposite하며,
resize 또는 affine change는 disposable display texture만 바꾼다. Closed CPU replay
tile은 renderer layer로 upload되므로 closed artwork의 유일한 representation이 GPU가
아니다. 현재 Desktop은 별도 native host가 GPU를 소유하고 실제 page 크기를 사용하며,
이 과거 fixture 크기를 현재 앱의 고정 크기나 지원 한계로 해석하지 않는다.

`gpu_layer_viewport --release` Windows 11 build 26200, RTX 3080/Vulkan 기록은
premultiplied source-over, nested-group/root의 incremental rebuild, visibility/
opacity/reorder, `pan=(384,-128)`, 2x zoom, 90-degree rotation sampling을 RGBA8
channel tolerance 2 안에서 확인했다. 8K/20-raster scene은 512 MiB persistent-surface
budget 전에 거부됐다. 같은 probe는 투명 display projection의 두 checker cell을
GPU readback으로 확인한다. Checkerboard는 document tile을 변경하지 않는다. 이는
real adapter/device submission/readback evidence이며
compositor timing percentile, physical pen, first present, screenshot evidence가 아니다.

Desktop synthetic smoke는 shared device에 two-raster/nested-group scene과 display
texture를 만들고 synthetic ink/affine submission을 validation error 없이 수행했다.
Synthetic input sample count is zero; it must not be used as pen or latency proof.

### 이전 Dioxus Native acceptance

최신 DX Native build의 실제 Windows 창에서 compact shell과 checkerboard를 관찰한
뒤 Zoom r3 → layer Hide r4 → Canvas Bottom dock r5 → post-dock Rotate r6 →
`Add white background` r7 → Fit r8을 실행했다. 모든 명령이 순차 승인됐고 revision,
viewport, untitled recovery repository가 DockTree remount를 넘어 유지됐다. 흰 배경은
view theme가 아니라 existing ink 뒤의 durable `StructuralChange`이며 재열기 core
invariant가 prior ink와 48개 background tile의 exact bytes를 확인한다.

### Dioxus Desktop migration evidence (2026-09-02)

Dioxus Native/Blitz 셸은 실제 사용 중 crash와 불완전한 UI pointer 경로 때문에
[ADR-0008](../decisions/ADR-0008-dioxus-desktop-child-canvas.md)에 따라 교체했다.
일반 Dioxus Desktop WebView는 UI만 소유하고, 별도 child HWND가 raw
`WM_POINTER`/mouse history와 WGPU surface를 소유한다. 펜 sample과 artwork pixel은
WebView IPC를 통과하지 않는다.

`dx build --windows --renderer webview --package nyatidraw-desktop --locked`와 desktop
Clippy가 통과했다. RTX 3080/DX12에서 child surface first-present 로그를 얻었고,
802-sample Windows mouse stroke가 한 번 닫혀 zero-blocking materialization enqueue와
untitled redb commit까지 진행됐다. Checkerboard visual, physical pen, live DPI/resize,
modal z-order는 post-migration manual acceptance가 남아 있다.

Windows startup은 `with_maximized(true)`의 OS 기본 모니터 선택에만 의존하지 않는다.
`with_on_window`에서 tao parent HWND를 얻은 직후 `GetCursorPos` →
`MonitorFromPoint` → `GetMonitorInfoW(rcWork)`로 작업 영역을 확인하고, 복원 사각형을
`SetWindowPos`로 해당 모니터에 둔 뒤 `ShowWindow(SW_MAXIMIZE)`를 요청한다. 어느 Win32
호출이 실패해도 로그만 남기고 Dioxus의 기본 최대화 경로로 계속 시작한다. 실제 다중
모니터 수동 검증은 아직 미검증이다.

## 목표 UI 계약

[편집기 UI 목표](../editor-ui-target.md)가 Sprint 3 화면과 상호작용의 권위다. 현재
배치는 유지하되 장식 상태의 컨트롤을 완료로 간주하지 않는다. 활성으로 보이는
도구는 실제 command와 authoritative projection에 연결하고, 범위 밖 기능은 명확히
비활성화한다.

캔버스는 유한한 출력 페이지와 무한한 signed 작업공간을 분리한다. 체크무늬와 검은
경계는 페이지에만 적용하고 바깥 stroke는 프로젝트/history에는 보존하되 export,
navigator, layer thumbnail에서는 자른다. 유한 스크롤바 대신 휠, 가운데 드래그,
Space+펜/마우스, Move 도구로 패닝한다.

## 재정렬 뒤 Sprint 3 범위

도구·색·무한 작업공간·sibling PNG는 Sprint 1로, layer/navigator/history는 Sprint
2로 앞당겼다. Sprint 3는 이미 쓸 수 있는 편집기가 장시간 사용과 창 상태 변화에서
망가지지 않도록 하는 단계다.

### 3A — 도킹과 native canvas 수명

- burger부터 redo까지 고정 구간으로 유지한다.
- 이후 canvas action, viewport, color/recent-color 도구는 top/left/right에 도킹할
  수 있는 별도 entry가 된다.
- top은 제목 없는 세로 grip과 가로 inline 배치, side는 위에서 쌓고 마지막 panel이
  남은 높이를 채운다. bottom drop target은 제거한다.
- drag/capture와 cyan insertion marker를 완성하고 재배치 중 child HWND, GPU device,
  project repository를 재생성하지 않는다.

### 3B — 파일 activation과 창 수명주기

- `.ntdr` double-click과 second activation에서 이미 동작하는 file lock을
  single-instance routing과 연결한다.
- named pipe는 path 전달만 하고 raw input/document pixels/project DB를 소유하지 않는다.
  inbound path는 bounded UTF-16 framing으로 읽고, target non-empty validity/PNG decode를
  old project quiesce 전 preflight한다. target failure는 old project reopen과 visible
  notice로 fail-closed 한다.
- pointer-monitor 최대화 시작, minimize/restore, live DPI/resize와 modal z-order를
  안정화한다.
- app update/close가 active stroke와 materialization/export barrier를 우회하지 않게
  한다.

### 3C — 실패와 복구 UX

- saved, export pending, export failed, recovered, read-only/locked project를 구분한다.
- retry 가능한 실패와 artwork 보호를 위해 중단해야 하는 실패를 구분한다.
- invalid non-empty project와 GPU/queue exhaustion에서 silent fallback을 금지한다.

### 3D — 측정과 통합 acceptance

- release profile에서 startup, input-to-present, undo/reopen, 4K export 간섭의
  p50/p95/p99를 기록한다.
- panel rerender, resize/minimize, pen drawing, Save/export를 장시간 반복한다.
- physical pen/high-refresh 증거는 장비가 없으면 미검증으로 남기되 synthetic 결과로
  대체하지 않는다.

## 남은 gate

- Toolbar top/side 배치와 layout restart는 ADR-0019/0020, bounded mouse
  drag/cancel은 ADR-0021에서 수용했다. Held-drag cyan marker의 시각 확인과 실제
  OS focus/capture 상실, 장시간 상호작용은 남아 있다.
- Native child canvas 위에 WebView modal/overlay가 필요한 경우 child를 잠시 숨기는
  명시적 z-order protocol이 필요하다.
- Physical pen under panel rerender, live resize/DPI, and first-visible/present
  timing acceptance.
- Development install update/remove의 소유권·artwork 보존은 ADR-0033에서 검증했다.
  실제 Explorer acceptance와 recovery dialog focus/capture는 남아 있다.
  설치된 release의 positional activation
  probe에서는 secondary exit, primary PID 유지, 이전 project unlock, 대상 project의
  `Locked` 상태까지 확인했다. Explorer `Open with`와 실제 foreground 결과는 아직
  수동 미검증이다.
- Startup/Undo/reopen 및 현재 16-bit PNG export 간섭 분포, UI thread surface 대기
  격리와 장시간 resize/minimize/Save acceptance가 남아 있다.
- Zoom-out mip, advanced blend modes, multi-window docks, vector/text/workspace sync는
  Sprint 4 이후 범위다.

## Test scope

Focused core tests cover composite invalidation/artwork ordering, corrupt dock
layout recovery, and required-panel reachability. UI layout/framework tests are
intentionally not added; desktop behavior requires compile/Clippy plus runtime
acceptance on the target hardware.

# Sprint 1 — Godot asset 즉석 스케치

## 사용자 결과

개발판을 현재 사용자 계정에 설치하고 Godot project의 PNG를 Windows에서 NyatiDraw로
연다. exact sibling `.ntdr`이 있으면 그것을 열고, 없으면 PNG를 보존한 새 project로
시작한다. 게임용 러프·인체 구조·UI 스케치를 페이지 안팎에 그리고 Save하면 두
파일이 pair로 유지된다.

## 실행 순서

1. 첫 positional argument로 `.ntdr` 또는 `.png`를 받는다. `.ntdr` 직접 열기는
   missing/zero-byte만 초기화하고 non-empty invalid 파일은 보존한다.
2. PNG activation은 exact sibling `.ntdr`을 찾는다. valid이면 project를 열고,
   absent/zero-byte이면 PNG를 첫 raster layer로 가져와 복구 가능한 project를 즉시 연다.
   non-empty invalid sibling은 오류로 중단한다.
3. 사용자별 development install과 `.ntdr` handler, `.png` Open With application
   등록을 제공한다. PNG 기본 앱과 `UserChoice`는 바꾸지 않는다.
4. 활성 도구를 Move, Pencil, Pen, Brush, Eraser에 연결한다. Pencil/Pen/Brush는 같은
   round engine의 서로 구별되는 preset이어도 된다. 크기·불투명도·현재색·최근색을
   실제 stroke snapshot에 반영하고 나머지 도구는 disabled다.
5. infinite workspace의 wheel/Shift+wheel/middle/Space/Move pan, pointer-centered
   zoom, Fit, 1:1을 완성한다.
6. Save가 `.ntdr` durable commit 뒤 sibling alpha PNG export를 FIFO worker에 예약한다.
   직렬 순서와 sibling temporary replacement로 이전 저장이나 부분 파일이 최신 PNG를
   덮지 않게 한다.
7. Godot가 열려 있다면 일반 file watcher/importer가 완성된 PNG 변경을 감지할 수
   있어야 한다. Godot 상태는 project 저장 성공 조건이 아니다.
8. Close가 active stroke를 정리하고 최신 project/export만 기다린 뒤 종료한다.

## 완료 gate

- 설치된 release executable이 PNG `Open with` 후보에 나타나고 `player.png` open ->
  sibling 확인/import -> draw -> undo/redo -> save -> close -> 같은 PNG로 pair reopen이
  한 번에 통과한다.
- `.ntdr` 직접 열기도 같은 project activation 경로로 통과한다.
- 페이지 밖 artwork는 project에 남고 PNG에는 나타나지 않는다.
- PNG만 있거나 sibling이 0-byte이면 원래 pixels가 보이는 새 project와 durable `.ntdr`
  복구 원본을 만들되 첫 Save 전에는 원래 PNG를 변경하지 않는다.
- non-empty invalid sibling은 보존하고 PNG import fallback을 만들지 않는다.
- 메뉴·열기·미구현 도구가 동작하는 것처럼 보이지 않는다.
- physical pen 결과는 실제 장비로 확인한다. first-visible percentile은 측정 장비가
  부족하면 미검증으로 남길 수 있지만 사용자가 체감할 수 있는 끊김은 gate 실패다.

## 이미 확보한 엔진 기반

### 현재 상태

**소프트웨어 경로 구현, 실물/지연 gate 미종료.** Windows desktop에는 Win32
`WM_POINTER` recorder, canvas-origin/DPI/affine viewport snapshot, bounded sample
queue와 transition retry lane, pressure-aware round-brush evaluation, shared
Dioxus-supplied `Device`/`Queue`의 GPU mutation path가 있다. Raw samples는
Dioxus state를 통과하지 않는다.

`Begin` 때의 viewport mapping은 active pointer에 고정되어 Move/End/Cancel이
다른 revision을 섞지 않는다. layout/surface mapping이 불완전하거나 canvas 밖이면
admission은 fail-closed다. 이 구현은 real-device pen acceptance가 아니라
coordinate/queue software invariant다.

실사용 점검에서 shell gap이 발견되어 보완했다. 일반 mouse left drag는
pressure 1.0 sample로 native queue에 들어간다. Windows에서는 raw hook이
`GetMouseMovePointsEx`의 최대 64개 display-point history를 anchor 이후부터
oldest-first로 복원하며, stale global history와 pen/touch-promoted mouse를
거부한다. Canvas 밖에서 시작한 Windows
pen/touch gesture는 Blitz가 처리할 수 있는 UI mouse transition으로 변환된다.
같은 render drain의 연속 Move dab은 generation별 한 GPU batch로 합쳐진다.
Raw pen Update는 Win32가 한 message에 합쳐 둔 history를 oldest-first batch로
복원하며, HIMETRIC 좌표를 fractional client pixel로 변환한다. Hover, non-pen,
orphan Move, cancelled pointer도 core adapter 경계에서 명시적으로 처리한다.
이 변경의 물리 펜 UX와 first-visible latency는 아직 사용자/hardware acceptance
대기 상태다.

2026-09-01 physical mouse release acceptance에서 첫 normalized high-resolution
좌표 해석은 Move를 canvas 밖으로 보내 점만 남기는 실패로 확인되어 폐기했다.
Display-point history로 전환한 다음에는 한 stroke에 3,482 sample, 다른 stroke에
594 sample이 보존됐고 사용자가 눈에 보이던 꺾임 제거를 확인했다. GPU drain은
대부분 16-19 ms 간격이었다. Cursor보다 ink가 뒤따르는 지연은 남아 있으므로
continuity만 수용하고 input-to-present gate는 닫지 않는다.

### 기록된 증거

- 240 Hz deterministic fixture와 20분 virtual producer overload는 bounded queue,
  coalescing, pressure extrema 보존, transition retry lane의 소프트웨어 계약을
  다룬다. Desktop의 2C+1 transition probe는 C=512 primary와 C=512 retry를 먼저
  보존하고 sequence 1025에서 discontinuity를 한 번 latch했다. Active partial
  stroke는 GPU Cancel되고 materialize되지 않았으며 clean Begin에서 복구됐다.
  이는 bounded fail-closed software evidence이지 물리 입력 손실 없음의 증명이 아니다.
- `gpu_cpu_diff --release`의 여섯 closed-stroke fixture는 RTX 3080/Vulkan에서
  RGBA8 channel delta 4 이하, 초과 pixel 0으로 기록됐다. 다른 backend/GPU/driver,
  장기 build-up, shader/blend 변경은 이 결과의 범위 밖이다.
- synthetic desktop/GPU smoke는 device/queue submission과 redraw 배선을 확인한다.
  synthetic sample 또는 GPU submit 로그는 physical pen, visible pixel, present
  timing의 증거가 아니다.
- Windows input core의 14개 focused test는 pen/mouse coalesced history 순서,
  transition 비확장, hover/orphan Move 거부, cancel, pen 4,096-sample/mouse
  64-sample 상한, promoted-pointer filtering, fractional/negative display 좌표를
  다룬다. OS/driver가 실제 history를 공급하는지는 release executable과 물리
  입력으로 별도 확인한다.

### 남은 engineering evidence

- 실제 펜으로 canvas admission과 document-coordinate mapping을 수용하고,
  begin/end/cancel 및 압력·고곡률 변화가 UI 조작 중에도 보존되는지 확인한다.
- physical pen부터 first-visible-pixel/present까지의 release p50/p95/p99를
  고정 fixture와 고주사율 display에서 측정한다.
- 4K/8K, 장시간 build-up, panel interaction 중 input-to-visible budget과 GPU/CPU
  strategy 비용을 측정한다.

### 2026-09-03 사용 가능성 체크포인트

- positional `.ntdr` 직접 열기와 `.png` -> exact sibling `.ntdr` activation을 연결했다.
- PNG decode가 성공한 뒤 imported raster/history/canvas metadata를 redb에 즉시 보존하고,
  non-empty invalid sibling은 기존 `ProjectDb` fail-closed 경로를 그대로 사용한다.
- Save는 최신 durable FIFO 뒤 CPU layer/group composite를 만들고 sibling temporary PNG를
  완전히 encode/sync한 다음 Windows atomic replacement로 게시한다.
- Pencil, Pen, Brush는 서로 다른 round preset으로 실제 stroke snapshot에 기록된다.
  크기, 불투명도, 현재색/최근색도 다음 Begin의 GPU/CPU stroke에 함께 고정된다.
- Eraser는 입력 sample의 기존 eraser bit를 semantic stroke에 고정하고, GPU preview와
  deterministic CPU materialization에서 같은 destination-out 합성을 사용한다. 별도 project
  wire 변경 없이 저장/reopen되며 premultiplied RGB/alpha를 함께 감소시킨다.
- Undo/Redo는 immutable history cursor를 redb에 먼저 반영한 뒤 CPU tile authority와 GPU
  display를 함께 교체한다. branch가 여럿이면 자동 선택하지 않고 거부한다.
- wheel/Shift+wheel/middle-drag, Ctrl+wheel pointer-centered zoom, Fit, 1:1이 연결됐다.
- `tools/install-dev.ps1`과 `tools/uninstall-dev.ps1`이 DX release asset bundle과 현재 사용자
  범위 `.ntdr`/PNG Open With 등록을 관리한다.
- release 실행으로 1920x1080 PNG 135 tiles import -> process 종료 -> `.ntdr` reopen을
  확인했다. 최초 import 첫 present는 약 2.0초, reopen은 약 1.44초였으며 RTX 3080/DX12,
  Windows 현재 장비의 단일 관측값이다.
- RTX 3080에서 paint와 eraser GPU/CPU diff가 허용된 RGBA8 UNORM channel delta 4 안에
  들었고, 24 paint + 8 eraser stroke를 close drain한 뒤 snapshot 32를 process restart로
  재열었다. redraw wake latch도 각 render 시작에 소비해 후속 command/materialization
  wake가 사라지지 않게 했다.

아직 남은 Sprint 1 gate는 실제 펜 재확인, UI에서 draw/erase/undo/redo/save/close 전체
수동 흐름, 설치된 Open With shell surface 확인이다. Move와 Space/middle drag는 mouse와
pen viewport path에 연결했다. 마법봉/올가미/채우기/그라데이션과 열기 버튼은 동작하는
척하지 않도록 disabled다.

### 범위와 보존 규칙

닫힌 stroke는 CPU replay로 immutable tile/root로 materialize하고 redb에 durable
commit할 수 있다. 그것은 Sprint 2 durability seam이며, GPU pixels만을 closed
artwork의 권위로 쓰지 않는다. GPU readback/hybrid checkpoint strategy는 아직
`UnsupportedStrategy`이고 CPU replay로 조용히 대체하지 않는다.

텍스처 브러시, advanced blend mode, sparse atlas, high-refresh/physical-pen proof는
이 Sprint의 완료 증거가 아니다.

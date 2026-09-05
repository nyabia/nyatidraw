# 2026-09-05 현황 점검과 실행 계획

기준 커밋은 `fa9603c`이며 점검 시작 시 작업 트리는 깨끗했다. 이 문서는
코드·기존 증거·새로 수행한 검증을 구분한다. 처음에는 현황을 점검했으며,
이후 개발 요청에 따라 저장·종료 경로를 수정했다. 아래 표와 실행 순서는 최초
점검 기준이며, 같은 날 후속 개발 결과는 마지막 절에 별도로 기록한다.

현재 판단은 **Windows 엔진 기반과 주요 편집 경로는 구현됐지만 Sprint 1~3의
사용자 완료 gate는 아직 닫히지 않았다**이다. 기존 문서의 “software vertical
slice accepted”는 재정렬된 제품 스프린트 전체 완료를 뜻하지 않는다.

## 구현 상태

| 영역 | 코드에서 확인한 상태 | 남은 작업 또는 증거 |
|---|---|---|
| 입력·브러시 | Win32 child canvas, pen/mouse history, bounded primary/retry lane, discontinuity quarantine, round presets/eraser | 실제 펜, DPI/resize, 체감 추종 지연과 표시 지연 측정 |
| 문서·저장 | signed sparse tiles, CPU closed-stroke authority, redb immediate commit, hash/root/history 검증 및 reopen | active stroke Save/Close 정책, 사용자에게 보이는 종료 진행·오류 |
| PNG 왕복 | positional PNG/ntdr activation, sibling 탐색/import, page crop export, 임시 파일 교체·generation gate | 설치 앱/Explorer/Godot 왕복 및 export crash-point 복구 검증 |
| Export 상태 | queued/running/current/failed 및 실패 시 Save 재요청 버튼 | 재시도 실사용 검증, project/export/recovery 상태의 일관된 의미 |
| 레이어 | raster/group 추가·rename·visibility·opacity·reorder·Solo와 실제 thumbnail | 삭제, drag reorder UI, durable Reference metadata |
| 히스토리 | 분기 보존 저장, current branch/cursor 표시, Undo/Redo | 명시적 분기 선택 UI/command |
| Viewport | pan/zoom/rotation, signed workspace, finite page, navigator 이동·viewport box | live DPI/resize 및 다양한 크기에서 통합 검증 |
| 편집 도구 | Move/Pencil/Pen/Brush/Eraser 활성 | Wand/Lasso/Fill/Gradient, 기본 transform, page resize/crop |
| 도킹 | bounded split/tab 모델, 패널 이동, top/left/right drop 표시 | 상단 toolbar 실제 이동, pointer capture, stack/fill, 재시작 layout 복원 |
| 창·activation | named mutex/pipe, second-process routing, target preflight, old worker drain/join | Explorer foreground, 다중 모니터, minimize/restore, modal z-order, 설치 갱신·제거 acceptance |
| 성능 | 일부 단계 로그와 GPU/queue 전용 probe | 현재 WebView 셸의 release p50/p95/p99 및 장시간 입력/export 간섭 측정 |

핵심 코어의 Cargo 의존 경로를 확인한 범위에서 `document/input/brush/tiles/history`에
Dioxus, wgpu, redb, OS adapter 의존은 발견하지 않았다. raw sample과 GPU handle은
`EditorCommand`에 들어가지 않는다. 다만 구조 분리 자체를 실제 latency 통과로
판정하지 않는다.

## 코드와 문서에서 특히 주의할 차이

- `apps/desktop/src/main.rs`의 `ActionBar`에는 export retry가 이미 있다.
  `sprint-02-document.md`의 “retry 없음”은 갱신이 필요하다.
- `crates/editor/src/lib.rs`에는 `prepare_redo_to_cursor`가 있지만
  `crates/api/src/protocol.rs`의 `HistoryCommand`는 Undo/Redo뿐이다.
  분기 지점에서 사용자가 redo 대상을 고를 수 있도록 연결해야 한다.
- `main.rs`의 상단 toolbar grip은 표시되지만 `PanelKind`에는 그 toolbar entry가
  없다. 패널 도킹 구현만으로 상단 toolbar 도킹을 완료 처리할 수 없다.
- `DockTree::recover`는 유효성 복구 기반이다. 현재 desktop 초기화는
  `DockTree::safe_default()`에서 시작하며 디스크 layout 저장·로드 연결은
  확인되지 않았다. remount 유지와 프로세스 재시작 복원을 구분해야 한다.
- `native_canvas.rs`의 Save는 닫힌 stroke backlog를 먼저 처리하고 export를
  예약한다. active stroke를 강제로 닫지는 않으며 Drop은 queued input과 writer를
  drain/join한다. 이 경로를 명시적 Close 진행 상태로 확장해야 한다.
- Sprint 1/3 일부 본문에는 이전 Dioxus Native/Blitz 및 Dioxus 제공 GPU 설명이
  남아 있다. 현재 권위는 Dioxus Desktop WebView와 별도 native child canvas다.
- AGENTS의 우선순위는 Sprint 1 소프트웨어 gate → 저장/편집/셸 안정화다.
  재정렬된 Sprint 2는 선택·채우기·기본 변형까지 포함하므로, 저장 기반만 닫은
  상태를 Sprint 2 제품 완료로 부르지 않는다.

## 실행 순서와 완료 기준

### 0. 재현 가능한 기준선

기존 workspace test, Clippy, DX release build 결과를 기록한다. 기존 GPU/desktop
probe는 현재 셸에서 적용 가능한지 확인한 뒤 scratch project로 실행한다.
위의 오래된 문구와 검증 상태를 기능별로 갱신한다.

완료 기준: 명령·커밋·환경·성공/실패·검증 한계가 한곳에 기록되고, 다음 단계의
회귀 비교에 사용할 기준선이 있다. 과거 로그를 이번 실행 결과로 재사용하지 않는다.

### 1. Sprint 1 저장·종료 왕복 닫기 — 최우선

Save/Close 시 active stroke 정책을 먼저 정하고 input admission 종료 → stroke
정리 → materialization → durable commit → 최신 export → exit의 상태 전이를
구현한다. 제안은 Save를 stroke 경계까지 보류하고, Close는 정상 active stroke의
마지막 수신 sample까지 보존하도록 명시적으로 seal하는 것이다. discontinuity로
quarantine된 stroke는 살리지 않는다. 합성 End를 물리 입력 증거로 기록하지 않는다.
긴 작업은 진행을 표시하고 export 실패는 project 저장 성공과 구분한다.

완료 기준: scratch PNG의 sibling이 valid/absent/zero-byte/invalid인 네 경우,
draw/erase/undo/redo/Save/Close/process restart/pair reopen이 통과한다. 첫 Save
전 원본 PNG와 invalid non-empty project bytes가 보존된다. 페이지 밖 작품은
reopen 후 남고 PNG에는 없다. export 도중 종료/실패와 N→N+1 Save에서 기존 완전한
PNG 또는 최신 완전한 PNG를 보장한다. 최종 설치 앱은 Explorer/Godot로 확인한다.

주요 변경 위치: `native_canvas.rs`, `desktop_shell.rs`, `desktop_canvas.rs`,
`live_ink.rs`, `main.rs`. 저장 로직을 UI 상태로 옮기지 않는다.

### 2. Sprint 2 레이어와 히스토리 완성

레이어/그룹 삭제 시 active layer fallback과 undo 가능성을 정의하고 durable
작품 변경으로 연결한다. Reference metadata와 Solo의 session-only 의미를 분리한다.
drag reorder를 기존 Reorder 명령에 연결하고 explicit redo branch 선택을
command/projection/worker까지 잇는다.

완료 기준: 20 raster와 nested group에서 삭제·재배치·visibility·opacity·분기
선택 뒤 save/restart/reopen하여 tree, root, history cursor와 합성 결과를 비교한다.
Solo는 일반 PNG export와 durable visibility에 영향을 주지 않는다.

주요 변경 위치: `crates/api`, `document`, `editor`, `history`, `project*` 및 desktop UI.

### 3. Sprint 2 기본 편집 도구를 개별 절단으로 구현

Reference 참조 범위와 tolerance → Wand/Lasso 선택 → Fill/Gradient → 기본
selection/layer 이동·변형 → page resize/crop 순서로 진행한다. 각 단계에서
CPU durable 결과와 history/reopen/export를 먼저 연결한 뒤 도구를 활성화한다.
crop은 출력 영역 변경이며 signed artwork를 자동 삭제하지 않는다.

완료 기준: 각 도구의 작품 변경을 save/process restart/reopen/export로 비교한다.
PNG의 alpha 및 page crop 결과가 일치하고, 대규모 영역에서 자원 한계를 넘으면
부분 작품 변경 없이 실패한다. 고급 selection refine·filter·brush는 확장하지 않는다.

### 4. Sprint 3 도킹과 native canvas 수명

고정 action과 이동 가능한 toolbar를 모델에서 분리한다. top/left/right,
side stack/fill, insertion marker, pointer capture/cancel과 layout 저장·복원을
연결한다. WebView modal이 canvas 위를 덮어야 할 때 child HWND 가시성을 조절하는
명시적 protocol을 둔다. 구현된 도구만 shortcut cycle에 포함한다.

완료 기준: 도킹·창 resize/DPI·minimize/restore·modal 열기 중에도 child HWND,
GPU device와 project authority의 소유권이 유지된다. 재시작 layout 복원 및 손상
설정의 safe default 복구를 확인한다. bottom docking은 제품 UI 범위에서 제외한다.

### 5. Sprint 3 오류·activation·개발판 수명

saved/export pending/export failed/recovered/locked 상태와 복구 행동을 정리한다.
동일 project의 반복 activation, 다른 project 전환 중 writer/export drain,
target 오류 시 원래 project 유지 경로를 점검한다. 설치 갱신·제거는 scratch
asset을 사용해 기존 파일과 기본 PNG 앱 설정 보존을 확인한다.

완료 기준: 두 writer가 동시에 열리지 않고, 실패한 activation이 현재 작품을
잃게 하지 않으며, Explorer 경로로 기존 창 활성화와 pair reopen이 된다.

### 6. release 계측과 장시간 acceptance

간단한 입력/worker/submit 계측은 1단계부터 수집하고 최종 통합 측정은 여기서 한다.
startup, raw input/dequeue, brush batch, GPU submit/present 요청, materialization,
commit, PNG render/encode/replace를 분리한다. 4K export on/off, 짧은 stroke 연속,
panel 갱신·window 변화가 있는 release 세션에서 병목을 찾은 뒤 해당 경로만 개선한다.

완료 기준: CPU/GPU/OS/backend/profile/화면 주사율/장면 조건/p50/p95/p99를 기록한다.
GPU submit이나 present 요청은 첫 가시 픽셀 증거가 아니다. 실제 펜·고주사율 장비가
없으면 해당 gate는 명시적으로 미검증으로 유지한다. 8K/20-layer budget 거부를
8K 지원 통과로 계산하지 않는다.

## 검증 방식과 범위

자동 테스트는 입력 전이 손실, 결정적 작품 결과, history/root, invalid-file 보존,
export generation과 recovery invariant에 한정한다. UI/OS 연결에는 compilation,
Clippy와 focused runtime acceptance를 사용한다. 모든 crash/fault 실험은 scratch
project에서만 한다. 새 의존성은 실제 필요가 생긴 spike에서만 추가하고 gate 통과
후 정확한 버전을 ADR에 기록한다.

macOS, vector/Vello, animation, advanced brush, 공개 배포는 이번 계획 밖이다.
large database/migration/compaction과 정밀 편집 확장은 후속 스프린트로 유지한다.

## 이번 점검의 검증 결과

- 환경: Windows, `rustc 1.96.0`, `cargo 1.96.0`; `dx` 실행 파일 존재 확인.
- `cargo test --workspace --locked`: 통과, unit/integration 52개 성공·실패 0개.
  Debug test profile의 결과이며 physical input/present 증거가 아니다.
- `cargo clippy --workspace --all-targets --locked`: 통과, 경고 없음.
- `git diff --check`: 통과.
- DX release build, desktop/GPU runtime probe, Explorer/Godot 왕복, physical pen 및
  표시 지연 측정은 이번 점검에서 수행하지 않았다. 위 실행 계획의 검증 항목이다.
- 현황 점검 당시에는 기능 소스·의존성·설치 등록을 변경하지 않았다.

## 같은 날 후속 개발

개발을 이어가며 기존 desktop smoke를 실제 실행하자 종료 직전 32개 closed
stroke 중 5개만 저장되고 completion 큐 포화로 중단되는 결함이 재현됐다.
종료 시 display owner를 명시적으로 retire한 뒤 durable worker를 drain하도록
수정했다. 실행 중 completion 포화의 fail-closed 동작은 유지한다.

이어 discontinuity로 GPU 제출 전에 Begin이 제거되면 semantic generation과
GPU token 번호가 달라지는 결함을 수정했다. 늦은 CPU completion과 새 preview의
비교는 semantic generation으로 통일했다. 기존 probe가 초기 Fit과 같은 revision의
명령을 보내던 순서 문제도 함께 바로잡았다.

Intel Core Ultra 7 155H / Intel Arc, Windows 11 Home 26200, DX12 debug에서
32-stroke 저장 → 프로세스 종료/재실행 → 동일 root/tile 복원, layer metadata,
stale-command 거부, 입력 단절 뒤 clean Begin 복구와 invalid-file 보존을 확인했다.
자세한 증거와 한계는 [구현 현황](implementation.md)의 2026-09-05 절에 기록했다.
정상 active stroke는 종료 시 마지막 수신 sample을 그대로 복사한 semantic End로
닫아 저장한다. cancel/discontinuity stroke는 살리지 않고 recording limit과 sequence
overflow는 명시적으로 실패한다. 합성 End는 OS 입력 경로와 물리 펜 증거에 포함하지 않는다.

Save는 active stroke와 기존 closed backlog가 끝날 때까지 단일 최신 요청으로
대기한다. 요청을 받는 순간 export generation을 갱신해 이전 export의 교체를 막고,
stroke가 닫히면 writer FIFO 뒤에 최신 PNG를 예약한다. 그 전에 창을 닫으면 active
stroke를 보존한 뒤 이미 요청한 Save도 끝까지 처리한다. UI에는 저장 요청 대기를
표시한다. worker 자리가 새로 생겨도 새 stroke가 기존 backlog를 추월하지 않도록
순서를 고정했다.

추가 runtime은 raw Begin/Move 직후 종료, 연속 Save 2회 뒤 종료, 연속 Save 2회 뒤
정상 End를 각각 검사한다. 별도 앱 프로세스로 재실행해 sample/history/tile을 비교하고,
Save fixture에서는 PNG 전체 canvas pixels가 마지막 stroke를 포함하는지 비교한다.
workspace 테스트 55개와 all-target Clippy가 통과했다. debug 및 DX release 번들
모두 desktop durability/reopen smoke를 통과했다. DX build는 성공했지만 로컬
CLI 0.7.5와 Dioxus 0.7.9의 버전 불일치 메시지는 남아 있어 build tooling 정리가 필요하다.

### 이어서 구현한 종료 진행·export 복구

닫기 요청을 받으면 입력과 명령의 새 admission을 멈추고 창을 유지한다. 별도 close
thread에서 정상 active stroke와 pending Save를 정리하고 writer를 join한다. 성공 시
자동 종료하며, 실패 시 프로젝트 저장 여부와 복구 경로를 표시한다. 사용자는 마지막
durable project를 다시 열어 Save를 재시도하거나 오류를 확인한 뒤 종료할 수 있다.
WebView 진행 창이 canvas 위에 표시되도록 native child를 숨기고, 종료 중 반복 Close와
새 activation은 저장 작업을 중단하지 않는다.

PNG encoder의 Drop이 마지막 IEND/flush 오류를 무시할 수 있던 부분을 수정했다.
이제 명시적인 finish 결과가 성공해야 임시 파일 sync와 최종 교체로 진행한다.
추가 테스트는 close boundary의 기존 입력/Save 보존과 늦은 admission 거부,
마지막 PNG flush 실패를 다룬다. Workspace invariant 테스트는 총 57개다.

새 scratch runtime은 인코딩 후·sync 후·교체 직전·교체 직후 자식 프로세스를 종료해
기존 PNG bytes 또는 최신 전체 canvas pixels를 확인하고 별도 프로세스로 project를
재실행한다. 인코딩된 구세대 export가 새 Save 뒤에 폐기되는 경우도 검증한다.
Windows 파일 잠금으로 실제 교체 실패를 발생시킨 뒤, 창의 가시성·응답·진행 dialog
mount를 확인하고 durable reopen 및 native Save 단축키로 재시도한다.
최종 검증은 57개 테스트, `-D warnings` all-target Clippy, DX release build,
debug/release export recovery와 최종 release durability/reopen 모두 통과했다.
CLI 버전 불일치 메시지는 남아 있으며 실제 펜·시각적 UI 실사용·latency 수치는
이 검증에 포함되지 않는다.

다음 우선순위는 DX 도구 버전 정리와 설치 앱/Explorer/Godot PNG pair 왕복이다.
실제 펜, DPI·modal·capture 실사용, 표시 지연 gate는 아직 완료 처리하지 않는다.
설계는 [ADR-0009](decisions/ADR-0009-close-and-export-recovery.md)를 따른다.

### DX 버전 고정 완료

프로젝트 전용 CLI 0.7.9 setup과 build wrapper를 추가하고 개발판 installer에
연결했다. 0.7.5의 사전 거부, 공식 asset을 통한 설치, setup 재실행과 0.7.9 release
bundle 성공을 확인했다. 전역 CLI와 PATH는 유지한다. 이전 DX 버전 불일치 항목은
해결됐으며, 설치 앱 및 PNG pair 왕복 확인을 이어간다. 새 자동화 테스트는 추가하지 않았다.

### 개발판 설치·제거 및 설치 binary 검증

설치 폴더 전체를 재귀 삭제하던 갱신/제거 경로를 파일 manifest/hash 기반 소유권
검사로 교체했다. 추가·수정 파일이 있는 갱신은 거부하며, 제거는 해당 파일을 보존한다.
실제 제거에서 read-only registry handle 오류를 찾아 고쳤다. 새 설치·반복 갱신·
추가 artwork/registry 보존·제거·재설치와 PNG 기본 앱 유지가 통과했다. 별도 scratch에서
수정 파일·경로 이탈·junction 거부도 확인했다.

설치된 release binary로 기존 durability/reopen과 export recovery smoke 모두 통과했다.
테스트 수는 57개를 유지하며 이번 설치 스크립트 변경은 실제 실행으로 검증했다.
상세 근거는 [개발판 설치](development-install.md#2026-09-05-설치판-실행-근거)에 기록했다.
다음은 positional PNG pair 및 Shell activation 왕복이고, Explorer 직접 조작과
Godot reimport는 아직 미검증이다.

### 설치판 PNG 왕복과 작은 이미지 Fit 수정

기존 export recovery runtime에 positional PNG activation을 연결하자 2×2 이미지의
Fit 배율이 입력 transform 상한을 넘어서 첫 canvas frame이 나오지 않는 문제가
재현됐다. Fit·휠·버튼 확대의 범위를 `ViewportTransform::MIN_ZOOM/MAX_ZOOM`으로
통일했다. 기존 unit test 수는 57개 그대로이며 전체 테스트와 all-target Clippy를
통과했다. DX release를 다시 설치한 뒤 전체 PNG/export recovery runtime도 통과했다.

없음·0-byte·정상·손상 sibling, Save 전 원본 보존, 기존 primary로 same-pair 전달,
페이지 밖 stroke의 Save/restart/reopen 및 PNG crop을 확인했다.
상세는 [PNG pair 실행 근거](godot-integration.md#2026-09-05-설치-release-png-pair-실행-근거)에
기록했다. Explorer 메뉴 직접 조작, Godot reimport와 하드웨어 증거는 미검증으로 남긴다.
이 환경에서 진행할 다음 구현은 레이어·히스토리의 남은 semantic/durable 기능이다.

### 명시적 redo 분기 선택 완료

히스토리 패널에서 현재 cursor의 직접 자식 분기를 선택하는 `RedoTo`와 64개씩
넘겨 보는 목록을 연결했다. 한 번에 전체 분기 목록을 만들지 않고 ID 순서의 bounded
range를 사용한다. history 이동은 대기 중인 stroke/export를 추월하지 않으며,
late completion을 처리하며 revision이 바뀌면 재시도를 요구한다.

새 session에서 최초 cursor를 잃어 첫 작업을 바로 Undo할 수 없던 결함도 수정했다.
이를 막는 core invariant 테스트 한 개만 추가해 총 58개이며, 전체 테스트와
all-target Clippy가 통과했다. 설치 release에서 66개 분기·페이지 경계·잘못된/모호한
선택 보존·67→Undo→2 선택·Save/restart/reopen/export, 첫 PNG import의 즉시 Undo와
재실행 뒤 Redo가 통과했다. [구현 근거](implementation.md#2026-09-05-explicit-desktop-history-branches)에
명령과 한계를 기록했다. 작업 트리의 layer metadata는 아직 current 값으로 저장되므로,
다음 단계는 삭제 및 metadata 변경을 history snapshot과 함께 복원하는 저장 구조다.
UI 직접 선택/시각 검증과 synchronous history 요청의 hot-path 분리는 남아 있다.

### 레이어 history 저장 경계 확보

현재 tree 하나만 저장하던 경로에 immutable snapshot layer metadata를 추가했다.
명시적 structural+tree commit, 이후 stroke의 tree 상속, cursor 이동과 current tree의
원자적 복원이 가능하다. 최초 metadata commit에서만 marker 2로 전환하며 기존
snapshot의 호환 tree를 한 번 고정한다. 과거에 저장되지 않은 metadata는 복원하지
않는다. 누락/손상된 과거 레코드와 current tree/cursor 불일치는 원본을 보존하고 거부한다.

필요한 core 테스트 두 개를 추가해 전체 60개 및 all-feature/all-target Clippy가
통과했다. Release scratch child를 commit 전/후에 종료한 뒤 pixel root와 tree가
함께 prior/new 상태로 복원되는 것을 확인했다. [ADR-0010](decisions/ADR-0010-snapshot-layer-history.md)에
근거를 기록했다. 이 체크포인트는 저장 경계이며 desktop 삭제 버튼이나 GPU tree
Undo 연결까지 완료한 것은 아니다. 다음 작업은 이 transaction을 layer command에
연결하고 삭제·active fallback·GPU/thumbnail/export 복원을 검증하는 것이다.

### Desktop 레이어 삭제·metadata Undo 완료

레이어 명령을 snapshot tree transaction에 연결했다. 삭제는 하위 raster의 모든
tile을 새 snapshot에서 제거하며, Undo/Redo는 기존 tree와 pixels를 함께 복원한다.
이름·visibility·opacity·추가·순서 변경도 history에 남는다. 살아 있는 raster를
active로 선택하고, 마지막 raster 삭제 시 빈 raster 하나를 만든다. Solo와 active
선택은 session 상태다. 저장된 상태를 GPU가 반영하지 못하면 workspace 오류로
추가 그리기를 막고 reopen을 안내한다.

설치 release에서 20 raster·2단계 중첩 그룹의 이름/opacity/삭제/순서 변경과 분기
Undo/Redo를 11개 별도 프로세스로 실행했다. Save와 process restart 뒤 tree, tile,
PNG 전체 픽셀이 일치했고 마지막 raster fallback도 유효했다. 기존 설치판
durability/reopen 및 PNG crash/recovery runtime도 통과했다. 전체 core 테스트는
60개를 유지하며 기존 tree 불변식만 보강했다. All-feature/all-target Clippy도 통과했다.
실행 근거는 `target/installed-layer-history.log`, `target/installed-layer-durability.log`,
`target/desktop-layers-tests.log`, `target/desktop-layers-clippy.log`에 있다.

남은 레이어 작업은 Reference metadata와 drag reorder UI다. 기본 편집 도구,
도킹/layout 복원과 synchronous metadata/history 요청의 hot-path 분리도 이어간다.
물리 펜, 직접 UI 조작과 표시 지연 gate는 계속 미검증이다.

### Reference metadata 완료

래스터별 참조 표시를 저장·history·projection·toolbar에 연결했다. 기존 파일의
layer payload version 1은 참조 꺼짐으로 읽으며 새 tree write는 version 2를 사용한다.
일반 그리기 대상·visibility·PNG는 영향을 받지 않는다. 설치 release의 20-raster
fixture에서 Reference 저장/재실행/Undo/Redo, 표시 개수와 기존 PNG 픽셀 불변을
확인했다. 호환성 위험을 막는 core 검사 한 개만 추가해 전체 61개이며 Clippy도 통과했다.
[ADR-0011](decisions/ADR-0011-reference-layer-metadata.md)에 형식과 한계를 기록했다.
선택/채우기 도구의 참조 범위와 tolerance 계약, drag reorder UI는 이어서 구현한다.

### 레이어 drag reorder와 깊이 제한 완료

썸네일을 끌어 행 위·아래 또는 그룹 안으로 이동하고 청록색 삽입 위치를 표시한다.
위·아래 버튼도 기존 Reorder 명령에 연결했다. 드래그 시작 revision과 sibling index를
사용해 source 제거 후 위치를 보정하며, 문서가 바뀐 뒤의 오래된 drop은 거부한다.
Windows에서 HTML drag를 가로막던 기본 native file-drop handler를 해제했다.

이동 후 깊이가 64를 넘어 저장 불가능한 tree가 될 수 있던 경로도 고쳤다. 후보를
검증한 뒤 채택하므로 실패해도 원본 subtree를 유지한다. 이 작품 보존 위험을 막는
core 검사 한 개만 추가해 총 62개다. 전체 테스트와 Clippy가 통과했다.

설치 release WebView의 실제 DOM/handler에 합성 drag 이벤트를 보내 그룹 내부,
위·아래, 그룹 추출, 취소와 toolbar 변경 뒤 stale drop 거부를 확인했다. 20 raster,
21회 프로세스 실행의 Save/reopen/PNG 비교와 기존 export 복구 시나리오가 통과했다.
[실행 근거](implementation.md#2026-09-05-layer-dragdrop-and-depth-preservation)에 기록했다.
실제 손으로 끌기·스크롤 중 drag와 표시 지연은 미검증이다. 다음 구현은 선택/채우기
도구의 Reference 범위·tolerance 계약과 기본 편집 명령이다.

### 기본 선택·채우기 CPU 결과 검증

Active/Reference/AllVisible source, RGBA seed tolerance와 4-connected Wand,
pixel-center even-odd Lasso, 단색 및 선형 gradient source-over를 CPU 모듈에 구현했다.
선택 밖·다른 레이어·페이지 밖·경계 tile padding을 유지하고 자원 한계는 부분 변경
없이 거부한다. 필요한 core invariant 세 개만 추가해 테스트 65개와 Clippy를 통과했다.

Scratch project를 별도 프로세스로 6회 열어 Wand fill, Lasso gradient, Undo,
새 fill branch와 기존 gradient branch Redo의 tile/tree/history 및 PNG를
독립 기대 픽셀과 비교했다. Release 검증이 통과했다. 4K CPU Wand+fill p95는
tile별 처리로 468.531ms에서 220.761ms가 됐지만 입력 스레드에 두기에는 길다.
근거와 한계는 [ADR-0012](decisions/ADR-0012-basic-selection-and-paint.md)에 기록했다.

이번 체크포인트는 CPU 편집 기반이며 desktop 도구는 아직 비활성이다. 다음은
native gesture, bounded 비동기 edit worker, 선택 영역 표시 및 durable 결과
adoption을 연결하는 것이다. 선택 중 brush/eraser clipping 계약도 별도로 완성해야 한다.

### 비동기 선택·채우기 worker 연결

선택·채우기 semantic 명령을 기존 project writer에 연결했다. 활성 편집 한 개와
buffered 응답 한 개로 제한하고 renderer는 완료를 nonblocking으로 받는다.
진행 중인 stroke와 새 Begin을 원자적으로 구분하며, 편집 중 시작된 제스처의
나머지 입력이 이후 stroke로 잘못 이어지지 않게 했다. 필요한 core 검사 한 개만
추가해 전체 66개다. 상태창에는 처리 중·선택 개수·실패와 선택 해제를 표시한다.

Windows 컴퓨터 사용 도구로 설치판을 직접 제어했다. Scratch fill worker를
대기시킨 동안 Save 유지, 확대 버튼 반응, 정상 종료 진행창을 확인했다. 이후
작업 완료·PNG 생성·writer join과 별도 프로세스의 픽셀/tree/history/PNG 비교가
통과했다. [ADR-0013](decisions/ADR-0013-asynchronous-selection-edit.md)에 근거와
제한을 기록했다. 실제 물리 펜이나 표시 지연 근거로 확대 해석하지 않는다.

Native 선택 제스처와 overlay, 선택 영역에 맞춘 brush/eraser 처리는 아직 남았다.
그 계약이 연결되기 전까지 일반 선택·채우기 도구 버튼은 비활성이며, semantic
경로로 만들어진 선택이 있는 동안은 선택 해제 전까지 brush 입력을 막는다.

최종 설치판에서 일반 완료 후 GPU 표시, 선택 해제, UI Undo → 저장/종료 → 별도
검증, 재실행 후 UI Redo → 저장/종료 → 별도 검증까지 통과했다. 페이지 밖 데이터와
history 두 노드는 유지됐다. All-target/all-feature Clippy와 전체 66개 테스트도 통과했다.

### 선택 브러시·지우개 저장 계약

선택 마스크를 닫힌 stroke의 불변 기록과 식별값에 포함했다. CPU 재생 시 선택 밖,
다른 레이어, 음수 타일과 페이지 경계 패딩을 보존한다. 기존 선택 없는 기록과 ID는
호환되며, 선택 stroke를 처음 저장할 때만 marker 3으로 원자적 전환한다. Undo 뒤에도
구버전이 선택 있는 redo branch를 잘못 다루지 않도록 marker를 유지한다.

Core 위험 검사 두 개를 추가했다. Release의 10개 별도 child process로 선택 브러시,
선택 지우개, Undo, Redo의 저장·재실행·마스크·타일·PNG 비교가 통과했다. 실제 구버전
reader가 새 파일을 거부하고 원본 바이트를 보존하는 것도 확인했다.
[ADR-0014](decisions/ADR-0014-selected-stroke-replay.md)에 형식과 근거를 기록했다.
다음은 같은 마스크를 GPU 경로에 적용하고 native 선택 제스처/표시를 연결하는 것이다.

### 선택 브러시·지우개 GPU 및 설치판 연결

동일한 불변 선택 마스크를 GPU brush/eraser와 닫힌 stroke 저장 경로에 연결했다.
선택이 있는 동안에도 그릴 수 있으며 선택 밖 픽셀은 그대로 유지한다. History와
레이어 변경은 입력 admission을 원자적으로 잠근 뒤 CPU/GPU 선택을 함께 해제한다.

Windows Intel Arc의 DX12/Vulkan release 비교에서 선택 밖·음수 타일·경계 패딩과
Cancel 복원이 정확히 일치했다. 선택 안 GPU/CPU 채널 오차는 brush 최대 2, eraser
최대 1이다. 컴퓨터 사용으로 설치판에서 경계 brush, eraser, Save/정상 종료,
재실행과 Undo/Redo를 조작하고 별도 프로세스로 저장 기록·픽셀·PNG를 검증했다.
[ADR-0015](decisions/ADR-0015-gpu-selected-painting.md)에 근거와 제한을 기록했다.

추가 단위 테스트 없이 전체 68개 검사와 Clippy를 통과했다. Native 선택 제스처와
선택 경계 표시, 일반 편집 도구 활성화, metadata/history의 renderer 격리가 남았다.

### Native 선택 도구와 선택 경계 초기 연결

마법봉·올가미·채우기·그라데이션 버튼과 단축키, 참조 범위/허용 오차 UI를 연결했다.
완료된 native 제스처만 비동기 편집 명령이 되며, 올가미는 4096점 한도를 넘기거나
취소되면 부분 적용하지 않는다. 선택 경계는 최종 viewport에만 표시한다.

설치판에서 실제 마법봉 → 채우기, 마법봉 → 드래그 그라데이션, 선택 없는 채우기를
조작했다. 각 scratch의 저장/종료 뒤 별도 프로세스로 전체 페이지와 페이지 밖,
history/tree 및 PNG를 독립 기대값과 비교해 통과했다. 마우스 history 조회 실패가
현재 이동까지 버리던 문제도 고쳤다. 필요한 위험 검사 한 개를 추가해 전체 69개다.
[ADR-0016](decisions/ADR-0016-native-selection-tools.md)에 실행 근거를 기록했다.

다음은 올가미/그라데이션 진행 중 경로 표시, 올가미 설치판 acceptance, 참조/오차와
종료 경계 시나리오다. 기본 편집 전체 gate와 metadata/history 격리는 아직 열려 있다.

### 편집 제스처 안내선과 Esc 취소

올가미 진행 경로·닫히는 선과 그라데이션 방향선을 최종 viewport에만 표시한다.
최대 4096점/64 KiB GPU buffer로 제한하며 작품·선택 데이터와 분리했다. Esc와
PointerCancel은 미완성 제스처만 취소하고 이후 끝점이 선택을 되살리지 않게 한다.

DX12/Vulkan의 실제 화면 readback에서 회전·DPI·음수 창 원점 하의 위치, 해제 후
화면 복원과 작품 불변을 확인했다. 설치판에서는 합성 native 올가미 경로를 단계별로
넣어 진행 표시 → 4050px 선택 → 실제 UI 채우기 → Save/종료/별도 reopen을 검증했다.
Esc 취소 뒤 늦은 End도 작품·history·PNG를 바꾸지 않았다. 이는 합성 경로와 컴퓨터
사용 UI 검증이며 물리 펜 근거는 아니다. [ADR-0017](decisions/ADR-0017-edit-gesture-guides.md).

추가 단위 테스트 없이 기존 69개 검사와 Clippy를 통과했다. 다음은 참조/오차 UI,
완료된 편집 입력의 종료 경계와 metadata/history renderer 격리다.

### 종료 직전 도구 변경의 입력 오해석 수정

채우기를 선택한 직후 Begin/End를 넣고 다음 renderer drain 전에 Save/Close하는
scratch 재현에서, 종료 경로가 이전 Brush 설정을 재사용해 채우기 대신 점을 저장했다.
종료 drain에 최신 승인된 도구·레이어 설정을 전달하도록 수정했다.

같은 설치판 재현을 수정 전후 비교했다. 수정 전 독립 픽셀 검사는 실패했고, 수정 후
snapshot 2 / history 2와 전체 8385개 페이지 픽셀, 페이지 밖 데이터, PNG가 정확히
일치했다. Edit commit → PNG export → writer join 순서도 확인했다. 입력은 종료 경계를
고정하는 합성 입력이며 Save/Close는 컴퓨터 사용 UI로 실행했다. 근거는
`target/close-native-edit-before/`, `target/close-native-edit-after/`에 있다.

새 단위 테스트 없이 기존 종료 시 작품 보존 검사와 전체 Clippy를 통과했다.
`close-native-fill` probe는 표시된 scratch project에서만 작동한다. 참조/오차 UI와
metadata/history renderer 격리는 계속 진행한다.

### 참조 범위와 허용 오차 설치판 왕복

서로 다른 위치에 반투명 참조선과 불투명 일반 레이어 선이 있는 129×65 scratch를
만들어, 설치판의 실제 드롭다운·슬라이더·마법봉·채우기·Save/Close를 조작했다.
현재 레이어/오차 0은 8385px, 참조 표시 레이어/오차 63은 4160px, 같은 범위/오차
64는 8385px, 보이는 모든 레이어/오차 0은 2080px를 선택했다. 참조선의 최대 RGBA
차이 64를 포함하는 경계 조건과 비참조 레이어 제외가 UI에서도 확인됐다.

네 프로젝트 모두 종료와 writer join 이후 별도 fixture 프로세스로 reopen했다.
선택 결과를 채운 전체 타일, 다른 레이어, 음수 타일, 페이지 경계 패딩, canvas/tree,
snapshot 2/history 2 및 PNG 합성이 독립 기대값과 정확히 일치했다. PNG 기대값은
production compositor를 재사용하지 않는 위치별 상수다. 실행 근거는
`target/source-ui-{active,reference63,reference64,visible}/`의 app/verify 로그에 있다.

`desktop_edit_sources_fixture` example으로 생성·재검증을 재현할 수 있다. 제품 코드나
단위 테스트는 추가하지 않았으며 example build와 focused Clippy가 통과했다.
Windows 설치판 release/DX12/Intel Arc에서 컴퓨터 사용으로 검증했으며 물리 펜이나
표시 지연 측정은 아니다. 다음 작업은 metadata/history의 renderer 대기 제거다.

### History·레이어 저장의 renderer 대기 제거

Undo/Redo/분기 조회와 durable 레이어 변경을 기존 선택·채우기와 같은 단일 pending
artwork job으로 통합했다. Renderer의 동기 `recv`를 제거하고 bounded 요청/응답과
완료 후 redraw로 연결했다. 작업 중 입력 admission은 잠그고 결과 채택 뒤 재개하며,
viewport·도킹·Save·Close는 계속 처리한다. History 저장/채택 오류는 fail-closed다.

종료 시 아직 화면에 채택되지 않은 레이어 변경도 PNG에 반영되도록 export는 writer의
최신 tree를 사용한다. 설치판에서 레이어 저장을 scratch barrier로 대기시켜 확대,
Save 보류와 종료 진행창을 확인했다. 종료 이후 변경 commit → 최신 PNG → join과
별도 reopen 비교가 통과했다. Undo 대기 중 확대/Save, 자동 완료 표시, 재실행 후
Redo도 tree·전체 타일·페이지 밖·history·독립 PNG 기대값과 정확히 일치했다.

기존 69개 테스트와 전체 Clippy를 통과했으며 단위 테스트는 늘리지 않았다.
[ADR-0018](decisions/ADR-0018-asynchronous-history-and-layers.md)에 계약과 설치판
근거를 기록했다. 이 변경은 저장 스레드 대기를 분리한 것이며 대형 페이지의 GPU
채택·projection 비용까지 성능 통과로 판정하지 않는다. 해당 계측, 기본 변형/page
크기 변경과 도킹·layout 복원 등 남은 gate를 계속 진행한다.

### 화면 배치 저장·재시작 복원과 설정 오류 복구

승인된 패널 이동·활성 탭을 사용자별 설정에 저장하고, UI/native canvas 생성 전에
복원한다. 저장은 최신 한 건을 합치는 전용 worker가 처리하며 작품 저장과 분리했다.
메뉴의 기본 화면 배치 복원도 연결했다. 잘못된 설정은 시작 시 원본을 보존하고
전체 기본 배치로 열며, 저장 실패는 작은 안내와 재시도 방법을 표시한다.

설치판에서 Color를 왼쪽으로 옮기고 History 탭을 선택한 뒤 종료·재실행하여 그대로
복원되는 것을 확인했다. 읽기 전용 파일은 실패 후 원본 hash가 같았고, 권한 복구와
메뉴 초기화 뒤 정상 저장됐다. 필수 패널이 빠진 손상 파일도 시작 시 보존됐으며
기본 화면과 캔버스가 열리고 명시적 초기화가 성공했다. 안내가 캔버스를 밀어내던
표시 문제는 수정 후 설치판에서 다시 검증했다.

각 종료 후 별도 fixture reopen에서 8385개 페이지 픽셀, 다른 레이어·페이지 밖 타일,
snapshot/history와 PNG 불변이 통과했다. 전체 Clippy, 기존 API 검사 2개와 release
설치 빌드가 통과했으며 단위 테스트를 추가하지 않았다.
[ADR-0019](decisions/ADR-0019-workspace-layout-persistence.md)에 계약과 근거를 기록했다.
도구막대 항목 도킹, 측면 stack/fill과 pointer capture는 아직 별도 작업으로 남아 있다.

### 독립 도구막대 도킹과 측면 stack/fill

캔버스 작업·보기·색상 도구막대를 authoritative layout의 독립 항목으로 연결했다.
상단은 제목 없는 grip과 가로 control, 측면은 같은 기능의 세로 panel로 표시한다.
고정 명령은 그대로 두고 상단 앞에 삽입·끝에 복귀·상단끼리 순서 변경이 가능하다.
측면의 연속 세로 split은 한 목록으로 표시해 앞 패널을 위로 쌓고 마지막 패널이
남은 높이를 채우게 했다. 여러 패널을 추가할 때 마지막 레이어 영역이 사라지던
중첩 비율 문제도 이 방식으로 수정했다.

설치판에서 세 항목을 좌우에 배치하고 확대·색상 변경을 실행한 뒤 종료·재시작해
복원을 확인했다. 다시 상단으로 모두 복귀하고 색상/보기/캔버스 작업 순서로 바꾼
배치도 별도 재시작에서 유지됐다. 기존 v1 설정은 읽고 새 배치는 v2로 저장한다.
각 세션의 native canvas/GPU 생성은 한 번이었으며 종료 후 독립 reopen에서 작품
8385픽셀·전체 타일·history·PNG 불변이 통과했다. 전체 Clippy, 기존 API 검사 2개,
release 설치 빌드가 통과했다. 단위 테스트는 추가하지 않았다.

[ADR-0020](decisions/ADR-0020-dockable-toolbar-entries.md)에 근거와 범위를 기록했다.
다음은 pointer capture, 창/target 밖 release와 취소 처리다. 실제 최근 색상 이력과
물리 펜 연속성·장시간 도킹 acceptance도 아직 완료로 판정하지 않는다.

### 도킹 포인터 캡처와 취소

도킹 handle이 pointer capture를 잡고 좌표·삽입 marker를 WebView 안에서 처리한다.
드래그 중 Dioxus signal 갱신은 제거했고 놓을 때 현재 좌표로 대상을 다시 판정해
시작 revision에 묶인 명령 하나만 보낸다. Esc/창 blur/pointercancel/capture loss 등은
배치 변경 없이 취소한다. 버튼의 일반 포커스 이동을 창 blur로 오인하던 문제도
설치판에서 발견해 수정했다.

실제 드래그의 캔버스 횡단 이동, canvas 위 target에서 놓기, canvas 중앙·제목 표시줄
release 취소, 일반 탭 클릭과 상단 순서 변경을 확인했다. Scratch 전용으로 네 취소
신호를 주입한 연속 드래그는 설정 hash를 보존했고 그다음 새 드래그는 성공했다.
취소 신호의 일부는 합성이며 실제 장치 capture loss나 창 전환의 증거로 취급하지
않는다. 종료·별도 reopen의 작품 전체 타일/history/PNG 불변도 통과했다.

전체 Clippy와 release 설치 빌드가 통과했으며 단위 테스트는 추가하지 않았다.
[ADR-0021](decisions/ADR-0021-dock-pointer-capture.md)에 근거와 남은 수동 범위를
기록했다. 실제 최근 색상 이력, 도킹 중 marker의 직접 시각 확인·장시간/물리 펜
acceptance와 나머지 기본 편집·성능 계측은 계속 열려 있다.

### 실제 최근 색상 목록 공유

고정 팔레트를 최근 색상으로 표시하던 부분을 바꿨다. 승인된 색상 변경은 renderer의
semantic projection에서 최신순 8개로 유지하고 같은 RGBA를 중복 저장하지 않는다.
상단/측면 Quick colors와 Color panel이 같은 목록과 swatch 명령을 사용한다. 기존
고정 색은 별도 기본 팔레트로 남겼다. 색상/최근 이력은 앱 세션 상태이며 project
activation 때 함께 보존하고, 앱 종료 이후의 사용자 설정 저장 범위에는 넣지 않았다.
입력 sample·brush stroke snapshot·작품/history 저장 형식은 바꾸지 않았다.

Windows 11 build 26200 / Core Ultra 7 155H / Intel Arc / DX12 설치판 release에서
컴퓨터 사용으로 기본 팔레트 8개를 차례로 선택했다. 초기 cyan을 포함한 9개 고유색
중 마지막 8개가 두 UI에 동일한 최신순으로 남았다. 상단에서 오래된 검정을 다시
선택하면 중복 없이 선두로 옮겼고, 현재 색을 재선택해도 목록은 바뀌지 않았다.
Quick colors를 왼쪽에 도킹한 뒤에도 목록이 같았으며, 별도 경로의 두 번째 scratch를
activation한 뒤 현재 검정과 목록을 유지했다. 측면의 주황 재선택도 양쪽에 반영됐다.

두 프로젝트에서 실제 Save와 PNG 완료를 확인했고, 각 writer 종료 후 별도 fixture
프로세스로 8385개 페이지 픽셀·다른 레이어·음수 타일·경계 패딩·snapshot 2/history 2
및 독립 PNG 기대값을 비교해 통과했다. 근거는 `target/recent-colors-ui/`의
`first-out.log`, `activation-out.log`, `first-verify.log`, `second-verify.log`다.
전체 Clippy와 release 설치가 통과했다(`target/recent-colors-clippy-final.log`,
`target/recent-colors-install-final.log`). UI 상태를 위한 새 단위 테스트는 추가하지 않았다.
색상환/SV/value 직접 조작과 나머지 기본 편집·성능 gate는 계속 진행한다.


### 색상환·SV·명도 직접 조절

색상 패널의 장식과 OS 색상 입력을 hue wheel, SV 사각형과 명도 strip의 실제
컨트롤로 교체했다. 드래그 preview는 WebView 안에서 처리하고 완료할 때 revision에
묶인 RGBA 명령 하나를 보낸다. 방향키·Shift·Home/End 조절, 영역 밖 좌표 clamp,
검정에서 명도를 다시 올릴 때 hue/saturation 보존과 도킹 재생성 정리를 포함한다.
상단 current/recent는 승인된 상태만 공유한다. 원시 펜 입력 경로는 바꾸지 않았다.

Windows 설치판에서 실제 hue/SV/value 드래그와 키보드로 정확한 빨강을 선택하고
채우기·Save·PNG 완료 후 정상 종료했다. 별도 fixture 프로세스가 8385개 페이지 픽셀,
다른 레이어·음수 타일·경계 패딩·snapshot 3/history 3와 독립 PNG 기대값을 정확히
검증했다. 최종 설치판 재시작 화면에서도 빨강이 복원됐다. Color를 Navigator 위로
도킹한 뒤 명도를 다시 조절했고, 종료 후 동일한 독립 검사도 통과했다.

전체 Clippy와 release 설치 빌드가 통과했고 새 단위 테스트는 추가하지 않았다.
근거는 `target/color-picker-ui/`의 `first-out.log`, `red-verify.log`, `reopen-out.log`,
`reopen-verify.log`와 [ADR-0022](decisions/ADR-0022-direct-color-picker.md)에 있다.
실제 held-drag 취소·장치 capture loss와 물리 펜/표시 지연을 검증한 것은 아니다.
기본 변형·page 크기 변경과 projection/hot-path 계측 등 남은 gate를 계속 진행한다.

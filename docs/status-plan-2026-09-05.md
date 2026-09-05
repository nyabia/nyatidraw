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

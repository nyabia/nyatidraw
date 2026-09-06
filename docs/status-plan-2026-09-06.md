# 2026-09-06 구현 상태와 다음 검증

Sprint 1~3의 주요 편집 기능은 구현됐지만 **사용자 완료 gate는 아직 열려 있다**.
과거 엔진 probe, 설치 앱의 제한된 시나리오, 실제 펜·표시 지연을 구분한다.
이 문서는 현재 작업 목록이며 [9월 5일 기록](status-plan-2026-09-05.md)은 개발 이력이다.

## 현재 코드와 설치판

- `cd71059`: WebView 초기 포커스 실패로 앱이 종료되는 결함 수정. 동일 조건에서
  원본 실패/수정본 성공을 재현했고, 설치판의 4K Undo/Redo·Save·정상 Close와 별도
  프로세스 전체 그림/PNG 비교를 통과했다. 두 번째 앱 실행과 기존 프로세스로의
  파일 전달도 확인했다. [ADR-0035](decisions/ADR-0035-webview-startup-focus.md).
- `b603673`: 한 root 읽기 안에서 같은 타일 객체를 반복 읽기·복사·검증하지 않도록
  수정했다. 매번 새 읽기에서는 저장 파일을 다시 검증한다. 전후 20회 측정과
  별도 프로세스 전체 재열기 비교, workspace 80개 테스트·Clippy를 통과했다.
  [ADR-0036](decisions/ADR-0036-deduplicate-root-object-loads.md).
- 설치판에는 첫 변경까지 반영됐다. 두 번째 변경의 새 설치판 UI 검증은 남아 있다.
  실행 중인 설치 앱을 컴퓨터 사용 도구가 활성화/읽지 못하고
  `foreground window did not report a process id`를 반환한 상태다.
  프로세스를 강제 종료하거나 다운로드 작업을 건드리지 않았다.

## Gate별 상태

| 영역 | 확보한 구현·증거 | 아직 필요한 완료 증거 |
|---|---|---|
| PNG/project 연결 | exact sibling 우선, PNG bootstrap, invalid non-empty 보존, alpha/sRGB export와 재열기 | Explorer의 실제 Open With 후보 → draw/erase → Undo/Redo → Save/Close → 같은 PNG 재열기 한 흐름 |
| 설치/제거 | 사용자별 설치, 소유 등록만 갱신·제거, 외부 OpenWithProgIds/UserChoice와 작품 보존 | 실제 Explorer 후보 표시/직접 열기·foreground; 등록 값 확인만으로 대체하지 않음 |
| 기본 도구·색 | 구별되는 round preset, 지우개, HSV/최근색, 크기/불투명도, native 선택/채우기/gradient와 clipping | 실제 펜 continuity·capture loss·장시간 도구 순환 |
| 레이어·history | 20 raster/중첩 group, 삭제/이름/순서/visibility/opacity, metadata Undo, Reference/Solo, 분기 선택과 페이지 목록 | 새 저장소 변경을 포함한 설치판 통합 재검증과 Undo 지연 목표 |
| 변형·page | 정수 이동/반전/90도 회전/최근접 resize, 비파괴 page resize/crop, signed/locked/hidden 픽셀과 PNG 독립 비교 | 큰 장면의 메모리·지연, 실제 장시간 반복 |
| Godot | 4.6.3 Compatibility editor에서 Fill→Gradient PNG를 focus 복귀 시 자동 재수입, cache 픽셀 확인 | 연속 background watcher percentile이나 다른 backend/game package 증거는 없음 |
| 도킹·설정 | toolbar top/side 이동, side stack/fill, layout 재시작·오류 복구, bounded mouse capture/cancel 검증 | held-drag marker의 시각 확인, 실제 OS capture/focus 상실, 펜 입력과 장시간 병행 |
| 창/종료/복구 | single-instance routing, writer drain/join, pending Save/Close, export 실패/retry, startup focus 결함 수정 | resize/minimize/DPI/다중 모니터, 실제 close 오류 dialog focus/capture·접근성 |
| hot path | raw input이 WebView state/IPC를 우회, bounded transition/discontinuity 처리, preview 별도 mailbox | UI thread가 여전히 surface acquire/present를 수행함. 렌더 대기 격리와 통합 지연 측정 필요 |

주요 근거: [선택·native 도구](decisions/ADR-0016-native-selection-tools.md),
[비동기 history/layer](decisions/ADR-0018-asynchronous-history-and-layers.md),
[도킹 배치](decisions/ADR-0020-dockable-toolbar-entries.md),
[capture](decisions/ADR-0021-dock-pointer-capture.md),
[색상](decisions/ADR-0022-direct-color-picker.md),
[변형](decisions/ADR-0023-basic-raster-transforms.md),
[page](decisions/ADR-0025-page-resize-crop.md),
[색 경계](decisions/ADR-0029-srgb-boundaries.md),
[Godot](decisions/ADR-0030-godot-png-acceptance.md),
[Open With](decisions/ADR-0033-open-with-progids.md).

## 성능의 현재 판단

| 실제 측정 범위 | p50 / p95 / p99 | 의미 |
|---|---|---|
| 이전 설치판: worker 접수→복원 frame present API | 131.071 / 155.647 / 164.815 ms | UI 20회, histogram 상한. Undo 16ms 목표 미통과 |
| 저장소 수정 전: 타일 읽기 | 84.992 / 90.378 / 90.902 ms | 별도 CPU/storage probe 20회 |
| 저장소 수정 후: 타일 읽기 | 10.955 / 12.776 / 14.136 ms | 동일 fixture; 902 keys/156 objects |
| 저장소 수정 후: 준비→저장→session 반영 | 13.944 / 16.361 / 217.607 ms | 첫 immediate persist 204.677ms를 포함함 |

표의 서로 다른 구간을 더하거나 빼서 UI 응답을 계산하지 않는다. 수정 후에도
UI adoption/composite/present와 저장 반영 tail이 남는다. 정확한 환경과 전체 표본은
[성능 기록](performance.md)에 연결되어 있다. 물리 visible pixel 측정은 없다.

## 다음 실행 순서

1. 화면 조작 가능 시 현재 앱을 정상 종료하고 새 release를 설치한다. 기존 4K
   reference의 별도 scratch에서 Undo/Redo·Save·Close·일반 재시작과 전체 비교를
   수행한다. 계측 session에서 queue/adoption/present 횟수를 함께 확인한다.
2. 수정 후 UI Undo 전체 분포를 기록하고, 저장 반영의 긴 표본을 별도로 조사한다.
   artwork durability를 낮추거나 fsync 결과를 제외해 목표를 맞추지 않는다.
3. Explorer Open With 후보/직접 열기와 primary foreground를 실제 UI로 검증한다.
   레지스트리 조회 성공과 Explorer 동작 차이는 미해결 상태다.
4. UI thread의 surface wait 격리를 설계·구현한다. child HWND 생존, renderer 종료/join,
   resize/suspend, bounded pending frame과 입력 phase 보존을 먼저 정한다.
   채널만 옮기고 HWND/GPU ownership 안전성이 불명확한 상태를 완료로 보지 않는다.
   현재 소유권 검토와 작은 실행 단위는 [렌더 스레드 초안](render-thread-design.md)에 기록했다.
5. 현재 16-bit PNG export를 포함한 4K drawing 간섭 campaign, startup/reopen 분포,
   resize/minimize/panel/Save 장시간 시나리오를 실행한다. 이전 8-bit export 측정과
   isolated encoder 시간은 새 campaign을 대신하지 않는다.

화면 검증을 기다리는 동안 core 성능 분석, 저장소 tail 조사, 렌더 소유권 설계와
문서 정리는 진행 가능하다. 실제 펜/고주사율/다중 모니터 상호작용은 필요한 환경이
없으면 미검증으로 유지한다. macOS, vector/Vello, animation, advanced brushes,
공개 배포는 계속 범위 밖이다.

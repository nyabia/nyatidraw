# Godot 우선 NyatiDraw 스프린트 계획

활성 스프린트는 내부 엔진 부품 수가 아니라 **Godot 작업 중 사용자가 끝까지 수행할
수 있는 작업**으로 판정한다. 독립 드로잉 소프트웨어 공개는 장기 목표지만, 당장은
Godot project 안의 PNG를 Windows `Open with`로 열고, paired `.ntdr`을 자동으로 찾아
수정한 PNG를 같은 위치에 돌려놓는 왕복 시간이 가장 중요하다.

플랫폼은 Windows 우선이며 macOS는 보류한다. 편집기 배치와 의미론은
[편집기 UI 목표](../editor-ui-target.md), PNG/project pairing 경계는
[Godot 작업 폴더와 PNG 연결](../godot-integration.md)이 정한다. Godot editor addon은
활성 스프린트 밖이다.

## 최우선 사용자 시나리오

```text
Godot project의 art/player.png를 Windows에서 NyatiDraw로 열기
  -> exact sibling art/player.ntdr 확인
  -> 있으면 project 열기, 없으면 PNG를 보존한 새 project 열기
  -> 펜/마우스로 페이지 안팎에 스케치
  -> 도구, 색, 레이어, undo 사용
  -> Save
  -> player.ntdr durable commit
  -> player.png가 완전한 파일로 교체
  -> Godot가 일반 file watcher로 PNG 변경 감지
  -> 닫고 player.png를 NyatiDraw로 다시 열어 작업 계속
```

`.ntdr` 직접 열기도 지원한다. Godot 전용 addon 없이 먼저 이 경로를 안정화하며,
고급 브러시, 애니메이션, 공개 배포 기능을 앞세우지 않는다.

## 활성 로드맵

| Sprint | 사용자가 얻게 되는 결과 | 핵심 기능 | 완료 gate |
|---|---|---|---|
| [0 - 검증된 엔진 기반](sprint-00-foundation.md) | 이미 존재하는 WGPU/Win32/redb vertical slice | 입력, live ink, signed tile, durable root | 역사적 기반; 사용자 완료로 계산하지 않음 |
| [1 - Godot asset 즉석 스케치](sprint-01-ink.md) | PNG Open With, sibling `.ntdr` 자동 탐색/생성, 즉시 그리기와 export | PNG import, Move, Pencil/Pen/Brush preset, Eraser, 색/크기, infinite workspace, undo/save/reopen | 설치된 앱으로 PNG-open/draw/save/pair-reopen 한 사이클 |
| [2 - 게임 프로토타이핑 편집기](sprint-02-document.md) | 여러 요소를 실제로 편집 | raster/group layer, thumbnail, Solo/Reference, Wand/Lasso, Fill/Gradient, 기본 transform/canvas size | 실제 prototype asset을 반복 편집해 project와 최신 PNG가 일치 |
| [3 - 매일 쓰는 개발판](sprint-03-editor.md) | 오래 켜 두어도 안정적인 작업 공간 | docking, navigator/history, shortcut cycle, file lock/open routing, pen/window/error/performance | Windows release 장시간 작업 및 restart/recovery acceptance |
| [4 - 브러시다운 브러시](sprint-04-brush.md) | Clip Studio를 참고한 실사용 브러시 | dynamics, spacing/flow, texture tip, stabilizer, preset library | golden corpus와 8K stroke stress |
| [5 - 래스터 편집과 대형 프로젝트](sprint-05-reliability.md) | 정밀 편집 및 큰 파일 운용 | selection refine, mask, crop/resize/transform, filters, color pipeline, lazy load, multi-profile export | disk-full/generation/large-recovery와 편집 결과 matrix |
| [6 - 애니메이션과 게임 산출물](sprint-06-animation.md) | cel 애니메이션을 게임 asset으로 내보냄 | timeline, onion skin, spritesheet/sequence export | save/reopen/playback/export frame equality |
| [7 - 벡터와 텍스트](sprint-07-vector.md) | 확대 가능한 도형과 텍스트를 raster와 혼합 | backend-neutral vector model, Vello adapter, vector layer/text | mixed document reopen과 CPU/headless export |
| [8 - 독립 소프트웨어 공개 알파](sprint-08-public-alpha.md) | 타인이 안전하게 설치하고 업데이트하는 Windows 앱 | signed installer, onboarding, recovery UX, accessibility, diagnostics, migration | clean VM install/update/uninstall; artwork 무삭제 |
| [9 - 플랫폼과 생태계](sprint-09-platforms.md) | Windows 밖에서도 같은 문서를 사용 | Linux/macOS input backends, web repository/viewer, plugin API, canonical pack/unpack | 플랫폼별 명시적 capability/evidence matrix |

## 기능 배치표

| 기능군 | 첫 사용 가능 | 완성 단계 | 범위 원칙 |
|---|---:|---:|---|
| PNG Open With/sibling `.ntdr`/PNG 갱신 | 1 | 3 | Windows activation adapter이며 문서 엔진과 저장 실패 영역을 분리 |
| 무한 작업공간과 출력 페이지 | 1 | 3 | 바깥 픽셀은 project에 남고 preview/export에서는 crop |
| Pencil/Pen/Brush/Eraser | 1 | 4 | Sprint 1은 한 round engine의 구별되는 preset; advanced dynamics는 4 |
| Move/viewport | 1 | 3 | selection transform과 viewport pan을 별도 command로 유지 |
| Wand/Lasso/Fill/Gradient | 2 | 5 | Reference layer 집합과 tolerance 의미를 문서화한 뒤 활성화 |
| 색상/최근 색/크기/불투명도 | 1 | 4 | 다음 stroke snapshot에 적용; raw input은 UI state를 우회 |
| raster layer/group | 2 | 3 | add/delete/rename/reorder/visibility/opacity/active가 모두 durable |
| thumbnail/navigator/history UI | 2 | 3 | page crop, 실제 projection, 장식 상태 금지 |
| Solo/Reference/레이어 색상화 | 2 | 5 | Solo는 session-only, Reference는 metadata, 색상화는 비파괴 render property |
| 선택/변형/crop/resize | 2 | 5 | 자주 쓰는 기본 동작부터, 정밀 mask와 filter는 후속 |
| 브러시 preset/dynamics | 1 | 4 | 기본 preset에서 시작해 결정적 semantic stroke로 확장 |
| 타임랩스 | 3 | 6 | editor history와 별도 소비 경로, 애니메이션 timeline과 혼동하지 않음 |
| 애니메이션/spritesheet | 6 | 6 | ContentRoot를 공유하는 cel 구조 |
| 벡터/text/Vello | 7 | 7 | 프로젝트 포맷에 Vello 타입을 저장하지 않음 |
| 공개 설치/업데이트/접근성 | 8 | 8 | 개발판 설치와 사용자 배포를 분리 |
| Linux/macOS/web/Git unpack | 9 | 9 | Windows 기능 완료를 가장하지 않고 backend별 검증 |

## Sprint 1에서 의도적으로 허용하는 축소

- Pencil, Pen, Brush는 같은 round engine을 서로 다른 고정 preset으로 사용해도 된다.
- Eraser는 픽셀 삭제, Move는 viewport pan만 정확하면 된다.
- Wand/Lasso/Fill/Gradient, layer colorize, transform은 disabled로 표시한다.
- 한 번에 한 문서만 제대로 열면 된다. single-instance routing은 Sprint 3이다.
- PNG를 열 때 valid sibling `.ntdr`이 우선이다. 없거나 0-byte이면 PNG를 첫 layer로
  가져와 durable `.ntdr` 복구 원본을 즉시 만들되 첫 Save 전에는 PNG를 변경하지 않는다.
- non-empty invalid sibling은 오류로 중단하고 새 project fallback으로 우회하지 않는다.
- 설치는 현재 사용자 전용 development registration이면 된다. 서명된 공개 설치는
  Sprint 8이다.

## 개발판 설치와 등록

[Windows 개발판 설치 계약](../development-install.md)을 따른다.

- 빠른 반복은 DX release app을 사용자별 고정 경로에 복사하고 `.ntdr` handler와
  `.png` Open With application을 등록한다.
- 앱은 첫 positional argument로 `.ntdr` 또는 `.png`를 받고 extension에 맞는 activation
  규칙을 적용한다. 환경변수 전용 seam은 개발 probe로만 남긴다.
- 기본 앱 `UserChoice`를 자동으로 덮지 않는다.

## 각 Sprint의 증거 규칙

- 활성으로 보이는 컨트롤은 실제 명령이어야 하며 장식이면 완료 실패다.
- artwork 변경은 save -> process restart -> reopen으로 검증한다.
- export는 현재 단일 FIFO worker와 sibling temporary replacement로 순서를 보장한다.
  병렬 encoder를 도입할 때 generation 확인을 추가한다.
- PNG activation은 valid/absent/zero-byte/invalid sibling 네 경우와 first Save,
  process restart, pair reopen까지 확인한다.
- UI layout은 자동 테스트를 늘리지 않고 DX build, Clippy, bounded runtime
  acceptance로 검증한다.
- core tests는 input transition, deterministic artwork/history, signed crop,
  invalid-file/recovery처럼 그림 손실 가능성이 있는 invariant에만 둔다.
- physical pen, high-refresh, first-visible percentile은 실제 장비가 없으면
  미검증으로 기록한다.

## 스프린트 밖 통합 backlog

Godot FileSystem의 만들기/편집 메뉴와 명시적 reimport를 제공하는 editor addon은
현재 Sprint 1~9의 완료 조건에 포함하지 않는다. Windows image-open 경로를 먼저 쓰고,
addon의 실제 필요가 확인되면 별도 통합 계획으로 연다.

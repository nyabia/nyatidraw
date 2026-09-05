# 편집기 UI 목표와 의미론

이 문서는 2026-09-03 현재 NyatiDraw 편집기 화면을 개발판의 시각적
출발점으로 고정하되, 각 부분이 실제로 무엇을 의미하고 어느 상태까지 구현되어야
하는지를 정의한다. 실제 프로그램의 `apps/desktop/src/main.rs`와
`apps/desktop/assets/styles.css`가 현재 배치의 기준이며, `docs/mockups`는 결정 과정을
남긴 참고 자료일 뿐 최종 권위가 아니다.

## 제품 장면

NyatiDraw는 설명을 읽는 화면이 아니라 그림을 그리는 **Operate** 모드의 도구다.
첫 창은 마우스가 있는 모니터에 최대화되어 열리고, 제목이나 소개 문구가 캔버스
공간을 소비하지 않아야 한다. 상단 명령 막대와 필요한 패널을 제외한 모든 공간은
작품과 그 주변 작업공간에 돌아간다.

보이는 컨트롤은 다음 중 하나여야 한다.

- 실제 명령에 연결되어 결과와 현재 상태를 즉시 표시한다.
- 아직 범위 밖이면 비활성 스타일과 툴팁으로 그 사실을 명확히 표시한다.
- 구현되지 않은 컨트롤을 활성 상태의 장식으로 두지 않는다.

## 화면 구조

```text
OS 제목 표시줄
고정 명령 | 도킹 가능한 작업 도구 | 보기 | 색상/최근 색상
┌─ 도구 ─┬─ 세부 도구 ─┬──── 무한 작업공간 / 출력 페이지 ────┬─ 내비게이터 ─┐
│         │             │                                      ├─ 색상 ──────┤
│         │             │                                      ├─ 레이어 ────┤
│         │             │                                      │ / 히스토리  │
└─────────┴─────────────┴──────────────────────────────────────┴──────────────┘
```

- 상단의 버거 메뉴, 열기, 저장, 실행 취소, 다시 실행까지는 고정 명령이다.
- 그 뒤의 캔버스 작업, 보기, 색상/최근 색상은 독립적으로 도킹 가능한 도구다.
- 왼쪽과 오른쪽 도킹 열은 상단 명령 막대와 작은 간격을 두고 세로 공간을 사용한다.
- 한 열에 패널이 여러 개면 위에서부터 쌓이고 마지막 패널이 남은 높이를 채운다.
- 하단 도킹은 현재 목표에서 제외한다.

## 캔버스와 무한 작업공간

`CanvasSpec`의 폭과 높이는 **출력 페이지**를 정의한다. 편집 좌표계의 경계가
아니다.

- 투명 체크무늬는 출력 페이지 안에만 표시한다.
- 페이지 경계 바깥쪽에 검은 테두리를 그려 출력 범위를 분명히 한다.
- 페이지 밖은 어두운 중립 작업공간이며 signed sparse tile 좌표로 계속 그릴 수 있다.
- 페이지 밖의 픽셀도 프로젝트와 히스토리에 저장하고 재실행 후 복구한다.
- 일반 이미지 export, 내비게이터 미리보기, 레이어 미리보기는 출력 페이지로 자른다.
- 패닝에는 임의의 문서 경계를 두지 않는다. 구현체의 정수 표현 한계와 자원 한도는
  오류로 보고하되 화면 크기의 인위적인 여백 제한으로 바꾸지 않는다.

기본 뷰포트 조작은 다음과 같다.

| 입력 | 의미 |
|---|---|
| 휠 | 세로 패닝 |
| `Shift` + 휠 | 가로 패닝 |
| 가운데 버튼 드래그 | 자유 패닝 |
| `Space` + 펜/왼쪽 버튼 드래그 | 자유 패닝 |
| `Ctrl` + 휠 | 포인터 위치 중심 확대·축소 |
| 이동 도구 드래그 | 자유 패닝 |
| 화면 맞춤 | 출력 페이지를 작업공간에 맞춤 |
| 1:1 | 출력 페이지의 문서 픽셀과 화면 물리 픽셀을 1:1로 표시 |

무한 작업공간에는 유한 스크롤바를 표시하지 않는다. 내비게이터의 viewport box와
직접 패닝이 현재 위치를 전달한다.

## 도구와 세부 도구

왼쪽 막대는 도구 종류를 선택하고, 세부 도구 패널은 선택한 종류의 preset을
보여준다. 같은 단축키를 반복해서 누르면 같은 그룹을 표시 순서대로 순환한다.
각 그룹 사이는 구분선으로 나눈다.

| 순서 | 도구 | 단축키 | 초기 구현 단계 |
|---|---|---|---|
| 1 | 이동 → 마법봉 → 올가미 | `G` | 이동 Sprint 1, 선택 계열 Sprint 2 |
| 2 | 연필 → 펜 → 브러시 | `B` | Sprint 1에서 같은 기본 엔진의 고정 preset |
| 3 | 지우개 | `E` | Sprint 1에서 픽셀 삭제 합성 |
| 4 | 채우기 → 그라데이션 | `F` | Sprint 2에서 Reference/tolerance 의미와 함께 구현 |

사각형 도구는 없다. 단축키 글자는 아이콘에 종속된 작은 장식이 아니라 첫 사용자도
읽을 수 있는 학습 정보다. 클릭, 단축키, active 표시, 커서, 세부 도구 내용은 항상
같은 authoritative tool state를 반영해야 한다.

브러시 크기 영역의 상단 네 칸은 최근 사용값이고, 하단은 수치와 함께 점차 커지는
preset 목록이다. 크기, 불투명도, 현재 색상은 라이브 stroke가 읽는 불변 snapshot을
갱신하되 raw sample이 Dioxus state를 통과하게 만들지 않는다. advanced dynamics,
texture tip, stabilizer는 Sprint 4 범위다.

## 상단 명령과 도킹 가능한 도구

- 고정 구간은 앱 전체 명령이며 다른 위치로 이동하지 않는다.
- 도킹 가능한 구간은 패널과 같은 색과 grip 문법을 사용한다.
- 상단에 있을 때는 제목 없이 세로 grip 뒤에 컨트롤이 가로로 이어지고 별도 외부
  여백을 만들지 않는다.
- 좌우에 도킹하면 같은 기능을 세로 패널 형태로 재배치한다.
- 색상/최근 색상은 상단과 색상 패널이 같은 current/recent state를 공유한다.
- 도킹 handle은 드래그만 담당한다. 버튼이나 상태 정보를 handle 안에 넣지 않는다.
- 드래그 중 삽입 위치는 하늘색/청색 막대와 끝점 표식으로 표시한다.

## 내비게이터와 색상

내비게이터는 출력 페이지의 실제 합성 미리보기와 현재 화면 범위를 보여준다. 페이지
밖 스케치는 미리보기에 포함하지 않는다. 아래에는 1:1, 화면 맞춤, 축소, 현재 배율,
확대가 있으며 배율 숫자만으로 기능을 대신하지 않는다.

색상 패널은 충분히 큰 hue wheel, 중앙 SV 사각형, value 조절, 현재 색, 최근 색을
제공한다. 패널과 상단 색상 도구는 같은 state를 공유하고 선택 즉시 다음 dab부터
적용한다. 색 변경은 문서 history가 아니라 다음 stroke의 입력 상태다.

## 레이어

- 래스터 레이어 행은 해당 레이어의 픽셀만 출력 페이지로 잘라 충분히 큰 실제
  미리보기로 표시한다. 다른 레이어의 픽셀이나 페이지 밖 스케치를 섞지 않는다.
- 보기/숨기기와 불투명도는 문서 속성이며 save/reopen과 export에 반영한다.
- Solo는 다른 레이어의 durable visibility를 바꾸지 않는 session-only 보기 필터다.
- 그룹은 실제 `LayerTree` node이며 접기, 활성화, reorder 대상이다.
- Reference 표시는 픽셀이나 일반 export를 바꾸지 않고 선택/채우기 도구가 참조할
  레이어 집합을 지정하는 문서 metadata다.
- 레이어 색상화는 비파괴 render property로 계획하되 정확한 혼합 및 export 의미는
  구현 전에 별도 결정한다. 구현자가 임의로 tint 규칙을 만들지 않는다.
- 레이어 추가, 그룹 추가, 삭제, reorder 버튼은 panel handle 아래의 별도 action
  영역에 둔다.

## 상태와 피드백

- 저장되지 않은 변경, 저장 중, export 대기/실패, 복구됨을 서로 다른 상태로 보인다.
- queue full, invalid project, GPU workspace exhaustion은 조용히 무시하지 않는다.
- 진행 중 stroke는 tool/viewport 변경과 충돌하면 기존 stroke mapping을 보존하거나
  명시적으로 cancel하고 clean Begin부터 재개한다.
- 패널 재배치와 Dioxus rerender는 raw pointer queue와 GPU live stroke를 소유하지
  않는다.

## 현재 구현 대조표

| 영역 | 2026-09-03 상태 | 개발판 목표까지 남은 것 |
|---|---|---|
| 고정 명령 | 저장, 실제 PNG queued/running/current/failed 상태, 실패한 export의 `PNG 재시도`, undo/redo, Windows second-activation file routing 연결; 메뉴와 앱 내부 열기 대화상자는 미구현 | 종료 진행 상태와 Explorer 수동 activation acceptance |
| 도구 막대 | Move, Pencil, Pen, Brush, Eraser의 클릭·G/B/E 단축키·active projection 연결; 범위 밖 도구 disabled | Wand/Lasso, Fill/Gradient와 도구별 커서 |
| 세부 도구/브러시 값 | round-engine preset, 크기, 불투명도 연결 | 실제 preset library와 최근 크기 기록 |
| 캔버스 | GPU live stroke, signed outside tiles, 휠/가운데/Space/Move 패닝, 포인터 zoom | 실제 펜·장시간 수동 acceptance와 device-loss 복구 |
| 내비게이터 | durable CPU page crop 미리보기, viewport overlay, click/press-drag recenter, Fit/1:1/zoom 연결 | 수동 회전/resize acceptance |
| 색상 | current/recent color와 brush snapshot 연결; 큰 wheel 표면은 OS color picker를 연다 | wheel/SV/value 직접 조작과 recent history |
| 레이어 | 선택, visibility, opacity, 흰 배경, raster/group 추가·삭제, 이름 변경, group 접기, session-only Solo, durable Reference/thumbnail, metadata Undo, drag reorder/위·아래 이동 연결 | 물리 drag·스크롤 중 끌기 실사용 |
| 히스토리 | 현재 branch ancestry와 active cursor를 최대 64행으로 투영; 상단 undo/redo 연결 | 분기 탐색 UI와 history panel 직접 이동 |
| 도킹 | 독립 toolbar의 top/left/right 배치·상단 순서, 좌우 stack/fill, v1→v2 layout 재시작 복원을 설치판에서 확인; bottom target 없음 | pointer capture·창/target 밖 release·취소, 장시간/물리 펜 연속성 acceptance |

시작 창은 커서가 놓인 모니터의 작업 영역을 기준으로 최대화를 시도한다. 이는 Windows
parent HWND와 최소 Win32 모니터 조회를 사용하는 best-effort 경로이며, 다중 모니터와
DPI 조합에서의 실제 수동 결과는 아직 보장하지 않는다.

## 개발판 UI 완료 판정

- 현재 화면 구성에서 활성으로 보이는 모든 컨트롤이 실제 state와 명령에 연결된다.
- 범위 밖 도구는 비활성으로 구별되고 artwork를 바꾸는 척하지 않는다.
- 출력 페이지를 화면 밖으로 완전히 옮겼다가 다시 찾을 수 있으며 어느 방향으로도
  페이지 밖 stroke를 이어 그릴 수 있다.
- 페이지 안팎을 가로지른 stroke를 저장하고 프로세스를 재시작했을 때 같은 signed
  tile/root가 복구된다. export와 모든 미리보기에는 페이지 내부만 나온다.
- 패널과 상단 도구를 재배치해도 live input, active tool, project session, GPU canvas가
  재생성되거나 끊기지 않는다.
- UI layout 자동 테스트는 추가하지 않는다. release DX build, Clippy, bounded Win32
  runtime acceptance와 필요한 core invariant test로 검증한다.

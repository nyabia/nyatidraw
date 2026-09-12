# 일러스트 워크플로우 기준 기능 감사와 실행 순서

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 방향과 판정 기준

이 문서가 새 작업의 우선순위 기준이다. `sprints/functionality-next.md`의 F1~F3a는
구현 기록으로 보존하지만 F3b→병합을 자동으로 이어가지 않는다.

제품 목표는 기능 목록을 채우는 것이 아니라, NyatiDraw에서 러프부터 완성 그림까지
작업할 때 필수 조작이 막히지 않게 하는 것이다. Godot의 PNG 열기/paired ntdr/저장 export는
진입·산출 경로로 유지한다. 이를 근거로 편집을 픽셀아트·오브젝트 복붙에만 한정하지 않는다.

아래 비교는 공식 매뉴얼과 공식 제작 튜토리얼에 나타난 대표 경로다. 모든 작가가 같은
순서나 기능을 사용한다는 뜻은 아니다. 두 앱에서 동일한 작품을 직접 제작하는 비교 실험은
아직 하지 않았다. 코드에서 기능 존재를 확인한 것과 물리 펜/UI 실사용 합격은 구분한다.

- 경로 A: 러프 → 형태 수정 → 선화 → 밑색 → 셀/부분 명암 → 색 조정 → 저장.
- 경로 B: 큰 색·명암 덩어리 → 형태 수정 → 색 채취·덧칠 → 경계/명암 정리 → 저장.
- 우선 A를 끝까지 통과시키되, 공통 필수 브러시·소프트 칠·색 채취는 B도 사용할 수 있게 한다.
  혼색/수채/질감 의존 화풍은 별도 추가 단계이며 기본 페인팅과 동일시하지 않는다.
- 실제 불가능, 반복 수작업으로 우회 가능, 편의 개선을 구분한다. 무조건 경쟁 앱의 모든
  기능을 복제하지 않는다. 저장 손상·크래시·입력 유실은 어느 단계보다 먼저 수정한다.

## 1. 단계별 비교

| 그리는 사람이 하는 일 | Krita의 대표 경로 | Clip Studio Paint의 대표 경로 | NyatiDraw가 제공해야 할 조작 |
|---|---|---|---|
| 준비·구도 탐색 | Reference Images로 자료 배치, 브러시로 덩어리/러프 | Sub View 자료·색 참고, 러프 레이어 | 자료 보기와 그림 입력을 오가기, 페이지 밖 인체 스케치, 팬/줌/회전/보기 반전 |
| 러프·선화 | freehand brush preset, smoothing, eraser mode | pencil/pen subtool, 필압·stabilization, 별도 선화 레이어 | 원하는 굵기/농도의 선, 지우기·도구 복귀, 러프를 흐리게 하고 새 선화 그리기 |
| 비율 수정 | 사각/올가미 선택, 추가·빼기, transform, 페이지 밖 선택 | selection area, selection launcher, 이동/scale/rotate | 눈·팔·머리를 선택해 직접 이동/회전/확대하고 취소; 선택과 픽셀의 이동 구별 |
| 밑색 | 선화 아래 paint layer, flood fill/grow; Colorize Mask는 다른 경로 | 선화 참조 fill, area scaling, close gap | 빈 채색 레이어에서 선을 경계로 채우기, 틈 누출·흰 테두리·미채움 부분 수정 |
| 명암·덧칠 | alpha lock, 그룹+alpha inheritance, brush/blending | lock transparent pixels, clip to layer below, brush/blending | 실루엣 밖으로 새지 않게 별도 명암을 칠하고 쉽게 색을 바꾸기 |
| 회화식 마무리 | 불투명/저불투명 덧칠·색 채취, 필요 시 color smudge | brush/airbrush와 색 채취, 화풍별 혼색 | 단단한 경계와 부드러운 경계를 모두 만들기; 농도 제어와 빠른 색 채취 |
| 정리·산출 | 레이어/마스크·색 보정·저장·이미지 export | 선화 색 변경·레이어/보정·저장·이미지 export | 부분 수정, 색/명도 조정, 재열기 뒤 동일한 픽셀·레이어·PNG |

경로 비교 출처: [Krita Common Workflows][K1], [Krita Flat Coloring][K2],
[CSP 공식 첫 일러스트][C1], [CSP 공식 기본 제작 과정][C2]. 세부 조작은 아래 출처와 연결한다.
Krita의 회화식 경로와 CSP의 선화식 튜토리얼은 비교 관점이지 각 제품의 배타적 용도가 아니다.
덧칠·색 채취와 smudge를 구별하는 근거는 [Krita Color Mixing][K8]을 참고한다.

## 2. 반드시 구별해야 할 의미

1. **레이어 잠금 ≠ 투명도 잠금 ≠ 클리핑.** 지금의 locked는 그리지 못하게 한다.
   Alpha lock은 기존 alpha를 보존하면서 색을 바꾸는 편집 제약이다. Clipping은 별도
   레이어의 표시 범위를 하위 그림에 제한하는 비파괴 합성이다. 체크박스 하나로 대체하지 않는다.
2. **Krita alpha inheritance ≠ CSP Clip to Layer Below.** Krita는 그룹의 아래쪽 합성 alpha를
   이용한다. CSP는 아래 레이어/대상 폴더에 클리핑하는 사용자 모델이다. NyatiDraw는 CSP형
   사용성을 우선 후보로 삼되, 연속 clipping stack·그룹·숨김·불투명도·reorder·export 규칙을
   정한 뒤 구현한다. 임의로 둘을 섞지 않는다. [Krita 알파 상속][K5], [CSP 레이어 설정][C5]
3. **자료 이미지 ≠ 참조 레이어.** Sub View/Reference Images는 옆에 놓고 보는 자료다.
   현재 NyatiDraw reference flag는 fill/wand/picker가 읽을 원본 레이어 집합이다.
   Navigator는 현재 그림의 미리보기이므로 자료 창을 대신하지 못한다. [K6], [C6]
4. **보기 반전 ≠ 픽셀 반전.** 좌우 균형 확인은 보기만 반전해야 하며 Undo·PNG를 바꾸지 않는다.
   기존 픽셀 transform의 flip을 이 용도로 연결하지 않는다.
5. **선택 해제 ≠ 선택 픽셀 삭제**, 선택 경계 이동 ≠ 선택된 그림 이동.
   현재 `ClearActiveLayer`를 Delete 선택 픽셀 명령으로 재사용하면 안 된다.
6. **레이어 색상화 ≠ alpha lock 재칠 ≠ 행 색깔 태그.** 러프를 청색으로 표시하는 비파괴 효과는
   별도 의미다. CSP Layer color는 black/white를 지정색으로 바꾸는 효과다. 단순 tint로
   같다고 주장하지 않는다. 현재 러프의 opacity 조절은 가능하므로 필수 선화 gate의 우회로다. [C10]
7. **펜·연필·브러시 이름 ≠ 다른 표현력.** 한 엔진을 재사용하는 것은 괜찮지만 단단한 선,
   불투명 면, 부드러운 명암이 실제로 달라야 한다. 크기 필압과 농도 필압도 독립 선택해야 한다.

## 3. 기능 재고: 현재·기존 계획·누락

현재는 작업 트리 기준이며 설치판/공개 alpha.8과 같다는 뜻이 아니다.
`부분`은 명령/알고리즘이 있어도 요구 작업을 매끄럽게 끝내지 못한다는 뜻이다.

| 도구/기능 | 현재 확인 | 기존 계획 | 작업에 주는 영향 / 새 위치 |
|---|---|---|---|
| 필압 펜·연필·브러시·지우개 | W1a: round engine에 크기/농도 필압 독립·최소값·경도 연결; 실사용 검증 대기 | 고급 브러시 Sprint 4에서 기본 부분을 당김 | 굵기만 변화하는 진한 선, 흐린 러프, 부드러운 칠을 구분 |
| 크기·불투명도 | 숫자/슬라이더/최근 크기 + W1a 도구별 세션 기억, `[`/`]` 상대 조절 | 도구별 preset 확장 | 앱 재시작 후 설정 기억은 미구현; 필압·경도 포함한 실제 전환 조작 확인 필요 |
| 딱딱한/부드러운 브러시, 끝맺음, 보정 | W1a hard/soft 경도 + E1a 끄기/강도 조절 위치 보정 | Sprint 4 / E1a | 기본 필터이며 강한 보정의 End 직선 연결 한계. 실제 펜 감각 검증 대기 |
| 색상환·최근 색·스포이트·X 교환 | UX-1 고정 C + W1b Alt 접촉의 임시 색 채취 연결 | F2b / W1b / UX-1 | 임시는 Solo를 포함한 그림 합성을 읽고 도구는 바꾸지 않음; 물리 조작 검증 대기 |
| 팬·줌·회전·Fit·1:1 | W1b 보기 좌우 반전·각도 초기화·중심/포인터 기준 확대 연결 | Sprint 3 / W1b | 임의 각도 연속 회전은 후속. 보기 조작은 픽셀 반전과 별개 |
| 페이지 밖 드로잉 | signed 타일 + E1a signed 사각/올가미·제한 칠·부분 삭제 + E1b 자유 변형 | Sprint 1~3 / E1 | 물리 펜/실제 작품 사용 gate는 별도 확인 |
| 자료 이미지 표시 | 별도 자료 기능 없음; PNG 열기/PNG 클립보드는 다른 용도 | 구체적 단기 항목 없음 | 새로 식별. 외부 자료 창을 W0~W1 우회로로 인정; 필요가 확인되면 비출력 자료 표시 도입 |
| 래스터/그룹 기본 작업 | 추가/삭제/이름/순서/가시성/opacity/복제/잠금 구현 | F1/F2 | 러프·선화·색 분리의 기반. 새 문서는 투명 래스터 1개 유지; 자동 Ink/배경 세트 금지 |
| 러프 레이어 색상화 | 없음 | Sprint 5 / 기존 사용자 요구 | P1; opacity로 우회 가능. 합성 의미를 정한 뒤, 선화 gate와 묶어 검토 |
| 사각/올가미/마법봉 | E1a signed ROI 사각/올가미·교체/추가/빼기·전체/반전/해제/삭제; E2 raw alpha 선택·원판 grow/shrink; wand는 page-bound | E1/E2 | binary coverage이며 fractional 선택 AA는 미구현 |
| 선택 이동·회전·크기 | E1b: 직접 handle·실제 GPU preview·임의 회전/scale·alpha 보간·선택 유지·확정/취소 연결 | W2 / E1 | 실제 actor/GPU·저장 검증과 창/펜 수동 사용을 구분. 대폭 축소 품질은 bilinear 한계 |
| 복사/잘라내기/붙여넣기 | F3a private+PNG 재사용, E1b 임시 레이어 변형 후 단일 확정/취소; OS 실사용 미검증 | W2 / E1 | DIB 호환은 실제 자료 흐름에서 막히면 우선 |
| 선화 참조 채우기 | E2 immutable active/reference/visible, tolerance, 4-connected fill; 선택은 연결 탐색의 통과 금지 clip | W3 / E2 | 빈 대상·참조 선 보존·재열기 자동 검증, 실제 작품 조작은 대기 |
| 틈 닫기·확장·축소·AA 경계 | E2 사각 closing·barrier-only 확장·최소 한 픽셀 AA 받침 | W3 / E2 | tolerance와 독립. 모든 틈 해결/fractional 경계 재구성이 아님; ADR-0056 한계 참조 |
| 선택 안 그리기/지우기/그라데이션 | E1a signed 선택과 CPU/GPU·저장 연결 | Sprint 2/5 / E1a | fractional coverage는 미구현. 원색→투명 외 gradient 편집은 P1 |
| 투명도 잠금(alpha lock) | E3 UI/Begin 고정·CPU/GPU 재칠·저장·별도 프로세스 재편집 검증. 편집 잠금과 별개 | Essentials E3 구현/자동 검증 완료 | 실루엣 유지 재칠/선화색 변경; 실제 펜 gate는 별도 |
| 하위 레이어 클리핑 | E3 연속 stack·그룹/base·숨김/순서와 CPU/GPU·actor·재열기 검증 | Essentials E3 / ADR-0057 구현/자동 검증 완료 | 별도 그림자 작업. raw raster 썸네일과 합성 결과 구별 |
| 레이어 blend mode | E3 Normal/Multiply와 opacity, CPU/GPU byte 일치 검증 | Essentials E3 | Screen/Add 및 다른 mode는 이번 목표 밖 |
| 레이어 마스크 | 없음. 선택 mask는 session 도구이지 layer mask 아님 | Sprint 5 | 지우지 않고 복원하는 마무리 작업 W5; W4는 clipping/selection으로 일부 우회 |
| 색상·명도 보정 | UI 색 선택과 다름; 그림 대상 보정 없음 | Sprint 5 | 색상/채도/명도와 밝기/대비의 최소 경로 W5. 조정 레이어 일반 프레임워크는 후속 |
| 블렌더·smudge·질감·수채 | 없음 | texture Sprint 4, smudge는 제외 | 경로 B에서 화풍별 필수일 수 있음. 기본 덧칠과 색 채취 gate 후 별도 브러시 단계 |
| 레이어 병합·그룹 복제 | 없음 | 병합 F3 / 그룹 복제 후속 | 작업을 끝내는 데 필수인 경우 우선; 단순 정리 목적이면 W5 이후 |
| 저장·Undo·PNG 연결 | 구현·핵심 검증 기록 있음; 128 Undo 제한 | Sprint 1~3 | 전 단계 필수. 실제 작업 파일 대신 scratch로 반복 저장/재시작/출력 검증 |

관련 공식 기능: [Krita brush smoothing/임시 조작][K3], [Krita fill][K4],
[CSP fill][C3], [CSP stabilization][C4], [Krita selections][K7],
[CSP selection][C8], [CSP transform][C9], [CSP modifier keys][C7].
단축키는 비교 자료일 뿐, 사용자가 지정한 NyatiDraw G/B/E/F 분류를 경쟁 앱 기본값으로 덮지 않는다.
사각형 **선택**과 사각형 **그리기 도구**도 별개이며 후자를 임의 추가하지 않는다.

### 코드 확인 지점

- `crates/brush/src/lib.rs`: W1a engine2는 size/opacity 필압 독립·최소 비율·경도를 기록한다.
  engine1 재생은 기존 sqrt-size/linear-opacity/hard circle을 보존한다. 임의 curve 없음.
  `crates/input/src/smoothing.rs`의 E1a 보정은 GPU 평가/저장 sample 전에 한 번만 적용한다.
- `apps/desktop/src/native_canvas.rs`: W1a `DrawingConfig`는 네 그리기 도구의 마지막 설정을
  세션 중 기억하고 Begin의 immutable preset으로 고정한다. 재시작 설정 보존은 후속이다.
- `crates/api/src/protocol.rs`: W1b ViewportCommand에 보기 mirror/reset 추가. RasterTransform의
  정수 offset/quarter-turn/flip/size와는 별개다. E3 LayerCommand에 alpha lock/clip/blend 추가.
- `crates/document/src/layers.rs`: visible/locked/reference/opacity/content_root와 E3 raster
  alpha lock·raster/group clipping·Normal/Multiply. `compositing.rs`는 원래 형제 순서의 stack과
  Solo/참조의 base 의존성을 제공한다. layer mask와 자료 이미지 타입은 없다.
- `crates/paint-cpu/src/selection.rs`, `apps/desktop/src/edit_worker.rs`: bounded signed ROI binary mask,
  사각/올가미 선택 조합·유한 반전·부분 삭제. E2 `flat_fill.rs`는 signed 유한 domain에서
  참조 연결 fill과 선택 clip·gap closing·barrier backing을 처리하고 `selection_morph.rs`는
  원판 확장/축소와 raw alpha 선택을 제공한다.
- `apps/desktop/src/edit_gesture.rs`: Fill은 선택 유무와 관계없이 연결 fill 명령을 생성한다.
  MoveSelection은 E1b 원본 고정 draft를 preview하고 명시 확정/취소하며 선택을 유지한다.
- `apps/desktop/src/artwork_clipboard.rs`: private+PNG 교환과 명시적 DIB 미지원.
- `docs/sprints/sprint-04-brush.md`, `sprint-05-reliability.md`: 기본 그림 도구와 고급 표현,
  필수 선택/채색과 대형 프로젝트 최적화가 너무 큰 후속 묶음으로 함께 밀려 있었다.

## 4. 새 실행 순서: W0~W5

[Essentials E1~E3](sprints/essentials-goal.md)의 자율 구현/자동 검증을 완료했다.
W1 잔여·W2 형태 수정 → W3 밑색 → W4 기본 명암을 연결했으며 아래 W 실사용 gate를
대체하지 않는다. 다음은 기준 그림을 실제로 그려 확인하는 단계이고 W5를 자동 추가하지 않는다.

시간 견적이나 기능 개수 대신 아래 실제 작업을 통과하면 다음 단계로 간다. 한 W 안에서도
가장 먼저 막힌 항목만 작은 변경으로 고치고 다시 같은 작업을 한다.

### W0 — 기준 그림으로 현재 막힘 확인

- 작은 캐릭터 반신/전신 한 장: 머리 일부/팔을 페이지 밖에도 그리고, 선화·머리/피부/옷
  밑색·그림자 한 단계·하이라이트·선화 재칠·PNG 출력까지 시도한다.
- 별도 작은 명암 덩어리 그림: 큰 칠→작은 칠→색 채취→부드러운 경계와 단단한 경계.
- 러프/선화/밑색 이름의 레이어는 작업자가 필요할 때 만든다. 앱 초기 레이어 세트가 아니다.
- 매번 실제로 못 한 동작, 우회 횟수, 잘못 선택된 도구/레이어, UI 이동, 펜 필압 결과를
  기록한다. 이미 있는 기능의 UI 고장과 기능 부재를 분리한다.
- 실제 펜 작업이 제공되지 않으면 미검증으로 남기고 현재 코드 감사 결과만으로 합격하지 않는다.

### W1 — 손을 떼지 않고 러프·선화·덧칠

- 크기 필압/농도 필압 독립 on/off·최소값; 불투명 선용 hard pen, 러프용 브러시,
  soft paint/eraser의 최소 preset. 대형 범용 브러시 편집기는 만들지 않는다.
- 필요량을 조절할 수 있는 최소 보정(끄기 포함), 빠른 색 채취 후 원래 도구 복귀,
  크기 빠른 조절·도구별 마지막 값, 보기 반전/회전 초기화.
- 보정 지연과 시스템 입력 지연을 구분하며 원래 샘플 유실을 보정으로 가리지 않는다.
- gate: 필압으로 굵기만 바꾸는 선, 흐린 러프, 진한 밑칠, 부드러운 칠, 지우기·도구 복귀,
  색 채취·크기 변화·보기 반전 뒤 동일 문서 좌표로 계속 그리기가 가능하다.

### W2 — 비율과 배치를 고쳐도 그림이 망가지지 않기

- page-independent signed selection 범위와 메모리 상한을 먼저 정한다. 이후 AA/feather를
  지원할 coverage 표현을 고려하되 무한 크기의 dense mask를 만들지 않는다.
- 사각/올가미의 replace/add/subtract, 전체선택/반전/해제/선택 픽셀 삭제를 구분한다.
- 선택 픽셀 이동·scale/rotate를 직접 보면서 조정하고 확정/취소; 선택 유지/재이동.
  nearest-only·90도 제한은 일반 일러스트용 변형으로 완료 처리하지 않는다.
- 기존 paste를 이 편집 상태로 연결하되, 새 floating UI 기능만 독립적으로 확장하지 않는다.
- gate: 페이지 경계를 넘는 팔/머리를 옮기고 회전/크기 수정→취소→재시도→Undo→저장→
  재열기. 다른 픽셀·레이어·반투명 경계를 보존한다.

### W3 — 선화 밑색을 편하게 끝내기

- 빈 색 레이어에서 참조 선화로 fill, 선택 grow/shrink, 틈 닫기, AA 가장자리 처리.
- 범위·tolerance·gap·확장을 구분하고 작은 미채움 부분은 선택 추가/빼기와 브러시로 보정.
- alpha에서 선택 만들기 및 alpha lock 최소 기능을 이 단계 말미부터 W4 시작에 배치한다.
- gate: 가는 선/굵은 선/AA선·작은 틈·이중 경계·서로 붙은 피부/머리카락 사례를 채우고,
  밝고 어두운 임시 배경 모두에서 흰 틈·누출 확인. 투명 가장자리와 PNG가 일치한다.

### W4 — 실루엣을 보존하며 명암과 색 수정

- alpha lock의 alpha 보존 의미, CSP형 clipping stack, 최소 blend(Normal/Multiply 우선).
- soft brush·임시 picker와 결합해 별도 그림자/하이라이트를 칠한다. 지우개·선택·fill·변형과
  각 제한의 관계를 정의한다. 기존 편집 lock은 별도로 유지한다.
- clipping/opacity/group 합성 순서는 CPU export, GPU 표시, picker, thumbnail, Undo/reopen에서
  일치해야 한다. UI 토글만으로 완료 처리하지 않는다.
- gate: 머리색 변경과 선화색 재칠, 피부 밖으로 넘치지 않는 별도 그림자, base를 수정했을 때
  clipping 결과 갱신, 색/alpha 경계와 PNG 일치.

### W5 — 마무리와 반복 사용

- 실제 기준 그림에서 필요했던 색상/채도/명도 보정, 최소 layer mask 및 정리 작업.
  아래 병합은 필요성 확인 후 이 단계에서 다룬다. 자료 이미지 기능도 외부 창 우회가
  작업을 자주 끊을 때 구체적인 작은 범위로 도입한다.
- 선화형 그림과 덧칠형 그림을 각각 끝내고 저장/닫기/paired PNG 재열기/재수정까지 수행.
- release build/정상 설치본의 클릭·텍스트 입력·단축키·펜·창 동작 확인. 113개 core 시험이
  통과했던 사실은 이 작업 흐름의 합격을 대신하지 않는다.

## 5. 채색 생산성 후속 요구 — 계획 기록만, 구현 보류

사용자 요구: 정밀하게 경계를 따라 칠하거나 미채움/넘침을 반복 정리하는 시간을 줄인다.
아래는 W3 채색 생산성의 후속 후보이며, 완료된 Essentials E1~E3의 범위를 늘리거나
새 자율 구현을 승인하는 항목이 아니다. 착수 순서와 알고리즘은 다음 계획에서 확정한다.

- **틈 닫기와 미채움 보정**: 덜 마감한 선화에서도 bucket fill의 누출과 작은 미채움 부분을
  줄이고, 남은 부분은 넓게 쓸어 보정하는 도구를 검토한다. 기존 E2의 최소 gap closing·
  선 밑 확장만으로 이 사용성이 완성되었다고 보지 않는다.
- **둘러싸서 채우기 / 올가미 채색**: 선 바깥까지 대충 둘러싸도 참조 선화로 구분되는
  의도한 영역을 채운다. 올가미는 탐색 범위이지 무조건 전부 칠할 모양이 아니다.
  기존 올가미 선택 + 선택 전체 칠하기와 구별한다. 여러 영역/외부 배경의 포함 기준은 미정이다.
- **선 넘침 방지 브러시**: 참조 선화의 경계를 인식해 브러시가 넓게 걸쳐도 의도한 쪽만
  칠한다. 색 판정 허용오차, 틈 닫기, 픽셀 단위 영역 확장은 별도 의미로 설계한다.
  스트로크 시작 영역 고정과 현재 브러시 중심 영역 추종은 다른 사용감이므로 혼동하지 않는다.
  기존 alpha lock·하위 레이어 clipping만으로 이 기능을 구현했다고 보지 않는다.
- **삐져나온 부분 정리**: 채색 넘침과 선화 자체의 교차점 너머 선 정리를 구별한다.
  후자는 머리카락/주름 같은 의도된 선을 임의 삭제하지 않도록 사용자 지정 범위와
  보존 기준을 먼저 정한다. 자동 선화 정리 방식은 아직 선택하지 않았다.

공통 후보는 참조 경계·연결 영역 마스크의 재사용이지만 구현 방식은 확정하지 않는다.
특허 검토도 미완료다. JP5059704B2·JP5128384B2는 조사 출발점일 뿐이며, 기능명이나
다른 앱의 제공 여부만으로 침해/비침해를 단정하지 않는다. 구현 착수 전 후보 알고리즘과
관련 독립 청구항을 비교하고, 공개 배포 전 대상 국가의 권리 상태와 추가 관련 특허를 검토한다.
현재 문서는 비침해 보증이나 법률 의견이 아니다.

## 6. 후순위와 검증 정책

질감/산포/복잡한 dynamics·smudge/wet paint, 고급 필터 전체, 벡터 선 수정·텍스트,
애니메이션, 고급 선택 보조·자/대칭·3D 소재, 광범위 PSD/외부 앱 호환은 별도 단계다.
필요가 확인되면 재평가하지만 기능 목록에 있다는 이유만으로 기본 그림 완성보다 먼저 하지 않는다.
macOS/Linux와 Godot addon 보류도 유지한다. 성능 최적화는 기능 완성 뒤로 두되,
작업을 막는 크래시·입력 지연/유실·저장 문제는 즉시 다룬다.

자동 시험은 기존 정책대로 artwork/input/history/recovery 핵심 불변식만 추가한다.
기능이 있어도 UI/물리 펜 조작이 안 되면 미완료다. 스프린트가 끝날 때 결과 그림/재현 동작,
작동하는 우회, 남은 막힘을 함께 남긴다. 제품 버전/날짜 변경·설치·배포는 이번 감사 범위 밖이다.

[K1]: https://docs.krita.org/en/tutorials/common_workflows.html
[K2]: https://docs.krita.org/en/tutorials/flat-coloring.html
[K3]: https://docs.krita.org/en/reference_manual/tools/freehand_brush.html
[K4]: https://docs.krita.org/en/reference_manual/tools/fill.html
[K5]: https://docs.krita.org/en/tutorials/clipping_masks_and_alpha_inheritance.html
[K6]: https://docs.krita.org/en/reference_manual/tools/reference_images_tool.html
[K7]: https://docs.krita.org/en/user_manual/selections.html
[K8]: https://docs.krita.org/en/general_concepts/colors/color_mixing.html
[C1]: https://tips.clip-studio.com/en-us/articles/1250
[C2]: https://tips.clip-studio.com/en-us/series/9
[C3]: https://tips.clip-studio.com/en-us/articles/590
[C4]: https://support.clip-studio.com/en-us/faq/articles/20200030
[C5]: https://help.clip-studio.com/en-us/manual_en/180_layers/Other_layer_settings.htm
[C6]: https://help.clip-studio.com/en-us/manual_en/210_file/Import_reference_images.htm
[C7]: https://help.clip-studio.com/en-us/manual_en/720_preferences/Modifier_Key_Settings.htm
[C8]: https://help.clip-studio.com/en-us/manual_en/330_selection/Selection_area.htm
[C9]: https://help.clip-studio.com/en-us/manual_en/360_transform/Types_of_transformations.htm
[C10]: https://help.clip-studio.com/en-us/manual_en/180_layers/Layer_properties.htm

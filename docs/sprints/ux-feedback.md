# 드로잉 UX 수정 묶음

[문서 목록](../README.md) · [요청별 구현 체크리스트](ux-feedback-checklist.md) ·
[UI 의미론](../editor-ui-target.md) · [W0~W5 우선순위](../illustration-workflow.md) ·
[브러시 비교 조사](../research/brush-engines.md) · [자체 연필 설계](../research/pencil-brush-design.md) ·
[반복 구현 경계](../research/repeat-implementation.md)

사용자의 실사용 피드백을 기준으로 묶었다. 엔진 기능 수를 늘리기보다 작업을 막힘 없이
사용하는 것이 우선이다. 후속 지시로 목록의 구현 가능한 항목과 최소 자체 연필 개발도 진행한다.
최초의 조사 한정을 현재 구현 금지로 재사용하거나, 어려운 항목을 동의된 보류로 바꾸지 않는다.

구현 여부와 검증 범위, 설치·배포를 구별한다. **최신 Clippy·통합 시험·DX 릴리즈 빌드가 통과했고,
Windows / RTX 3080 / Dx12 / release / scale 1의 scratch 창에서 아래 조작과 저장·재시작을 확인했다.**
버블의 hold/drag 중간 표시·추종 감각, Alt/취소·빠른 입력·다른 DPI·물리 펜 검수는 남는다.
이번 별도 릴리즈 실행본을 기존 설치판 갱신과 혼동하지 않는다.

## 병렬 경계와 현재 상태

| 묶음 | 구현 범위 | 검증 상태·남은 일 |
|---|---|---|
| UX-1 조작 | G 전환, 변형 Begin/preview/취소, Esc, native C | template identity 전환 보완, 통합·변형 actor GPU·별도 프로세스 재열기 통과. 모든 template/변형 UI 조합은 미검수 |
| UX-1 picker | native C/Alt press/drag의 13×13 확대 patch·후보색, End 확정·Cancel/token guard | 구현·통합 시험 통과. 새 창 C drag의 End 색 확정·추가 획 없음 확인. hold/drag 버블·Alt/Cancel 조합은 미검수 |
| UX-1 색상 | SV fit, 공유 최근색 10칸·고정 사용자 설정, accepted painting Begin MRU | 새 창에서 색상환 선택만으로 MRU 불변, accepted 획 뒤 양쪽 같은 최근색 추가 확인. pin 전체 조합은 별도 |
| UX-1 셸 | 메뉴 overlay, page dialog, 취소·알림, 도구·레이어 설정, selection 5아이콘, opacity 드래그·재배치 | 새 DX 스타일 번들·아이콘 간격·opacity 100→48·Undo→100·Redo→48 확인. 그룹 설정 전체 조합은 별도 |
| 브러시 연구 | Procreate/CSP/Krita/MyPaint/Fresco 표현 원리 비교와 자체 설계 | 조사 작성. 외부 엔진 채택·코드 복사·C FFI가 목표가 아님 |
| UX-2 최소 연필 | 자체 upright dry-pencil v3, 2H/2B template, 세션 다섯 설정 슬롯, 실제 preset preview | core/GPU·통합·DX 통과. 새 창 2H/2B 획·preview·기본값·경도 숨김, 저장·재시작 픽셀 보존 확인. 물리 펜은 별도 |
| UX-3 반복 | 독립 프로젝트 X/Y·읽기 전용 복사본·offpage 보존·단일 PNG의 설계 | **미구현**. clip/wire/CPU·GPU/입력까지 필요한 독립 스프린트 경계를 문서화 |
| 후속 표현 | 비파괴 레이어 색상화, 메시 변형 엔진, 범용 wet·smudge | **미구현**. 준비 중 표시나 두 dry 연필로 완료 처리하지 않음 |

## UX-1 의미

### 도구와 속성

- 현재 시작 도구는 검정 **2H 샤프펜슬**, 크기 5px, opacity 100%다.
  도구 전환은 전경색을 초기화하지 않는다. 새 릴리즈 창에서 이 기본값과 2H/2B의 다른 획·preview를 확인했다.
- 스포이트는 **C**. Ctrl+C 복사, Ctrl+Shift+I 선택 반전은 유지한다.
- 세부 도구는 현재 가족 내부다. 연필은 2H 샤프·2B 연필을 고르며 펜/브러시를 같은 목록에 섞지 않는다.
  다른 가족은 해당 기본형을 표시한다. 범용 preset 라이브러리가 구현됐다는 뜻은 아니다.
- 속성 상단은 현재 template의 실제 preset, evaluator와 CPU rasterizer를 사용하는 160×48 scratch 획이다.
  흑백·합성 필압이며 큰 크기는 표시용으로 제한한다. 1:1 크기·보정 감각·물리 펜 품질의 증거는 아니다.
  실제 Begin, Undo, 문서 dirty, 최근 크기·색을 발생시키지 않는다.
- 일반 브러시의 경도는 흑연 H/B가 아니라 **가장자리 경도**다. 0% 부드러움/100% 단단함을 설명하고
  필압과 함께 추가 옵션에 둔다. 자체 Pencil은 round 경도 조절을 숨긴다.
- 주요 수치는 우측 정렬하며 크기 빠른 선택 패널은 분리한다. 최근 크기 네 개는 accepted Begin에 기록한다.

### 변형과 취소

- G 계열의 명칭은 **변형**이며 상단과 같은 자유 변형 작업이다. 도구 선택만으로 draft를 생성하지 않는다.
  직접 드래그 또는 명시적 시작이 생성하며 현재 선택 영역이 필요하다. 선택 없이 전체 레이어 자동 변형으로
  표시하지 않고 시작 버튼도 선택이 없으면 비활성화한다.
- X/Y, 폭/높이는 각각 한 줄에 나란히 둔다. 숫자 편집 중 문서 단축키를 실행하지 않는다.
  수치 변경은 preview 검증 뒤 확정하며 무효 수치를 artwork에 적용하지 않는다.
- 다른 도구로 바꾸면 미변경 draft는 취소하고 유효한 변경은 최신 preview 확정 뒤 전환한다.
  Begin/preview 중 취소·전환 의도는 bounded 상태로 보존한다.
  시작·확정·취소 버튼이 사라지거나 비활성화되기 전에 편집기 scope로 포커스를 옮긴다.
- 새 `SelectPencilTemplate` 명령도 `selected_tool`에서 전환으로 인식한다. `pending_pencil_template`에
  2H/2B identity를 보존하고 draft 확정/취소 뒤 적용하도록 누락을 수정했다. 명시 취소·실패에서는
  pending identity도 함께 정리한다. 이 소스 보완과 실제 창의 모든 전환 조합 검증은 구별한다.
- Esc는 한 번에 열린 UI → 진행 중 조작 → 선택의 한 단계만 처리한다.
  반복 keydown으로 취소 직후 선택까지 연달아 지우지 않는다.
- 상단은 비우기·흰 배경·변형·페이지·취소 순서다. 취소는 가능한 작업/선택이 있을 때 활성화하며,
  Begin/preview와 이미 수락한 durable commit의 취소 불가를 구별한다.
- 메시 변형은 준비 중 비활성 표시뿐이다. **메시 엔진은 미구현**이며 affine을 메시로 가장하지 않는다.
- 선택 픽셀 수는 상시 표시하지 않는다. 선택 액션은 전체 선택·반전·해제·삭제·그림에서 선택의
  아이콘 다섯 개와 별도 반경·확장/축소로 배치했고 누락된 간격·비활성 스타일을 추가했다.
  최신 DX asset에 CSS가 포함됐고 scale 1 창에서 아이콘과 확장/축소 간격을 확인했다. 다른 DPI·모든 비활성 상태는 별도다.

### 메뉴·페이지·레이어·알림

- 버거 메뉴는 같은 상단 줄을 불투명 배경으로 덮고 toolbar·캔버스를 밀지 않는다.
  뒤쪽 컨트롤의 포인터·키보드 실행 차단은 전체 조합의 수동 검수를 남긴다.
- 페이지는 출력 크기 dialog다. 도구 속성을 대체하지 않으며 page 밖 픽셀 보존과 Resize/Crop Undo는 유지한다.
- 레이어 목록 위 첫 줄에 mode·잠금·alpha lock·clipping·참조를 모으고 둘째 줄에 opacity를 둔다.
  행에는 큰 썸네일·이름·가시성·Solo·상태를 남긴다. 그룹 썸네일/이름은 그룹 설정 대상을,
  래스터 행은 그릴 레이어 설정을 선택한다. 그룹 설정 대상과 그릴 래스터를 혼동하지 않는다.
- opacity 슬라이더는 드래그 중 수치를 표시하고 **놓을 때 명령 한 번으로 적용**한다.
  숫자 직접 입력도 유지한다. **드래그 중 그림을 바꾸는 실시간 preview는 구현하지 않았다.**
  새 창에서 100→48 드래그, Undo 한 번→100, Redo→48 및 저장·프로세스 재시작 뒤 48% 보존을 확인했다.
- Solo는 보기 격리, 참조는 선택·채우기 source membership이다. 레이어 색상화와 다르다.
  **비파괴 레이어 색상화는 미구현**이며 준비 중 표시·alpha lock 재칠로 대신하지 않는다.
- 일반 오류 알림은 닫기 버튼과 8초 표시 수명을 가진다. 알림을 숨겨도 PNG 재시도·종료 복구 상태는 유지한다.

### 색상과 picker

- SV 정사각형 대각선은 색상환 안쪽 지름보다 작다. 기존 패널 높이 안에 wheel과 하단 최근색 한 줄을 맞춘다.
  HEX 상시 표시·기본 팔레트·중복 설명 라벨은 제거했다.
- 상단·색상 패널은 **고정 포함 최대 10칸**을 공유한다. 고정색은 위치를 유지하며 새 최근색은
  빈 칸 또는 가장 오래된 비고정 칸만 사용한다. 전부 고정이면 현재 전경색은 바꿀 수 있지만 고정 칸을 밀지 않는다.
- pin은 사용자 설정 파일에 보존하며 작품 저장·Undo와 별개다. 읽지 못한 손상 설정을 자동 덮어쓰지 않는다.
  전경/배경 well과 X 교환은 최근색과 별도로 유지한다.
- 최근색은 색을 고른 시점이 아니라 **accepted painting Begin**의 원본 sRGB 색으로 갱신한다.
  bounded 10칸 경로이며 C/Alt picker·지우개·Undo/Redo는 사용색으로 기록하지 않는다.
  새 창에서 색상환 선택 시 목록 불변, 실제 accepted 획 뒤 양쪽에 같은 최근색 추가를 확인했다.
- native C/Alt 접촉 중에는 13×13 원본 픽셀 patch를 확대하고 후보색을 보여 준다.
  preview는 전경색·artwork를 수정하지 않으며 End에서만 채취 결과를 확정한다.
  Cancel·gesture token·viewport 변경 등으로 오래된 결과가 나중에 색을 덮어쓰는 것을 차단한다.
  새 창에서 C 단축키 뒤 캔버스 drag·End로 빨강 전경색 확정과 추가 획 없음을 확인했다.
  **hold/drag 중간 버블 표시는 직접 capture하지 못했다.** 추종 감각·Alt·Cancel·빠른 입력 조합은 미검수다.

## UX-2: 최소 자체 2H/2B 연필

[자체 연필 설계](../research/pencil-brush-design.md)의 upright dry-pencil v3를 구현했다.
외부 엔진을 채택하지 않고 기존 native input → evaluator → GPU live → CPU replay → immutable 저장 구조를 유지한다.

- 2H 샤프·2B 연필은 고유 preset identity와 서로 다른 최소 굵기 비율·flow·종이 결 침착량을 가진다.
  흑연 H/B를 round 경도 수치로 이름만 바꾼 것이 아니다.
- 문서 좌표에 고정된 절차적 종이 결을 사용해 획·타일마다 결을 재시작하지 않는다.
- 2H·2B·펜·브러시·지우개의 다섯 세션 슬롯에 마지막 크기·opacity·필압·보정 설정을 보존한다.
  앱 재시작까지 개인 preset 설정을 저장하는 기능과는 다르다.
- v3만 새 연필 분기를 사용하며 v1/v2 stroke bytes/hash·재생 경로의 호환을 유지한다.
- 최소 upright 모델이다. 물리 tilt 접촉·범용 bitmap 재질·wet·smudge·실제 캔버스 혼색은 미구현이다.
- 새 2H/2B GPU/CPU 비교는 엄격한 차이 0, 기존 round 검사는 기존 허용차 4를 유지한 채 통과 보고를 받았다.
  새 프로세스 저장·재열기 core 검사도 통과 보고가 있다. 상세 명령·대상·환경은 담당자가 후속 기록한다.
  core 증거와 별도로 새 Dx12 릴리즈 창의 두 연필 획·preview·저장 재시작을 확인했다. 물리 펜 표현 검증으로 확대하지 않는다.

## UX-3: 반복 (반전과 다름)

**아직 미구현이다.** 가로 반복(X), 세로 반복(Y)을 독립 프로젝트 속성으로 저장하고 출력 페이지를
해당 축으로 반복 표시한다는 요구다. 반복 축 밖 기존 artwork는 지우지 않고 가렸다가 끄면 다시 보여야 한다.
복사본은 읽기 전용이고 꺼진 축의 무한 작업공간은 유지하며 PNG는 원본 페이지 한 장이다.
보기 반전·실제 픽셀 반전·복사본에 그려 원본으로 wrap하는 기능과 구별한다.

[반복 구현 설계](../research/repeat-implementation.md)에 실제 저장 backend와 심볼별 변경 지도를 기록했다.
현재 production은 redb이며 SQLite 비교 실험을 마이그레이션 완료로 해석하지 않는다.
`CanvasSpec` 확장 대신 독립 설정 metadata를 둘 수 있지만, 표시만으로 완료할 수 없다.
Begin에 고정한 analytic clip을 GPU dab 전체 footprint·stroke hash/wire·CPU replay에 전달하고,
sparse 원본 가림·source page cache·hit-test·다른 pixel edit 보존까지 연결해야 한다.

현재 브러시/picker의 동일 파일 변경과 충돌하지 않도록 독립 스프린트 경계를 설계했다.
이는 기술적 분할이며 사용자 요청 완료나 동의된 보류가 아니다. 중앙점 검사나 표시 토글만으로
반복 전체를 지원한다고 표시하지 않는다. 저장·프로세스 종료·재열기·원본 PNG 대조가 필요하다.

## 검증 기록과 아직 하지 않은 확인

### 최신 추가분

- 구현 범위: accepted Begin 최근색, opacity release 적용·재배치, selection 아이콘 CSS,
  C/Alt 13×13 확대·End commit·취소 guard, 자체 연필 v3와 template UI·preview·Pencil 경도 숨김.
- 자체 연필의 개별 core/GPU·새 프로세스 저장·재열기는 담당자의 통과 보고가 있다.
  상세 결과는 후속으로 기록하며, 이 문서 갱신에서 Cargo·GUI·웹을 실행하지 않았다.
- workspace all-target/all-feature Clippy `-D warnings` 통과. 기존 vendored Wry 경고와 앱 코드 결과를 구분한다.
- 통합 시험 총 118개 통과: desktop 50개 통과·4개 ignored 포함. 마지막 brush/native 취소 수정 뒤
  brush 3개와 desktop 50개를 다시 검사해 통과했다. ignored 항목을 실행한 것으로 세지 않는다.
- 실제 변형 actor GPU 검사를 Windows / RTX 3080 / Vulkan / debug에서 다시 통과했다.
  Save 뒤 새 프로세스의 artwork root·PNG 대조도 재통과했다. 합성 입력/actor 증거이며 실제 펜·창 UI와 다르다.
- `tools/build-dev.ps1` DX 릴리즈 빌드가 54.32초에 성공했고 `styles.css`·`colors.css`가 번들에 포함됐다.
  이는 단일 빌드 기록이며 성능 벤치마크나 설치판 갱신이 아니다.
- 최신 실제 창 환경은 **Windows / RTX 3080 / Dx12 / release / scale 1**이다. 위 Vulkan/debug actor 시험과 구별한다.
  설치본과 분리한 scratch에서 검정 2H·5px·100%, 2H/2B template와 서로 다른 실제 획·preview,
  Pencil 경도 숨김, 색상환 선택 시 MRU 불변 → accepted 획 뒤 양쪽 같은 최근색 추가를 확인했다.
- opacity 100→48 드래그, Undo 한 번→100, Redo→48을 확인했다. 선택 아이콘 다섯 개·확장/축소 간격도 확인했다.
- C 단축키 → 캔버스 drag → End 뒤 빨강 전경색 확정, 불필요한 추가 획 없음을 확인했다.
  버블을 hold/drag하는 중간 표시를 직접 capture하지 못했으므로 확대 표시·추종 감각 전체를 합격으로 기록하지 않는다.
- 저장 뒤 프로세스를 종료하고 재기동해 2H/2B 픽셀·레이어 opacity 48%·artwork root 보존을 확인했다.
  재출력 PNG의 SHA-256도 동일했다:
  `31FCC28FFCCA8411DB36944E6D6EAA35C245DEBD1DBE7EB7949C19873B0AA552`.
- template 전환 intent 누락은 수정됐다. 다섯 세션 슬롯·변형 전환의 모든 UI 조합까지 검수한 것은 아니다.

### 이전 통합·실행본에서 확인한 범위

다음은 최신 추가분 이전의 기록이며 새 소스 전체의 성공 근거가 아니다.

- `cargo check --workspace --all-features --locked`, workspace Clippy와 desktop 전체 target/feature
  Clippy 통과. vendored Wry/Dioxus 경고와 새 앱 코드 검증은 구분했다.
- `tools/build-dev.ps1` DX Windows WebView 릴리즈 빌드·CSS asset 번들링 통과.
- `cargo test -p nyatidraw-api -p nyatidraw-editor --locked`: 핵심 4개 통과.
  desktop 기본 49개 통과, 명시 실행 GPU/레지스트리 관련 4개는 기본 실행에서 제외.
- `actor_preview_cancel_commit_matches_gpu_and_durable_artwork` 명시 실행 통과:
  Windows / RTX 3080 / Vulkan / debug. Begin/preview 취소, 직접 변형 시작, 미변경 draft 전환,
  최신 preview 확정 후 전환, 무효 preview 뒤 마지막 유효값 재검증, Undo를 확인했다.
  Save 뒤 새 테스트 프로세스의 artwork root·history·PNG 픽셀 동일성도 확인했다.
  합성 입력/actor 증거이며 실제 창 키보드·물리 펜·first-visible-pixel 증거가 아니다.
- 설치본과 분리한 scratch 프로젝트·layout·palette의 실제 릴리즈 창에서 기존 round 검정 연필·preview,
  메뉴 덮기/닫기, page modal·Tab·Esc, 마우스 획·재열기, 변형 G, C, Esc 선택 해제를 확인했다.
  기본 높이의 변형 수치·세 버튼, SV fit, 상단/사이드 색상·pin 동기화와 설정 기록도 확인했다.

### 남은 수동 gate

- picker 버블의 hold/drag 중간 표시·추종 감각, Alt/Cancel·빠른 입력 조합, 숫자 편집/IME,
  modal 뒤 native 입력 차단, 좁은 도크·다른 DPI, pin 재시작·10칸 포화, 래스터·그룹·template 전환 전체 조합.
  자동 core 검증을 이 실제 UI 조합의 확인으로 바꾸지 않는다.
- 재기동 직후 S 단축키는 한 번 성공했다. 기존 `initial-focus-deferred` 로그는 남아 있어
  최초 WebView 포커스의 모든 조합까지 해결됐다고 하지 않는다.
- 물리 펜·고주사율·다른 DPI/모니터·설치판 업데이트는 별도 수동 gate다.
- 검수 창은 종료했다. 이번 작업은 commit/tag/push/release·설치판 갱신을 수행하지 않았다.

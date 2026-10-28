# 필요한 드로잉 작업의 공백 감사

2026-10-28 최초 코드 감사와 같은 날 승인된 구현의 후속 상태를 함께 기록한다. 기준은
[W0–W5](../illustration-workflow.md)와 [기본 작업 연결](../core-workflow-completion.md)이며,
CSP·Krita·Photoshop의 전체 기능 복제를 목표로 하지 않는다. 작은 일러스트와 게임용 PNG를
Windows 및 공유 웹 편집기에서 안전하게 수정하는 데 필요한 작업만 판정했다.
이 문서는 실제 세 제품에서 같은 작품을 그린 비교 실험이 아니다. 후속 성능 측정은 아래 별도 증거에 연결한다.

감사 후 사용자 범위가 **일러스트와 게임 개발에서 CSP를 대체하는 그림·애니메이션 작업**으로 명확해졌다.
따라서 아래 정적 그림 감사는 유지하되, 애니메이션은 더 이상 보류 기능이 아니다.
Aseprite의 frame/cel/timing/onion/playback/spritesheet 경로를 추가 비교하고, native/web의
동일 문서·복구 모델을 설계하는 후속 절을 추가한다. UI 레이어 가시성 토글만으로
애니메이션을 구현했다고 보지 않는다.

## 승인된 구현의 현재 상태 (2026-10-28)

아래는 최초 감사 이후 작업 트리에 연결된 구현이다. `소스 연결`은 실제 앱 UI·설치본·
물리 입력 acceptance 통과를 뜻하지 않는다. 기능별 자동 검증과 실제 호스트 검증은
분리해서 기록하며, 초기 감사의 미구현 판정은 이 표로 갱신한다.

| 항목 | 현재 구현과 문서 | 확인 경계 |
|---|---|---|
| Screen/Add/Overlay | CPU/GPU, picker/export, 명시 wire tag와 공유 레이어 선택기 연결. [혼합 모드](../blend-modes.md) | [측정 기록](../measurements/composite-sparse-2026-10-28.md)에 RTX 3080/Vulkan 184 cases, raw 오차 0/display 최대 1 byte. 실제 드롭다운·작품 acceptance 별도 |
| 참조 자료 색 채취 | 공유 reference panel의 좌표 역변환·놓을 때 SetColor·epoch/generation 취소 연결. [참조 채취 구현과 격리 브라우저 증거](#감사-뒤-승인된-참조-색-채취-구현) | 동일 JS/CSS fixture의 13경우 브라우저 확인. 앱 host command/Undo/저장 acceptance를 대신하지 않음 |
| 그룹 affine | 모든 하위 래스터의 고정 원본·공통 pivot·단일 commit/Undo 및 native/web 명령 연결. [그룹 변형](../group-transform.md) | 구조·타일·취소·예산·재열기 invariants가 대상. 실제 핸들 조작·native 앱 acceptance 별도 |
| 래스터 애니메이션 | 명시 FrameId/CelId/duration, master tree/all-cel tiles, 원자적 snapshot timeline, NYWORK07(마스크 포함), 현재 cel 편집, 재생/onion, 공유 timeline UI 연결. [설계와 웹 연결](../animation-design.md), [native 연결](../native-animation.md) | 도메인/복구/metadata Undo·재열기와 앱 UI acceptance를 구분. 물리 입력·실제 설치본 통과를 선언하지 않음 |
| 크기 지정 PNG | 원본 캔버스/픽셀/Undo를 변경하지 않는 alpha-correct area reduction, Windows/web/CLI 별도 출력 연결. [크기 지정 PNG](../scaled-png-export.md) | 기본 Save companion 크기는 유지. 실제 파일 선택·다운로드 acceptance 별도 |
| 스프라이트 시트 | 전체 frozen timeline의 row-major PNG + frame rect/duration JSON, native/CLI 새 폴더·manifest-last, 웹 독립 Worker와 고유 PNG/JSON 이름 연결. [시트 출력](../animation-export.md) | 공유 픽셀·순서·예산 invariants와 호스트 게시를 분리. 웹은 두 다운로드 요청이며 실제 저장·게임 엔진 import는 미확인 |
| 지속 래스터 마스크 | typed R8 plane·명시 image/mask target·검정 숨김/흰색 및 지우개 복원·disable/remove·선택→mask·linked transform·복제/cel·동일 NTDR/NYWORK07 연결. [계약](../layer-mask-design.md), [동작](../layer-masks.md) | 원본 RGBA·Undo·coverage·합성 불변식과 실제 호스트의 mask stroke/저장/프로세스 재시작 증거를 구분. [통합 기록](../workflow-expansion-validation.md)에서 runtime gate를 추적 |
| 부드러운 선택/feather | 0–255 SelectionMask·Begin 고정·공유 CPU/GPU brush·fill/cut/보정/affine·bounded feather와 선택 UI 연결 | binary legacy 보존과 soft-edge 소비 경로를 자동 검증. GPU UNORM/기존 pencil 오차와 실제 앱 선택 조작 증거를 구분; 물리 펜 완료를 뜻하지 않음 |
| 희소 CPU 합성 | 없는 소스/클리핑 base 생략과 Normal 정수식 경로를 구현. [전후 측정](../measurements/composite-sparse-2026-10-28.md), [원시 JSON](../measurements/composite-sparse-2026-10-28.json) | release 4K 세 fixture 각 20회, 전후 전체 RGBA SHA256 동일. 수치는 아래 표이며 장치 전체 UI/펜 지연을 뜻하지 않음 |

**남은 실제 공백:** 그룹 마스크·마스크 단독/unlinked 변형·마스크 굽기·외형 보존 merge는
구현하지 않았다. 지속 raster mask와 fractional selection/feather의 최초 미구현 판정은
위 구현 상태로 갱신하지만, 실제 호스트 사용·저장/재시작 검증 완료 여부는 별도 기록을
따른다. 이 구현을 CSP/Aseprite 전체 기능 대응 완료로 보지 않는다.

**현재 runtime gate:** [웹 마스크·feather](../mask-web-acceptance.md)는 실제 조작과
저장/재시작/동일 파일 재열기를 통과했다. 네이티브 actor의 원본·복구는 통과했지만,
[Windows 재표시 문제](native-mask-presentation.md)가 남아 native visual acceptance는
미통과다. 실패한 FIFO/Vulkan/입력 영역/복사 비트 실험을 기능 완료 근거로 쓰지 않는다.

## 최초 감사: 공식 기능과 당시 코드

`구현`은 소스 경로가 있다는 뜻이다. 설치본·물리 펜·실제 작품 합격과 구별한다.
다음 표의 진행 중/미구현 표기는 **최초 감사 시점**이다. 위 현재 상태 표와 기능별 문서가 후속 구현의 기준이다.

| 작업 | 공식 제품의 관련 경로 | NyatiDraw 판정과 코드 근거 | 필요한 범위 |
|---|---|---|---|
| 러프·선화·부드러운 명암 | [Photoshop 크기/농도 필압][P-brush], [Krita freehand][K-brush], [CSP 보정][C-smooth] | **구현, 실펜 미검증.** `crates/brush/src/lib.rs::dab_for`의 독립 필압/최소값/경도, `crates/input/src/smoothing.rs`, `crates/api/src/brush_presets.rs` | 이름만 다른 브러시를 더 만들기보다 W1 hard/soft/eraser 전환과 끝점 감각 검증. 임의 필압 곡선·wet/smudge는 현재 필수 아님 |
| 자료를 보고 색을 가져오기 | [CSP Sub View][C-ref]는 그림을 누르면 전경색을 얻는다. [Krita Reference Images][K-ref]는 자료를 그림과 분리해서 배치한다 | **부분.** `crates/editor-ui/src/reference_panel.rs`는 PNG 표시·맞춤·확대·이동만 있고 `ReferenceFrame`에는 이름과 data URI만 있다. 캔버스 picker는 `crates/editor/src/pixel_edit.rs::sample_picker_pixel` | 자료를 작품 레이어로 가져왔다 지우는 우회 없이 자료창에서 색 채취. 다중 자료 보드/자료 저장은 별도 |
| 선택 후 한 부분 이동·회전·확대 | [CSP transform][C-transform], [Krita transform][K-transform], [Photoshop transform][P-transform] | **단일 래스터 구현.** `crates/editor/src/transform.rs::Transaction`의 `layer: LayerId`, `execute(... target: LayerId ...)`, `apps/desktop/src/transform_runtime.rs`가 active layer 전달. 원본 고정 preview/취소/단일 확정은 이미 있음 | 기존 affine 경로 재사용. mesh/puppet/liquify는 현재 불필요 |
| 선화·밑색·그림자를 함께 형태 수정 | [CSP 폴더/다중 레이어 변형][C-transform], [Krita 여러 레이어 변형][K-transform], [Photoshop 그룹][P-group] | **미구현, 큰 작업 공백.** 위 단일 target 구조이며 그룹 복제는 함께 변형을 대신하지 않음. `crates/web-core/src/transform.rs`도 공유 session 사용 | 먼저 **선택한 그룹의 하위 래스터 전체에 동일 affine**을 적용하는 원자적 transaction. 임의 다중 선택 UI보다 작은 범위 |
| 선택 경계와 부분 색 보정을 부드럽게 하기 | [Krita 선택][K-selection]의 grayscale selection, [Photoshop AA/feather][P-selection] | **부분.** `crates/paint-cpu/src/selection.rs::SelectionMask`는 0/1 및 packed bits; `selection_morph.rs`는 binary grow/shrink. `color_adjustment.rs`도 `contains_signed`로 통과/차단 | 실제 soft local correction이 필요하면 fractional coverage를 모든 소비 경로에 일관되게 연결. feather 버튼만 추가하거나 grow를 feather로 부르면 안 됨 |
| 경계를 기준으로 밑색·명암 칠하기 | 기존 W3/W4의 [Krita flat coloring][K-flat], [CSP 레이어 설정][C-layer] | **구현, 실제 작품 gate 별도.** `crates/paint-cpu/src/flat_fill.rs`, `crates/document/src/compositing.rs`, `crates/paint-cpu/src/compositing.rs`, `crates/paint-gpu/src`; 참조 fill·alpha lock·clipping·Multiply 있음 | 작은 틈/밝고 어두운 배경 가장자리 acceptance를 우선. 추가 Screen/Add/Overlay는 다른 작업에서 진행 중 |
| 지우지 않고 숨긴 뒤 복원 | [CSP layer mask][C-mask], [Krita transparency mask][K-mask], [Photoshop layer mask][P-mask] | **미구현.** `crates/document/src/layers.rs`의 `LayerNode/GroupNode`에는 mask root/편집 대상이 없고, `crates/api/src/protocol.rs::LayerCommand`에도 mask 명령 없음 | W5에서 유용하고 실제로 다른 기능으로 대체 불가. 지속 mask 타일·단일 Undo·합성/picker/export·재열기를 함께 설계해야 하므로 단기 UI 패치로 취급하지 않음 |
| 레이어 정리/병합 | [CSP 레이어 작업][C-layer-ui]에 아래 레이어와 병합 및 마스크 적용 제공 | **미구현, 당장 그림 완성을 막지는 않음.** `LayerCommand`에 merge 없음. 그룹/복제/가시성/잠금은 구현 | 그룹으로 정리는 가능. 아래 배경 의존 blend·clipping 외형 보존 계약 없이 merge down 금지. 필요하면 나중에 명시적인 visible-copy부터 평가 |
| 저장·PNG 크기 지정 | `docs/core-workflow-completion.md`에 별도 프로세스 재열기/PNG 일치 증거 있음 | **저장/기본 출력 구현, scaled export 진행 중.** `crates/web-core/src/lib.rs::export_rgba8`는 현재 페이지 크기 출력; `set_canvas`는 출력 영역 변경 | 원본 프로젝트 크기/타일을 바꾸지 않는 명시적 scaled PNG만 먼저. 원본 전체 리샘플링은 별도 명령 |

마스크 의미도 그대로 복제하지 않는다. Krita/Photoshop은 흑백 명도 방식이고 CSP는
그리기 색과 별개인 alpha 방식이다. NyatiDraw가 채택할 편집 모델을 먼저 정해야 한다.
벡터, macOS/Linux, wet/textured brush, 광범위 PSD, AI 자동 보정은 현재
범위에 필요하지 않다. 둘러싸서 채우기/선 넘침 방지는 기존 별도 보류를 유지한다.

## 최초 감사에서 제안한 변경 세 개

이 세 제안은 이후 모두 구현 작업으로 이어졌다. 아래는 당시 우선순위와 보존 조건의 기록이다.

1. **참조창 색 채취.** 공유 패널에서 누른 위치의 자료 픽셀을 미리 보여 주고 놓을 때
   기존 `ToolCommand::SetColor`로 확정한다. fitted/zoomed/panned 위치를 정확히 역변환하고,
   바깥·완전 투명·취소에서는 기존 색을 보존한다. 도구, 문서, 이력, 최근 사용 색을
   별도로 바꾸지 않는다. 최근 색은 실제 획에서만 갱신한다. 자료는 계속 세션 전용이다.
2. **그룹 단위 affine.** 선화·밑색·clipped shade의 같은 부분을 한 번에 고치는 것이
   다음 큰 우선순위다. 그룹 하위의 고정 대상 집합/원본/공통 pivot을 만들고, 모든
   preview를 통과한 경우에만 한 번 commit한다. 잠금 대상은 조용히 누락하지 말고
   작업 전체를 거절한다. bounds/메모리 한도, 취소·Undo, signed 영역, clipping 외형,
   저장→프로세스 재시작→재열기 픽셀/트리를 검증한다. 그룹 밖 clipping 관계도
   사전에 거절 또는 명시적으로 정의해야 한다.
3. **희소 합성의 빈 타일 처리.** 아래 측정 대상처럼 빈 소스 타일을 만들고 전부
   혼합하는 작업을 먼저 측정한 뒤 생략한다. 새 합성 모드 변경과 충돌하지 않도록
   `blend_pixel` 수식은 그대로 두고 `raw_tile/group_tile`의 기여 없는 분기만 다룬다.

그룹 변형은 이미 분리해서 그린 그림의 비율 수정을 직접 막는 공백이므로 최소 마스크보다
먼저다. 그 다음 W5에서 비파괴 복원이 실제로 필요한지 확인해 raster mask를 설계한다.
Feather는 편의 하나처럼 보이지만 binary selection의 저장된 획 재생·GPU clip·cut/move·
fill·색 보정까지 연결되므로 별도 coverage 변경으로 다룬다. 이 세 항목을 한 번에
완성했다고 선언하는 큰 변경은 권하지 않는다.

## 성능: 최초 관찰과 후속 측정

**최적화 전 소스에서 확인한 비용:** `crates/paint-cpu/src/compositing.rs::raw_tile`은
없는 래스터 타일에도 `vec![0; TILE_BYTE_LEN]`을 만든다. `group_tile`은 union 좌표마다
모든 visible stack의 이 버퍼를 전체 픽셀 혼합한다. 따라서 서로 겹치지 않는 많은
레이어에서도 union tile 수 × visible layer 수에 비례하는 불필요한 루프가 생긴다.
없는 source의 Normal/Multiply/Screen/Add/Overlay 기여는 0이지만, clipping base가
없을 때 clip을 독립 그림으로 올리는 식으로 바꾸면 안 된다. 숨김/opacity 0/group
격리/색 효과/clip 순서를 유지한 채 empty contribution을 전달하는 후보가 적합하다.

최초 감사에서는 새 측정 전이었다. 이후 [2026-10-28 전후 측정](../measurements/composite-sparse-2026-10-28.md)을 수행했고
세 fixture의 전체 RGBA SHA256 일치를 확인했다. 당시 제안한 측정 방법은 다음과 같았다.
기존
[`layer_composite_cost`](../../crates/paint-cpu/examples/layer_composite_cost.rs) release
fixture와 원본 byte oracle을 사용하고, 서로 떨어진 32개 래스터·중첩 group·clipping을
추가해 20회 이상 p50/p95/p99 및 타일 수/할당 횟수를 전후 기록한다. 정확한 픽셀 일치,
실제 export 및 별도 프로세스 재열기 확인을 통과한 뒤에만 개선 수치를 보고한다.

후속 측정 환경은 Ryzen 7 5800X3D, Windows NT 10.0.26300.0, Rust 1.96.0,
CPU backend, `cargo --release`, 3840×2160, warmup 1회 후 20회다. p99는 표본 최댓값이다.
PNG 압축·GPU 표시·펜 입력은 포함하지 않는다. OS 부하/온도 통제나 신뢰구간 추정은 없다.

| fixture | 전 p50 / p95 / p99 ms | 후 p50 / p95 / p99 ms |
|---|---|---|
| sparse8, 135 tiles | 263.059 / 309.314 / 317.013 | 51.318 / 70.055 / 87.631 |
| dense2, 1020 tiles | 339.701 / 398.038 / 411.942 | 224.021 / 236.332 / 277.181 |
| sparse-clipped8, 135 tiles | 254.718 / 302.242 / 337.416 | 25.489 / 29.826 / 29.922 |

기록된 과거 수치는 현재 버전의 성능이라고 읽지 않는다:

| 기록 | 환경과 범위 | p50 / p95 / p99 | 해석 |
|---|---|---|---|
| [2026-09-08 CPU 합성](../measurements/cpu-layer-composite-4k-2026-09-08.json), sparse8 | Ryzen 7 5800X3D, Windows 11 Pro 10.0.26200, CPU only, Rust 1.96 release, 3840×2160, 8 layers/135 tiles | 75.659 / 83.177 / 99.243 ms | 당시 최적화 후 20회. 현재 clipping/색 효과 합성기의 baseline은 아니므로 현재 regression으로 단정 불가 |
| 같은 기록, dense2 | 같은 환경, 2 layers/1020 tiles | 151.056 / 171.700 / 172.827 ms | CPU 합성만, PNG encode/문서 저장/실펜 제외 |
| [2026-09-06 전체 Undo 구간](../measurements/history-worker-timing-4k-2026-09-06.json) | Core Ultra 7 155H/Intel Arc DX12, Windows 11 Home 10.0.26200, release pinned Dioxus CLI 0.7.9, 4K, 20회 | 42.681 / 46.861 / 47.752 ms | worker admission→present API. 16ms 목표 미통과, 물리 표시 지연 아님. 현재 기계에서 먼저 재현 필요 |

Undo 최악 표본에서 당시 worker 17.103ms·복원/게시 13.243ms·나머지 frame 17.078ms로
기록됐으므로 queue 크기 변경부터 할 근거가 없다. 과거 157.958ms tail은 원인 미확인이다.
브러시 보정 지연과 시스템 지연도 분리한다. [Photoshop 공식 성능 문서][P-perf] 역시
보정·dab 간격에 따른 반응 차이를 설명하지만, 그것이 NyatiDraw의 측정 결과는 아니다.

## 검증 경계

최초 감사 단계는 코드/문서 읽기와 공식 문서 조회만 수행했다. 이후 구현과 통합 검증은
별도 단계이며 위 기능 문서·측정 기록에 연결한다. 이 감사 문서 갱신으로 사용자 작품 변경,
설치·태그·push·배포나 실제 앱 acceptance를 수행했다고 주장하지 않는다. W0 기준 그림 완성, 실제 펜 압력/끝점,
120Hz first-visible-pixel, Windows 클립보드 실사용은 기존 미검증 경계를 유지한다.
경쟁 제품 기능 출처는 아래 공식 문서이며 접근일은 2026-10-28이다.

### 감사 뒤 승인된 참조 색 채취 구현

`crates/editor-ui/src/reference_panel.rs`, `reference_picker.js`, 공유 CSS와
`lib.rs`의 한 컴포넌트 인자에 연결했다. 표의 참조 색 채취는 이제 **구현·통합 확인 대기**다.
브라우저에 이미 표시한 정규화 PNG를 1×1 canvas로 읽으므로 별도 64MiB 픽셀 버퍼를
유지하지 않는다. object-fit 여백과 CSS 확대/이동을 역변환한다. 드래그 견본은 DOM 로컬이며
놓을 때만 현재 project epoch·전경색·참조 generation을 검사한 `SetColor`를 보낸다.
다른 도구 선택·그림/이력 변경·최근 색 추가 명령은 보내지 않는다.

2026-10-28 Edge 156.0.4314.50의 격리된 실제 브라우저에서 동일 JS/CSS를 사용한
4×2 RGBA fixture 13경우를 통과했다: fitted 픽셀, 드래그 마지막 색, 투명/여백/밖 놓기,
Esc, pointercancel, 프로젝트/참조/전경색 변경 중 취소, 2배 확대+(30,-20) 이동,
부분 alpha의 RGB 채취, 채취 모드 끄기. 최초 접촉 중에는 메시지가 없고 견본만 표시됐으며
놓은 뒤 단 한 번의 RGBA 메시지를 확인했다. 이미지 캡처도 직접 확인했다.
증거는 ignored `output/playwright/reference-picker/{harness.html,accept.js,results.txt,preview.png,zoomed.png}`.
`node --check crates/editor-ui/src/reference_picker.js` 통과. 확인용 브라우저/루프백 서버 종료.
이것은 JS 브라우저 동작 검증이며 Dioxus↔host command·실제 앱 전경색/Undo/저장·
Windows 창·물리 펜 검증을 대신하지 않는다. Cargo/build는 통합 담당에서 수행한다.

## 최초 애니메이션 감사와 설계 제안 (후속 구현과 구분)

사용자 후속 지시와 [공통 제품 방향](../game-and-illustration.md)에 따라 애니메이션은
필수다. 아래는 구현 전에 작성한 설계 기록이다. 현재 구현은 위 상태 표와
[설계의 웹 구현 절](../animation-design.md#웹-구현-연결-2026-10-28), [native 연결](../native-animation.md),
[시트 출력](../animation-export.md)을 따른다. 아래 제안의 명칭·이전 프로토콜·미래 단계가
현재 코드의 상태를 대체하지 않는다. 일반 일러스트의
soft brush/AA/그룹/클리핑도 그대로 보존한다. 모든 그림을 indexed palette나 nearest
sampling으로 강제하는 픽셀아트 전용 모델은 채택하지 않는다.

### 공식 작업 흐름의 차이

| 동작 | Aseprite | Clip Studio Paint | NyatiDraw 첫 범위 |
|---|---|---|---|
| 그림과 시간의 관계 | [Cel][A-cel]은 특정 layer/frame의 그림, frame은 그 시각의 모든 cel 집합 | [Animation folders and cels][C-cels]는 래스터 또는 레이어 폴더를 하나의 cel로 지정 | **한 트랙의 layered cel**부터: 한 cel 안에 선화/밑색/명암 그룹을 유지하고, frame은 cel과 표시 시간을 참조 |
| 생성·복제·전환 | [Animation][A-animation]의 새/빈 frame, 복사·이동·이전/다음 frame | timeline의 지정 시각에 새 cel 생성 | 빈 cel frame·독립 복제·삭제·이전/다음·순서 변경을 Undo 가능한 명령으로 제공 |
| 시간 | [Frame Duration][A-duration]으로 frame별 길이 조절 | timeline의 cel 노출과 재생 범위 | 양수 정수 duration_ms, 누적 시간으로 playback. 시간 편집은 문서 변경, 재생 cursor는 세션 상태 |
| 같은 그림 재사용 | [Linked Cels][A-linked]는 한 그림 수정이 연결된 모든 cel에 반영 | timeline에서 cel 지정 | 첫 복제는 **독립 복제**. 같은 CelId를 여러 frame이 공유하는 hold/reuse는 명시적으로 표시하는 후속 기능 |
| 어니언 | [Onion Skinning][A-onion]의 앞/뒤 수와 색조 | [Onion skin][C-onion]의 앞/뒤 cel, 색/불투명도 | 앞/뒤 하나씩부터, 세션 overlay. picker·fill reference·저장 픽셀·PNG에 섞지 않음 |
| 게임 산출 | [Sprite sheets][A-sheet]의 가로/세로/행렬, [CLI][A-cli]의 texture+JSON | [Export animation][C-animation-export]의 PNG sequence 등 | 고정 cell grid PNG + 순서/rect/duration JSON. 기본은 전체 페이지 크기 유지; packing/trim은 후순위 |
| 여러 동작 묶음 | [Tags][A-tags]로 구간 이름과 forward/reverse/ping-pong | 여러 timeline/범위 작업 | 처음에는 전체 또는 명시 frame range. idle/walk/attack 이름 구간은 그 다음 |

이 첫 범위는 Aseprite의 여러 레이어 각각에 독립적인 노출을 지정하는 grid와 같지 않다.
CSP의 폴더 cel 방식을 명시적으로 좁힌 것이다. 배경만 모든 frame에서 동시에 수정하는
공유 track, 오디오, 카메라, bone/mesh tween, GIF/video, 고급 atlas packing은 첫 gate 밖이다.

### 최초 감사 당시 저장 구조의 제약

1. `crates/tiles/src/lib.rs::TileKey`는 `LayerId,mip,x,y`다. frame 차원이 없다.
   `TileSnapshot`은 content-addressed immutable tile object를 소유한다.
   `crates/history/src/lib.rs::HistoryNode`와 `crates/project/src/lib.rs::ReopenedProject::new`
   는 현재 history root와 현재 **전체 TileSnapshot root**의 일치를 검사한다.
2. `crates/project-redb/src/lib.rs::commit_structural_with_metadata`는 타일/root/history와
   `layer_history.rs::capture_snapshot_layers`, `canvas_history.rs::capture_snapshot_canvas`를
   같은 immediate transaction으로 게시한다. Undo의 `persist_history_cursor`도
   해당 snapshot의 tree/page를 현재 metadata로 함께 복원한다.
3. `crates/project-redb/src/history_retention.rs`는 128개 이력의 baseline을 재지정하면서
   snapshot layer/canvas 테이블을 정리한다. `object_retention.rs`는 유지된 root가
   참조하는 object를 센다. 별도 cel root를 나중에 metadata에만 추가하면 기존 GC에서
   도달 가능성을 놓칠 수 있다.
4. 웹은 별도 최종 포맷이 아니다. `crates/project-web/src/lib.rs::decode_ntdr/encode_ntdr`는
   `MemoryProjectDb`를 통해 같은 redb `.ntdr`를 읽고 쓴다. `NYWEB001/002`는 import-only다.
   다만 runtime `crates/web-core/src/lib.rs::Artwork`와 undo/redo는 현재 하나의
   canvas/tiles/layers/active를 가진다. 파일 encode 때 현재 결과를 한 structural snapshot으로
   기록하므로 웹 세션의 모든 undo step이 재열기 후 그대로 남는다는 보장은 현재도 없다.
5. `crates/project-web/src/recovery.rs`의 **NYWORK05**는 backing + 현재 tiles delta +
   canvas/tree/tool을 보낸다. worker가 같은 `.ntdr`로 checkpoint를 만든다. timeline을
   이 경로에 넣지 않으면 브라우저 복구에서 그림은 남고 frame 순서/시간이 사라질 수 있다.
6. `crates/web-core/src/interchange.rs::from_snapshot`는 모든 tile의 layer가 전달된 tree에
   있어야 한다. `editing.rs::execute_layer_command`는 다음 tree에 없는 layer의 타일을
   제거한다. **현재 cel의 tree만 넣고 다른 cel 타일을 같이 보관하는 단순 변경은 위험**하다.
   현재 web caps는 4096px edge, resident tile key 1024개, 총 layer/group node 32개다.

### 권장 최소 도메인: 한 트랙의 독립 layered cels

순수 domain은 `crates/document/src/animation.rs`에 두고 Dioxus/wgpu/redb 타입을 넣지 않는다.
권장 관계는 다음과 같다. 이름은 제안이며 구현 전 공유 API를 고정한다.

```text
AnimationDocument
  canvas                              모든 frame의 동일 출력 영역
  layers: LayerTree                    모든 cel의 실제 그룹/래스터, 전역 고유 ID
  tiles: TileSnapshot                  모든 cel의 타일을 가진 단일 authoritative root
  timeline: AnimationTimeline
    frames: ordered Frame { FrameId, CelId, duration_ms }
    cels: CelId -> Cel { root_group: GroupId }

EditorSession
  editing_frame: FrameId               작업 대상, 문서 픽셀/Undo를 바꾸지 않음
  playback_position                   표시용 시각, editing_frame과 분리
  onion_settings                      표시용 설정
```

각 cel은 한 전용 그룹의 subtree를 소유한다. frame의 `CelId` 매핑과 duration은 실제
문서 metadata이며 사용자가 group visibility를 바꿔 frame처럼 보이게 하는 UI 트릭이 아니다.
all-cel tree/root가 원본이고, `resolve_cel_tree(FrameId)`는 그 cel만 보는 disposable
composition/projection이다. export·picker·fill·thumbnail·GPU 모두 이 resolver를 사용한다.
Root/group opacity와 clipping의 의미를 바꾸지 않도록 cel container는 visible/Normal/100%이며
다른 cel과 clipping 관계를 만들 수 없게 한다. cel 안에서는 기존 속성을 보존한다.

- 기존 정지 그림은 timeline이 없는 상태 그대로 읽는다. 첫 animation 명령에서 기존
  tree의 자식들을 새 cel group에 넣고 frame 1을 만드는 **원자적 구조 변경**을 한다.
  단순 열기만으로 원본 파일이나 예전 획을 재작성하지 않는다.
- 독립 복제는 새 CelId 및 group/raster ID를 발급하고 `LayerPixelChange::DuplicateMany`
  방식으로 같은 immutable object를 새 tile key에서 참조한다. 수정은 copy-on-write이므로
  원본 cel이 변하지 않는다. frame 순번을 ID로 쓰지 않는다.
- 모든 레이어/그룹 명령은 현재 cel subtree를 대상으로 한 후 master tree에 splice한다.
  cell 밖으로 reorder/delete/참조 범위가 빠져나가지 않게 한다. snapshot에서 타일을
  삭제할 때도 **master tree의 생존 ID**로 판정한다. 표시 tree로 전체 타일을 필터하지 않는다.
- frame 전환은 live stroke, partial input, 변형/보정 draft, pending edit가 끝난 뒤에만
  허용한다. 선택/Solo를 정리하고 새 cel의 유효 active layer를 선택한다. admission 시
  FrameId/CelId 또는 session generation을 고정해 늦은 sample/preview가 다른 cel에 닿지 않게 한다.
- frame order/duration/cel 만들기·삭제는 단일 structural Undo다. 시간만 바뀌어 tile root가
  같아도 새로운 SnapshotId와 timeline metadata가 이력 차이를 보존한다.
- inactive cel도 master root에 있으므로 기존 immutable-root GC 관계가 유지된다.
  삭제한 cel은 현재 root에서 제거되지만 이전 이력이 참조하는 동안 복원할 수 있다.
  별도 per-cel root forest를 도입하지 않아 첫 단계에서 TileKey/획 replay/hash ABI를 바꾸지 않는다.
- frame 수/총 node/총 tile key/고유 tile bytes/합성 cache bytes를 각각 제한한다. 기존
  web 32-node 제한을 그대로 두면 cel container 몇 개만으로 막히므로 **cel당 편집 제한과
  전체 문서 제한**을 분리해 정해야 한다. 1024 tile key 한도를 무작정 올리지 않으며,
  고유 object 공유와 실제 사용 메모리를 측정한 뒤 조정한다. 한도 초과는 기존 문서를 보존한다.

이 설계는 큰 그림 1000 frame을 모두 메모리에 담는 약속이 아니다. 처음에는 작은 sprite와
짧은 일러스트 loop를 안전하게 편집하며 이후 lazy loading을 별도 gate로 확장한다.

### 도메인과 동시에 바꿔야 할 내구성 계약

`AnimationTimeline`은 versioned bounded codec와 checksum envelope를 갖고, 새 project
capability flag로 이전 writer가 열기 전 거절하도록 한다. 구체 flag 값은 기존 사용 비트와
대조한 뒤 예약한다. `snapshot_animation`은 SnapshotId별 immutable record,
`current_animation`은 현재 cursor와 일치하는 metadata로 둔다.

- `crates/project/src/wire.rs` 또는 새 `animation_wire.rs`: ID uniqueness, 0 duration,
  빈 frame 목록, 없는 cel/group, 중복 subtree 소유권, overflow/과대 count/trailing bytes 거절.
  metadata에 current tree/root의 cel ownership을 검증하는 함수도 둔다.
- `crates/project-redb/src/animation_history.rs`: 기존 layer/canvas 패턴을 따르되 initial
  timeline 없는 baseline도 명시적으로 기록한다. stroke·구조 commit 모두 매 snapshot에
  timeline을 capture한다. duration-only 변경도 tree/root와 같은 transaction에서 게시한다.
- `lib.rs`: structural/stroke commit, `persist_history_cursor`, readonly open preflight와
  `validate_existing`에 timeline 검증/복원 연결. 단독 `persist_animation` 호출 후 타일
  commit을 뒤따르게 하면 crash 중 metadata/pixels가 갈라지므로 금지한다.
- `history_retention.rs`: 새 테이블을 보존/삭제 목록에 포함하고 rebased baseline의
  timeline을 유지한다. 129개 이상 작업 뒤 inactive cel, 삭제 Undo, redo branch 재열기 시험.
  master root 모델이어도 GC가 다른 cel을 보존하는지 실제 검증한다.
- `project-web/lib.rs`: decode는 전체 tree/root+timeline을 복원하고 encode의 `changed`
  비교에 timeline을 포함한다. frame navigation은 자체로 artwork 변경을 만들지 않는다.
  NYWEB import는 기존처럼 static document로 변환한다.
- `project-web/recovery.rs`: version을 올려 timeline을 checkpoint/request와 worker
  재구성에 포함한다. 이전 NYWORK05는 static으로 읽고, 잘린/알 수 없는 새 버전은 보존·거절.
  `apps/web-worker/src/lib.rs`, `storage.js`, `recovery.js`의 generation/revision stale
  rejection을 유지하며 늦은 checkpoint가 새 프로젝트의 frame을 덮지 못하게 한다.

### 최초 제안의 순서와 분업 경계

| 단계 | 수직 slice와 합격 조건 | 주 소유 파일 |
|---|---|---|
| A0 | 순수 Frame/Cel 도메인·공유 명령/codec·immutable snapshot metadata. 기존 static 읽기, frame 생성/복제/삭제/time 변경 Undo, crash before/after commit, native→web→native 재열기에서 모든 cel pixels·time 일치 | `api/src/protocol.rs`, `document/src/animation.rs`, `project/src`, `project-redb/src/animation_history.rs`, `history_retention.rs` |
| A1 | native/web 편집 대상 전환과 작은 timeline UI. cel마다 기존 brush/fill/transform/색보정·레이어 작업을 실행하고 다른 cel 불변. 저장/프로세스 종료/재열기 뒤 재수정 | 공유 `editor/src/animation.rs`, `editor-ui/src/animation_panel.rs`; native `native_canvas.rs`의 actor/open/adopt; web `web-core/{lib,editing,interchange,transform,drawing}.rs`, `project-web/{lib,recovery}.rs`, `apps/web/src/main.rs` |
| A2 | monotonic clock의 duration playback·stop/loop와 이전/다음 1개 onion. idle 때 animation tick 없음, background에서 stop/pause. PNG/picker/참조 fill에 onion 미포함. playback이 Undo/dirty를 만들지 않음 | 공유 timeline evaluator + UI/renderer adapter; `paint-cpu`/`paint-gpu` 합성, native render loop와 웹 requestAnimationFrame |
| A3 | 같은 frozen document snapshot에서 grid PNG+JSON 산출. frame 순서·rect·duration·전체 페이지·alpha 보존. 해시로 PNG/JSON generation 연결, 실패 시 프로젝트는 영향 없음 | 공유 sprite-sheet layout/metadata core, `png-io`, native export actor, web download/worker |

A0 API/codec를 먼저 한 담당이 고정한 뒤, **storage 담당**은 redb/retention,
**web 담당**은 runtime/recovery, **native/UI 담당**은 actor/input/공유 panel로 나눈다.
`api/protocol.rs`, `project/wire.rs`, `editor-ui/lib.rs`는 한 명이 통합하며 각 담당이 동시에
다른 이름으로 수정하지 않는다. A0 storage 성공만으로 A1의 브러시 targeting을 완료로
간주하지 않는다. A2/A3는 frame resolver 계약이 안정된 뒤 병렬 진행할 수 있다.

Spritesheet는 두 sibling 파일의 replace가 한 원자적 transaction이라고 주장할 수 없다.
권장 최소 산출은 generation별 PNG를 먼저 완성하고, 그 파일 이름과 hash를 담은 JSON을
마지막에 atomically replace하는 manifest 방식이다. 웹에서 두 파일 다운로드가 따로라면
동일 generation/hash를 표시하고 둘 다 저장해야 함을 명시한다. ZIP 번들/packing은 별도다.
이미지 크기·frame 수 곱을 사전 검사하고 row-wise 또는 frame-wise 합성하며 모든 frame
RGBA를 중복 보관하지 않는다. 현재 paired PNG는 정지 그림/명시 editing frame 산출로
한정하고 playback cursor에 따라 자동으로 덮어쓰지 않는다.

[A-animation]: https://www.aseprite.org/docs/animation/
[A-cel]: https://www.aseprite.org/docs/cel/
[A-duration]: https://www.aseprite.org/docs/frame-duration/
[A-linked]: https://www.aseprite.org/docs/linked-cels/
[A-onion]: https://www.aseprite.org/docs/onion-skinning/
[A-sheet]: https://www.aseprite.org/docs/sprite-sheet/
[A-cli]: https://www.aseprite.org/docs/cli/
[A-tags]: https://www.aseprite.org/docs/tags/
[C-cels]: https://help.clip-studio.com/en-us/manual_en/600_animation/Animation_folders_and_cels.htm
[C-onion]: https://help.clip-studio.com/en-us/manual_en/600_animation/Onion_skin.htm
[C-animation-export]: https://help.clip-studio.com/en-us/manual_en/600_animation/Export_animation.htm

[K-brush]: https://docs.krita.org/en/reference_manual/tools/freehand_brush.html
[K-ref]: https://docs.krita.org/en/reference_manual/tools/reference_images_tool.html
[K-transform]: https://docs.krita.org/en/reference_manual/tools/transform.html
[K-selection]: https://docs.krita.org/en/user_manual/selections.html
[K-flat]: https://docs.krita.org/en/tutorials/flat-coloring.html
[K-mask]: https://docs.krita.org/en/reference_manual/layers_and_masks/transparency_masks.html
[C-smooth]: https://support.clip-studio.com/en-us/faq/articles/20200030
[C-ref]: https://help.clip-studio.com/en-us/manual_en/210_file/Import_reference_images.htm
[C-transform]: https://help.clip-studio.com/en-us/manual_en/360_transform/Transform_using_the_bounding_box.htm
[C-layer]: https://help.clip-studio.com/en-us/manual_en/180_layers/Other_layer_settings.htm
[C-layer-ui]: https://help.clip-studio.com/en-us/manual_en/180_layers/Using_layers.htm
[C-mask]: https://help.clip-studio.com/en-us/manual_en/180_layers/Layer_masks.htm
[P-brush]: https://helpx.adobe.com/photoshop/desktop/apply-painting-techniques/brushes-presets/paint-with-brush-or-pencil-tool.html
[P-transform]: https://helpx.adobe.com/photoshop/desktop/crop-resize-transform/resize-adjust-resolution/transform-images-shapes-and-paths.html
[P-group]: https://helpx.adobe.com/photoshop/desktop/create-manage-layers/create-layer-compositions/create-layers-and-layer-groups.html
[P-selection]: https://helpx.adobe.com/photoshop/desktop/make-selections/refine-modify-selections/refine-and-soften-selection-edges.html
[P-mask]: https://helpx.adobe.com/photoshop/desktop/create-masks/layer-masks/edit-and-refine-layer-masks.html
[P-perf]: https://helpx.adobe.com/photoshop/desktop/apply-painting-techniques/brushes-presets/optimize-painting-and-cloning-using-brush-settings.html

# 최소 자체 연필 엔진: 2H 샤프펜슬과 2B 연필

[브러시 원리 비교](brush-engines.md) · [일러스트 워크플로우](../illustration-workflow.md) ·
[기존 round 계약](../decisions/ADR-0051-basic-brush-controls.md)

## 범위와 상태

목적은 타사 엔진 도입이 아니라 NyatiDraw 자체 엔진으로 기본 연필 두 가지를 제공하는
것이다. 사용자 승인 범위는 **2H 샤프펜슬, 2B 연필의 실제 건식 자국**이며, 외부 엔진 코드
복사·FFI·외부 preset 가져오기·추가 외부 의존성은 사용하지 않는다. 다양한 Brush 가족
템플릿, 범용 bitmap texture, smudge, wet paint는 후속이다.

처음에는 연구 문서만 작성하는 범위였으며, 이후 사용자 승인에 따라 아래 제한된 v3
구현으로 진행했다. 구현된 계약과 아직 검증하지 않은 물리 펜 감각을 구분한다.
이 문서는 연필의 물리 재현을 완료했다는 선언이 아니다.

## 보이는 결과가 먼저인 두 템플릿

Faber-Castell의 공식 등급 설명은 2B를 부드럽고 검은 쪽, 2H를 더 단단한 쪽으로 구분한다.
이는 표현 방향의 근거이며, 디지털 수치나 특정 제조사의 자국을 측정해 복제한 것은 아니다.
[공식 흑연 등급 설명](https://fabercastell.com/blogs/creativity-for-life/graphite-pencils-grip-writing)

| 관찰 상황 | 2H 샤프펜슬의 목표 | 2B 연필의 목표 |
|---|---|---|
| 가벼운 필압 | 가늘고 비교적 일정한 접촉 폭, 낮은 농도, 종이 빈 결이 남는 러프 | 더 약하게 닿는 좁은 접촉에서 시작하고, 부드럽게 농도가 올라감 |
| 필압 증가 | 폭은 조금만 변하고 흑연 부착량이 증가 | 폭과 부착량이 모두 더 크게 변하고 종이 골을 더 채움 |
| 같은 최대 크기·같은 필압 | 단단한 접촉과 낮은 부착량 | 더 높은 부착량과 다른 필압 폭 반응; 이름만 다른 같은 자국이 아님 |
| 반복 덧칠 | 밝은 러프에서 점차 진해지지만 남은 종이 결은 같은 위치 | 더 빠르게 진한 명암으로 누적, 밝은 종이 골이 무작위로 이동하지 않음 |
| 첫 버전의 기울기 | 기기와 관계없이 세운 접촉으로 고정 | 기기와 관계없이 세운 접촉으로 고정; 넓은 옆면 음영은 아직 없음 |

현재 round 엔진의 `hardness`는 원의 불투명한 내부 반지름 비율이며 **흑연 등급이 아니다**.
`hardness = 2H/2B` 같은 대응은 하지 않는다. v3의 두 연필은 grade별 부착 곡선과 고정된
종이 결에 의해 실제 coverage가 달라진다. 공통 접촉 외곽은 현재 원이며, 타원 tip·마모·분말
입자·빛 반사·종이 눌림을 모델링하지 않는다.

## 최소 shape / grain 계약

| 역할 | v3 구현 계약 | 이번 범위 밖 |
|---|---|---|
| shape | 최대 지름은 문서 px, 필압에 따른 원형 접촉. 중심·반지름은 1/16 px로 양자화 | 임의 bitmap tip, 여러 stamp, 타원 기울기 접촉, 심 마모 |
| grain | 하나의 절차적 종이, 문서 정수 pixel 좌표와 고정 seed로 결정 | 사용자 종이 이미지, 보기 좌표 grain, 획마다 랜덤 재배치 |
| deposition | grade·필압에 따른 정수 부착량에서 종이 골 값을 차감 | 유체·안료 혼합, 캔버스 색 채취, graphite 높이 상태 |
| coverage | 동일 4×4 위치의 정수 원 포함 판정 × 부착량 | 자동 고해상도 재질 생성, 무제한 texture 해상도 |
| accumulation | 정량화한 dab alpha를 기존 source-over로 쌓음 | 한 획 opacity 상한을 강제하는 wash |

이 구분은 Procreate의 shape/grain 분리, CSP의 tip density/texture 적용 단위, Krita의
flow/opacity 구분을 참고한 자체 계약이다. 상용 앱의 비공개 수식을 추정한 결과가 아니다.
[Procreate 설정](https://help.procreate.com/procreate/handbook/brushes/brush-studio-settings),
[CSP tip](https://help.clip-studio.com/en-us/manual_en/810_subtools/B.htm),
[Krita 농도](https://docs.krita.org/en/reference_manual/brushes/brush_settings/opacity_and_flow.html)

### 크기·필압·fallback

`pencil_preset(PencilKind)`는 아래 기본값을 제공한다. 앱은 두 템플릿의 최초 표시 크기를
기존 사용자 기본값인 5px/100%로 맞추고, 이후 템플릿별 설정을 기억할 수 있다.
크기를 똑같이 설정해도 부착 곡선·flow와 크기 필압 최소값은 서로 다르다.

| 값 | 2H | 2B |
|---|---|---|
| helper의 지름 기본값 | 2px | 5px |
| UI 최초 크기 / 불투명도 | 5px / 100% | 5px / 100% |
| 크기 필압 / 농도 필압 | 켬 / 켬 | 켬 / 켬 |
| 크기 최소 비율 | 0.82 | 0.35 |
| 농도 최소 비율 | 0.05 | 0.05 |
| flow | 0.32 | 0.65 |
| 간격 / 최대 지름 | 0.15 | 0.12 |
| 부착량 | `round(80 + 128 × q)` | `round(128 + 127 × q)` |

`p`는 정규화된 필압이다. 크기 배율은 `size_min + (1 - size_min) × p`, 농도 배율 `q`는
`opacity_min + (1 - opacity_min) × p`다. 각 필압 축을 끄면 해당 배율은 1이다. 최대 지름
`D`는 0.1~200 문서 px이며, 접촉 반지름은 `D × 크기배율 / 2`를 1/16 px로 양자화한다.
이 식은 디지털 사용감의 초기값이며 흑연의 물리 상수가 아니다. 매우 작은 지름의 점은
4×4 coverage 해상도의 한계를 가지므로 실사용 gallery에서 별도로 평가해야 한다.

v3의 외곽 경도는 1로 고정하며 UI 경도 변경으로 grade를 바꾸지 않는다. 잘못된 v3 preset은
유효한 기록으로 재생하지 않는다. 압력이 없는 마우스 입력은 현재 정규화 경로의 고정 압력을
받고, 압력 토글을 끈 출력은 해당 preset의 최대 반응을 사용한다. 마우스 결과를 필압 펜
감각의 증거로 제시하지 않는다.

현재 입력의 `tilt: Option<[f32;2]>`는 플랫폼에서 받은 각도이며 문서 방향으로 변환된 tip
축이라는 계약이 없다. v3는 `Some`/`None` 모두 upright로 처리한다. 기울기를 지원한다고
표시하지 않으며, 추후 도입하려면 단위·부호·보기 회전/반전 변환·보간·미지원 기기 fallback을
새 엔진 계약으로 함께 고정해야 한다. 방향 없는 입력에 임의 난수 각도를 넣지 않는다.

### 간격·flow·획 누적

간격은 최대 지름 기준 거리이며 최소 0.25px다. 시간당 dab 생성은 없고, 입력을 멈춘 동안
에어브러시처럼 농도가 계속 쌓이지 않는다. 같은 입력 stream을 다른 batch로 전달해도 같은
거리 잔여량을 유지한다. 서로 다른 장치가 다른 sample stream을 만든 경우까지 동일한
자국이 나온다는 주장은 하지 않는다.

종이 값 `tooth`는 0~255, 부착값 `deposit`도 0~255다. 각 pixel의 종이 부착 계수는
`max(deposit - tooth, 0) / 255`이고, shape의 포함 subpixel 수를 16으로 나눈 값과 곱한다.
이를 preset opacity·농도 필압·flow와 곱한 후 **1/255 단계의 dab alpha**로 양자화하여
CPU/GPU의 source-over 입력을 맞춘다. v1/v2의 coverage·alpha 식은 변경하지 않는다.

한 획 안의 자가 교차와 별도 획의 중첩 모두 build-up이다. 낮은 opacity에서도 반복 통과하면
더 진해질 수 있다. 이것을 획 전체 opacity 상한이라고 설명하지 않는다. 필수 건식 연필은
wash 없이 먼저 검증하고, wash가 실제 작업을 막는 요구로 확인되면 원본 타일+획 중간 표면,
취소, 선택, alpha lock, 저장 재생을 포함한 별도 단계로 설계한다.

### 종이 좌표·seed·asset

종이는 보기 zoom/rotation/mirror, GPU 타일 분할, 프리뷰 갱신 횟수에 영향을 받지 않는다.
`PENCIL_PAPER_SEED = 0x4e594154`와 grain algorithm 1은 engine v3의 고정 자원이다.
32bit wrapping 정수 hash로 signed 문서 위치를 처리하며 좌표의 주기는 2^32 문서 pixel이다.
두 연필과 여러 획이 같은 종이를 공유한다. 획 `random_seed`는 계속 기록하지만 v3에는
획별 tip jitter가 없으므로 그 seed로 종이를 재배치하지 않는다.

CPU/GPU 타일로 dab를 옮길 때 반드시 `BrushDab::to_local`을 사용한다. 이 함수는 shape
중심에서 타일 원점을 빼고 grain 원점에는 같은 정수 원점을 더한다. 중심만 빼면 타일마다
종이가 반복되는 seam이 생긴다. 신규 pixel 변형은 기존 저장 pixel을 이동하는 작업이며,
이미 그린 연필을 새 문서 위치에서 재질 평가하여 다시 칠하지 않는다.

외부 이미지 자산은 없고 새 라이선스 의존성도 없다. 향후 실제 종이 asset을 추가할 때는
자체 제작/배포 권한, 원본 bytes의 content hash, 크기·채널·sampling 규칙, immutable
snapshot에서의 참조를 같이 기록해야 한다. 파일 이름만 저장하거나 누락된 asset을 비슷한
이미지로 조용히 바꾸는 동작은 허용하지 않는다. 이 자원 시스템은 첫 두 연필에 필요하지 않다.

## 버전·저장·CPU/GPU 경계

- `ROUND_BRUSH_ENGINE_VERSION = 2`는 그대로 둔다. 기존 engine 1/2 식·wire·hash는 보존한다.
- `PENCIL_ENGINE_VERSION = 3`, preset schema 3을 별도로 사용한다. 고정된 두 preset ID가
  material identity다. 알 수 없는 ID를 2H로 바꿔 저장 재생하지 않는다. 사용자 임의 material
  편집기·자유 preset ID는 현재 계약에 없으며 후속 schema에서 분리해야 한다.
- v3 wire/hash에만 grain version 1과 고정 paper seed의 5 bytes를 추가한다. 잘린 payload,
  다른 grain version/seed, 비정상 preset은 거절한다. Begin에서 고정한 설정·color·sample·
  selection·alpha lock과 before root가 재생 기준이며 현재 UI 설정으로 덮지 않는다.
- 프로젝트 capability `0x2000`은 v3 stroke와 같은 redb 트랜잭션으로 기록한다. 기존 flag와
  독립적이며 Undo/Redo·레이어·페이지 metadata 변경에도 보존한다. 해당 bit를 모르는 writer는
  프로젝트를 수정하기 전에 거절해야 하고, v3 기록의 bit가 사라진 경우는 corruption이다.
- CPU는 닫힌 획의 권위 pixel을 재생한다. GPU는 같은 grain·양자화 계약으로 live 작업 타일을
  그리며, 저장 가능한 유일한 표현이 GPU 상태뿐이 되는 변경은 하지 않는다.
- 기본 검정 fixture의 byte 동일성과 모든 색·alpha·backend의 합성 동일성은 다른 주장이다.
  기존 UNORM 합성 허용값을 숨기거나 높이지 않으며 strict 결과도 함께 기록한다.

## 제한된 작업 단위와 gate

| 단계 | 산출물 | 의존 / 통과 조건 |
|---|---|---|
| P0. 원리와 목표 | 이 문서와 두 자국 목표 | 자체 구현 범위 승인; 범용/습식 연구와 분리 |
| P1. 자체 평가·CPU | 두 고정 material, 필압 부착, 문서 grain, packet/tilt fallback 불변식 | 기존 v1/v2 golden 불변, grade별 실제 pixel 차이 |
| P2. live GPU·wire | 정수 coverage/WGSL, v3 wire/hash/capability, 타일 원점 보존 | signed 경계·기존 fixture·새 2H/2B strict 비교 |
| P3. history·재열기 | atomic gate, Undo/Redo, metadata, 별도 프로세스 CPU replay | 저장·프로세스 재시작·재열기 후 pixel/root 보존 |
| P4. UI·실사용 | Pencil 안의 두 template, 실제 엔진 미리보기, 설정별 기억 | 새 seed/dirty/Undo를 만들지 않는 scratch preview; 실제 펜 감각 별도 확인 |
| 이후 | 옆면 tilt, 필요 시 wash, 다양한 Brush template | 별도 범위 승인과 새 의미 검증; smudge/wet는 계속 분리 |

v3는 지름 200px, segment당 4,096 dab, stroke당 1,048,576 dab를 상한으로 둔다. 초과할
경우 생성 전에 discontinuity를 latch하고 이후 dab를 내보내지 않는다. native는 token의
`is_discontinuous()`를 확인하여 부분 획을 취소해야 하며, seal/replay도
`DabBudgetExceeded`로 거절한다. 예산 때문에 짧아진 획을 정상 저장하면 안 된다.
기존 sample·dirty tile·bounded queue 제한도 유지하며, 이 상한이 성능 목표 달성 증거는 아니다.

작은 gallery는 점·얇은 선·일정/증감 필압·느린/빠른 곡선·자가 교차·여러 획 덧칠·negative
좌표와 타일 경계를 포함한다. 보기 변환은 종이 결을 바꾸지 않아야 하며 저장 pixel transform은
기존 pixel을 보존하는 규칙으로 확인한다. 자동 시험은 artwork/결정성/history/recovery 위험을
보호하는 불변식에 한정한다. UI 레이아웃을 위한 mock 시험은 추가하지 않는다.

## 확인된 증거와 남은 범위

초기 core 검사에서 brush·CPU paint·stroke·project wire·redb 기존 시험과 v3 packet/grade/
wire 재생 시험이 통과했다. 추가 redb 검사는 두 연필 각각 signed stroke를 저장하고 별도
프로세스에서 재생하여 같은 pixel을 확인했으며, Undo/Redo·페이지 metadata 변경·실패한 commit의
capability 원자성·누락 flag 거절도 확인했다.

GPU 차분은 Windows / NVIDIA GeForce RTX 3080 / Vulkan / release에서 수행했다.
2H 161 dab, 2B 201 dab의 signed pressure/grain 검정 fixture는 CPU와 strict byte 차이 0이었다.
첫 검사에서는 기존 UNORM 합성의 누적 오차가 나타났고, 허용값을 늘리지 않고 v3 dab alpha를
양쪽에서 1/255로 맞춘 뒤 통과했다. 기존 round fixture 결과는 바뀌지 않았다.

이 증거는 다른 backend·GPU·물리 펜·고주사율 화면의 검증이 아니다. 실제 펜으로 2H/2B의
농도·가는 선·시작과 끝을 사용하는 gallery 검토, UI integration의 최종 compilation/acceptance,
별도 p50/p95/p99 성능 측정은 남아 있다. 확대된 wet/범용 질감 구현은 하지 않았다.

코드: [자체 연필](../../crates/brush/src/pencil.rs), [평가/예산](../../crates/brush/src/lib.rs),
[CPU](../../crates/paint-cpu/src/lib.rs), [WGSL](../../crates/paint-gpu/src/round_dab.wgsl),
[획](../../crates/stroke/src/lib.rs), [wire](../../crates/project/src/wire.rs),
[redb](../../crates/project-redb/src/lib.rs), [GPU 차분](../../crates/paint-gpu/examples/gpu_cpu_diff.rs).

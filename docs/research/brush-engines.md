# 브러시 엔진 비교와 NyatiDraw 확장 방향

[문서 목록](../README.md) · [일러스트 워크플로우](../illustration-workflow.md) ·
[기본 브러시 결정](../decisions/ADR-0051-basic-brush-controls.md) ·
[최소 자체 연필 설계](pencil-brush-design.md) ·
[후속 브러시 스프린트](../sprints/sprint-04-brush.md)

## 목적과 결론

Procreate, Clip Studio Paint(CSP), Krita, MyPaint/libmypaint와 보조 비교 대상 Adobe
Fresco의 공식 문서·공개 소스를 비교한다. Procreate는 iPad 드로잉 앱의 주요 참고 대상으로
포함한다. 목적은 외부 엔진을 가져오는 것이 아니라 표현 원리를 비교하여 NyatiDraw
자체 엔진의 설계에 참고하는 것이다. 외부 엔진 채택, 코드 복사, C FFI 연결은 하지 않는다.
필수 템플릿은 **2H 샤프펜슬과 2B 연필**이며, 다양한 Brush 가족 템플릿은 후속이다.
현재 우선순위는 W0~W5의 기본 그림 작업이다.

NyatiDraw에는 이미 입력 평가, dab 생성, CPU/GPU 그리기, 저장 재생을 구분하는 기반이 있다.
이를 유지하면서 **세부 프리셋 선택 → 실제 엔진의 획 미리보기 → 기본 선과 농도의 제어**를
먼저 정리한다. 두 필수 연필에 필요한 최소 건식 흑연·종이 결은 우선 범위에 포함하되,
범용 질감 엔진·캔버스 색을 읽는 혼색·습식 물감과 분리한다. 사용자 승인에 따라 자체
upright dry-pencil v3 구현이 진행 중이며, 세부 계약과 검증 상태는
[연필 설계](pencil-brush-design.md)에 기록한다.
도구 이름이나 프리셋 개수만 늘려 자연스러운 연필 표현이 구현되었다고 판단하지 않는다.

아래의 경쟁 제품 기능은 공식 자료에서 확인한 사용자 동작이다. 별도로 표시한
NyatiDraw 제안은 그 자료와 현재 코드에서 도출한 설계 판단이다. 동일 장비·동일 입력으로
경쟁 앱을 측정하지 않았으므로 속도 순위, 지연 수치, 선 품질 우열은 제시하지 않는다.
상용 앱의 GPU 배치, 타일 형식, 난수 상태, 혼색 수식, 저장 재생 구현은 공개 자료만으로
확인하지 못했다.

## 먼저 구별할 개념

| 구분 | 의미 | NyatiDraw에서의 용도 |
|---|---|---|
| 도구 가족 | 연필·펜·브러시·지우개와 같은 사용 목적의 분류 | 주 도구를 고른 뒤 그 안의 세부 도구를 보여 준다 |
| 세부 프리셋 | tip, 필압 반응, 농도, 간격 등 구체적인 설정 묶음 | 여러 프리셋이 같은 엔진을 사용할 수 있다 |
| tip / shape | 한 번 찍히는 자국의 외곽과 내부 마스크 | 원형, 납작한 펜촉, 비정형 자국을 구별한다 |
| grain / texture | tip 안이나 문서 좌표에 적용되는 결 | 종이 질감과 획을 따라 움직이는 결은 좌표 의미가 다르다 |
| dynamics | 필압·속도·기울기·방향 등을 출력값으로 바꾸는 규칙 | 크기와 농도의 반응을 독립적으로 다룬다 |
| 획 누적 | 겹치는 dab와 하나의 획 전체를 합치는 방식 | flow와 획 불투명도의 의미를 결정한다 |
| 혼색 | 기존 캔버스 색을 읽고 끌거나 섞는 동작 | 단순 source-over 덧칠과 별도의 상태·읽기 경로가 필요하다 |

Krita는 tip과 preset을 명시적으로 구분한다. CSP는 도구 가족 아래 구체적인 도구를
선택하게 한다. 따라서 세부 도구 패널에 연필·펜·브러시 가족을 다시 나열하는 것은
사용자가 요청한 모델과 다르다. [Krita 프리셋](https://docs.krita.org/en/user_manual/loading_saving_brushes.html),
[CSP 도구 팔레트](https://help.clip-studio.com/en-us/manual_en/150_tools/The_Tool_palette.htm)

## 제품별로 확인한 표현 모델

### Procreate: shape와 grain을 조합하는 프리셋

공식 설명은 shape 안에 grain을 두고 경로를 따라 자국을 배치하는 모델이다. Moving grain과
Texturized grain을 구별하고, 필압·기울기·속도에 따른 반응과 taper를 조절한다. Rendering은
glaze 계열과 blending 계열을 제공하며, flow와 pressure opacity를 별도로 노출한다.
Wet Mix에는 희석, 시작 시 물감량, 부착량, 기존 색을 끄는 양 등의 조절이 있다.
이 이름들을 Krita wash/build-up이나 물리 유체 방정식과 일대일 대응시키지는 않는다.
[Brush Studio Settings](https://help.procreate.com/procreate/handbook/brushes/brush-studio-settings)

Brush Studio에는 직접 그려 보는 Drawing Pad가 있고, 라이브러리는 프리셋을 세트로 묶어
관리한다. 공식 연필 세트 설명도 종이 결 및 기울기에 반응하는 동작을 함께 언급한다.
NyatiDraw가 참고할 점은 프리셋의 설정과 보이는 자국을 가까이 두는 구성이다.
[Brush Studio](https://help.procreate.com/procreate/handbook/brushes/brush-studio),
[Brush Libraries](https://help.procreate.com/procreate/handbook/brushes/brush-library)

### CSP: tip 농도·획 속성·혼색을 구분하는 도구

원형 또는 이미지 소재 tip을 사용하며, 여러 tip의 반복 순서와 간격을 설정한다.
Brush density는 개별 tip의 불투명도이고, 좁은 간격에서 농도를 보정하는 옵션이 있다.
Texture의 Apply by each plot은 결을 tip마다 적용할지 획 단위로 적용할지 구별한다.
크기·불투명도 등에는 필압 곡선과 기울기·속도 반응을 설정할 수 있다.
[Brush tip](https://help.clip-studio.com/en-us/manual_en/810_subtools/B.htm),
[Stroke](https://help.clip-studio.com/en-us/manual_en/810_subtools/S.htm),
[Texture](https://help.clip-studio.com/en-us/manual_en/810_subtools/T.htm),
[Brush dynamics](https://help.clip-studio.com/en-us/manual_en/240_brushes/Customizing_brush_tools.htm)

Ink의 Opacity와 Color mixing은 별도 설정이다. 혼색에는 Blend, Running color, Smear와
물감량·투명 성분·색 유지 거리 등의 조절이 있으며, 혼색 방식에 따라 일부 opacity dynamics와
blend mode를 함께 사용할 수 없다. 따라서 모든 옵션을 독립 토글처럼 조합할 수 있다고
가정해서는 안 된다. Perceptual mixing이라는 이름만으로 특정 색 공간이나 안료 수식을
사용한다고 결론 내리지 않는다. [Ink](https://help.clip-studio.com/en-us/manual_en/810_subtools/I.htm)

### Krita Pixel: 일반 dab 엔진의 명확한 기준

생성형 tip·bitmap tip, 간격, 센서 반응, 합성 모드를 조합하는 엔진이다. tip은 자국을,
preset은 그 자국이 획을 이루는 규칙을 담는다. Pixel 엔진 하나로 선화·불투명 면·부드러운
칠에 맞는 여러 프리셋을 만들 수 있다. [Pixel Brush Engine](https://docs.krita.org/en/reference_manual/brushes/brush_engines/pixel_brush_engine.html),
[Brush Tips](https://docs.krita.org/en/reference_manual/brushes/brush_settings/brush_tips.html)

Krita 문서는 opacity를 획의 투명도, flow를 개별 dab의 투명도로 구분하며, build-up은
opacity도 dab 누적으로 취급하고 wash는 획 투명도로 취급한다고 설명한다. 이 구분이 현재
NyatiDraw의 농도 제어를 평가하는 가장 명확한 참고점이다.
[Opacity and Flow](https://docs.krita.org/en/reference_manual/brushes/brush_settings/opacity_and_flow.html)

### Krita Color Smudge: 캔버스 읽기가 추가되는 다른 계약

Pixel과 tip 기반을 공유하지만, 이전 위치의 픽셀을 옮기는 Smearing과 주변 색을 채취하는
Dulling을 제공한다. Color Rate는 새 전경색, Smudge Length는 끌고 가는 색의 영향을
조절한다. 간격은 자국 개수뿐 아니라 색을 채취하는 횟수에도 영향을 준다. Overlay 설정은
현재 레이어와 다른 레이어를 포함한 읽기 범위를 바꾼다.
[Color Smudge Engine](https://docs.krita.org/en/reference_manual/brushes/brush_engines/color_smudge_engine.html)

**NyatiDraw 설계 판단:** smudge를 새 preset 몇 개로 추가할 수는 없다. dab 평가 시점의
캔버스 색, 읽는 레이어, 이전 dab 상태와 쓰기 순서가 결과에 포함되므로 저장 재생과 GPU
작업 순서를 먼저 정해야 한다. 기본 soft brush와 색 채취가 이 기능의 구현 증거는 아니다.

### MyPaint / libmypaint: 입력·표면·상태 계약을 관찰할 공개 사례

libmypaint의 공개 API는 입력을 `stroke_to`로 받고, surface의 `draw_dab`와 `get_color`를
통해 그리기와 색 채취를 연결한다. 원형·타원형 dab와 hardness, angle 등의 계약이 보이며,
일반 bitmap tip·grain asset 시스템과 동일한 인터페이스는 아니다.
[Brush API](https://raw.githubusercontent.com/mypaint/libmypaint/master/mypaint-brush.h),
[Surface API](https://raw.githubusercontent.com/mypaint/libmypaint/master/mypaint-surface.h)

공개 설정에는 필압·속도·기울기·방향·난수, 기본 반지름/현재 반지름/시간 기준 dab 수,
smudge 등이 있다. `opaque_linearize`는 예상 dab 중첩에 따른 농도 비선형을 보정한다.
이는 획 전용 합성 표면에서 opacity 상한을 강제하는 wash와 같은 보장이라고 해석하지
않는다. 설정·입력 범위는 조사한 master 소스 기준이며 안정 릴리스 채택을 뜻하지 않는다.
[Brush settings](https://raw.githubusercontent.com/mypaint/libmypaint/master/brushsettings.json)

### Adobe Fresco: 기본 Pixel과 Live의 경계를 비교할 iPad 참고 앱

보조 비교 대상으로 Fresco를 고른 이유는 일반 Pixel 브러시와 Live 브러시를 제품에서
구분하기 때문이다. Pixel에는 hardness·spacing·scatter·shape/color dynamics,
설정 패널에서 직접 그리는 미리보기, ABR 가져오기가 있다. 재질 결의 모든 좌표 규칙이나
획 opacity 수식은 해당 공식 문서에서 확인되지 않는다.
[Pixel brushes](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/pixel-brushes.html)

Adobe는 Live를 픽셀 기반의 물리 시뮬레이션 엔진이라고 설명하며, 수채의 번짐과 유채의
색 섞임을 구분한다. 수채 예제는 water flow와 color flow를 따로 사용한다. 이것은
공식적인 제품 설명이며, solver·시간 간격·물감 상태 저장 형식까지 공개했다는 뜻은 아니다.
NyatiDraw의 wet paint 보류 범위가 기본 dry brush보다 큰 별도 작업임을 보여 주는 사례다.
[Live brushes](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/live-brushes.html)

## 농도·누적·혼색 비교

다음 표는 이름이 같은 슬라이더를 그대로 변환할 수 있는 호환표가 아니다.

| 엔진/제품 | 자국 배치와 농도 | 캔버스 색과의 상호작용 | 설계 참고 |
|---|---|---|---|
| Procreate | shape 간격, flow, pressure opacity, 여러 Rendering mode | Wet Mix로 기존 색을 끌고 섞는 동작 | grain과 tip, 획 합성, wet 설정을 구분한다. 정확한 수식은 미확인. [공식 설정](https://help.procreate.com/procreate/handbook/brushes/brush-studio-settings) |
| CSP | tip density, gap에 따른 농도 보정, 전체 opacity | 선택한 Color mixing 방식에 따라 동작·옵션 조합이 달라짐 | density와 opacity를 같은 수치로 대체하지 않는다. [tip](https://help.clip-studio.com/en-us/manual_en/810_subtools/B.htm), [ink](https://help.clip-studio.com/en-us/manual_en/810_subtools/I.htm) |
| Krita Pixel | 개별 dab flow와 wash/build-up 구분 | 기본 덧칠과 blend mode | 획 안의 겹침과 별도 획의 겹침을 구분해 평가한다. [농도](https://docs.krita.org/en/reference_manual/brushes/brush_settings/opacity_and_flow.html) |
| Krita Color Smudge | 간격이 자국과 채취 빈도 모두에 관여 | Smearing / Dulling 및 읽기 레이어 범위 | 색 읽기와 쓰기의 순서도 엔진 계약이다. [smudge](https://docs.krita.org/en/reference_manual/brushes/brush_engines/color_smudge_engine.html) |
| libmypaint | 반지름·시간 기준 dab 수, 중첩 농도 보정 | surface 색 채취와 smudge 상태 | API 연결 외에 상태·색 공간·동기화 계약이 필요하다. [settings](https://raw.githubusercontent.com/mypaint/libmypaint/master/brushsettings.json), [surface](https://raw.githubusercontent.com/mypaint/libmypaint/master/mypaint-surface.h) |
| Fresco | Pixel의 flow·spacing과 Live의 물감/물 제어가 별도 | Live 수채·유채 시뮬레이션 | 기본 브러시와 wet 모델의 완료 기준을 나눈다. [Pixel](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/pixel-brushes.html), [Live](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/live-brushes.html) |

## 프리셋과 미리보기에서 가져올 점

Procreate의 Preview는 필압이 증가했다 줄어드는 시범 획이며, 별도 설정을 가진다.
Krita의 live preview도 합성 입력을 사용하며 직접 그리는 scratchpad와 구분된다.
CSP는 도구 속성에서 stroke preview를 표시할 수 있다. 자동 시범 획과 사용자가 직접
그리는 시험 공간은 서로 보완적이다.
[Procreate Preview](https://help.procreate.com/procreate/handbook/brushes/brush-studio-settings),
[Krita preview / scratchpad](https://docs.krita.org/en/user_manual/loading_saving_brushes.html),
[CSP 속성 팔레트](https://help.clip-studio.com/en-us/manual_en/150_tools/Customizing_the_Tool_and_Sub_Tool_palettes.htm)

NyatiDraw에 대한 제안은 다음과 같다.

- 세부 도구 목록은 선택한 가족 안의 preset을 보여 준다. 이름·작은 자국·현재 선택만으로
  구별하고, 모든 엔진 옵션을 목록에 노출하지 않는다.
- 속성 맨 위에는 현재 preset·색·크기로 만든 실제 엔진 획을 둔다. 단순 SVG 선의 굵기나
  흐림으로 자국을 흉내 내지 않는다. 큰 브러시를 축소 표시할 때에는 표시 배율을 별도로
  취급하여 문서 px와 혼동하지 않는다.
- 미리보기는 별도 seed·시간·필압 경로를 갖는 작은 scratch surface에서 평가한다.
  크기·색·설정 변경을 묶어 갱신하고, 최신 요청만 반영하며, 면적·sample·dab 수를 제한한다.
  실제 입력 queue나 문서 Begin을 경유하지 않아 dirty·Undo·최근 크기·획 seed에 영향이 없다.
- 지우개는 지워질 밑색이 있는 scratch surface를 사용한다. 향후 smudge는 두 색 경계를
  갖춘 시험 바탕이 필요하다. 흰 빈 바탕의 한 획만으로 혼색을 평가할 수는 없다.
- 기본 선택은 Pencil, 초기 전경색은 검정으로 한다. preset 전환과 전경색 선택은 독립적인
  사용자 동작으로 유지한다. 개인 preset 저장과 작품 안에 고정되는 preset snapshot도
  별도 수명으로 관리한다.

## 공개 범위·라이선스·호환성과 결정성

아래 라이선스 정보는 공개 자료의 경계를 구별하기 위한 기록이며 채택 후보 목록이 아니다.
타사 엔진·코드·프리셋·tip 이미지를 NyatiDraw에 복사하거나 연결하지 않는다.

| 대상 | 확인한 공개/저장 경계 | 자체 설계에 참고할 점 |
|---|---|---|
| Procreate | `.brush`, `.brushset`, `.brushlibrary`, `.abr` 가져오기를 문서화. [Import and Share](https://help.procreate.com/procreate/handbook/brushes/brushes-share) | 가져오기 지원은 원본 앱과 픽셀 결과가 동일하다는 증거가 아니다. 이번 공식 자료에서 재사용 가능한 엔진 SDK나 결정적 재생 계약은 확인하지 못했다. |
| CSP | 설정을 기본값으로 저장·복원하는 UX 제공. [Customizing brushes](https://help.clip-studio.com/en-us/manual_en/240_brushes/Customizing_brush_tools.htm) | 사용자 기능을 참고한다. 이번 자료에서 공개 엔진 소스·SDK, 동일 seed의 버전 간 동일 픽셀 계약은 확인하지 못했다. |
| Krita | `.kpp`는 tip·pattern 자원과 구별됨. 앱 전체는 GPL v3이며 파일별로 더 허용적인 라이선스가 있을 수 있음. [preset](https://docs.krita.org/en/user_manual/loading_saving_brushes.html), [license](https://krita.org/en/about/license/) | 기능 원리 연구와 코드 복사를 구분한다. NyatiDraw 자체 구현이며 Krita 엔진 코드는 가져오지 않는다. |
| libmypaint | C 라이브러리, ISC, json-c 의존성. 설정 문자열 입력과 내부 상태 접근 API, API/ABI 버전 정책 공개. [README](https://github.com/mypaint/libmypaint), [API](https://raw.githubusercontent.com/mypaint/libmypaint/master/mypaint-brush.h), [versioning](https://raw.githubusercontent.com/mypaint/libmypaint/master/VERSIONING.md) | 상태·입력·표면의 분리만 참고한다. 통합이나 FFI 후보로 권하지 않는다. API/ABI 호환과 artwork 픽셀 재생 호환을 구분한다. |
| Fresco | Pixel brush의 ABR 가져오기와 별도 Live 동작을 문서화. [Pixel](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/pixel-brushes.html), [Live](https://helpx.adobe.com/fresco/desktop/draw-paint-animate-and-share/live-brushes.html) | 가져온 preset의 지원 범위와 결과는 실제 샘플로 확인해야 한다. 이번 자료에서 공개 Live 엔진 SDK·재생 형식은 확인하지 못했다. |

브러시 파일을 읽을 수 있다는 것, preset의 모든 의미가 대응한다는 것, 기존 artwork가
동일 픽셀로 재생된다는 것은 서로 다른 호환성 수준이다. 예를 들어 Krita의 MyPaint 엔진은
새 preset을 `.kpp`로 저장하여 MyPaint의 `.myb`로 곧바로 되돌려 쓰지 못한다고 명시한다.
[Krita MyPaint Engine](https://docs.krita.org/en/reference_manual/brushes/brush_engines/mypaint_engine.html)

NyatiDraw가 직접 정할 것은 dab 평가와 캔버스 읽기의 경계, 고정된 preset snapshot,
종이 좌표와 난수 수명, CPU/GPU 합성 계약이다. 외부 엔진의 API를 그대로 내부 모델로
채택하지 않는다. 최소 연필은 기존 CPU 참조 렌더러와 GPU 타일 경로를 확장하며, 새 외부
의존성이나 이미지 자원 없이 제한된 절차적 grain을 사용한다.

## 현재 NyatiDraw 엔진의 출발점

코드 근거는 [brush](../../crates/brush/src/lib.rs),
[CPU paint](../../crates/paint-cpu/src/lib.rs),
[GPU dab shader](../../crates/paint-gpu/src/round_dab.wgsl),
[stroke 계약](../../crates/stroke/src/lib.rs),
[ADR-0051](../decisions/ADR-0051-basic-brush-controls.md)이다.

| 영역 | 엔진에서 확인한 범위 | 다음 단계에서 지킬 의미 |
|---|---|---|
| 입력 평가 | 원형 dab, arc-length 재표본화, 크기 sqrt 필압·농도 linear 필압의 독립 on/off·최소값 | 기본 필압 곡선과 가는 선 간격은 제한된 새 버전에서 검토 |
| tip·grain | 기존 round v1/v2에는 반지름과 hardness만 있고 bitmap tip·grain 없음 | hardness는 외곽 감쇠이지 2H/2B 흑연 등급이 아님. 새 연필 v3는 별도 부착·grain 계약을 사용 |
| 누적 | CPU/GPU의 per-dab build-up, `coverage × opacity × flow` | wash는 획의 중간 표면과 원본 합성까지 포함하는 별도 의미 |
| 저장 | Begin에서 설정 고정, stroke snapshot·sample·seed·before root, CPU materialization | preset 편집이 이미 그린 획을 바꾸지 않아야 함 |
| 호환 | round engine v1 재생 보존, v2 필압/경도 wire·hash·capability | 기존 버전을 새 공식을 사용해 재해석하지 않음 |
| 보정 | 위치를 한 번 보정한 sample을 live와 저장에 함께 사용 | 미리보기용 압력 경로와 실제 펜 감각을 같은 증거로 취급하지 않음 |

특히 현재 opacity와 flow는 필드가 분리되어 있어도 기본 renderer에서는 둘의 곱으로
작동한다. UI에 슬라이더 하나를 추가하는 것으로 획 전체 불투명도와 dab 물감량이 독립적인
사용 결과를 갖게 되지는 않는다. 이 동작은 ADR에 명시된 기존 계약이다.

## 제안하는 순서와 독립 작업 단위

필수 2H/2B 연필과 선택적 후속 엔진 기능을 구별한다. 원리 비교를 근거로 범용 엔진을
한꺼번에 재작성하지 않으며, 최소 자체 연필의 승인은 wet/범용 texture 승인으로 넓히지 않는다.

| 단계 | 구체적인 산출물 | 의존성과 완료 기준 |
|---|---|---|
| A. 가족·프리셋 UI | 선택 가족 안의 세부 프리셋, 검정 Pencil 기본값, preset별 마지막 설정 | 엔진 재작성 없이 진행 가능. 전환·임시 지우개·설정 기억이 올바른 preset을 가리킴 |
| B. 실제 획 미리보기 | 속성 상단의 bounded scratch 렌더, 현재 설정 반영 | A의 preset 조회 계약을 공유. 문서·최근 크기·실제 입력 상태를 변경하지 않음 |
| C. 필수 2H/2B 자체 연필 | 두 template의 다른 크기/부착 반응, 고정 문서좌표 procedural grain, upright fallback | 별도 v3 wire/hash/reader gate, CPU/GPU·signed 타일·history·새 프로세스 재열기. [세부 설계](pencil-brush-design.md) |
| D. 연필 사용감 검토 | tap·얇은 선·필압·교차·덧칠 gallery와 실제 펜 확인 | C의 합성 입력 증거와 물리 펜 감각을 구분. tilt의 document 좌표 계약 이후 옆면 접촉을 별도 검토 |
| E. 선택적 wash / Brush 템플릿 | 획 opacity 상한이 실제 작업에 필요한지 확인, 다양한 Brush 표현 | 필수 건식 연필의 선행 조건이 아님. wash는 원본 타일+획 표면·선택·취소까지 별도 검증 |
| F. 후속 혼색/범용 질감 연구 | 기존 색 읽기·쓰기 순서, 자원 시스템, wet 상태의 독립 설계 | 보류 유지. 외부 엔진 통합 단계가 아니며, 이번 두 연필 범위에 포함하지 않음 |

최소 연필에서 먼저 정할 질문은 '질감 이미지를 얹을 것인가'보다 '종이 결이 문서 좌표에 고정되는가,
tip을 따라 움직이는가, 필압이 결의 채워지는 정도를 바꾸는가'이다. 같은 흑연 preset이라도
이 답에 따라 확대·회전·겹침·타일 경계에서 결과가 달라진다. Procreate의 grain 구분과 CSP의
per-plot texture가 이 질문을 구체화하는 참고가 된다. 범용 node graph, 광범위 외부 preset
호환, 고급 wet/texture 기능을 A~C의 선행 조건으로 만들지는 않는다.

## artwork와 성능 검증 기준

새 엔진 단계의 작은 gallery는 점, 일정 필압, 증가·감소 필압, 느린/빠른 곡선, 한 획의
자가 교차와 두 획의 교차, 작은 크기와 큰 크기, signed 타일 경계를 포함한다. 질감 단계에는
같은 위치에 겹친 자국과 보기 확대·회전, 혼색 단계에는 두 색 경계와 투명 경계를 추가한다.
각각 무엇이 달라져야 하는지 기대 동작을 먼저 적는다.

자동 시험은 artwork·입력 transition·history·recovery를 보호하는 핵심 불변식에 한정한다.
같은 엔진 버전·설정·seed·입력을 나누어 전달해도 같은 결과가 나오는지, 기존 stroke를
다른 버전으로 재해석하지 않는지, Cancel·Undo·저장·새 프로세스 재열기에서 그림을 보존하는지
검증한다. CPU/GPU 비교는 합의한 허용 오차와 byte 동일 영역을 구별하며, 새 기능이 그림을
바꾸면 저장 후 재열기까지 완료해야 한다.

큰 tip, 작은 spacing, 여러 stamp, texture sampling은 처리량을 늘릴 수 있는 후보 요인이다.
실제 비용은 고정된 corpus와 release build에서 측정한다. sample·dab·dirty tile·preview
메모리 상한을 두고, hardware·OS·backend·p50/p95/p99와 실패 시 중단/복구 동작을 기록한다.
GPU 제출 완료나 readback 시간은 물리 펜에서 첫 가시 픽셀까지의 지연 증거로 바꾸지 않는다.
[성능 정책](../performance.md), [일러스트 gate](../illustration-workflow.md)

경쟁 앱을 직접 그려 비교하는 실험, 외부 라이브러리 통합, 물리 펜·고주사율 화면 측정은
수행하지 않았다. 자체 연필 구현과 검증 상태는 비교 자료와 섞지 않고
[최소 자체 연필 설계](pencil-brush-design.md)에 별도 기록한다.

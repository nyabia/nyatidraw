# Procreate·CSP의 작업 동선과 NyatiDraw 적용 원칙

이 문서는 공식 온라인 매뉴얼과 `ClipStudioOfficial` 튜토리얼을 읽어 비교한 조사다.
두 경쟁 앱에서 직접 같은 그림을 제작하거나 iPad·펜의 조작감과 속도를 측정하지 않았다.
아래 순서는 문서의 기능을 일러스트 작업에 맞춰 연결한 대표 경로이며 모든 작가의
작업 순서를 뜻하지 않는다. NyatiDraw의 구현 완료·실행 검증 판정도 아니다.

Procreate는 iPad용 현재 Help Center/Handbook을 사용했다. Pocket·Dreams와 오래된
PDF는 비교 근거에서 제외했다. CSP는 태블릿 Simple/Studio를 구분했다. 공식 매뉴얼은
5.0에서 `Sub Tool`을 `Tool`, `Sub tool palette`를 `Tool Group palette`로 바꿨다고
명시한다. 구버전 화면의 용어를 현재 배치로 단정하지 않는다.
[CSP 5.0 매뉴얼 변경](https://help.clip-studio.com/en-us/manual_en/030_new/030_new.htm),
[Procreate 현재 Help Center](https://help.procreate.com/procreate).

## 화면에 남기는 것과 필요할 때 여는 것

| 구분 | 상시 접근 가능한 기본 UI | 도구·상태에 따라 나타나는 UI | 별도로 여는 UI |
| --- | --- | --- | --- |
| Procreate iPad | 상단 Paint/Smudge/Erase·Layers·색, 반대편 편집 도구, 측면 크기·불투명도·Undo/Redo | 선택·변형을 시작하면 해당 툴바와 캔버스 표시 | 선택된 그리기 도구 재탭으로 브러시 목록, Layers 버튼으로 레이어 목록, 레이어 탭으로 옵션, Actions로 공유·설정 |
| CSP Simple 태블릿 | 좌측 도구·색, 크기·불투명도 슬라이더, 우측 레이어, 저장·Undo/Redo | 선택 시 하단 런처, 변형 시 핸들·설정 | 도구 재탭으로 설정/브러시 목록, 레이어 재탭/스와이프로 옵션, 우상단 메뉴로 출력 |
| CSP Studio 태블릿 | Command Bar, Tool 팔레트, Tool Slider, Layer와 Layer Properties 팔레트가 기본 구성 | 선택 런처, 현재 도구에 맞는 Tool Settings | 도구 더블탭으로 Tool Group/설정, 필요 팔레트 표시·숨김 및 작업공간 조정 |

이 표의 ‘상시’는 기본 편집 화면 기준이다. 전체화면·팔레트 숨김까지 고정이라는 뜻은
아니다. CSP Simple은 브러시 목록을 연 뒤 캔버스를 탭하면 목록을 닫는다.
이는 팝오버의 종료 규칙이지, NyatiDraw에서 그 탭을 곧바로 획으로 처리해야 한다는
근거는 아니다.
[Procreate Interface](https://help.procreate.com/en/procreate/handbook/interface-gestures/interface),
[CSP Tablet interface](https://help.clip-studio.com/en-us/manual_en/090_tablet/Tablet_interface.htm),
[CSP Using Simple Mode](https://help.clip-studio.com/en-us/manual_en/090_tablet/Using_Simple_Mode.htm).

## 러프에서 출력까지의 조작

### 1. 러프 → 선화: 브러시와 레이어의 농도를 구분한다

- **Procreate:** Paint 선택 → 재탭해서 브러시 선택 → 측면에서 크기·불투명도 조절 → 러프.
  Erase로 지운 뒤 Paint로 돌아온다. 선택되지 않은 Paint/Erase 아이콘을 길게 누르면
  현재 브러시 설정을 전달할 수도 있다. 브러시별 크기·불투명도 기억 지점은 각각 최대
  네 개이며, 세부 브러시 편집을 열지 않고 슬라이더에서 다시 선택한다.
  [Paint, Smudge, and Erase](https://help.procreate.com/procreate/handbook/brushes/paint-smudge-erase).
- **CSP Simple:** Brush → Sketch/Pencil 선택 → 크기 조절 → 러프 레이어에 그리기 →
  러프 **레이어**의 불투명도를 낮추기 → 별도 선화 레이어 선택 → 선화.
  공식 입문 예제는 같은 펜 느낌으로 지우기 위해 투명색도 사용한다.
  브러시 농도와 이미 그린 러프 전체의 레이어 농도는 다른 조작이다.
  [공식 Simple 입문 튜토리얼](https://tips.clip-studio.com/en-us/articles/16712).
- **CSP Studio:** Tool에서 Pencil/Pen → Tool Group에서 종류 선택 → 크기/설정 조절 →
  그리기. 수정에는 Eraser 또는 투명색을 사용하고 원래 그리기 도구/색으로 돌아온다.
  [Using Studio Mode](https://help.clip-studio.com/en-us/manual_en/090_tablet/Using_Studio_Mode.htm).

**적용 판단:** ‘지운 뒤 같은 펜으로 계속’이 먼저다. 새로운 지우개 제스처보다
원래 세부도구·크기·불투명도·색의 복귀 상태를 확인해야 한다. 경쟁 앱의 동작만으로
NyatiDraw에 아직 없는 임시 지우개·투명색을 이미 지원한다고 표현하지 않는다.

### 2. 선택 → 이동/변형 → 선화 계속: 선택과 도구는 별도 상태다

- **Procreate:** 상단 Selection → 방식 선택 → 영역 지정 → Transform → 핸들/변형
  툴바로 조절 → Transform 버튼으로 확정. 변형 중 Reset은 원래 상태로 돌린다.
  선택 후 다른 도구로 바꾸면 선택은 유지되고 선택 버튼과 현재 도구가 함께 강조된다.
  선택 자체를 끝내려면 Selection을 다시 누른다.
  [선택](https://help.procreate.com/procreate/handbook/5.0/selections/selections-interface),
  [변형](https://help.procreate.com/procreate/handbook/transform/transform-interface-gestures).
- **CSP Simple:** Selection area → 하단 런처에서 Lasso/Rectangle/Circle와 추가/빼기 →
  영역 지정 → 런처 또는 도구 막대의 Move/Transform → 핸들 조절. 상세 변형 방식과
  비율 유지 설정은 변형 팝업에서 고른다.
- **CSP Studio:** 선택 도구로 영역 지정 → 선택 아래 Selection Launcher의 Scale/Rotate →
  핸들 조절. 수치와 비율 설정은 변형에 맞춰 바뀐 Tool Settings에서 조절한다.
  수정이 끝나면 선택을 해제해야 이후 그림이 제한되지 않는다.
  [공식 선택·변형 안내](https://tips.clip-studio.com/en-us/articles/9939),
  [5.1 변형 안내](https://tips.clip-studio.com/en-us/articles/11035).

**적용 판단:** ‘화면 이동’, ‘그림 이동’, ‘변형’의 대상과 효과를 계속 구분한다.
선택 해제·변형 취소·확정·원래 도구 복귀를 하나의 모호한 닫기 동작으로 합치지 않는다.
NyatiDraw에서는 명시적 적용/취소와 기존 Undo 계약을 우선하며 경쟁 앱의
도구 버튼 재탭 확정을 그대로 옮길 이유는 없다.

### 3. 밑색: 경계 참조와 색을 기록할 레이어를 분리한다

| 앱/모드 | 대표 조작 순서 | 사용자가 확인할 참조 상태 |
| --- | --- | --- |
| Procreate | 선화 레이어 옵션 → Reference → 별도 밑색 레이어 선택 → 현재 색을 영역에 ColorDrop | 선화에 Reference 표시. 드롭 후 누른 채 좌우로 Threshold를 조절하고 손을 떼 확정 |
| CSP Simple | 밑색 레이어 선택 → Fill → 색 선택 → 영역 탭. Fill 재탭/하단 설정에서 참조·허용차·틈 닫기 조절 | Refer all layers를 켜면 다른 레이어의 선/색을 참조하고, 끄면 선택 레이어만 참조 |
| CSP Studio | 선화를 Reference Layer로 지정 → 밑색 레이어 선택 → Fill/Refer other layers → 필요하면 Refer multiple을 Reference layer로 지정 → 영역 탭 | Layer 팔레트의 참조 아이콘과 Fill의 참조 범위를 각각 설정. 숨긴 레이어는 참조하지 않음 |

[Procreate Reference](https://help.procreate.com/procreate/handbook/5.4/layers/layers-options),
[ColorDrop 조작](https://help.procreate.com/articles/zmlayd-fill-an-area-using-colordrop),
[CSP 공식 채색 안내](https://tips.clip-studio.com/en-us/articles/9941),
[CSP Reference layers](https://help.clip-studio.com/en-us/manual_en/180_layers/Reference_layers.htm),
[CSP 참조 범위](https://help.clip-studio.com/en-us/manual_en/810_subtools/R.htm).

CSP 공식 레이어 안내에서는 **특정 Reference Layer 지정/편집은 Simple에서 지원하지
않으며 Studio로 전환**하라고 설명한다. Simple의 ‘전체 레이어 참조’와 같은 기능으로
기록하면 안 된다. [공식 모드별 제한](https://tips.clip-studio.com/en-us/articles/9938).

**적용 판단:** NyatiDraw에서는 채우기 속성에 ‘칠할 곳’과 ‘경계를 읽을 곳’을 짧게
표시하는 것이 우선이다. ‘참조’ 하나만 표시하면 자료 이미지 창과도 혼동한다.
숨긴 러프·종이·다른 프레임까지 무엇을 읽는지 기존 엔진 계약과 일치시켜야 한다.
ColorDrop 제스처나 자동 틈 닫기 추가는 이 표시 문제의 해결과 별개다.

### 4. 클리핑 명암: 원본 밑색을 건드리지 않는 대상이 보여야 한다

- **Procreate:** 밑색 위에 새 레이어 → 그 레이어 옵션에서 Clipping Mask → 명암 그리기.
  아래 레이어의 내용이 보이는 범위를 제한한다. Alpha Lock은 별도 레이어를 만드는
  클리핑과 구별된다.
  [Clipping Mask](https://help.procreate.com/procreate/handbook/5.3/layers/layers-mask).
- **CSP Simple:** 밑색 위 새 레이어 → 레이어를 탭해 옵션 → Clip to Layer Below.
  **Studio:** 같은 대상 선택 후 Layer 팔레트 상단 명령에서 적용한다. 투명 픽셀 잠금은
  현재 레이어 안에서 다시 칠하고, 클리핑은 별도 레이어에서 명암을 쌓는 용도다.
  [공식 레이어 기능 안내](https://tips.clip-studio.com/en-us/articles/9938).

**적용 판단:** 클리핑 상태, 현재 레이어, 이미지/마스크 편집 대상을 한눈에 구별한다.
아이콘을 더 늘리기보다 선택된 대상의 작은 속성 영역에서 이름과 상태를 읽게 한다.
마스크 흑백·지우개 의미는 앱마다 다를 수 있으므로 NyatiDraw 계약을 따라 안내한다.

### 5. 저장 → PNG: 계속 수정할 파일과 전달할 결과물을 구분한다

- **Procreate:** Actions → Share → `.procreate` 또는 PNG → 목적지. 전자는 레이어·마스크
  등을 보존하며 PNG는 합쳐진 이미지다. 같은 Share 안에 있어도 결과물의 의미가 다르다.
  [Share](https://help.procreate.com/procreate/handbook/actions/actions-share).
- **CSP Simple:** 저장 아이콘으로 레이어를 유지하는 `.clip` 저장. 상단 `…` → Export
  image로 PNG/JPEG 등 출력. Simple의 PSD 출력도 단일 레이어로 합쳐진다는 주의가 있다.
  [Using Simple Mode](https://help.clip-studio.com/en-us/manual_en/090_tablet/Using_Simple_Mode.htm).
- **CSP Studio:** Save/명령 막대로 편집 파일 저장 → File → Export (Single Layer) → 형식,
  설정·미리보기·목적지 확인. PNG에는 투명도와 출력 크기 설정이 있다.
  [Save file](https://help.clip-studio.com/en-us/manual_en/210_file/Save_file.htm),
  [Exporting files](https://help.clip-studio.com/en-us/manual_en/210_file/Exporting_files.htm).

**적용 판단:** NyatiDraw의 NTDR/paired 저장과 독립 PNG 출력은 기존 계약대로 유지한다.
자동 복구가 됐다는 상태와 파일 저장·다운로드가 끝났다는 상태를 같은 문구로 보여주지
않는다. 출력 취소/실패가 저장된 작품 손실처럼 보이지 않아야 한다.

## NyatiDraw에 적용할 7개 원칙

다음은 위 공식 문서에서 도출한 **제안**이다. 코드 결함이나 구현 완료 판정은 실제
NyatiDraw 조작 감사에서 별도로 해야 한다. 사용자 확정사항인 무채색·공간 절약,
좌측 도구/세부도구/속성/크기 빠른 선택 분리, G/B/E/F/C 단축키를 유지한다.

1. **현재 작업의 값은 한곳에서 계속 읽히게 한다.** 브러시 크기·불투명도와 레이어
   불투명도의 이름/대상을 구분한다. 세부도구 목록은 선택, 속성은 현재 도구 값,
   크기 빠른 선택은 반복 크기로 역할을 고정한다. 같은 설정을 여러 곳에 복제하지 않는다.
2. **지우기와 색 채취 후 돌아갈 도구를 보존한다.** 팝오버·값 입력을 닫은 뒤 입력 초점과
   이전 세부도구가 유지되는지 확인한다. Procreate 길게 누르기나 기기 전용 제스처를
   필수 통로로 삼지 않고 화면 버튼과 기존 키보드 경로를 유지한다.
3. **선택 상태를 도구보다 오래 유지하더라도 숨기지 않는다.** 선택 경계와 해제 조작은
   그리기로 돌아온 후에도 찾을 수 있게 한다. 새 문맥 막대는 기존 속성 영역의 역할을
   우선 활용하며, 선택할 때마다 캔버스 옆에 별도 대형 팔레트를 늘리지 않는다.
4. **변형에는 결과와 탈출구가 함께 있어야 한다.** 미리보기, 적용, 취소를 보이게 하고
   캔버스 팬/줌과 픽셀 변형을 구별한다. 화면을 줄이려고 적용/취소를 숨은 제스처에만
   맡기거나 포인터 해제와 패널 닫기에 서로 다른 확정 의미를 암묵적으로 부여하지 않는다.
5. **밑색과 명암에서 편집 대상·참조 대상을 같이 확인한다.** ‘밑색 레이어에 칠함 / 선화
   참조’처럼 실제 이름/범위가 도움이 된다. 클리핑·잠금·마스크 상태는 짧은 설명을
   제공하되 모든 행에 영구 버튼을 추가하지 않는다.
6. **임시 UI는 닫혀도 작품 상태를 바꾸지 않는다.** 브러시 목록·레이어 옵션·타임라인의
   닫기와 작품 삭제/선택 해제/변형 확정을 혼동시키지 않는다. 캔버스 복귀 탭이 의도치
   않은 획이 되는지, 재정렬과 목록 스크롤이 충돌하는지 실제 터치 경로로 확인한다.
7. **저장과 출력의 목적·완료·실패를 각각 알린다.** 계속 편집할 파일과 게임에 넣을 PNG를
   구별한다. 메뉴 이름을 경쟁 앱처럼 바꾸는 것보다 현재 저장 경로·복구·출력 결과를
   정확히 안내하고 저장→프로세스 종료→재열기에서 작품 보존을 확인하는 것이 우선이다.

Simple/Studio 전체 모드 복제, 무조건 떠다니는 메뉴, 제스처 전용 명령, 경쟁 앱의 모든
브러시/변형/색상 기능 추가는 이 조사에서 요구하지 않는다. 특히 네이티브 iPad 앱의
Pencil 기능을 웹에서도 가능하다고 간주하지 않는다. 새 UX의 합격은 버튼 수가 아니라
러프→선화→밑색→명암→출력 중 잘못된 대상에 그리거나 복귀 경로를 잃는 일이 줄었는지로
판정한다. 물리 타블렛·Apple Pencil·iPad Safari 감각과 표시 지연은 계속 미검증이다.

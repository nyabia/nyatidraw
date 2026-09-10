# ADR-0057 — Alpha lock와 하위 clipping stack, Normal/Multiply

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

상태: E3 구현·핵심 시험·실제 GPU/별도 프로세스 재열기 검증 완료. 실제 창/물리 펜 수동 gate는 별도.
정확한 실행 범위와 남은 확인은 [Essentials E3](../sprints/essentials-goal.md)에 기록한다.

## 사용자 의미

- 투명도 잠금은 raster의 기존 alpha를 정확히 보존한 재칠이다. 편집 잠금과 다르다.
  붓/fill/선택 칠/gradient는 source-atop 재칠로 동작한다. 지우개/픽셀 삭제/비우기/cut/
  기하 변형은 잠금 중 명시적으로 거절한다. 잠금을 풀면 원래 기능을 쓸 수 있다.
  alpha만 사후 원복하는 방식은 RGB premultiplication과 색을 깨뜨리므로 사용하지 않는다.
  이 잠금은 픽셀 편집 정책이다. 레이어 자체 삭제/복제나 출력 페이지 크기 변경을 막는
  편집 잠금이 아니다. 페이지 crop의 전체 작품 좌표 원점 재배치는 모든 레이어를 같은
  정수 거리만큼 옮기며 alpha/형태를 보존하므로 허용한다. 선택 조각의 개별 변형과 구별한다.
- clipping은 같은 부모 아래의 **연속 clipped 형제들이 하나의 아래 non-clipped base**를
  공유하는 모델이다. 직전 clipped 레이어의 alpha를 다음 레이어의 mask로 쓰지 않는다.
  base는 raster 또는 isolated group이며 group 자체도 clipped될 수 있다. Through는 없다.
- stack 내부는 base raw alpha를 유지한다. clipped 레이어 opacity는 그 레이어를 합성할 때,
  base opacity는 완성된 stack 전체에 한 번 적용한다. base blend mode는 stack이 부모
  backdrop에 올라갈 때 적용한다. clipped blend mode는 stack 내부 재칠에 적용한다.
- 숨겨진 base는 stack 전체를 숨긴다. opacity 0 base도 출력하지 않는다. 숨김 때문에 더
  아래의 다른 base를 검색하지 않는다. 부모 안에 아래 base가 없는 orphan clip은 출력하지
  않고 픽셀을 보존한다. 순서 변경/base 삭제 후에는 현재 구조로 관계를 다시 계산한다.
  그룹 경계를 넘어 base를 검색하지 않는다. 레이어 생성 시 자동 Ink/배경 세트를 만들지 않는다.
- Solo/참조 레이어 합성은 선택된 clipped 레이어의 base를 **합성 의존성**으로 포함한다.
  base 색과 alpha가 있어야 Multiply 결과와 원래 문맥을 보존할 수 있다. 다른 clipped
  형제는 선택 범위에 포함된 경우만 합성한다. 일반/참조/출력은 원래 숨김을 따른다.
  Solo만 기존 ADR-0052처럼 대상과 조상 경로를 임시 표시하고 필수 base도 표시한다.
  Solo 그룹/base 그룹 내부의 자식은 원래 가시성을 따른다. 저장된 가시성은 바꾸지 않는다.
  따라서 clipped 레이어 Solo는 base와 해당 레이어의 문맥 보기이며 독립적인 raw 픽셀 보기가 아니다.
- 레이어 썸네일은 기존처럼 해당 raster의 raw 페이지 내 픽셀을 보여줄 수 있다. 완성 합성,
  navigator, 보이는 색의 스포이트, 출력은 동일한 stack 의미를 사용한다. 썸네일을 실제
  clipping 출력인 것처럼 표시하지 않는다. 그룹 미리보기는 그룹 내부 합성을 사용한다.

## 수학

모든 색은 기존과 같은 linear-light premultiplied RGBA다. `S,a`는 opacity 적용 후 source
RGB/alpha, `D,b`는 destination RGB/alpha이며 아래 식은 0~1 범위다.

| 연산 | RGB | Alpha |
|---|---|---|
| Normal over | `S + D*(1-a)` | `a + b*(1-a)` |
| Multiply over | `S*(1-b) + D*(1-a) + S*D` | `a + b*(1-a)` |
| Normal clipped / alpha lock | `S*b + D*(1-a)` | `b` |
| Multiply clipped | `D*(1-a+S)` | `b` |

clipped source에 base alpha를 미리 곱하고 다시 atop하면 AA 경계가 두 번 약해지므로 금지한다.
CPU는 정수 반올림과 premultiplied 상한을 명시하고 GPU도 같은 단계에서 양자화한다.
정확한 불변식은 alpha 보존·투명 입력·다른 레이어 보존이며 CPU/GPU 허용 색 오차는 실제
검증에서 보고한다. blend 계산에 sRGB bytes를 직접 사용하는 변경은 하지 않는다.

## 저장과 실행 경계

- layer wire v3에 alpha lock/clip/blend를 저장한다. v1/v2는 false/false/Normal로 읽는다.
  새 capability marker와 metadata/history publication은 같은 DB 트랜잭션에서 갱신한다.
  지원하지 않는 태그/손상/root 설정은 원본을 덮지 않고 거절한다.
- alpha lock은 stroke Begin의 불변 실행 설정에 고정하고 commit/hash/replay에도 보존한다.
  현재 레이어 설정을 과거 stroke 재생에 다시 읽지 않는다. legacy false 경로의 hash/wire와
  예전 그림 결과를 유지한다. 새 기능을 구버전이 정상이라고 오해해 읽게 하지 않는다.
- raw 입력은 UI를 우회한다. UI 토글은 actor가 적용하며 이미 진행 중인 stroke와 섞지 않는다.
  clipping은 비파괴 표시/합성 속성이고 alpha lock은 픽셀 변경 정책이다.
- CPU 출력/picker/미리보기와 GPU page/sparse 합성을 함께 구현한다. dirty base는 동일 좌표의
  stack/부모 합성을 무효화한다. GPU-only 원본이나 저장 전용 다른 합성 모델을 만들지 않는다.

## 근거와 범위

CSP는 투명도 잠금과 아래 레이어 clipping을 별도 기능으로 제공하고, Through가 아닌
폴더를 clipping 대상으로 허용한다. 이것이 사용자 모델의 참고이며 위 세부 stack/숨김/
Solo/삭제 계약 전체를 CSP와 bit-exact 일치한다고 주장하지 않는다.
[CSP 공식 설명](https://help.clip-studio.com/en-us/manual_en/180_layers/Other_layer_settings.htm).

Multiply over의 투명 backdrop 항은 생략할 수 없다. 일반 합성/Multiply 기준은
[W3C Compositing and Blending](https://www.w3.org/TR/compositing-1/#blendingmultiply)을 참고한다.
layer mask/Through/Screen/Add/색 보정은 E3 밖이다. 실제 창·펜·작품 사용 gate는 별도다.

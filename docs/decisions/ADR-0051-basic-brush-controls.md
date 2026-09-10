# ADR-0051 — 기본 브러시 표현과 구버전 그림 보존

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 결정

일러스트 W1의 기본 선화/덧칠을 위해 기존 round engine에 독립 필압 축과 경도를 추가한다.
새 엔진·preset schema는 2이며, 텍스처·혼색·노드 그래프는 도입하지 않는다. 추가 dependency는 없다.

- 크기 필압: `minimum + (1 - minimum) * sqrt(pressure)`.
- 불투명도 필압: `minimum + (1 - minimum) * pressure`.
- 각 필압 축을 끄면 해당 배율은 1이다. UI의 크기/불투명도 최댓값은 별도로 적용한다.
- 경도는 불투명한 내부 반지름의 비율이다. 경도 1은 기존 원, 0은 중심에서 외곽까지
  부드러워지는 원이다. 내부 밖은 정규화한 반지름으로 smoothstep 감쇠한다.
- CPU/GPU 모두 기존 4×4 subpixel 위치에서 coverage를 계산한다. GPU 인스턴스에 경도를
  추가하지만 레이어 권위 픽셀·타일 주소·선택 mask와 합성 방식은 바꾸지 않는다.
- `opacity × flow × coverage` 누적 방식은 기존 build-up이다. stroke 전체의 최대 opacity를
  제한하는 wash 방식이 새로 생겼다는 의미가 아니다.

## 도구와 UI

크기 5px·불투명도 100%로 시작하며 Pencil/Pen/Brush/Eraser별 설정을 세션에서 기억한다.
Pen/Eraser는 hard, Pencil은 중간 경도와 농도 필압, Brush는 soft를 기본값으로 한다.
UI의 숫자·필압 토글·최소 비율·경도는 semantic command만 전달하며 raw sample을 다루지 않는다.

스트로크 Begin에서 preset을 고정한다. 이후 설정 변경으로 진행 중/저장 중 stroke를
재해석하지 않는다. 펜 뒤집기는 기억된 Eraser preset 복사본을 사용하고 선택 도구는 유지한다.
선택/fill/picker 같은 편집 도구를 뒤집기로 임시 override하는 것은 아직 지원하지 않는다.

`[`/`]`는 actor 현재 크기를 약 10%씩 조절한다(0.1~200px). 명령에 대상 도구를 명시하여
같은 도구의 연속 상대 변경만 stale UI revision에서 적용 가능하다. 다른 도구나 비그리기
도구로 전달된 명령, 미래 revision은 기존 거절 경계를 유지한다.

## 속성 / 크기 빠른 선택 UX 후속

- 세부 도구·도구 속성·크기 빠른 선택은 독립 도킹 패널로 유지한다.
- 기본 속성은 크기·불투명도·경도·선 보정이며, 숫자 입력과 단위는 오른쪽 정렬한다.
  크기/불투명도 필압과 최소값은 기본 닫힘 `추가 옵션` 안에 둔다.
- 빠른 크기는 최근 4칸과 3열×4행 기본 크기를 제공한다. 각 기본 칸의 점/숫자는
  flex 1:1로 배분하며 숫자는 오른쪽 정렬한다. 좁게 줄인 패널의 접근성을 위해 overflow는
  허용하지만 기본 높이에서는 기본 크기 전체를 스크롤 없이 표시한다.
- 최근 크기는 **승인된 그리기 Begin**에서만 기록한다. 크기 변경/도구 전환/스포이트/
  선택/채우기와 거절된 입력은 기록하지 않는다. 지우개 획은 실제 지우개 크기를 기록한다.
  최신 사용을 맨 앞에 놓고 중복 제거 후 4개를 유지하며 처음에는 비어 있다.
- 최근 크기는 세션 UI 상태다. 획이 뒤에 취소되거나 Undo/Redo되어도 사용 기록은 남는다.
  문서 픽셀·history root·dirty·저장 포맷을 변경하지 않고 새 히스토리 항목도 만들지 않는다.
  native drain마다 최대 4개의 크기만 모아 UI에 의미적 변경을 발행하며 샘플은 보내지 않는다.

## 저장 호환

engine1은 기존 sqrt-size/linear-opacity/hard circle을 그대로 재생한다. 기존 preset wire와
stroke hash에는 새 필드를 넣지 않는다. engine2의 wire/hash에만 두 bool과 세 f32를 기록한다.
잘못된 bool, 잘린 payload, 비정상 preset은 유효한 stroke로 복구하지 않는다.

프로젝트에는 새 brush capability bit `0x200`을 사용한다. engine2 stroke commit과 같은
트랜잭션에서 표시하고, 레이어/page metadata 업그레이드에서도 보존한다. 기존 압축 bit
`0x100`과 독립적이며 구버전 앱이 새 그림의 히스토리를 잘못 해석하기 전에 거절하게 한다.
새 reader는 기존 marker와 engine1 그림을 읽는다. 구버전으로의 downgrade 저장은 지원하지 않는다.

## 검증 범위

필압 축 조합·engine1 bytes·신규 stroke round-trip/재생·capability 보존·하드웨어 지우개
snapshot 불변식을 핵심 시험으로 확인한다. GPU differential은 hard와 soft fixture를 함께 쓴다.
CLI smoke는 임시 프로젝트 저장 후 별도 validate/export 프로세스를 실행해 출력 바이트를 비교한다.

실행 결과와 미검증 항목은 [W1 기록](../sprints/workflow-w1.md)에 남긴다. synthetic sample과
GPU readback은 물리 펜 감각, 화면 지연 또는 실제 UI acceptance의 증거가 아니다.

UX 후속 검증: desktop/editor all-target Clippy 통과. 기존 release actor/GPU
acceptance에 최근 크기 불변식을 포함해 Windows / RTX 3080 / Vulkan에서 통과했다.
설정만 변경한 경우, 승인된 Begin, Undo/Redo, 스포이트, 거절된 하드웨어 지우개,
취소된 획과 4칸 MRU 중복 제거를 확인했다. 같은 검증은 artwork 저장 후 별도 프로세스
재열기와 PNG 비교도 수행한다. 실제 WebView 레이아웃·물리 펜 수동 검증은 별도다.

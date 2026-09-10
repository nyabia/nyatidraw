# ADR-0053 — 페이지 밖 선택과 유한 작업 범위

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 결정

선택은 페이지의 크기가 아니라 `origin: [i32; 2]`와 양의 ROI 크기를 가진 문서 좌표 영역이다.
현재 coverage는 binary이며 packed bit로 직렬화한다. 이 변경을 fractional AA 구현이라고
부르지 않는다. E1 자유 변형/E2 AA 경계 작업은 coverage 의미를 별도로 확장해야 한다.

ROI는 최대 16M 픽셀, 편집 작업 공간은 최대 256MiB로 제한한다. 멀리 떨어진 선택의 합집합이
상한을 넘으면 기존 그림/선택을 보존하고 거절한다. 무한 캔버스라고 무한 dense mask를 만들지 않는다.
전체선택/반전은 페이지 + 현재 레이어의 저장 타일 범위 + 현재 선택의 유한 합집합 사각형이다.
화면의 팬/줌에 따라 그 의미가 달라지지 않는다. 타일 경계에 포함된 투명 픽셀도 범위에 포함된다.

## 입력과 편집

사각/올가미는 페이지 밖에서 생성할 수 있고 교체/추가/빼기를 도구 속성에서 지정한다.
전체/반전/해제/선택 픽셀 삭제는 별도 명령이다. Delete는 레이어 비우기가 아니다.
Ctrl+A 전체, Ctrl+D 해제, Ctrl+Shift+I 반전, Delete 선택 픽셀 삭제를 native와 Web UI에 연결한다.
텍스트 입력은 기존 입력 격리 정책을 따른다. 잠긴 레이어의 픽셀 삭제는 거절한다.

CPU 채색·그라데이션·copy/cut·정수 변형과 GPU 브러시/지우개 마스크·선택 overlay 모두
signed origin을 사용한다. 화면 밖/페이지 밖 그림은 보존하되 PNG는 기존 페이지에 한정한다.
GPU uniform의 tile origin과 mask origin을 구별하고, ROI 바깥 타일을 잘라내는 최적화도
mask 원점을 반영한다. integer subtraction은 signed overflow로 먼 좌표를 mask 안에 감지하지 않아야 한다.

## 저장 호환

- 원점 0은 기존 `NYSEL001` 태그, 크기, packed bits 및 selected-v1 hash를 그대로 유지한다.
- 원점이 0이 아닐 때만 `NYSEL002`, signed x/y, 크기, packed bits와 selected-v2 hash를 쓴다.
  새 태그의 원점 0은 비정규 표현으로 거절한다. 원점도 commit identity에 포함된다.
- capability `0x400`을 같은 저장 트랜잭션에서 켜고 layer/page/압축/Undo 변경 뒤에도 유지한다.
  구버전 reader가 지원하지 않는 문서를 조용히 다른 좌표로 재생하게 하지 않는다.
- 현재 stroke를 decode할 때 capability와 payload를 검사한다. 전체 과거 semantic history를
  새로 선행 스캔하는 보장은 추가하지 않으며 기존 지연 검증 구조를 유지한다.

추가 dependency 없음. 자유 변형 preview·선택 유지·임의 회전은 다음 E1 작업이다.

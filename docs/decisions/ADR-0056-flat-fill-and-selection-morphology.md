# ADR-0056 — 참조 선화 밑색과 선택 경계 연산

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

상태: E2 구현, 코어/worker/실제 GPU·release 빌드 검증 통과. 실사용 합격과 구별한다.

## 결정

- 채우기는 불변 원본에서 연결 영역을 찾고 활성 raster에 쓴다. 원본은 현재 레이어,
  보이는 합성, 표시된 참조 레이어 합성 중 선택한다. 참조 선 자체는 수정하지 않는다.
- 선택이 있어도 전체 선택 칠하기로 바꾸지 않는다. 선택의 구멍과 외부는 탐색과 확장의
  통과 금지 영역이다. 명시적 선택 전체 칠하기는 별도 명령으로 남는다.
- tolerance는 seed와 premultiplied linear RGBA의 최대 채널 차이다. 연결은 4방향이다.
  유한 domain은 페이지·관련 signed 원본 타일·seed를 포함하고 선택 bounds와 교차한다.
  무한 dense 할당을 하지 않으며 예산 초과는 결과 적용 전에 거절한다.
- 틈 닫기 0~8은 barrier map의 사각 구조 요소 반경이다. 분리 가능한 dilation/erosion으로
  closing한다. 특정 폭 이하의 모든 틈을 해결한다는 뜻이 아니며 좁은 의도된 통로도 닫을 수 있다.
- 확장 0~64는 barrier 픽셀로 들어가는 맨해튼 거리다. 다른 seed-matching 연결 영역이나
  선택 밖으로 확장하지 않는다. 선 아래 받침은 destination-over로 기존 대상 선을 보존하고,
  일반 내부 채우기는 source-over다.
- AA 경계 받침은 최소 한 픽셀 underpaint다: `max(expand, AA ? 1 : 0)`.
  supersampling이나 fractional coverage 복원은 아니다. 굵은 선의 안쪽 반투명 경계
  흰 틈을 줄이지만, 반투명 한 픽셀 선 아래를 받치면 그 구간이 불투명해질 수 있다.
  선 바깥 fractional coverage 재구성은 후속 범위다.
- 선택 확장/축소는 픽셀 중심의 정확한 Euclidean 원판 반경 0~64다. 정수 squared-distance
  transform으로 선형 시간 처리한다. signed 범위를 보존하고 축소의 빈 결과를 허용한다.
- alpha에서 선택은 활성 raster의 raw alpha > 0이다. 숨김/opacity/편집 잠금과 독립적이며
  다른 레이어를 읽지 않는다. binary mask이므로 희미한 alpha도 전체 선택 픽셀로 취급한다.

## 소유권과 제한

`paint-cpu`가 불변 입력과 유한 예산으로 결과를 생성하고 worker가 기존 structural history
트랜잭션에 적용한다. 선택-only 연산은 artwork history/타일/ID를 바꾸지 않는다.
GPU는 worker 결과를 기존 adoption 경로에서 받는다. UI/원시 펜 경계와 저장 형식은 그대로다.
새 dependency는 없다. 설정은 이번 범위에서 session-only다.

실제 GPU 검증에서 기존 경계 타일 캐시의 별칭 문제를 발견했다. 페이지 크기가 타일 배수가
아니면 같은 좌표에 page surface와 signed workspace surface가 함께 존재한다. 한쪽 합성
완료가 다른 쪽의 오래된 texture를 유효하게 만들지 않도록 두 캐시를 분리한다. 그림/트리/
Solo 변경은 둘 다 무효화하고 sparse atlas eviction은 sparse 유효성만 제거한다.

최대 픽셀 수 16 Mi, workspace 256 MiB와 frontier 제한을 유지한다. retained 선택·출력·작업
버퍼를 예산에 포함한다. 기존 polygon work 한도를 재사용하면 일반 4K 그림이 거절되므로
선형 필터용 `max_filter_work` 기본 1 Gi work unit을 별도로 둔다. 단위는 시간 보장이 아니다.
좌표·radius·메모리·연산·frontier·잠금·참조 오류는 원본/history 변경 없이 실패해야 한다.

## 검증

- 코어: 열린/닫힌 틈, 굵은 AA 선의 밝고 어두운 배경, 인접 영역, 대상 선 보존,
  signed 선택 구멍·disabled 옵션, 세 원본 모드, 예산 초과의 원자성.
- 선택: 원판 geometry/구멍/음수 좌표, 희미한 alpha, 다른 레이어 보존, 큰 sparse ROI 거절.
- 통합: 실제 fill gesture→worker, 빈 대상과 별도 참조 선, 선택 clipping, selection-only history
  불변, branch를 명시한 Undo/Redo, 저장 후 독립 자식 프로세스 root·PNG decoded pixels 일치.
- 최종 실행 결과와 남은 실제 조작은 [E2 진행 기록](../sprints/essentials-goal.md)에 기록한다.
  합성 fixture를 실제 작품 완성·물리 펜·HWND present 증거로 해석하지 않는다.

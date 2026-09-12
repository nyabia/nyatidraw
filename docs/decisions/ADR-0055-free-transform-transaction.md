# ADR-0055 — 원본 고정 자유 변형과 미리보기 트랜잭션

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 목적과 현재 경계

E1의 임의 각도 회전·크기 조절·선택 유지에 필요한 코어와 worker/actor/UI 경계다.
`AffineTransform`과 `AffineDraft`를 실제 preview·드래그 핸들·붙여넣기에 연결했다.
기존 정수 변형 API는 과거 코어 검증용으로 남지만 사용자 변형 패널은 새 경로를 사용한다.
구현 및 자동 검증과 실제 펜/Windows 창 조작 합격은 구분한다.

## 좌표와 픽셀

선택 bounds의 중심을 pivot으로 flip → scale → 시계방향 회전 → 문서 좌표 이동한다.
명령은 이동 1/1000px, 배율 1/1,000,000, 각도 1/1000도 정수로 표현한다.
이는 보기 반전/줌이 아닌 작품 편집이며 저장할 픽셀을 바꾼다.

selected source를 immutable snapshot에서 읽고, 기존 selected 픽셀을 cut한 대상에
역변환 bilinear 샘플을 source-over 한다. 보간은 premultiplied linear RGBA로 하여
투명 경계 RGB의 잘못된 혼합을 피한다. 기존 선택 밖/다른 레이어는 cut하지 않는다.
필터의 반 픽셀 support도 출력 ROI에 포함한다. Identity는 픽셀과 선택을 그대로 돌려준다.

선택 출력은 현재 binary coverage다. 선택된 투명 픽셀도 기하학적으로 이동하고,
보간 support에 포함된 가장자리도 선택에 남긴다. fractional 선택 mask/area-filter
축소/고급 interpolation까지 구현했다고 하지 않는다. 극단 축소로 선택 coverage가
완전히 사라지는 변형은 원본을 삭제하는 대신 원자적으로 거절한다.

## 미리보기와 확정

`AffineDraft`는 source snapshot id/root, tree, target layer, selection을 고정한다.
모든 preview는 이 source에서 다시 계산한다. 이전 preview를 다음 source로 쓰지 않는다.
candidate는 하나만 유지하며 재계산 중 기존 candidate의 타일/선택 메모리도 작업 예산에서 뺀다.
source/destination extent, 좌표, 픽셀/연산/작업 공간을 검사한 뒤 계산한다.

generation은 증가해야 한다. 새 preview가 실패해도 마지막 유효 화면을 유지할 수 있지만
그 이전 candidate를 실패한 새 generation의 확정 결과로 쓰면 안 된다.
`prepare_commit`은 generation과 현재 source id/root/tree/target/selection을 다시 확인한다.
실제 저장자는 성공한 결과를 durable commit한 뒤 editor head를 변경한다. draft를 버리는
취소는 original pixels/history를 바꾸지 않는다. selection은 결과와 함께 반환해 재이동에 쓴다.

## 실제 UI 통합

- 계산은 writer/worker에서 수행하고 native 입력/main UI에서 전체 이미지 처리를 하지 않는다.
- 최신 preview 하나와 확정/취소를 구별한다. 확정은 사용자가 본 generation을 대상으로 한다.
- preview는 renderer 표시만 바꾸며 프로젝트/PNG/Undo에는 확정 전 픽셀이 들어가지 않는다.
- 선택 이동 도구를 고르는 것만으로 draft를 열지 않는다. 네이티브 선택 안/핸들 접촉이나
  명시적 변형 시작이 draft를 연다. 실제 drag 중 Move는 갱신, End는 drag 종료다.
  Enter는 확정, Escape는 취소이며, 다른 도구를 고르면 아래의 원자적 전환 규칙을 따른다.
- Save/Close/history/layer 변경과 provisional paste의 의미를 actor/worker 경계에서 명시한다.
  paste 취소 시 임시 레이어까지 복원해야 하며, 성공한 확정만 하나의 history operation이다.
- 렌더 snapshot이 미리보기라고 durable CPU 상태를 덮어쓰거나 실패한 preview를 저장하면 안 된다.

`TransformWorker`는 durable 원본과 paste의 임시 작업 source를 별도로 고정한다.
`transform_runtime`의 `transform_display`만 미리보기 타일을 참조하며, actor의 `cpu_tiles`와
worker의 session은 확정 전 그대로다. 이전 표시의 타일 키까지 합쳐 빈 픽셀을 upload하여
취소/반복 preview에서 이전 위치가 잔상으로 남지 않게 한다. 썸네일/내비게이터는 확정 상태다.

네이티브 입력은 드래그 하나의 시작 transform과 viewport revision을 고정한다.
안쪽 드래그는 이동, 네 모서리는 중심 기준 크기 조절, 바깥 회전 핸들은 임의 회전이다.
worker 실행 중에도 샘플은 처리하되 최신 변형 파라미터 한 개만 대기한다. End는 마지막
파라미터를 남기고 드래그만 끝낸다. 대기 preview/드래그가 있으면 Commit은 거절한다.
취소는 Begin/Paste 시작 응답 전과 preview 실행 중에도 하나의 보류 요청으로 남겨
다음 응답 후 처리한다. 취소한 시작 요청이 늦게 도착해 새 draft로 살아나면 안 된다.

숫자 이동/배율/회전과 반전은 `미리보기`로 갱신한 뒤 `확정`한다. 캔버스 Enter는 확정,
Escape는 취소다. HTML 숫자 필드의 키는 패널 내부에서 처리한다. 확정/취소는 접수 시점이
아닌 authoritative draft 제거 후 패널을 닫는다. 결과 선택은 유지하고 올가미 모드로
복귀한다. 선택 이동 도구의 다음 접촉 또는 변형 시작 버튼은 보존된 선택으로 새 draft를 연다.

다른 도구 선택/분류 순환은 최신 목표 도구 하나만 보류한다. 변경 없는 일반 draft는
취소하고, 변경된 draft 및 임시 붙여넣기는 최신 대기 preview의 처리 뒤 확정한다.
실패한 preview가 있으면 마지막 유효 화면의 transform을 다시 검증하여 새 generation으로
확정한다. 재검증이나 저장 실패 시 도구를 바꾸지 않고 draft를 남긴다. 확정/취소 결과가
actor에 채택된 뒤 도구를 바꾸므로 새 도구 입력이 이전 작업을 덮어쓸 수 없다.
이미 writer에 접수된 Commit은 취소할 수 없다.

Escape 한 번은 draft(시작 중 포함), 활성 네이티브 제스처, 완료된 선택의 순서에서
한 단계만 처리한다. 선택 해제는 그림 삭제가 아니다. 키 자동 반복은 양쪽 키 입력 경로에서
무시한다. `EditProjection.can_cancel`은 시작/preview/draft/활성 제스처의 실제 취소 가능성을
반영하며, 일반 artwork 작업이나 접수된 Commit/Cancel을 취소 가능하다고 표시하지 않는다.

변형 중 일반 편집·레이어/히스토리·Save/Save As는 확정/취소를 요구한다.
보기 이동/줌/회전과 도킹은 허용하되, 드래그 도중 viewport가 바뀌면 그 드래그 시작 상태로
되돌린다. Alt 임시 스포이트는 변형 중 그림을 바꾸지 않으며 읽기 장벽만 정상 해제한다.
Close/다른 프로젝트 열기처럼 worker를 폐기하는 경로는 미확정 draft를 버리고 마지막
확정 상태를 저장/export한다. 이미 수락한 Commit은 FIFO 순서대로 완료한다.
Save As는 bridge뿐 아니라 actor에서 처리 중인 변형 요청도 검사한다.

새 dependency나 프로젝트 wire extension은 없다. 확정 결과는 기존 structural pixel commit으로
저장할 수 있다. 실제 사용 gate와 남은 검증은 `docs/sprints/essentials-goal.md`에서 관리한다.

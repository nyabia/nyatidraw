# ADR-0052 — 보기 좌표와 임시 색 채취

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 범위

W1b의 목적은 그림을 그리던 도구를 잃지 않고 색을 가져오고, 그림을 훼손하지 않고 방향을
바꿔 보는 것이다. 보정, 픽셀 변형, 선택 편집 확장은 이 결정에 포함하지 않는다. 새 dependency는 없다.

## 보기 반전과 회전

`ViewportTransform`이 문서 X축 반전 → 회전 → 확대/이동의 정방향과 역방향을 함께 제공한다.
`mirrored_horizontal`은 UI projection과 실제 presented-frame 입력 snapshot 양쪽에 전달되며,
반전만 바뀌어도 입력 viewport revision을 갱신한다. 미표시 frame의 좌표로 입력을 열지 않는다.

반전 토글·45도 회전·회전 초기화는 현재 화면 중앙의 문서 지점을 고정한다. 초기화는 각도만
0도로 하고 확대율/반전은 유지한다. 툴바 확대도 중앙 기준, 휠 확대는 포인터 기준이다.
화면 맞춤과 1:1은 페이지를 다시 가운데 놓고 회전과 반전을 해제한다.

GPU가 사용하는 inverse affine, 선택 overlay, signed tile 표시, navigator 영역과 펜/마우스
역변환은 같은 함수를 따른다. WGSL에 별도 반전 수식을 중복 추가하지 않는다.
이 상태는 문서 픽셀이나 히스토리를 바꾸지 않으며 PNG에 적용되지 않는다.

## 임시 스포이트

Windows native Begin에서 Alt 상태를 읽어 이미 있는 bounded Begin 메타데이터와 함께 전달한다.
UI의 선택 도구를 Eyedropper로 바꿨다 되돌리는 비동기 명령 두 개로 구현하지 않는다.
시작 후 Alt 해제는 해당 제스처를 그리기로 바꾸지 않으며, 다음 Begin은 새 키 상태를 읽는다.
Space/가운데 버튼 팬은 우선한다. 키보드 Alt 메뉴나 텍스트 입력 전역을 가로채지 않는다.

임시는 문서 합성의 표시 색을 읽는다. Solo 상태를 캡처한 `PickDisplayColor`를 사용하며,
체크무늬·선택 표시·커서는 제외한다. 투명 픽셀은 현재 색을 유지한다. 기존 고정 I 도구의
active/reference/all-visible 선택과 fill 참조 원본은 바꾸지 않는다.

읽기와 다음 페인트 사이에는 입력 승인 경계를 둔다. 결과 반영 이전에 시작하려는 새 제스처는
승인하지 않고 다음 clean Begin을 요구한다. 승인된 스트로크를 지워서 색 적용 순서를 맞추면 안 된다.
포커스/캡처 상실은 진행 중 입력을 취소하며 실패·취소 뒤 임시 도구나 읽기 대기가 남으면 안 된다.
선택 mask·레이어·히스토리는 색 채취로 수정하지 않는다.

End 승인과 같은 producer mutex 구간에서 읽기 장벽을 세우고 `(device_id, End sequence)`로
해제 소유권을 확인한다. recorder별 sequence만으로 장치를 혼동하지 않는다. 이전 취소는
새 읽기 장벽을 풀지 않는다. discontinuity acknowledgment는 이전 임시 의도도 같은 mutex에서
제거하되 다른 읽기의 장벽을 무조건 해제하지 않는다.

이전 픽셀 확정/export로 busy인 읽기는 한 슬롯에서 재시도하며, 최초 point/Solo를 유지한다.
완료 전 명시적 색 변경도 직렬화한다. 종료할 때 아직 enqueue하지 않은 임시 읽기는 버릴 수
있지만 완료된 픽셀 편집은 저장한다. 임시 읽기 실패가 작품 저장 실패로 바뀌면 안 된다.

## Solo 표시 규칙 공유

GPU와 임시 CPU 색 채취가 `LayerTree::display_child_visible`을 공유한다. Solo 대상까지의
조상 경로는 표시하고, 그룹 Solo 내부는 모든 깊이에서 각 자식의 원래 가시성을 따른다.
기존 구현이 Solo 그룹 내부의 중첩 그룹 픽셀을 누락하던 것도 이 범위에서 수정한다.
가시성 원본이나 export 합성을 임시로 변경하는 방식은 사용하지 않는다.

## 검증

좌표 왕복·반전 두 번·중앙/포인터 anchor·viewport revision, 임시 Begin/End/Cancel와 승인 경계,
Solo 색 채취와 원본 불변식을 핵심 검증 대상으로 한다. 기존 GPU probe에는 반전/회전된 독립
사분면, signed 좌표, 중첩 Solo를 포함한다. 결과는 [W1 기록](../sprints/workflow-w1.md)에 남긴다.
실제 펜 감각·UI 조작·표시 지연은 별도 acceptance이며 합성 입력/픽셀 probe로 대체하지 않는다.

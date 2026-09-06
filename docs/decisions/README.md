# 아키텍처 결정 기록

| 상태 | 결정 | 근거 |
|---|---|---|
| 확정 | UI shell과 drawing/document core를 분리 | Dioxus Native 위험을 제품 전체 위험으로 전파하지 않기 위해 |
| 확정 | stroke 중 GPU-first working tiles | input-to-visible latency를 최우선으로 하기 위해 |
| 확정 | 닫힌 stroke는 immutable snapshot으로 materialize | 저장, undo, crash recovery, headless export를 위해 |
| 확정 | Save와 export는 별도 failure domain | export 실패가 프로젝트 보존을 취소하지 않게 하기 위해 |
| 확정 | semantic stroke와 exact tile result를 함께 기록 | timelapse와 미래 brush version 변화 모두 견디기 위해 |
| 확정 | Vello는 vector renderer adapter | raster brush, document schema, history의 소유자가 아니므로 |
| 확정 | [Dioxus Desktop UI + native WGPU child canvas](ADR-0008-dioxus-desktop-child-canvas.md) | Native/Blitz 불안정성을 제거하면서 raw input과 GPU hot path를 WebView 밖에 유지 |
| 조건부 | [redb를 native project backend로 채택](ADR-0002-project-store.md) | durable stroke/DAG reopen, invalid 보존, scratch process-kill probe 통과; 대용량·power-loss recovery 대기 |
| 조건부 | [persistent GPU texture로 첫 잉크 표시](ADR-0003-gpu-live-stroke.md) | release GPU 제출과 layer/viewport readback 통과; 실제 input-to-visible/present 대기 |
| 조건부 | [닫힌 round stroke를 CPU replay로 durable materialize](ADR-0005-stroke-materialization.md) | deterministic bytes/root, live End→writer, redb reopen 통과; GPU strategy 비교·latency benchmark 대기 |
| 확정 | [history branch와 root cursor를 보존](ADR-0006-branch-preserving-history.md) | 모든 child 보존·명시 redo 선택·4K pointer-only 측정 통과 |
| 확정 | [validated layer/dock trees와 coordinate-scoped composite invalidation](ADR-0007-editor-layer-and-layout-model.md) | exact invalidation, versioned durable LayerTree, typed dispatcher와 dock-remount authority 수용성 통과 |
| 확정 | [종료 중 창 유지와 PNG 복구](ADR-0009-close-and-export-recovery.md) | bounded admission 종료, 비동기 drain/join, 실패 시 durable reopen과 Save retry |
| 확정 | [snapshot별 레이어 상태](ADR-0010-snapshot-layer-history.md) | tree/pixel 원자적 commit과 삭제·metadata Undo/reopen |
| 확정 | [래스터 Reference metadata](ADR-0011-reference-layer-metadata.md) | 구버전 호환성, history 복원과 일반 PNG 불변 |
| 조건부 | [기본 선택·채우기 CPU 계약](ADR-0012-basic-selection-and-paint.md) | 독립 기대 픽셀·history·프로세스 reopen·PNG 통과; desktop 비동기 도구 연결 대기 |
| 검증 대기 | 128×128 tile | 64/128/256 latency·memory·metadata benchmark 필요 |
| 검증 대기 | GPU→CPU materialization 방식 | readback, CPU replay, hybrid 비교 필요 |

결정이 바뀌면 이 표만 덮어쓰지 않는다. `ADR-xxxx-title.md`를 추가해 상황, 선택지, 결정, 결과, 되돌림 조건을 기록한다.

## 2026-09-05~06 후속 결정

아래 기록은 앞 표의 당시 연결 대기 상태를 보완한다. 전체 Sprint 완료 판정은
[현재 gate 목록](../status-plan-2026-09-06.md)을 따른다.

| 범위 | 결정과 근거 |
|---|---|
| 선택·편집의 native 경로 | [비동기 작업](ADR-0013-asynchronous-selection-edit.md), [선택 stroke 저장](ADR-0014-selected-stroke-replay.md), [GPU clipping](ADR-0015-gpu-selected-painting.md), [native 도구](ADR-0016-native-selection-tools.md), [gesture 표시](ADR-0017-edit-gesture-guides.md) |
| History/layer 비동기 처리 | [ADR-0018](ADR-0018-asynchronous-history-and-layers.md) |
| 도킹 | [설정 복원](ADR-0019-workspace-layout-persistence.md), [toolbar/stack](ADR-0020-dockable-toolbar-entries.md), [mouse capture/cancel](ADR-0021-dock-pointer-capture.md) |
| 색·기본 편집 | [HSV](ADR-0022-direct-color-picker.md), [정수 raster 변형](ADR-0023-basic-raster-transforms.md), [page 저장](ADR-0024-page-history-storage.md), [page resize/crop](ADR-0025-page-resize-crop.md) |
| 성능 계측과 UI 대기 | [고정 histogram](ADR-0026-bounded-desktop-timing.md), [paced workload](ADR-0027-paced-desktop-performance.md), [WM_PAINT wake](ADR-0028-windows-paint-wakeup.md) |
| PNG 색상과 Godot | [sRGB 경계](ADR-0029-srgb-boundaries.md), [실제 Godot 재수입](ADR-0030-godot-png-acceptance.md) |
| History 복원 비용 | [GPU 타일 유지](ADR-0031-retain-unchanged-history-tiles.md), [CPU 버퍼 재사용](ADR-0032-reuse-history-cpu-buffers.md), [worker 접수→복원 frame](ADR-0034-history-adoption-frame-timing.md), [root 읽기 중복 제거](ADR-0036-deduplicate-root-object-loads.md) |
| Windows 셸 | [Open With 소유권 보존](ADR-0033-open-with-progids.md), [초기 포커스 실패](ADR-0035-webview-startup-focus.md) |

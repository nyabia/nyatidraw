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
| 검증 대기 | 128×128 tile | 64/128/256 latency·memory·metadata benchmark 필요 |
| 검증 대기 | GPU→CPU materialization 방식 | readback, CPU replay, hybrid 비교 필요 |

결정이 바뀌면 이 표만 덮어쓰지 않는다. `ADR-xxxx-title.md`를 추가해 상황, 선택지, 결정, 결과, 되돌림 조건을 기록한다.

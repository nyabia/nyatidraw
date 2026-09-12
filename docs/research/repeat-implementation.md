# X/Y 반복의 최소 구현 경계

상태: **설계만 완료, 기능 미구현**. 기준은 [UX-3](../sprints/ux-feedback.md#ux-3-반복-반전과-다름)이다.
표시 토글만 연결한 상태를 완료로 인정하지 않는다. 이 문서는 현재 소스의 읽기 전용 조사 결과이며
Cargo, GUI, 새 실행 파일, 저장·재시작 시험을 수행한 결과가 아니다.

## 요구 의미

- `repeat_x`, `repeat_y`는 독립된 프로젝트 속성이다. 세션 보기 반전이나 사용자 환경 설정이 아니다.
- 출력 페이지를 켜진 축으로 반복 표시한다. 복사본은 읽기 전용이며 입력을 원본으로 wrap하지 않는다.
- 켜진 축의 원본 페이지 범위 밖 기존 픽셀은 가리기만 한다. 끄면 원래 위치의 그림이 다시 보인다.
- 꺼진 축의 무한 작업공간은 유지한다. PNG는 계속 원본 페이지 한 장이다.

페이지가 `[0,W) × [0,H)`일 때 직접 편집 가능한 영역은
`(!repeat_x || 0 <= x < W) && (!repeat_y || 0 <= y < H)`다.
예를 들어 X만 켜면 페이지 안의 Y 구간에는 가로 복사본을 표시하고, 원본 X 구간의 위아래
무한 작업공간은 유지한다. 그 밖의 원본 sparse 픽셀을 복사본 위에 덮어 그리지 않는다.
표시 좌표의 음수 반복은 Euclidean modulo를 사용하되 편집 좌표 자체에는 modulo를 적용하지 않는다.

## 실제 저장 backend 확인

현재 checkout의 production 저장 backend는 **redb**다. crate 이름만으로 판단한 결과가 아니다.

- `crates/project-redb/Cargo.toml`: 일반 의존성 `redb = "=4.2.0"`; `rusqlite = "=0.40.2"`는 dev-dependency다.
- `crates/project-redb/src/lib.rs`: `redb::Database`, `TableDefinition`과 실제 `META`, `STATE` 테이블을 사용한다.
- `apps/desktop/src/native_canvas.rs`: `nyatidraw_project_redb::ProjectDb`를 열고 저장 worker가 소유한다.
- SQLite 연결은 `crates/project-redb/examples/storage_compare/backend.rs::Store::Sqlite` 비교 실험에 있다.
  이것을 production SQLite 마이그레이션 완료로 해석하지 않는다.

저장 형식 설계는 `ProjectStore` 경계에 두어 향후 backend 변경과 분리한다.

## 심볼별 변경 지도

| 경계 | 현재 근거 | 최소로 필요한 변경 |
|---|---|---|
| 프로젝트 모델·명령 | `crates/api/src/lib.rs::CanvasSpec`, `crates/api/src/protocol.rs::{ProjectCommand, UiProjection, ViewportProjection}` | 순수 데이터 `ProjectWorkspaceSettings`의 독립 두 축, 설정 명령과 authoritative projection. `ViewportProjection`의 세션 mirror에 섞지 않는다. |
| 저장·호환 | `crates/project/src/lib.rs::{ProjectStore, SCHEMA_CAPABILITY_FLAGS}`, `crates/project-redb/src/lib.rs::{META, STATE, ProjectDb::has_valid_marker, ProjectDb::load_canvas_spec}` | 설정 load/persist, 엄격한 버전·값 검증, 원자적 기록, 구 writer 차단이 필요한 새 semantic capability. 새 flag 값은 병행 브러시 작업과 조율한다. |
| page history | `crates/project-redb/src/canvas_history.rs::{RECORD_BYTES, decode_canvas_record, encode_canvas, capture_snapshot_canvas}` | `CanvasSpec`는 그대로 유지한다. 현재 page payload는 정확히 12바이트이며 snapshot별로 검증하므로 필드 추가는 불필요한 기존 형식·history 이관을 일으킨다. |
| actor·worker | `apps/desktop/src/native_canvas.rs::{ActiveCanvas::apply_editor_command, ensure_artwork_command_idle, WorkerRequest, materialization_worker, apply_history_outcome}` | 입력·변형·저장 중 토글 정책, worker 저장 응답 뒤 authoritative 설정 반영, 재열기 로드. 페이지 resize/Undo 시 반복 period와 다음 획의 clip 크기를 갱신한다. |
| GPU 표시 | `crates/paint-gpu/src/compositor.rs::{GpuCompositeScene::render_viewport, visible_tile_coordinates, visible_sparse_coordinates, viewport_uniform_values}`, `viewport.wgsl`, `workspace_tiles.wgsl` | finite page를 반복 샘플링하고 켜진 축 밖 sparse 원본을 가린다. 원본 페이지가 화면 밖이어도 복사본의 source page cache가 갱신돼야 한다. |
| native hit-test·획 | `apps/desktop/src/native_canvas.rs::{StrokePipeline::drain, LiveStroke, GpuStrokeOp, ClosedStrokeRequest, install_selection}`, `apps/desktop/src/edit_gesture.rs` | 복사본에서 시작한 편집은 End/Cancel까지 정상 격리한다. accepted Begin에 페이지 크기와 두 축의 analytic 쓰기 clip을 고정한다. selection과 교집합으로 적용한다. |
| GPU dab 전체 범위 | `crates/paint-gpu/src/round_dab.wgsl::shade_dab`, `crates/paint-gpu/src/selection.rs::SelectionBinding` | dab 중심이 아니라 각 document pixel에서 analytic clip을 검사한다. 일반 칠·지우개·alpha lock에 동일 적용하고 live GPU 상태만의 제약으로 끝내지 않는다. |
| durable stroke·CPU 재생 | `crates/stroke/src/lib.rs::{StrokeCommit::seal_with_options, CpuReplayMaterializer::materialize}`, `crates/project/src/wire.rs::{encode_stroke_commit, decode_stroke_commit}`, `crates/editor/src/lib.rs` | Begin에 고정한 clip을 commit identity와 wire에 포함한다. CPU replay도 범위 밖 원래 픽셀을 보존한다. 이후 토글·page resize에 따라 과거 획을 다르게 재생하지 않는다. |
| 다른 픽셀 편집 | `apps/desktop/src/edit_worker.rs::{execute_with_clipboard, selection_domain}`, `apps/desktop/src/transform_worker.rs::execute_with_clipboard` | fill/gradient/delete/cut와 기존 signed selection의 유효 쓰기 영역을 제한한다. transform/paste는 원본 제거와 목적지 쓰기의 제약을 함께 검증한다. |
| picker·export | `crates/paint-cpu/src/selection.rs::sample_display_pixel`, `crates/paint-cpu/src/lib.rs::flatten_layer_tree_rgba8`, `apps/desktop/src/save_as.rs::copy_closed_project` | 표시용 picker/확대 bubble은 복사본을 원본 페이지 좌표로 읽을 수 있게 별도 매핑한다. 편집용 raw/reference source에는 무조건 같은 매핑을 적용하지 않는다. PNG flatten은 변경하지 않는다. |

## 저장 최소안

기존의 범용 프로젝트 설정 레코드는 없다. `CanvasSpec`를 늘리는 대신 `META`에
`workspace_settings_version`와 `repeat_axes`의 두 값, 또는 `STATE`의 작은 versioned envelope를 둔다.
축 값은 0~3만 허용한다. 둘 다 없는 구 프로젝트는 false/false로 읽고, 일부만 있거나 버전·값이
잘못된 파일은 거부한다. 열기만으로 invalid 파일을 초기화하거나 수정하지 않는다.

최소 제안은 반복을 artwork Undo 밖의 프로젝트 설정으로 저장하는 것이다. 토글은 worker의
단일 durable transaction으로 기록하고 성공 응답 뒤 반영한다. 이는 사용자별 palette/layout 설정과
다르다. 토글 자체도 Undo 대상이어야 한다면 snapshot별 설정 기록·cursor 복원을 추가해야 하므로
그 의미는 구현 착수 때 확정한다. 이 제안이 이미 결정·구현된 것으로 표시하지 않는다.

새 clip stroke를 기록한 프로젝트는 해당 의미를 모르는 writer가 열지 못하도록 capability를
유지해야 한다. 반복을 나중에 껐다고 과거 constrained stroke의 capability를 제거하지 않는다.
기존 selection 확장은 남은 바이트 전체를 coverage로 소비하므로 새 clip marker를 단순히 그 뒤에
붙일 수 없다. 현재 alpha-lock 확장처럼 selection 앞의 명시 tagged extension 또는 새 버전을
설계하고, legacy no-clip stroke의 bytes/hash와 구 프로젝트 재생을 그대로 보존한다.

`copy_closed_project`는 닫힌 DB를 byte-for-byte 복사하므로 새 설정도 그대로 보존된다.
단, pending 설정 저장까지 drain한 뒤 writer를 닫아야 한다. 복사본 검증이 새 metadata 검증을
실행해야 하며, PNG 경로는 `CanvasSpec` 크기의 finite flatten을 계속 사용한다.

## 가장 작은 올바른 쓰기 제한

순수 core 데이터 `StrokeWriteClip`에 축별 optional `[min,max)` 경계를 둔다. 반복을 켠 축만
`[0,W)` 또는 `[0,H)`로 고정하고 다른 축은 제한하지 않는다. 선택 coverage, alpha lock과 독립된
제약이며 결과 쓰기 조건은 이들의 교집합이다. immutable 값 하나를 native Begin → GPU → closed
request → sealed stroke → CPU replay에 전달한다. 현재 설정을 replay 시점에 다시 읽지 않는다.

기존 selection mask를 거대한 사각형으로 대체하는 방법은 적합하지 않다. 현재 GPU selection은
유한 dense coverage와 16MP 상한이 있어 한 축의 무한 strip을 표현하지 못한다. 종료 시 획 bounds로
유한 mask를 만들어 wire를 재사용하는 방안도 긴 획·넓은 offpage 작업에서 상한에 걸린다.
중앙점 필터만으로는 페이지 가장자리에 걸친 큰 dab가 보호할 원본 픽셀을 덮어쓴다.

복사본에서 시작한 Begin을 무시하고 이후 Move를 새 획으로 취급해서도 안 된다. 한 접촉은
End/Cancel까지 격리하고, 원본에서 시작한 획은 이동 중 경계를 넘어도 동일 clip을 유지한다.
selection/gesture overlay 역시 숨긴 원본을 복사본 위에 보여 주지 않도록 같은 표시 영역을 따른다.

fill·gradient·부분 삭제는 유효 영역 밖 원본 픽셀을 보존한다. transform은 목적지만 잘라 source를
지우면 그림을 잃을 수 있다. 최소 안전안은 source 또는 destination footprint가 보호 구간과
충돌하는 preview/commit을 명시 거부하고 원본과 마지막 유효 draft를 유지하는 것이다.
향후 경계에서 잘리는 transform을 허용하려면 별도의 사용자 의미와 보존 규칙이 필요하다.
레이어 전체 삭제처럼 명시적으로 전체 원본을 대상으로 하는 명령과 복사본 직접 편집 차단은
구별한다. 모든 버튼을 무조건 막거나 기존 레이어 의미를 암묵적으로 바꾸지 않는다.

## GPU 표시의 함정

현재 viewport 경로는 보이는 원본 finite page tile만 재합성한 뒤 sparse pass를 별도로 그린다.
shader에서 좌표에 modulo만 추가하면 원본이 화면 밖인 경우 오래된 page cache를 복사할 수 있고,
sparse 원본이 복사본 위에 나타날 수도 있다. 반복 활성 시 필요한 source tile 집합을 계산해야 한다.
최초 최소 구현은 기존 page 상한 내 전체 source page의 dirty tile을 합성하는 방식도 가능하지만,
현재 sparse/visible-only 경로보다 비용이 커질 수 있으므로 측정 전 성능 완료로 주장하지 않는다.

## 독립 스프린트로 묶는 이유와 gate

작업량의 중심은 토글 두 개가 아니라 **signed 무한 작업공간 + GPU/CPU 일치 + durable replay**다.
현재 자체 브러시 작업은 brush/paint CPU·GPU/stroke wire를, picker 작업은 native/live ink/gesture를
수정하고 있어 동시에 clip 계약을 바꾸면 동일 commit format과 Begin 경계를 충돌시키기 쉽다.
이 변경을 제외한 표시 전용 구현을 출하하지 않고, 해당 작업 통합 뒤 아래 순서의 독립 스프린트로
다룬다. 이것은 미구현 항목을 남기는 기술적 분할이며 사용자 요청 완료나 범위 삭제가 아니다.

1. project 설정·clip 계약·legacy wire 기본값과 schema gate를 먼저 고정한다.
2. Begin 고정, GPU/CPU footprint 제한, 다른 pixel edit의 안전 거부/제한을 연결한다.
3. 반복 shader·sparse 가림·source cache·hit-test·display picker를 연결한 뒤 토글 UI를 노출한다.
4. 아래 핵심 불변식과 별도 프로세스 reopen을 통과하고 실제 창 acceptance를 확인한다.

필수 자동 검증은 UI 테스트가 아니라 작품 보존 위험을 막는 작은 table-driven 사례로 제한한다.

- 두 축 4조합, 음수·경계 ±1 픽셀과 큰 dab: enabled 축 밖 원본 hash 보존, disabled 축의 offpage
  쓰기 허용, GPU/CPU 동일 픽셀. 지우개·selection·alpha lock 교집합 포함.
- 복사본 Begin 및 경계 왕복: phantom stroke 없음, Begin/End/Cancel 보존, 토글이 active stroke에
  소급 적용되지 않음. 중간 실패 시 partial GPU stroke의 rollback/clean Begin 회복.
- 반복 해제 시 숨겼던 원본 그대로 복구. transform/cut/fill의 경계 실패가 원본·history를 바꾸지 않음.
- legacy stroke bytes/hash 유지, malformed metadata/clip 거부와 원본 파일 보존, 새 capability 보호.
- Save → 프로세스 종료 → 별도 프로세스 reopen: 두 축·픽셀 root·Undo/Redo·원본 PNG 동일.
  page resize/Undo 및 Save As 뒤에도 과거 stroke clip은 그대로이고 새 stroke period만 갱신됨.

## 러프 파랑 색상화의 별도 경계

현재 `crates/document/src/layers.rs::{LayerNode, GroupNode}`에는 tint/color effect가 없다.
`reference`는 fill/wand/picker source membership이며 일반 합성 색을 바꾸지 않는다.
`crates/paint-cpu/src/compositing.rs::{properties, group_pixel, group_tile}`와
`crates/paint-gpu/src/{compositor.rs,stack_composite.wgsl}`의 합성 설정은 opacity/blend/clip이다.

최소 비파괴 색상화는 원본 tile을 보존하는 raster `LayerColorEffect::None/Monochrome` 같은
별도 metadata와 CPU/GPU 동일 premultiplied 색 변환이다. 단순 alpha 유지 파랑 치환과
명도 유지 파랑화는 결과가 다르므로 변환 의미를 먼저 정한다. layer tree wire는 현재 v3이므로
구 버전은 None으로 읽고 새 effect는 명시 version/capability로 보호한다. layer history·Undo·export·
navigator·display picker를 연결하고 raw raster thumbnail/source의 기존 의미는 의도적으로 유지한다.
opacity·reference·alpha lock을 색상화 대신 완료 처리하지 않는다. 반복보다 범위는 작지만 기존
paint 및 layer wire 병행 변경과 통합 순서를 조율해야 하며 현재 구현된 효과는 아니다.

# 전체 아키텍처

## 설계 원칙

핫 패스와 영속 경로를 분리한다. 핫 패스는 펜 샘플을 같은 프레임의 픽셀로 바꾸며, 영속 경로는 닫힌 stroke를 재현·저장·export 가능한 객체로 바꾼다. 둘 사이의 계약은 `StrokeCommit`과 immutable content root다.

```text
┌────────────── main / render thread ──────────────┐
│ winit events → raw input adapter                 │
│ Dioxus panels     │                              │
│                   ├── UiCommand ─────┐           │
│ GPU compositor ◀──┼── RenderDelta    │           │
│ surface present   │                  │           │
└───────────────────┼──────────────────┼───────────┘
                    ▼                  ▼
            bounded sample ring   editor mailbox
                    │                  │
┌───────────────────▼──────────────────▼───────────┐
│ paint/editor thread                              │
│ normalize → resample → dynamics → dab batches   │
│ document reducer · active stroke · history head │
└──────────┬───────────────────────────┬───────────┘
           │ GPU paint packet          │ StrokeCommit
           ▼                           ▼
    GPU working tiles            materializer pool
           │                           │
           └── present                 ├── CPU tile objects
                                       ├── compression/hash
                                       └── CommitBatch
                                                │
                                      project writer / exporter
```

현재 durable 절단에서 materializer는 `CpuReplay`만 구현한다. 준비 단계는 editor
root를 변경하지 않고 immutable `ProjectCommitBatch`를 만들며, redb durable
transaction 성공 뒤에만 editor가 그 batch를 accept한다. Windows canvas의 closed
stroke는 bounded materialization backlog를 거쳐 이 경로로 전달된다. GPU
readback/hybrid checkpoint는 아직 `UnsupportedStrategy`이며, GPU 결과를 durable
권위로 삼지 않는다.

## 런타임 상태

### Document model

저장되는 의미 상태다. 캔버스, 레이어 트리, content root, 색상 규약, 선택 영역, history head를 가진다. GPU handle과 UI signal은 절대 포함하지 않는다.

초기 `LayerTree`는 implicit document-root group 아래에 raster layer와
nested group을 bottom-to-top 순서로 둔다. 모든 tagged node ID는 tree 전체에서
유일하며 root의 visibility/opacity는 고정된다. visibility, opacity, reorder는
실패 조건을 mutation 전에 검증하고, group을 자기 descendant로 옮기는 cycle을
거부한다. Raster node만 immutable `ContentRootId`를 소유한다.

`CanvasSpec`은 유한한 page의 권위 있는 경계다. 문서 좌표의 page는
`[0, width) × [0, height)`이고 checkerboard와 page 판정은 이 범위 안에서만
적용한다. signed sparse tile plane은 page 바깥 artwork도 보존·표시하며,
후속 transform으로 다시 page 안에 들어올 수 있게 durable 상태로 유지한다.
thumbnail, navigator, export는 항상 유한 page로 crop한다. viewport의 dark
workspace와 black outside border는 presentation chrome이며 artwork가 아니다.

### Active stroke

아직 commit되지 않은 입력과 GPU working tile 집합이다. stroke begin에서 영향을 받은 원본 tile root를 잡고, dab batch마다 작업 타일을 갱신한다. 프로세스가 비정상 종료되면 마지막 닫힌 stroke까지 복구하는 것이 기본 보장이다.

### Render state

wgpu device, queue, surface, tile atlas, composite cache, pipelines와 overlay를 소유한다. 언제든 document snapshot에서 재생성할 수 있어야 한다.

### Editor session

viewport, active tool/layer, dock tree, cursor, 진행 중 transform처럼 프로젝트와 분리 가능한 상태다. 사용자 설정과 프로젝트 작업공간 저장은 후속 정책으로 분리한다.

`DockTree`는 backend-neutral split/tab/panel tree다. Sprint 3 panel 집합은 canvas,
layers, brush, color, history로 고정하며 duplicate/missing panel, invalid active tab,
극단 split, 과도한 depth를 거부한다. decode 실패나 invalid tree는 일부 상태를
살리지 않고 한 번에 complete safe default layout으로 교체한다.

### UI projection

Dioxus가 구독하는 작은 불변 모델이다. tile pixels, pressure samples, GPU handles를 포함하지 않는다. projection 갱신 빈도는 화면에 의미 있는 변화로 제한한다. Navigator와 raster thumbnail PNG data URI는 이 projection 밖의 bounded desktop mailbox에만 있고, durable CPU page crop 뒤에만 갱신된다.

`UiProjection` revision은 semantic publish마다 checked increment한다. Desktop
control은 실행 시점의 authoritative mailbox revision으로 bounded
`CommandEnvelope`를 만들고, renderer가 `EditorEvent`와 새 projection을 함께
publish한다. 오래된 projection은 최신 UI/mailbox를 덮을 수 없다. Root-owned
`use_wgpu` source는 DockTree remount를 넘어 유지되고 resume은 mailbox의 viewport,
active layer, revision을 복원한다. `api` crate는 `input`에 의존하지 않으며 command
type에는 `StylusSample`을 표현할 field/variant가 없다. Native pen hook과 bounded
sample queue는 계속 Dioxus state를 우회한다.

## 실시간 잉크 경로

1. 플랫폼 backend가 timestamp, 위치, pressure, tilt, twist, buttons, eraser를 `StylusSample`로 정규화한다.
2. 고정 용량 ring buffer가 transition을 보존하고 move sample만 기하학적으로 coalesce한다.
3. paint thread가 viewport revision을 검증하고 document 좌표로 확정한다.
4. stabilizer와 arc-length resampler가 일정한 dab 간격을 만든다.
5. brush evaluator가 입력 curve와 seed를 적용해 `DabBatch`를 생성한다.
6. render thread가 해당 GPU working tile에 compute/render pass를 적용하고 즉시 composite한다.
7. stroke end에서 샘플 stream과 before roots를 `StrokeCommit`으로 봉인한다.
8. materializer가 after tile을 CPU/storage 객체로 만들고 history root와 저장 가능 revision을 전진시킨다.

GPU readback이 병목이면 Sprint 1에서 세 구현을 비교한다.

- 직접 readback 후 압축
- GPU 결과와 동일 규약의 CPU replay
- 긴 stroke 중간 checkpoint + 끝부분만 readback/replay

측정 없이 하나를 영구 선택하지 않는다.

현재 strategy API는 세 선택을 모두 이름으로 보존하지만 CPU replay만 성공한다.
GPU readback과 hybrid checkpoint는 구현 전까지 `UnsupportedStrategy`이며 CPU
경로로 조용히 대체하지 않는다.

## 브러시 경계

```rust
pub trait BrushEvaluator {
    fn begin(&mut self, preset: &BrushPreset, first: StylusSample) -> StrokeToken;
    fn push(&mut self, token: &mut StrokeToken, samples: &[StylusSample], out: &mut DabBatch);
    fn end(&mut self, token: StrokeToken, out: &mut DabBatch) -> RecordedStroke;
}

pub trait RasterPaintBackend {
    fn begin_working_set(&mut self, base: ContentRootId, bounds: TileBounds) -> WorkingSetId;
    fn apply(&mut self, working: WorkingSetId, dabs: &DabBatch) -> DirtyTileSet;
    fn seal(&mut self, working: WorkingSetId) -> GpuStrokeResult;
}
```

`BrushPreset`은 직렬화 가능하고 불변이다. `StrokeToken`은 stabilizer, traveled distance, random state, smudge state를 가진다. opacity, flow, spacing, density를 별도 축으로 유지한다. 첫 버전은 round tip, pressure-size/opacity, build-up만 구현한다.

## 타일과 합성

- sparse signed tile coordinates를 사용한다.
- signed sparse layer plane의 바깥 좌표는 page crop 때문에 삭제하지 않는다.
- 시작 후보는 128×128이지만 64/128/256을 벤치마크하고 파일 헤더에 tile edge를 기록한다.
- GPU에는 atlas 또는 texture array를 사용하며 tile당 texture를 만들지 않는다.
- dirty rect와 연속된 dab을 frame당 tile 하나의 작업으로 합친다.
- group composite cache는 `(GroupId, mip, tile x, tile y)`로 주소화한다. Raster
  tile 변경은 같은 좌표의 parent→root cache entry만 정확히 무효화한다. 다른
  좌표와 sibling group은 유지한다. visibility/opacity/reorder처럼 모든 좌표의
  결과를 바꾸는 structural edit는 old/new ancestor group만 whole-group
  invalidation하며 cache owner가 현재 보유한 좌표만 열거한다.
- zoom-out mip는 lazy 생성한다.
- GPU device loss 시 닫힌 commit의 CPU/storage tile로 복구하고, 진행 중 stroke는 recorded samples로 재생을 시도한다.

## 스레드와 backpressure

| 실행 위치 | 소유권 | 금지 |
|---|---|---|
| main/render | winit, Dioxus, GPU, present | 압축, fsync, PNG encode |
| paint/editor | active stroke, reducer, history head | DOM 접근, DB transaction |
| materializer pool | replay/readback 후처리, hash, compression | document head 변경 |
| project writer | repository transaction | mutable document 참조 |
| export coordinator | snapshot별 최신 job | live working tiles 직접 참조 |

모든 queue는 bounded다. Primary와 transition retry lane의 목표 용량 안에서는
`Begin`/`End`/`Cancel`을 보존하고 move sample만 끝점, 압력 극값, 곡률이 큰 점을
보존하면서 합친다. 두 lane을 모두 소진하면 첫 손실 sequence/phase를 한 번 latch하고
producer를 격리한다. Consumer는 앞선 샘플을 drain한 뒤 partial GPU/evaluator stroke를
transactional Cancel하며 materialize하지 않고, 다음 clean `Begin`에서만 재개한다.
overflow, discontinuity/ack, quarantined samples, coalesced count, paint backlog는
telemetry로 남긴다.

## crate 구조

```text
nyatidraw/
├── apps/
│   ├── desktop/          # Dioxus Desktop UI + child-HWND WGPU/input host
│   └── cli/              # validate/PPM export/diagnostic smoke
├── crates/
│   ├── api/              # command, event, projection contracts
│   ├── document/         # canvas, layers, color, vector-neutral model
│   ├── input/            # StylusSample and platform-neutral processing
│   ├── input-platform/   # Win32/AppKit/Wayland adapters
│   ├── brush/            # preset, dynamics, resampler, recorded stroke
│   ├── tiles/            # keys, roots, COW/storage representation
│   ├── paint-gpu/        # working tiles and paint pipelines
│   ├── paint-cpu/        # reference backend and headless fallback
│   ├── stroke/           # seal contract and materialization strategies
│   ├── history/          # DAG, roots, timelapse records
│   ├── project/          # repository trait, wire schema, migrations
│   ├── project-redb/     # candidate native repository backend
│   ├── editor/           # reducer, lifecycle, orchestration
│   └── diagnostics/      # tracing and latency probes
└── docs/
```

## 의존성 규칙

`document`, `input`, `brush`, `tiles`, `history`는 Dioxus/wgpu/Vello/redb에
의존하지 않는다. `stroke`도 platform/GPU/storage 타입을 받지 않고 CPU
materializer 계약만 소유한다. `ui-dioxus`는 `api`와 projection만 본다. 저장
writer는 immutable `ProjectCommitBatch`만 받는다. native 타입은
`input-platform` 밖으로 새지 않는다.

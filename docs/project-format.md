# 프로젝트, 저장, export

## 목표 사용자 규약

```text
portrait.ntdr      NyatiDraw editable project
portrait.png       colocated game-ready export target
```

- 존재하지 않거나 정확히 0바이트인 project path만 새 프로젝트로 초기화한다.
- 0바이트가 아닌 invalid 파일은 오류로 열고 절대 자동 덮어쓰지 않는다.
- `.ntdr`은 다른 Nyatif 제품과 충돌하지 않는 NyatiDraw 전용 확장자다. 확장자를
  신뢰 경계로 사용하지 않고 내부 magic/version/checksum을 항상 검증한다.
- Closed-stroke durable commit과 image export는 별도 failure domain이다. desktop
  `Save`는 colocated PNG를 writer FIFO에 요청하고, `Idle`/`Queued`/`Running`/
  `Current`/`Failed` 상태를 UI에 별도 mailbox로 보인다. PNG 실패는 이미 durable인
  project 상태를 되돌리지 않는다.

CLI 산출물은 explicit PPM이며, desktop은 colocated alpha PNG를 별도 worker로 저장한다.
PNG activation/Save/restart와 export 실패 복구의 실행 근거는 구현 현황 문서에 기록한다.

현재 native backend는 `redb 2.6.3`을 사용한다. 이 선택은 headless recovery
slice에는 유효하지만 production format 안정화 선언은 아니다.

## 논리 객체 모델

저장소 구현과 무관하게 다음 키 공간을 가진다.

```text
meta                 schema version marker + versioned finite canvas metadata
state                current snapshot pointer + current history cursor envelope
                     + immutable initial-root cursor envelope
objects/<hash>       immutable tile blobs
roots/<hash>         canonical signed-tile manifests
strokes/<hash>       sealed semantic stroke records
snapshots/<id>       before/after roots, history, optional stroke reference
history/<id>         operation nodes
layer-tree           versioned raster/group hierarchy record
snapshot_layers/<id> immutable hierarchy at a history cursor (project marker 2)
```

2026-09-05 저장 경계에 `commit_structural_with_layer_tree`를 추가했다. 첫 명시적
metadata-history commit이 project marker를 1에서 2로 바꾸며, pixel/history/cursor와
레이어 트리를 같은 transaction에 기록한다. 이후 stroke도 현재 트리를 snapshot에
보존하고 Undo/Redo는 cursor와 current tree를 함께 복원한다. 기존 snapshot들은
한 번 고정한 legacy tree를 참조한다. 과거에 저장하지 않은 레이어 상태를 복원한
것은 아니다. 단순 open은 전환하지 않는다. 구 marker-1 writer는 marker 2를 거부한다.
자세한 형식·검증·미연결 desktop 범위는 [ADR-0010](decisions/ADR-0010-snapshot-layer-history.md)을 따른다.

### 유한 page와 signed sparse layer plane

`meta`의 `CanvasSpec`이 유한 page 크기와 해상도를 정의한다. page 유효 범위는
`[0, width) × [0, height)`이며 checkerboard, thumbnail/navigator, PPM을 포함한
모든 export는 이 범위로만 crop한다. 레이어 content root와 tile manifest는
signed 좌표를 사용하므로 page 바깥 artwork도 durable 데이터로 남긴다. 바깥
타일은 버리지 않으며, 후속 layer transform으로 page 안에 돌아오면 다시 보인다.

viewport 바깥은 dark workspace와 black page border로 표시한다. 이는 저장
픽셀이나 export 대상이 아닌 presentation chrome이다. finite crop/persistence와
viewport chrome과 signed GPU workspace가 구현되어 있다. page 안쪽의 기존
full-texture fast path는 유지하고, page 밖의 visible signed tile만 bounded
texture-array atlas에서 합성한다.

Rust struct의 메모리 배치를 그대로 저장하지 않는다. 모든 immutable record에는
`NYREC001` magic, object type, schema version 1, codec 0(raw), uncompressed
length, tagged BLAKE3 checksum과 payload가 있는 명시적 envelope를 둔다.
`state/current_snapshot`은 같은 redb transaction 안에서 마지막에 쓰는 16-byte
little-endian pointer다.

Sprint 3에서 structural head를 위해 snapshot의 stroke reference가 optional이 됐다.
Encoder는 option tag가 있는 새 형식을 쓰며, decoder는 원래의 fixed 32-byte
stroke-only tail도 strict length로 식별해 기존 개발 프로젝트를 계속 연다.

## `StrokeCommit`

```rust
pub struct StrokeCommit {
    pub id: StrokeCommitId,
    pub parent_snapshot: SnapshotId,
    pub layer: LayerId,
    pub before_root: ContentRoot,
    pub affected_tiles: TileBounds,
    pub brush: BrushSnapshot,
    pub recorded: RecordedStroke,
    pub color: StrokeColor, // RGBA8 premultiplied linear-light
    samples: Arc<[StylusSample]>,
}
```

seal은 Begin → Move* → End, 단조 증가 sequence/timestamp, 같은 device,
brush preset/version/seed/sample metadata를 검증한다. 한 stroke는 최대 262,144
samples와 65,536 affected tiles로 제한된다. decoder는 record의 count를 믿고
할당하지 않고 남은 payload의 최소 encoded size와 같은 sample 상한을 먼저
검증한다.

현재 round brush는 `CpuReplay` materializer가 signed 128×128 tile별 RGBA8
premultiplied linear-light source-over를 실행한다. 투명 tile은 canonical root에서
생략하고 tile bytes 및 정렬된 `(TileKey, tile hash)` manifest를 tagged BLAKE3로
hash한다. `GpuReadback`과 `HybridCheckpoint`는 비교 가능한 strategy enum에는
있지만 명시적으로 `UnsupportedStrategy`를 반환한다. exact after tile state가
undo/reopen의 권위이며 semantic stroke는 결정론 replay와 향후 timelapse용이다.

## Durable commit seam

```text
closed stroke at revision N
  → wait for revision N materialization
  → build immutable CommitBatch
  → repository transaction + durability policy
  → publish SnapshotId N
```

writer는 mutable `Document`를 읽지 않는다. transaction 성공 전에는 UI의 durable revision이 전진하지 않는다.

현재 headless seam은 `HeadlessStrokeSession::prepare_round_stroke`가 상태를
바꾸지 않고 `ProjectCommitBatch`를 만들고, `ProjectDb::commit` 성공 뒤에만
`accept_committed`로 live root를 전진시킨다. redb writer는 before/after roots와
objects, stroke, history node, snapshot head, current pointer를 하나의
`Durability::Immediate` transaction으로 기록한다. reopen은 envelope/checksum,
object hash, canonical root, stroke reseal, CPU replay 결과의 exact after bytes를
current snapshot에 대해 다시 검증한다. 또한 `history` table은 최대 100,000
node까지 전부 읽어 node-key/record ID, parent link, root transition, cycle 및
current cursor/root를 검증하고 ordered child index를 재구성한다. 따라서 current
ancestor 밖의 redo branch도 storage-neutral `ReopenedProject`에 남는다.

새 closed-stroke commit은 transaction을 열기 전에 durable cursor와의 lineage를
검증한다. non-empty project에서는 `StrokeCommit.parent_snapshot`, batch before root,
그리고 `HistoryNode.parent`가 모두 current cursor와 같아야 한다. initial cursor에
있는 경우에만 parent `None`의 새 root child가 가능하다. 따라서 fresh session으로
만든 second root node는 object/history record를 쓰기 전에 거부된다.

Undo나 explicit redo branch selection은 artwork commit이 아니다. editor는 target
`ProjectHistoryCursor`와 immutable tile root를 먼저 준비하고 hydrate한다. backend는
target snapshot/head/root가 정확히 일치하는지 검증한 뒤 `state/current_snapshot`과
enveloped `state/current_history_cursor`만 하나의 `Durability::Immediate`
transaction으로 바꾼다. 그 commit이 성공한 뒤에만 editor가 cursor와 tiles를
교체하므로 storage failure는 live cursor를 전진시키지 않는다.

첫 commit은 `state/initial_history_cursor`에 그 stroke의 `parent_snapshot`과 exact
before root를 `InitialHistoryCursor` envelope으로 함께 기록한다. 이 record는
`HistoryNode`를 만들지 않는다. first node를 undo할 때 current snapshot/cursor를
제거하고 이 initial cursor를 선택하며, reopen은 head `None`과 그 canonical root를
복원한다. redo는 first node의 normal snapshot cursor를 다시 state에 publish한다.

비-stroke artwork 변경은 `ProjectStructuralBatch`로 같은 lineage를 연장한다.
`AddWhiteBackground`는 현재 root의 non-background tile을 byte-for-byte 보존하고
1024×768 Background layer에 정확히 48개의 128×128 opaque-white tile을 병합한다.
`OperationRecord::StructuralChange`, optional-stroke snapshot head, history node,
root objects와 current cursor는 하나의 `Durability::Immediate` transaction으로
commit되고 성공 뒤에만 editor/GPU가 새 root를 accept한다. Stroke와 structural
head가 섞인 history도 reopen/cursor 검증을 통과해야 한다.

현재 Windows desktop seam은 `NAYATI_PROJECT_PATH=<path>`를 explicit project
선택으로 사용한다. 경로가 없으면 한 `SharedGpuCanvas` 수명 동안 고정된
`Untitled (Recovery)` redb path를 만들고 dock remount/child-canvas lifecycle마다 같은
repository를 재사용한다. 복구 파일은 프로세스 종료 때 삭제하지 않는다. 경로가
있으면 `ProjectDb::open` 정책을 그대로 적용하며 invalid non-empty file에서 다른
project로 fallback하지 않는다. reopen한
`ReopenedProject`는 background writer의 `HeadlessStrokeSession::from_reopened`로
이관되고 current CPU tile 전체는 첫 viewport render 전에 GPU scene에 upload된다.

## 현재 export와 후속 pipeline

CLI의 explicit PPM(P6)과 desktop의 colocated alpha PNG는 별도 entry point다.
desktop PNG는 writer FIFO에서 durable project 작업 뒤에 실행한다. Save마다
monotonic generation을 부여한다. N, N+1이 연속 요청되면 worker가 시작하지
않은 N은 skip하고, N이 이미 encode 중이면 완료할 수 있으나 final target을
교체하기 직전 generation gate를 다시 검사한다. 새 Save의 admission과 그
검사는 같은 gate로 직렬화되므로 오래된 N은 N+1이 accepted된 뒤 target을
교체할 수 없다. 실패는 export status만 `Failed`로 만들며 project durability나
dirty 상태를 되돌리지 않는다.

```text
portrait.png.~tmp-<job>
  → encode complete
  → flush/close
  → latest-generation check
  → atomic replacement where supported
  → UI Current(generation)
```

Durable `ExportReceipt(snapshot, hash, timestamp)`는 이후 재시작에서도 export 최신 여부를
판별할 때 추가한다. 현재 receipt는 project file에 기록하지 않는다.

운영체제별 atomic replace와 directory durability 차이는 platform adapter와 crash test matrix로 검증한다.

## 종료 state machine

```text
Running
 → Quiescing
 → FinishActiveStroke
 → MaterializeLatest
 → CommitLatestSnapshot
 → EnsureRequiredExports
 → CloseRepository
 → Exit
```

창을 얼린 채 blocking join하지 않는다. event loop는 종료 진행과 오류 선택지를 계속 렌더한다. export가 이미 최신이면 추가 encode를 시작하지 않는다.

현재 desktop vertical slice에는 bounded writer queue와 generation-safe PNG export,
실제 worker 완료/실패 상태가 있다. 종료 진행 UI는 아직 없으며 window teardown은 이미
닫혀 writer/backlog에 들어간 stroke와 export를 처리하고 repository를 닫기 위해 마지막에
worker를 join한다. End 전 active stroke는 직전 durable snapshot까지만 보장한다.

## 복구 보장과 미검증 영역

- 정상 종료: 현재 구현은 worker에 들어간 최신 닫힌 stroke까지만 대상으로 한다.
  required export 보장은 automatic export coordinator가 구현된 뒤의 목표다.
- 저장 중 강제 종료: 마지막 durable snapshot은 항상 열려야 한다.
- active stroke 중 강제 종료: 직전 durable snapshot까지 보장한다.
- export 중 강제 종료: 프로젝트는 안전하고, 임시 export는 다음 시작 시 정리 또는 재개 후보로 식별한다.
- invalid checksum/object reference: 손상 범위와 마지막 유효 snapshot을 보고하며 자동 overwrite하지 않는다.

현재 구현은 손상된 current head 또는 bounded history DAG를 `Corrupt`로 거부하고
원본 파일을 보존한다. 마지막 유효 이전 head 탐색, migration, compaction, 그리고
실제 강제종료 crash matrix의 full-DAG branch coverage는 아직 구현되지 않았다.

## 후속 Git 친화 기능

초기 파일은 바이너리다. 현재 CLI는 `validate`, PPM `export`, owned
`diagnostic-smoke`만 제공한다. deterministic `inspect`, `unpack`, `pack`은 후속
과제다. 일반 durable commit에서 compaction이나 history GC를 실행하지 않는다.

# 공유 래스터 애니메이션: 첫 구현 설계

2026-10-28 코드 감사에 따른 구현 전 제안이다. 사용자 지시로 래스터 애니메이션은
필수 범위에 들어왔으며 [일러스트·게임 공통 방향](game-and-illustration.md)과 W0–W5를
함께 유지한다. 실제 구현·설치판 완료 선언이 아니다. 경쟁 앱의 전체 기능을 복제하지 않는다.

첫 범위는 **레이어가 들어 있는 셀(layered cel) 한 트랙**이다. CSP는 래스터나 레이어
폴더를 cel로 사용한다. Aseprite는 layer×frame의 cel grid를 제공한다. 첫 NyatiDraw는
전자에 가까우며 Aseprite의 여러 layer 독립 노출·linked cel parity를 주장하지 않는다.
[CSP 공식 cel](https://help.clip-studio.com/en-us/manual_en/600_animation/Animation_folders_and_cels.htm),
[Aseprite cel](https://www.aseprite.org/docs/cel/).

## 필요한 실제 작업

한 cel에 기존 선화·밑색·클리핑 그림자를 유지한 채 다음 frame을 독립 복제해 수정하고,
앞/뒤 그림을 참고하며 시간과 재생을 확인한다. 저장·종료·재열기 뒤 계속 수정하고,
게임용 grid PNG+frame rect/duration JSON을 내보낸다. 부드러운 붓/반투명도/페이지 밖
그림도 유지한다. indexed palette나 nearest sampling을 전체 문서에 강제하지 않는다.

빈/복제 frame, 이전/다음, 삭제·순서·시간 조절, playback, onion, grid sheet를 우선한다.
레이어별 독립 노출 track, shared backgrounds, linked edit, tag 구간, 오디오, 카메라,
bone/mesh tween, GIF/video, trim/packing은 다음 범위다. Aseprite가 제공하는
[frame 작업](https://www.aseprite.org/docs/animation/),
[duration](https://www.aseprite.org/docs/frame-duration/),
[onion](https://www.aseprite.org/docs/onion-skinning/),
[sheet](https://www.aseprite.org/docs/sprite-sheet/)에서 필요한 기본 경로만 가져온다.

## 원본과 표시를 분리하는 도메인

```text
Authoritative document
  canvas: CanvasSpec                         전체 frame 공통
  layers: LayerTree                          모든 cel의 그룹/래스터, 전역 고유 ID
  tiles: TileSnapshot                        모든 cel 타일, 하나의 content root
  timeline: Option<AnimationTimeline>
    frames: ordered Frame { id: FrameId, cel: CelId, duration_ms: u32 }
    cels: CelId -> Cel { root_group: GroupId }

Editor session
  editing_frame: FrameId
  active_layer: LayerId                      현재 cel에 속해야 함
  playback_position / onion_settings         표시 전용, Undo/dirty 밖

Immutable render projection
  source snapshot/revision + FrameId + CelId
  active_tree: LayerTree                     현재 cel만 포함한 disposable tree
  active_rasters: bounded set<LayerId>        업로드/표시 타일 허용 집합
```

`crates/document/src/animation.rs`에 이 관계와 유효성·frame resolver를 둔다.
FrameId/CelId는 순번이 아닌 안정적인 식별자다. GroupId/LayerId는 전체 문서에서 고유하며
각 cel은 다른 cel과 겹치지 않는 subtree 하나를 소유한다. cel container는
visible/Normal/opacity100%로 고정하고 바깥 cel과 clipping 관계를 금지한다. 내부 레이어의
가시성·blend·clipping·alpha lock·color effect는 그대로 보존한다.

`timeline=None`인 기존 정지 그림은 읽기만으로 다시 저장하지 않는다. 첫 animation
명령에서 기존 root 자식을 새 cel group에 넣고 frame 1을 만든다. 기존 획의 raster ID는
그대로 보존하며 새 cel/group ID만 발급한다. 이 전환도 한 번의 구조 Undo다.

독립 복제는 새 CelId·group/raster ID를 만들고 기존 tile object를 새 TileKey에서 공유한다.
`LayerPixelChange::DuplicateMany`와 같은 copy-on-write 경로를 재사용한다. 원본 cel을
수정하지 않는다. 같은 CelId를 여러 frame에 노출하는 hold/reuse는 명시적 linked 동작으로
나중에 도입한다. 독립 복제와 linked copy를 같은 버튼으로 몰래 혼합하지 않는다.

**프레임 전환으로 영속 layer visibility를 바꾸지 않는다.** 원본 tree/root는 모든 cel을
계속 포함한다. 해석된 active tree는 표시·picker·참조 fill·thumbnail·export에만 쓴다.
편집은 현재 cel의 ID를 검사하고 master tree에 적용한다. subtree를 대상으로 준비한
레이어 변경은 전체 tree에 splice한 후 검증·commit한다. 타일 생존 여부는 항상 master
tree로 판정한다. onion은 resolver 입력을 바꾸는 것이 아니라 별도 display overlay다.

모든 명령은 live stroke/입력 queue/변형·보정 draft/pending edit 상태를 확인한다.
frame 전환은 유휴 상태에서만 허용한다. Begin 때 cel/target/generation을 고정하고,
늦은 sample/preview가 다른 cel에 들어오면 기존 fail-closed 정책으로 거절한다.
전환 시 선택/Solo를 해제하고 새 cel 안의 active layer를 선택한다. 재생 중에는 편집을
거절하거나 먼저 명시적으로 정지시키며 playback tick을 artwork command로 보내지 않는다.

## 반드시 먼저 분리할 현재 코드 경계

| 현재 코드 | 위험 | 필요한 변경 |
|---|---|---|
| `apps/desktop/src/native_canvas.rs`의 `apply_layer_command`, adoption, UI projection이 `scene.tree()`를 권위로 사용 | scene에 active subtree만 넣으면 다음 편집에서 inactive cel이 사라짐 | actor/worker 원본 `authoritative_tree`와 `render_projection`을 별도 필드로 소유; 모든 편집/저장/ID 발급은 원본 사용 |
| native `WorkerRequest::CommitLayerTree` 처리의 `removed = old preview_tree - new tree` | render projection을 new tree로 전달하면 inactive frame 타일을 삭제 | `preview_tree`는 전체 원본 유지. 표시 tree를 commit request 타입으로 넘기지 않도록 API 경계 명시 |
| `web-core/src/editing.rs`의 next tree에 없는 tile filter | active tree만 Artwork에 넣으면 inactive cel 삭제 | `Artwork.layers`와 `Artwork.tiles`는 전체. 표시 accessor를 별도로 추가하고 삭제는 전체 tree 기준 |
| `web-core/src/interchange.rs::from_snapshot`의 모든 tile layer 존재 검사 | active tree+전체 tiles를 그대로 전달하면 거절 또는 검증 우회 유혹 | 전체 metadata+전체 tiles+timeline을 검증한 뒤 active projection 생성 |
| `apps/web/src/gpu.rs`의 new는 `doc.layers()`, sync는 `display_layers()`, upload는 `display_snapshot()` 전체 | 첫 생성 또는 재시작에서 모든 cel이 GPU에 올라감 | 생성/재생성/adoption/upload 모두 같은 active projection의 tree 및 허용 tile iterator 사용 |
| `web-core/src/transform.rs`의 display snapshot/tree는 현재 transform preview에도 사용 | frame projection이 transform 결과를 덮거나 반대로 다른 frame draft를 표시 | projection 순서 고정: 전체 문서 → 현재 cel → 현재 cel에 귀속된 draft → view/onion |

기존 `TileKey{LayerId,mip,x,y}`·stroke replay·tile hash ABI는 첫 단계에서 바꾸지 않는다.
전체 cel 타일이 한 authoritative root에 있으므로 기존 history root 일치 검사와 GC가
모든 cel을 함께 보호한다. 별도 cel root forest 도입은 history/GC를 재설계하는 후속 작업이다.

## GPU와 메모리 상한

현재 `crates/paint-gpu/src/compositor.rs::new`, `replace_tree`,
`replacement_atlas_slots_for`는 visibility와 무관하게 모든 raster/group을 수집하고
각각 페이지 크기의 surface를 만든다. scene 예산은 512MiB, sparse atlas는 최대 2048 slots다.
따라서 all-cel tree를 그대로 넣고 visibility만 끄는 방법은 그림이 작아도 frame 수에 따라
full-page GPU 할당이 늘어난다. 그 방법은 허용하지 않는다.

- `GpuCompositeScene`에는 **현재 frame의 active_tree만** 전달한다. 업로드도
  active_rasters에 포함된 tile만 순회한다. 전체 CPU 원본에서 scene에 없는 layer를
  upload하고 예외를 무시하는 방식은 금지한다.
- frame 전환은 같은 scene의 active tree/surface를 교체한다. 이전 frame의 독립 scene을
  하나씩 cache하지 않는다. tree replacement를 먼저 preflight하고 실패하면 현재 표시와
  editing_frame을 유지한다. 업로드 실패 뒤에도 원본은 보존하고 renderer 복구를 알린다.
- onion은 최대 이전/다음 하나의 **최종 합성**만 별도 제한된 overlay texture로 가진다.
  각 이웃의 전체 layer/group scene을 영구 보관하지 않는다. 작업용 scratch를 순차 재사용하거나
  제한된 CPU 합성을 사용한다. 이 비용은 기존 512MiB 예산에 추가로 계산하며 자동 무제한 할당 금지.
- playback cache key는 `document generation + source snapshot + cel/frame + display settings`다.
  COW pixel 변경·visibility/blend·canvas 변경 때 관련 합성을 무효화한다. stale worker 결과를
  다른 프로젝트/프레임에 채택하지 않는다. 재생 정지/백그라운드에서는 tick을 중단한다.
- shared web 현재 한도는 edge4096, tile key1024, 전체 recursive node32, NTDR128MiB다.
  cel당 node 상한과 문서 전체 node 상한을 구분하고, 전체 tile key/고유 object bytes/cache
  bytes를 각각 preflight한다. 4K 여러 frame을 기존 한도로 받을 수 있다고 약속하지 않는다.
  한도를 올리려면 release의 실제 사용량/상호작용을 측정하고 실패 시 기존 문서를 보존한다.

성능 수용 예: 같은 cel 내용으로 frame 수를 1→8→32 늘려도 active GPU page surface 수는
현재 cel의 raster/group 수에만 비례해야 한다. onion on/off 사용량을 별도로 기록한다.
재생 frame deadlines와 p50/p95/p99 decode/composite/upload 구간을 기록하며
present API를 실제 첫 가시 픽셀이라고 부르지 않는다.

## 원자적 지속 형식

`crates/project/src`의 bounded versioned timeline codec와 새 `RecordKind`/capability flag를
사용한다. flag는 코드의 기존 비트와 대조한 뒤 고정한다. 구버전 writer는 capability를
모르면 read-only preflight에서 거절해야 한다. Undo로 static 위치로 가더라도 유지된
이력에 animation이 있으면 flag를 제거하지 않는다.

`snapshot_animation: SnapshotId -> checksummed timeline record`와
`current_animation`을 둔다. timeline 없는 legacy/static snapshot도 명시적으로 표현한다.
frame count/duration/total duration overflow, ID 중복, 없는 cel/group, 겹치는 cel 소유권,
예상하지 못한 trailing bytes, 과대 metadata를 거절한다. 각 snapshot은 그 snapshot의
layer tree와 함께 검증한다. animation flag가 있는데 record가 빠진 경우 static으로
fallback하지 않는다.

1. `project-redb/src/animation_history.rs`에 layer/canvas history와 같은 capture/load/validate
   경로를 만든다. `lib.rs::commit_structural_inner`와 **ordinary stroke `commit_inner` 둘 다**
   매 SnapshotId에 timeline을 같은 immediate transaction으로 capture한다. override가
   없으면 현재 timeline을 상속한다. 시간 변경으로 tile root가 같아도 새 SnapshotId는 필요하다.
2. 새 `commit_structural_with_document_metadata(batch, tree, canvas, timeline)`에서
   pixels/root/history/tree/canvas/timeline/capability가 함께 게시된다. 타일 commit 뒤 별도의
   `persist_animation`을 호출하는 two-phase 업데이트는 금지한다.
3. `persist_history_cursor`는 current tree/page와 current timeline을 같은 transaction에서
   복원한다. load/reopen은 현재 cursor의 immutable timeline을 권위로 삼는다.
4. `history_retention.rs`의 pruning 테이블 목록에 timeline을 추가한다. 기존 128-operation
   cutoff에서 새 initial snapshot_id에 해당하는 timeline을 남긴다. `object_retention.rs`
   root indexing은 모든 cel 타일을 가진 master root를 계속 사용한다. 129+ 동작 이후
   inactive cel/삭제 Undo/redo branch의 재열기를 시험한다.
5. readonly `validate_existing`, migrate-copy/compact/CLI validate에도 새 capability와
   timeline 검사를 통과시킨다. nonempty invalid 원본은 그대로 보존한다.

## 웹과 복구

`crates/project-web`는 `MemoryProjectDb`로 native와 같은 `.ntdr`를 읽고 쓴다.
`NYWEB001/002` portable은 import-only이므로 새 웹 전용 작품 형식을 만들지 않는다.
`decode_ntdr`는 전체 tree/tiles/timeline을 복원하고 `encode_ntdr`의 changed 판정에
timeline을 포함한다. `WebDocument::Artwork`·difference·undo/redo에도 timeline이 들어가야
duration/order만 수정한 문서가 저장된다. frame navigation은 view-only로 둔다.

`project-web/src/recovery.rs`의 NYWORK05는 timeline이 없으므로 새 버전(NYWORK06)에
bounded timeline blob을 추가한다. 이전 버전은 static backing에만 기존 의미로 읽고,
animation backing/base에 old packet이 들어오면 새 timeline을 덮지 않도록 거절한다.
worker reconstruction도 전체 tree/tile delta/timeline을 한 checkpoint로 적용한다.
`apps/web-worker/{src/lib.rs,storage.js,recovery.js}`의 epoch/revision 검사를 유지한다.
tab restart, worker restart, 새 프로젝트로 바꾼 후 늦은 checkpoint를 각각 검증한다.

현재 web session의 전체 undo stack은 파일 encode 때 모두 이력으로 보존되는 구조가 아니다.
한 최종 structural state를 저장하는 현재 의미를 명시하며 애니메이션 도입만으로 native와
완전한 history parity를 주장하지 않는다. 최소 gate는 모든 cel 그림/순서/시간의 동일 재열기다.

## 수직 slice와 소유권

| 단계 | 산출과 종료 조건 | 담당 경계 |
|---|---|---|
| A0 계약 | 순수 domain, ID/commands/projection, codec, active resolver, static migration의 인터페이스 고정. 불변식 시험 | **통합/domain 한 명**: `api`, `document`, `project` wire/API. 다른 담당은 이 파일을 동시에 수정하지 않음 |
| A1 내구성 | create/duplicate/delete/reorder/time의 core command → atomic metadata → Undo/redo → 별도 프로세스 reopen. crash before/after commit 및 129+ retention | **storage 담당**: `project-redb/animation_history.rs`, `lib.rs`, retention, focused scratch probes |
| A2 편집 연결 | native+web timeline UI와 frame switch, 기존 pen/fill/transform/adjustment의 cel isolation, save/restart/re-edit | **native/UI 담당**: native actor, root-authority split, shared panel. **web 담당**: web-core master Artwork, project-web persistence/recovery, web adapter/GPU filtering |
| A3 표시 | monotonic duration playback, stop/loop, 이전/다음1 onion, bounded GPU surfaces, no dirty/history on tick, picker/PNG onion 제외 | renderer 담당, A2 projection 계약 이후. timeline display는 shared UI가 소유 |
| A4 산출 | frozen snapshot으로 grid PNG+JSON, 순서·시간·alpha·페이지 보존, generation 검증, 프로젝트와 분리된 실패 | export 담당, A1 frame resolver 이후 |

A0 합의 직후 storage와 web/runtime는 서로 다른 파일에서 병렬 진행 가능하다. native와
공유 UI는 한 사람이 연결해 `editor-ui/lib.rs` 충돌을 피한다. 통합/domain 담당만
`api/protocol.rs`, `project/wire.rs`를 조정한다. 모든 담당은 동일 scratch fixture의
2 cel×선화/밑색/클립명암, 서로 다른 duration, signed off-page 타일을 사용한다.
그룹 affine 작업과 충돌하는 `editor/transform.rs`는 native/UI 통합 소유로 고정한다.

## 시트 산출과 실제 완료 기준

[Aseprite sheet](https://www.aseprite.org/docs/sprite-sheet/)의 고정 grid부터 제공한다.
각 frame에 page-size cell을 쓰고 JSON에 안정 frame ID, 순서, rect, duration_ms를 기록한다.
기본 산출에서는 trim하지 않으므로 애니메이션의 pivot/배치가 흔들리지 않는다.
PNG frame 배열과 metadata를 같은 frozen snapshot에서 만들고 dimension×frame overflow를
미리 검사한다. frame 단위로 합성해 모든 frame RGBA 중복 보관을 피한다.

PNG/JSON 두 파일을 연속 rename하는 것은 하나의 atomic transaction이 아니다.
generation별 PNG를 먼저 완성하고 그 파일 이름/hash를 담은 JSON manifest를 마지막에
replace하는 경로가 안전하다. 기존 sibling temporary/generation 확인 규칙을 유지한다.
웹은 같은 generation/hash를 가진 두 download 또는 별도 bundle 흐름이 필요하며, 한 파일
저장을 두 파일 성공으로 보고하지 않는다. paired PNG는 명시 editing frame 또는 정지
그림의 출력이며 playback cursor가 움직였다는 이유로 자동 덮어쓰지 않는다.

최소 완료는 실제 앱에서 두 cel을 만들고 다르게 수정→시간 변경→재생→onion 확인→
Undo/redo→저장→프로세스 종료→재열기→수정→sheet export까지 연결한 것이다. native와
web `.ntdr` 왕복에서 모든 cel pixel/tree/duration이 같아야 한다. UI 조작 증거와
core/crash 시험을 분리하고 물리 펜·고주사율·실제 게임 import는 별도로 표시한다.


## 웹 구현 연결 (2026-10-28)

`web-core/src/animation.rs`는 master tree/tiles/timeline을 작품 이력에 보관하고,
`render_tree`는 편집 프레임만 투영한다. `playback_tree`와 `animation_display`는
재생 및 onion 전용이며 `export_rgba8`·picker·저장에는 들어가지 않는다.
`layer_panel_tree`는 cel 컨테이너를 숨기고 현재 transform preview의 내부 레이어를 보인다.
일반 layer/selection/transform/color adjustment는 `active_edit_tree`에서 실행한 뒤
`merge_active_edit_tree`로 master에 재결합한다. 전체 snapshot의 비활성 cel 타일을
보존하며 CropPageToSelection은 기존 전체 snapshot 이동으로 모든 cel을 같이 재배치한다.

`project-web`의 native encode changed 판정은 timeline을 포함한다. NYWORK06에는
bounded timeline과 선택 frame을 담고, animation backing/base에 NYWORK03–05를 적용하는
경로를 거절한다. 기존 portable encode는 animation을 명시적으로 거절한다.
웹은 기존 1024 전체 타일 한도를 유지하며 animation의 전체 노드는 4096,
셀 내부는 32개로 제한한다. 큰 네이티브 파일을 일부 셀만 잘라 열지 않는다.

`apps/web/src/runtime/animation.rs`는 monotonic clock과 rAF로 재생하며 프레임 경계에서만
GPU scene을 바꾼다. Play 전 모든 cel의 scene budget을 preflight하고 Stop은 편집 프레임으로
돌아온다. Onion은 최대 128 MiB의 proxy/scratch 예산과 GPU scene 예산을 모두 통과해야 한다.
드래그·획·보정 preview 동안 frame 전환을 거절한다. Onion 캐시는 획/preview 중 제거하고
commit/구조 변경 때 다시 만든다. 매 rAF에 이웃 프레임을 CPU 합성하지 않는다.

회귀 검증 위치: `web-core/src/animation_tests.rs` (cel 편집/표시/출력 격리, 자원 거절의
원자성, metadata Undo/redo), `project-web/src/animation_tests.rs` (동일 tile root의 시간 변경,
worker checkpoint, 별도 프로세스 재열기, old worker protocol 거절).
실제 브라우저 타임라인 UI/재시작 및 네이티브↔웹 교환 검증은 통합 단계에서 별도 기록한다.

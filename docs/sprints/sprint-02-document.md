# Sprint 2 — 게임 프로토타이핑 편집기

## 사용자 결과

한 장짜리 스케치판을 넘어 캐릭터, 배경, 주석과 reference를 레이어·그룹으로
정리한다. 선택·채우기·기본 변형을 사용하고,
여러 번 저장해도 Godot가 항상 최신 합성 PNG만 읽는다.

## 실행 순서

1. raster layer 추가·삭제·이름 변경, group 추가·접기, cross-parent reorder를
   실제 LayerTree와 연결한다. Raster/group 추가, 이름 변경, group 접기와
   cross-parent reorder는 현재 실제 command, GPU composite surface, UI projection,
   redb layer tree까지 연결됐다. 삭제와 drag reorder UI는 남아 있다.
2. visibility, opacity, active layer, session-only Solo, durable Reference metadata를
   완성한다. Solo는 현재 GPU 합성에만 적용되고 durable visibility와 PNG export
   tree를 바꾸지 않는다. Reference metadata는 남아 있다.
3. Wand/Lasso 선택, Fill/Gradient와 selection/layer 기본 이동·변형을 실제 도구
   command에 연결한다. tolerance와 Reference layer 참조 범위를 명시한다.
4. 출력 page 크기 변경과 crop을 제공하되 page 밖 signed artwork를 자동 삭제하지
   않는다.
5. 출력 페이지로 crop한 실제 layer thumbnail과 navigator composite/viewport box를
   제공한다.
6. branch-preserving history 목록과 cursor를 UI projection에 연결한다.
7. 반복 Save 중 오래된 export가 최신 PNG를 덮지 못하게 하고 export 실패를 project
   저장 실패와 분리해 표시한다.
8. 실제 Godot prototype 폴더에서 반복 PNG activation과 watcher 갱신을 확인한다.

## 완료 gate

- 20개 raster와 nested group을 가진 scratch project에서 edit/save/restart/reopen 후
  tree, active history cursor와 합성 결과가 같다.
- PNG import 후 layer/selection/fill/transform 결과가 저장·재실행·export에서 같다.
- navigator와 layer thumbnail에는 page 밖 artwork가 없고 main workspace에는 남는다.
- N export 중 N+1 Save를 수행해도 watcher가 부분 PNG나 오래된 최종 PNG를 보지 않는다.
- reference와 Solo가 일반 export 및 durable visibility 의미를 오염시키지 않는다.

## 이미 확보한 저장 기반

### 현재 상태 (2026-09-03)

**desktop durability/reopen/PNG export 절단과 Save generation gate, 실제 worker
완료/오류 UI가 구현·검증됐다.** Closed round stroke는 CPU replay로 immutable signed tile
root와 `ProjectCommitBatch`가 된다. `ProjectDb`는 objects, roots, stroke, history,
snapshot/current cursor를 하나의 `redb 2.6.3` `Durability::Immediate` transaction에
기록하고, 성공 뒤에만 session이 state를 accept한다.

Reopen은 envelope/checksum, object hash, canonical root, current snapshot replay,
그리고 최대 100,000-node history DAG의 key/parent/root transition/cycle/current
cursor를 검증한다. current path 밖의 redo child도 `ReopenedProject`에 복원한다.
Undo와 explicit redo 선택은 artwork를 다시 쓰지 않고 target root/tile을 준비한
뒤 durable cursor transaction 성공 후에만 live session을 바꾼다. 첫 stroke를
initial root로 undo한 뒤의 redo도 reopen을 넘어 보존한다.

`NAYATI_PROJECT_PATH`가 있으면 desktop은 그 path만 explicit project로 연다.
없는 path 또는 정확히 0-byte만 초기화하며 non-empty invalid/corrupt file은
다른 project로 fallback하거나 덮어쓰지 않는다. Path가 없으면 process-lifetime
`Untitled (Recovery)` redb를 만들고 custom-paint suspend/resume에서 같은 파일을
재사용한다. Reopened CPU tiles are uploaded before the first viewport render;
unsupported fixed-scene tiles fail startup rather than being silently skipped.

### 완료된 export evidence

CLI `validate`와 `export`는 validated current snapshot만 읽는다. CLI `export`는
명시된 mip-zero raster layer를 finite signed bounds로 flatten해 PPM(P6)으로 쓴다.
64 Mi-pixel/256 MiB raw limit을 넘으면 실패하고, alpha는 PNG처럼 보존하지 않으며
premultiplied RGB는 black background로 내보낸다. Multiple-layer ordering/blend를
임의로 합성하지 않는다.

Desktop Save는 writer thread의 immutable CPU snapshot과 layer tree를 finite page로
합성해 sibling temporary PNG를 기록·sync한 뒤 교체한다. Project commit과 PNG
실패는 별도 경로다. 연속 Save는 monotonic generation을 부여해 시작 전 구세대
작업을 건너뛰고, 인코딩 중 구세대도 최종 교체 직전 generation gate에서 폐기한다.
UI의 queued/running/current/failed는 worker의 실제 단계만 반영한다.

Navigator는 같은 durable CPU snapshot을 최대 160×160의 유한 page crop으로만
저주파 합성해 Dioxus 전용 단일-frame mailbox로 보낸다. 따라서 signed page 밖
artwork는 project에 남지만 navigator에는 나오지 않으며, raw input queue와
`UiProjection`에는 preview pixel이 들어가지 않는다. 현재 viewport overlay는
보기/resize 변화에 맞춰 표시된다. Preview를 클릭하거나 press하면 현재 zoom/rotation을
유지한 채 해당 page 좌표를 작업영역 중앙으로 옮긴다. drag 중에는 pointer id와
client-to-preview 원점을 유지해 루트 pointermove에서 8ms 간격으로 최신 좌표만
합쳐 보낸다. pointerup은 마지막 좌표를 flush하고 pointercancel은 상태를 해제한다.
Raster layer row도 같은 durable CPU
`TileSnapshot`에서 50×56 이내 page crop을 별도 latest-only desktop mailbox로 받는다.
빈 raster는 투명 checker로 보이며, group은 folder placeholder다. Bootstrap/reopen,
durable stroke(바뀐 raster만), history cursor move, 흰 배경 materialization 뒤에만
갱신한다. visibility/opacity/solo/rename/reorder는 raw raster pixels를 바꾸지 않으므로
thumbnail을 다시 만들지 않는다. Cache는 최대 128 raster frames와 frame당 32 KiB
data URI로 제한되며, cap 밖 row는 checker fallback이다. Navigator drag는 UI 명령
경로이며 native stylus queue와 분리된다.

`nyatidraw-cli diagnostic-smoke`는 owned temporary directory에서 non-empty real
project를 만들고 immediate commit → reopen/validate → export를 같은 public CLI
path로 실행한다. reopened root, pre-save flattened RGBA hash, expected PPM bytes가
exact equality여야 한다. Export는 sibling temporary file을 새로 만들며 existing
destination replacement를 거부한다.

### 기록된 durability evidence와 한계

- Scratch-only crash probe는 populated transaction의 before-commit 및
  after-durable-commit에서 spawned child PID만 종료한 뒤 prior/new durable root와
  flattened hash를 확인했다. 이는 해당 host/filesystem/redb 조합의 process-kill
  evidence이며 sudden power loss, drive cache, directory durability를 뜻하지 않는다.
- RTX 3080/Vulkan probe는 closed CPU RGBA8 snapshot을 새 WGPU device에 upload하고
  exact readback 0 diff를 확인했다. Full GPU compositor/device-loss recovery 또는
  multi-layer project recovery evidence는 아니다.
- Current desktop shutdown drains closed/materialization/export work and joins
  its writer at teardown. End 전 active stroke는 durable하지 않고, export retry와
  close-progress UI는 없다.
- Windows release desktop smoke는 24 paint + 8 erase stroke를 close-drain하고,
  process restart 뒤 동일 snapshot/root/tile을 복구했다. 같은 smoke에서 새 raster와
  group의 ID, 이름, parent/index를 redb에서 다시 열어 확인했다.
- 한 render wake에 함께 들어온 raw `End`는 GPU 종료와 materialization enqueue를 먼저
  수행한 뒤 Save export를 같은 writer FIFO에 넣는다. 닫힌 stroke 단위로만 dirty를
  게시하며 raw sample마다 Dioxus를 깨우지 않는다. 아직 `End`가 오지 않은 active stroke를
  Save가 강제로 닫지는 않는다.
- closed-stroke completion payload도 writer request와 같은 4-entry bounded lane이며,
  포화/단절은 조용히 tile을 잃지 않고 durable CPU authority를 남긴 채 workspace를
  fail-closed한다.

### 남은 engineering evidence

- 화면 복원/present를 포함한 undo/redo p95, large database/repair, migration,
  compaction, last-valid-head recovery.
- active stroke를 포함하는 Save 정책, export retry와 사용자에게
  보이는 close-progress/error 상태.

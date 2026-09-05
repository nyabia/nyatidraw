# 성능 계약

## 목표와 측정값을 분리한다

아래 수치는 초기 설계 목표이며 아직 검증 결과가 아니다. 모든 측정은 p50뿐 아니라 p95/p99, 하드웨어, OS, backend, 캔버스 크기, brush 크기, layer 수를 함께 기록한다.

| 사용자 경험 | 초기 목표 | 측정 지점 |
|---|---:|---|
| cold start 첫 창 | ≤ 350 ms | process entry → first present |
| warm start 첫 창 | ≤ 180 ms | process entry → first present |
| 펜 입력 → 첫 가시 픽셀 | p95 ≤ 12 ms | OS event timestamp → presenting frame |
| 연속 stroke cadence | 120 Hz display에서 frame miss < 1% | active stroke frame intervals |
| idle GPU/CPU | event-driven, 지속 animation 없음 | 30초 idle trace |
| undo | p95 ≤ 16 ms | command accepted → restored frame |
| 4K PNG export 중 드로잉 | p95 ink latency 악화 ≤ 2 ms | export on/off 비교 |
| 최신 상태 종료 | 보통 ≤ 500 ms, 긴 encode는 진행 표시 | close request → process exit |

120Hz 한 프레임은 8.33ms이므로 `p95 ≤ 12ms`는 “모든 샘플이 같은 프레임”이 아니라 OS event가 늦게 도착하는 현실을 포함한 사용자 관찰 목표다. 실험 장비에서 timestamp 신뢰도가 낮으면 high-speed camera 또는 photodiode 검증을 별도 수행한다.

## 프레임 예산

```text
event drain + normalize      0.4 ms
resample + brush evaluate   0.8 ms
GPU paint dispatch          1.5 ms
tile/group composite        2.0 ms
UI and overlays             0.8 ms
submit/present headroom      2.0 ms
reserve                     0.8 ms
total                       8.3 ms
```

이는 목표 분배다. 실제 profiler 결과에 따라 이동할 수 있지만 합계와 reserve는 유지한다.

## 반드시 계측할 timestamp

- raw OS event arrival 및 원본 event timestamp
- normalized sample enqueue/dequeue
- brush batch ready
- GPU command encode/submit
- surface present request와 완료 추정
- stroke seal, materialization queued/completed
- durable commit start/end
- export render/encode/replace start/end

trace에는 문서 내용이나 좌표 원본을 기본 저장하지 않는다. 장치 ID는 세션 내 익명 ID로 변환한다.

## 벤치마크 장면

1. 4K canvas, 단일 layer, 8px round brush, 240Hz input.
2. 8K canvas, 20 layers, 256px textured brush, 빠른 지그재그.
3. viewport 밖을 왕복하는 long stroke와 360° 회전 viewport.
4. 4K PNG background export 중 그리기.
5. GPU atlas thrash를 유발하는 빠른 pan 후 즉시 stroke.
6. 10GB synthetic project cold open과 latest-view hydration.
7. queue saturation fixture에서 begin/end/pressure extrema 보존 확인.

## 성능 회귀 규칙

- 각 benchmark 결과를 JSON으로 저장하고 CI baseline과 비교한다.
- p95 ink latency가 10% 또는 1ms 이상 나빠지면 실패한다.
- startup은 20ms 이상 악화되면 원인 annotation이 필요하다.
- 평균만 좋아지고 p99 hitch가 악화된 변경은 통과하지 않는다.
- debug build 숫자를 release 성능 주장에 사용하지 않는다.
- GPU vendor별 결과를 합쳐 평균내지 않는다.

## Input queue overload artifact

다음 명령은 UI나 GPU를 거치지 않는 queue-only 측정이다. 가상 240Hz input,
8K (7680×4320) document 좌표, 256px round-brush scenario와 60Hz consumer를
고정해 20분간 producer overload를 재현한다. 따라서 physical pen, GPU backend,
input-to-present latency의 증거가 아니며 그 gate를 닫지 않는다.

```powershell
cargo run --release -p nyatidraw-input-queue --example queue_stress -- target/input-queue-stress.json
```

artifact는 release profile, CPU/OS, queue-only backend 표기, fixture 조건,
`max_len`, coalesced move 수, transition rejection 수, 전체/실제 coalescing
push의 p50/p95/p99/max 비용, begin/end와 최신 endpoint 보존 판정을 담는다.
`pass: true`는 queue capacity와 해당 보존 invariant만 의미한다.

## 품질과 속도의 경계

sample coalescing은 끝점, phase transition, 압력 극값, 큰 방향 변화를 보존해야 한다. 품질 golden test가 깨지는 최적화는 성능 개선으로 인정하지 않는다. CPU reference backend와 GPU 결과는 허용 오차를 명시한 이미지 diff로 비교한다.

## 2026-09-01 live-path latency correction

초기 실제 펜 사용에서 debug 앱의 ink 추종 지연이 확인됐다. 코드 감사 결과,
Move 중에는 redb commit이나 export가 실행되지 않았다. export는 아직 desktop
live path에 연결되지 않았고, closed stroke만 별도 writer에서 CPU replay와
immediate redb commit을 수행한다.

수정 전에는 한 custom-paint drain에 들어온 각 Move가 독립 `GpuStrokeOp::Dabs`와
독립 WGPU paint submit을 만들 수 있었고, redraw wake가 winit의 OS redraw 합성을
거치지 않고 즉시 `RedrawRequested`를 직접 실행했다. 수정 후에는 같은 generation의
연속 dab을 한 GPU batch로 합치고 `Window::request_redraw`를 사용한다. writer queue가
잠깐 찬 경우에도 raw input/GPU preview를 계속 drain한다.

이것은 구조적 수정이지 latency 합격 증거가 아니다. release DX bundle에서 실제
mouse/pen acceptance 후 OS event-to-visible p50/p95/p99와 frame interval을 측정해야
목표를 통과한 것으로 기록한다. 짧은 stroke를 매우 빠르게 반복할 때 남는 hitch는
CPU replay와 redb immediate commit service time을 각각 분리 계측한다.

## 2026-09-05 basic selection and fill CPU measurement

Windows 11 Home 10.0.26200 / Intel Core Ultra 7 155H에서 release CPU backend를
사용했다. 투명 3840×2160 active layer 전체를 Wand로 선택하고 불투명 단색을
source-over로 채우는 연속 작업이다. 각 버전은 warmup 1회 뒤 20회 측정했다.

| 구현 | p50 | p95 | p99 |
|---|---:|---:|---:|
| 픽셀마다 후보 tile map 조회 | 439.207 ms | 468.531 ms | 473.043 ms |
| 선택된 tile별 순회 | 210.183 ms | 220.761 ms | 242.417 ms |

두 구현 모두 같은 독립 기대 픽셀, process restart, history branch 및 PNG
검증을 통과했다. 후보 tile budget도 실제 선택 tile을 기준으로 계산한다.
이 수치는 메모리에서 source flatten/mask/fill/tile root를 만드는 CPU 시간이며
redb commit, PNG encode, GPU upload, UI/입력/display 지연은 포함하지 않는다.
20회 표본으로 관측한 percentile이고 안정적인 장시간 p99나 Sprint 3 gate
통과를 주장하지 않는다. 다음 desktop 연결은 비동기 worker를 사용해야 한다.

실행: `cargo run --locked -p nyatidraw-desktop --example basic_edit_reopen_probe --release -- --measure`.
원본 로그는 `target/basic-edit-release-before-tile-loop.log`,
`target/basic-edit-release.log`이며 CPU/OS를 함께 기록한 JSON artifact는
`target/basic-edit-cpu-measure.json`이다. 현재 수치는 단일 host의 국소 비교이며
CI latency baseline으로 채택한 것은 아니다.

## 2026-09-05 installed desktop timing instrumentation

`NAYATI_PERFORMANCE=1`로 입력 묶음의 앱 admission→dequeue/present 요청, brush,
GPU encode/submit, composite, surface 대기, history GPU 채택, committed-history
projection, CPU replay/commit, navigator/thumbnail, export 단계를 분리한다.
스레드별 고정 histogram으로 정상 종료 때 count/min/max 및 p50/p95/p99 **상한**을
출력한다. 기본 실행은 비활성이다. 계측 정의와 한계는
[ADR-0026](decisions/ADR-0026-bounded-desktop-timing.md)에 기록했다.

설치 release에서 실제 UI를 통한 도구 선택·mouse 입력 주입·Undo·Save·Close와
별도 process reopen/독립 PNG 비교를 통과했다. 16×16 scratch의 입력 묶음 3개로
배선만 확인한 결과이며 성능 합격이나 4K export 간섭 결과로 사용하지 않는다.
현재 환경의 Windows/WMI는 120 Hz 설정을 보고하지만 실제 frame cadence나 첫
가시 픽셀을 입증하지 않는다. 사용자 영상 다운로드가 병행 중인 환경임도 기록했다.

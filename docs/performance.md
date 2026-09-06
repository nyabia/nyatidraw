# 성능 계약

## 2026-09-06 Undo 대기열부터 화면 제출까지

4K 작품의 실제 Undo/Redo 20회에서 worker queue 접수 → 복원 frame의 present API
반환까지 p50/p95/p99 상한은 **131.071/155.647/164.815ms**였다. 접수·변경 복원·해당
frame 제출은 모두 20회였다. 복원 함수 내부만의 p95는 같은 세션에서 30.719ms여서
이 구간만으로 사용자 대기를 판단할 수 없다. Undo 16ms 목표는 미통과다.
OS/UI 명령 전달 전 대기와 물리 가시 픽셀은 측정하지 않는다.

독립 WebView 프로필로 정상 실행된 한 세션의 조건부 결과다. 기본 프로필과 이후
재시작에서 WebView2 초기화 오류가 발생해 GUI 재시작 gate는 미완료로 남긴다.
Save/Close 후 별도 프로세스의 전체 artwork/tree/page/PNG 검증은 통과했다.
[전체 행·환경](measurements/history-present-4k-2026-09-06.json)과
[계측 경계·누락 횟수·실행 한계](decisions/ADR-0034-history-adoption-frame-timing.md)를
함께 읽는다.

## 2026-09-06 CPU 타일 버퍼 재사용

같은 4K fixture를 오늘 환경에서 전후 각각 Undo/Redo 20회 측정했다. History
adoption CPU p50/p95/p99 상한은 전체 CPU 복사 때 45.055/48.369/48.369ms,
버퍼 재사용과 GPU upload 임시 복사 제거 뒤 21.503/23.161/23.161ms였다.
새로 분리한 CPU snapshot 갱신 구간은 8.703/9.727/10.105ms였다.
원본 전체 artwork/PNG, Save/정상 종료/일반 재시작 검증을 통과했다.
이 수치는 command/worker 대기와 composite/present를 제외하며 Undo 전체 16ms
목표를 통과한 것이 아니다. 한 세션씩 20회이고 첫 복원도 포함했다.
환경과 모든 stage 원본 행은 [JSON](measurements/history-cpu-reuse-4k-2026-09-06.json),
변경·검증 범위는 [ADR-0032](decisions/ADR-0032-reuse-history-cpu-buffers.md)에 기록했다.

## 최신 국소 측정: 4K history 타일 재사용

동일 4K 작품의 UI Undo/Redo 각 20회에서 history adoption CPU 구간의 p50/p95/p99
상한은 전체 업로드 53.247/55.295/56.138ms, 동일 타일 재사용 후
45.055/48.408/48.408ms였다. 902개 중 846개를 유지하고 56개만 업로드했다.
이는 worker/command 대기 및 후속 composite/present를 제외한 한 구간이다.
Undo 전체 응답 목표 통과로 해석하지 않는다. Windows 11 10.0.26200 / Core Ultra 7
155H / Intel Arc DX12 / pinned-DX release, 전후 각 한 세션이며 첫 표본도 포함했다.
설치판 Save/정상 종료/재시작과 전체 artwork/PNG 비교가 통과했다.
[ADR-0031](decisions/ADR-0031-retain-unchanged-history-tiles.md)과
[원본 행·환경 JSON](measurements/history-upload-4k-2026-09-05.json)에 한계를 기록했다.

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

## 2026-09-05 installed 4K foreground / export comparison

Windows 11 Home 10.0.26200, Core Ultra 7 155H, Intel Arc/DX12 Mailbox,
Dioxus CLI 0.7.9 release, surface 2096×1458/scale 2에서 각 모드를 3번 실행했다.
창을 전면으로 가져와 확인한 뒤 시작 marker를 만들었다. 3840×2160 페이지의
잠긴 배경+잉크 2개 레이어, 64px round brush, 32개 직선 stroke에 각각 121개
sample을 명목 240Hz로 보냈다. 매 stroke의 durable history 채택을 기다린다.
export 모드는 각 stroke 시작 전 실제 4K PNG의 Running을 확인하고 입력을 보낸다.
상세 절차는 [ADR-0027](decisions/ADR-0027-paced-desktop-performance.md)에 있다.

아래는 입력 묶음의 앱 admission → present API 반환 시간이다. 각 percentile은
histogram **상한**, 단위는 ms이며 실행 간 percentile을 평균하지 않았다.
export 실행의 inactive 행도 보고하여 한 실행의 일부만 골라 비교하지 않는다.

| 실행 | export 구간 | 입력 묶음 수 | p50 상한 | p95 상한 | p99 상한 | 관측 max |
|---|---|---:|---:|---:|---:|---:|
| baseline 1 | inactive | 1483 | 21.503 | 31.743 | 34.815 | 45.950 |
| baseline 2 | inactive | 1319 | 22.527 | 34.815 | 40.959 | 99.998 |
| baseline 3 | inactive | 1520 | 21.503 | 31.743 | 34.815 | 40.535 |
| export 1 | inactive | 777 | 23.551 | 32.767 | 38.911 | 40.575 |
| export 1 | active | 504 | 24.575 | 34.815 | 38.911 | 47.098 |
| export 2 | inactive | 752 | 23.551 | 34.815 | 38.911 | 49.479 |
| export 2 | active | 490 | 24.575 | 34.815 | 40.959 | 41.693 |
| export 3 | inactive | 735 | 22.527 | 34.815 | 38.911 | 47.698 |
| export 3 | active | 509 | 29.695 | 34.815 | 43.007 | 46.731 |

기준 실행의 surface acquire p95 상한은 모두 15.871ms였고, GPU paint
encode/submit은 0.607~0.639ms였다. Export active의 surface acquire p95는
16.383~17.407ms였다. Committed-history projection publication p95는
14~61µs 범위였다. 다만 이는 WebView 전체 비용을 측정한 값이 아니다.
baseline 2의 drain/brush max 91.013ms도 남아 있어 surface만으로 모든 hitch를
설명하지 않는다. 해당 span은 stroke 종료/대기열 전달도 포함한다.

모든 실행에서 32×121개 sample의 sequence와 phase 종료, snapshot 33/history 33,
전체 타일과 PNG의 별도 프로세스 재생 일치 및 정상 writer join을 확인했다.
최종 프로젝트를 계측/입력 드라이버 없이 다시 실행해 복원 화면을 확인한 뒤
동일 검증도 재통과했다. 새 단위 테스트는 추가하지 않았다.

**성능 gate는 통과하지 않았다.** 일부 비교에서 p95 상한 차이가 3.072ms이며,
histogram 구간과 실행 변동까지 있어 export 악화 ≤2ms를 입증하지 못한다.
OS event/physical pen/첫 가시 픽셀은 측정하지 않았다. 입력 driver의 최대
schedule lateness는 실행별 0.848~4.852ms였다. 사용자 영상 다운로드와 다른
desktop 작업은 통제하지 않았고 warmup도 제외하지 않았다. 3회/모드 결과를
장시간 p99나 CI baseline으로 승격하지 않는다. 이 장면은 위의 8px 단일 레이어
표준 장면과도 다르다.

[전체 JSON 결과](measurements/desktop-4k-foreground-2026-09-05.json)에 hardware,
OS/backend/profile/설치 EXE hash/소스 commit/표본 수와 모든 단계의 분포를 보존했다.
원본은 `target/performance-foreground/{baseline,export}-{1,2,3}/`의
`out.log`, `err.log`, `verify.log`이며 최종 재실행은 `export-3/reopen-*.log`다.
입력 전에 전면 상태를 맞추지 못했던 `target/performance-paced/`의 두 탐색 실행은
이 비교에서 제외했다. 다음 조사 대상은 surface 대기와 stroke 종료 시 일시적
hitch, 반복 reopen/undo 및 장시간 UI/입력이다.

## 2026-09-05 WM_PAINT redraw comparison

[ADR-0028](decisions/ADR-0028-windows-paint-wakeup.md)의 입력 메시지 내 직접
렌더링 제거 및 WM_PAINT 합성을 설치 release에서 같은 전면 시작 절차로 비교했다.
하드웨어, 4K 장면, 32×121 direct-admission 입력, 모드별 3회 조건은 위와 같다.
아래 admission→present API 반환 percentile은 실행별 histogram 상한(ms)이다.

| 실행 | export 구간 | 입력 묶음 수 | p50 상한 | p95 상한 | p99 상한 | 관측 max |
|---|---|---:|---:|---:|---:|---:|
| baseline 1 | inactive | 1537 | 18.431 | 32.767 | 34.815 | 46.275 |
| baseline 2 | inactive | 1440 | 21.503 | 31.743 | 34.815 | 50.119 |
| baseline 3 | inactive | 1313 | 23.551 | 32.767 | 36.863 | 46.872 |
| export 1 | inactive | 736 | 23.551 | 32.767 | 34.815 | 40.918 |
| export 1 | active | 526 | 29.695 | 34.815 | 38.911 | 50.105 |
| export 2 | inactive | 788 | 22.527 | 32.767 | 34.815 | 41.131 |
| export 2 | active | 532 | 23.551 | 34.815 | 40.959 | 51.965 |
| export 3 | inactive | 783 | 22.527 | 32.767 | 34.815 | 41.121 |
| export 3 | active | 525 | 23.551 | 34.815 | 38.911 | 50.957 |

변경 전 기준 p95 31.743~34.815ms와 변경 후 31.743~32.767ms는 겹친다.
Export active p95는 전후 모두 34.815ms다. 실행 간 percentile을 평균하거나
차이를 개선율로 해석하지 않는다. 변경 후 기준 surface acquire p95는
15.359~15.871ms, export active는 15.359ms이며 여전히 큰 대기다.
이 6회에는 앞서 관측한 약 100ms hitch가 없었지만 해결을 입증하지 않는다.
UI thread와 렌더링을 분리한 것은 아니며, 합성 driver는 Win32 입력 dispatch를
우회하므로 제거한 per-input 직접 렌더링의 효과를 직접 측정하지도 않는다.
**지연 및 export 악화 ≤2ms gate는 계속 미통과다.**

6회 모두 입력 sequence/phase 종료, snapshot 33/history 33, 전체 tile/PNG의
별도 process replay 일치와 정상 writer join을 통과했다. 마지막 파일을 계측과
입력 driver 없이 재시작해 복원 화면을 확인하고 다시 닫아 동일 검증을 통과했다.
Driver 최대 schedule lateness는 실행별 0.903~4.704ms였다. 사용자 다운로드를
그대로 유지한 통제되지 않은 desktop 부하, warmup 미제외, 실제 pen/가시 픽셀/
120Hz cadence 미측정이라는 한계도 동일하다. 새 테스트나 의존성은 없다.

[전체 JSON 결과](measurements/desktop-4k-paint-redraw-2026-09-05.json)에 설치
hash와 소스 commit, 모든 단계 분포를 보존했다. 원본은
`target/paint-redraw/{baseline,export}-{1,2,3}/{out,err,verify}.log`, 최종 정상
재시작은 `export-3/reopen-*.log`다. 실제 UI 입력·Undo·pan·Fit 검증은 ADR-0028을
참조한다. 구조적 중복 렌더링 제거는 유지하되 성능 gate 해결과 구분한다.

## 2026-09-05 corrected color export CPU cost

색상 경계 수정(047cf6f)의 16-bit sRGB file export를 Windows 11 Home
10.0.26200 / Core Ultra 7 155H / cargo release / CPU backend에서 측정했다.
3840×2160의 유효 premultiplied 색·알파 패턴, warmup 1회 뒤 20회다.
동일 scratch 파일을 반복 인코딩·buffered write·close하는 시간이며 fsync,
writer queue, desktop 입력·GPU·화면 지연은 포함하지 않는다.

| p50 | p95 | p99 | PNG 크기 |
|---:|---:|---:|---:|
| 65.291ms | 67.305ms | 67.733ms | 2,376,932 bytes |

Nearest-rank percentile이며 [전체 측정 JSON](measurements/color-export-4k-2026-09-05.json)에
원시 20개 시간과 독립 PNG 관측을 보존했다. 고정 변환 lookup은 128KiB,
이 장면의 추가 encoded row는 30,720 bytes다. 16-bit 전체 이미지 복사 대신
행 단위 streaming을 사용한다. 이 값은 전체 프로세스 peak memory 측정은 아니다.
8,294,400개 픽셀의 export/import를 byte-exact 비교했고, 별도 Python zlib/CRC
검사로 file PNG의 16-bit/sRGB tag 및 비원색·반투명 첫 픽셀과 preview PNG의
8-bit 대응값을 독립 확인했다.

실행: `cargo run --locked -p nyatidraw-png-io --example color_export_probe --release -- target/color-export-cost`.
출력 폴더는 새 경로여야 하며 기존 폴더를 거부한다. 원본 로그는
`target/color-export-cost.log`다. 사용자 다운로드와 다른 desktop 부하는 통제하지
않았다. 단일 warm file-cache run이고 이전 export와 직접 비교한 장면이 아니므로
개선율, export 간섭 gate 또는 설치판 지연 합격을 주장하지 않는다.

# 2026-09-08: Startup / frame-tail 후속 진단

문서 기준 시각: 2026-09-08T21:24:30+09:00

범위가 제한된 후속 진단이다. [전후 12회 측정](status-performance-2026-09-08.md)의
결과를 대체하지 않으며 startup 정지 또는 입력 지연을 해결했다는 문서가 아니다.
기준 커밋은 `04016b2`다.

## 두 개의 독립 진단

### 첫 DOM mutation

`NAYATI_STARTUP_DIAGNOSTICS=1` 아래 첫 `NativeInterpreter.rafEdits`의
entry/return/throw를 기존 socket/ACK/poll-ready 로그와 연결한다. Windows의
현재 pinned interpreter는 `headless=true`이므로 해당 함수는 rAF를 기다리지
않는다. 함수 이름만 보고 rAF throttling을 원인으로 해석하면 안 된다.

진단 wrapper는 원래 this·인자·반환값·throw를 보존한다. 보고 실패는 원래
실행으로 전파하지 않으며 method/stage whitelist, byte 길이와 bool만 출력한다.
Payload·URL·키는 출력하지 않는다. 이 wrapper는 flag가 꺼지면 삽입되지 않는다.
이전 진단의 mounted IPC/tagged geometry는 남아 있으며 모든 진단이 완전히
타이밍 투명하다는 뜻은 아니다. 재시도/timeout/visibility/ACK 정책은 바꾸지 않는다.

### Actor batch / frame correlation

`NAYATI_PERFORMANCE=1`과 `NAYATI_FRAME_TRACE=1`일 때만 기록한다.
Actor mailbox를 꺼내고 잠금을 해제한 시점을 `wake=mailbox-observed`로 정의한다.
실제 producer의 wake 요청 시각이나 OS scheduling 시간의 직접 측정은 아니다.

- 하나의 batch ID에 render/scene/acquire/blit-present의 경계를 기록한다.
- Earliest admission과 첫/마지막 dequeue를 연결한다. 이전 skipped batch에서
  입력이 넘어오면 원래 batch ID, 병합 수, 문서 control 경계 통과 여부를 남긴다.
- 출력은 signed microsecond offset과 `None`으로 부재를 구분한다. 이전 batch의
  admission은 음수가 될 수 있으며 오류로 잘라내지 않는다.
- 8ms 이상 batch/input tail 또는 dequeue했으나 present하지 않은 batch 중
  처음 128건만 보존한다. 전체 outlier 분포나 최악의 128건이 아니다.
- 고정 상한 메모리를 쓰며 hot path에 파일 I/O, 로그 출력, 새 lock을 추가하지
  않는다. 출력은 flush에서만 한다. 기록된 thread 밖의 helper thread는 관측하지
  않으며 생략/미표시/병합 계수를 별도로 출력한다.

이 기록은 같은 CPU 실행 batch의 시간 관계다. Present API 반환이 화면 표시나
해당 입력의 실제 픽셀 포함을 증명하지 않는다. Shader/hardware GPU 실행시간도
아니며 서로 다른 stage의 p99를 빼서 같은 프레임 비용을 만들어내지 않는다.

## 빌드 / 초기 실행 검증

- Desktop 핵심 25 passed / 1 명시적 registry acceptance ignored.
- 전체 targets/features Clippy와 fmt 통과. 새 UI/mock 자동 테스트는 추가하지 않았다.
- DX release 23.79초에 빌드 성공. SHA256:
  `8337efcc0f6713eaa04c57454c71117f7e3c0ad710df9611158727ff1aef7c14`.
- 첫 mutation 진단 full export/recovery 1회 PASS. 원본은
  `target/startup-frame-triage-full-1.log`다. 50개 primary 각각 첫 queue/socket
  send/interpreter entry·return(`headless=true`)/ACK/poll-ready/connected geometry를
  기록했다. Throw/누락/실패 hold는 0이었다. 이는 의도적으로 invalid project를
  거부하는 경우도 포함한 DOM bootstrap 기록이지 50개 작품 로딩 성공의 주장은
  아니다. 과거 정지는 재현되지 않았으며 원인은 여전히 미확정이다.
  실패하면 opt-in harness가 최대 60초만 창을 보존하고 늦게 회복해도 원래 실패를
  PASS로 바꾸지 않도록 유지했다.
- 기존 alpha.4 설치판과 작품을 교체하지 않는다. 실패 scratch/raw log만 로컬
  `target`에 보존하고 개인정보가 없는 집계·판정만 문서화한다.

## Frame trace 실험

기존과 같은 Windows 11 Pro 10.0.26200 / Ryzen 7 5800X3D / RTX 3080
(driver 32.0.15.9621), DX12 Mailbox, 1353×953 scale 1 surface다. WMI의 60Hz는
실측 frame cadence가 아니다. 위 hash의 DX release를 쓰고 startup 진단 flag는
끄며 performance/frame trace만 켰다. 3840×2160 두 레이어에서 64px round brush
32 stroke × 121 sample을 nominal 240Hz로 native bridge에 직접 admission한다.
실제 Win32 펜 이벤트 경로는 거치지 않는다. Warmup을 제외하지 않는다.

창을 활성화하고 기본 31% viewport를 관찰한 다음 start marker를 만든다.
측정 중 다른 build/CPU benchmark는 실행하지 않았으나 일반 desktop activity는
통제하지 않았다. Scratch 준비·독립 검증기는 debug이며 측정 구간 밖에서 실행한다.

`export-1`은 작업자 context 인계 중 marker 생성이 harness의 2분 제한을 넘었다.
`start-marker-timeout` 뒤 stroke workload가 시작되지 않았으므로 측정값으로 쓰지
않는다. 원본을 보존하고 정상 종료한 뒤 새 `export-2`로 수행했다. 명시적
`metadata.excluded_runs`에 사유와 원본을 남기며 parser는 다른 오류를 자동 제외하지
않는다. 이는 성능 결과가 나쁜 실행을 숨기거나 timeout을 PASS로 바꾼 것이 아니다.

| 실행 | Actor batches | Present 반환 | Control only | 보존된 tail | 생략 |
|---|---:|---:|---:|---:|---:|
| baseline-1 | 3897 | 3891 | 6 | 51 | 0 |
| export-2 | 3965 | 3958 | 7 | 57 | 0 |

두 실행 모두 skipped render, pending unpresented input, coalesced presentations,
기록 thread의 unframed dequeue는 0이다. 보존되지 않은 threshold 미만 batch는
각각 3846/3908이다. 128건 상한이 차지 않았지만 전체 frame 분포가 저장된 것은
아니다. Startup의 긴 첫 scene도 포함하므로 steady-state tail과 혼합하지 않는다.

기존 histogram의 admission→present API 분포는 다음과 같다(단위 ms, 각 percentile은
해당 실행 histogram 상한). Trace 경계와 histogram은 별개의 `Instant`에서 읽으므로
같은 batch의 숫자도 수 µs 차이가 난다. Export phase는 admission이 아닌 dequeue
시점에 분류한다. 이 1회씩의 진단 실행으로 전후 개선율을 주장하지 않는다.

| 실행 / export phase | 표본 | p50 | p95 | p99 | max |
|---|---:|---:|---:|---:|---:|
| baseline-1 inactive | 3820 | 1.407 | 2.431 | 5.887 | 15.527 |
| export-2 inactive | 2446 | 1.407 | 2.303 | 3.455 | 17.564 |
| export-2 active | 1399 | 1.535 | 2.559 | 12.799 | 18.393 |

두 실행 모두 32×121 workload, PNG 최신 표시, 정상 Close와 surface-retired→HWND
파괴를 확인했다. 이후 별도 프로세스에서 3840×2160 / history 33의 전체 tile·PNG가
deterministic replay와 정확히 일치했다. 루트 ID는 양쪽 모두
`150107427654205048364740414203780730923`이다. 이는 shared brush 구현을 사용하는
재연산 대조이며 독립 브러시 알고리즘 또는 실제 GPU 화면 픽셀 대조는 아니다.

[집계와 보존된 행 전체](measurements/desktop-frame-triage-2026-09-08.json)는
`python -B tools/summarize-frame-trace.py target/frame-triage OUTPUT.json`으로
생성한다. 실제 두 실행에서 parser의 경계 순서·계수·정상 종료·재열기 검증은
`valid=true`였다. 원본은 각 run의 `out.log`, `err.log`, `verify.log`에 남는다.

### 현재 읽을 수 있는 관계

baseline batch 3523의 scene은 18.443ms이고 dequeue는 없었다. 바로 다음
batch 3524에서 admission은 actor 관찰보다 13.877ms 먼저였고, 현재 scene은
1.357ms, acquire는 0.010ms, blit/present는 0.281ms였다. 해당 입력의
admission→present API 반환은 15.525ms다. 또 다른 인접 쌍 3159→3160에서도
긴 무입력 scene(17.333ms) 뒤 입력의 admission offset -12.595ms가 관측됐다.

반면 export batch 27의 18.392ms tail에는 actor 관찰 전 8.438ms와 현재 scene
3.352ms, acquire 6.237ms가 함께 포함됐다. Batch 1218은 export-inactive인데도
scene 17.047ms가 대부분을 차지했다. 따라서 acquire와 scene 양쪽을 분리해서
보아야 하며 export-active 여부 하나로 모든 tail을 설명할 수 없다.

즉 이 표본에서는 현재 프레임의 present만 느린 것이 아니라, actor가 다음 입력을
꺼내기 전에 수행한 작업을 조사할 이유가 있다. 이것이 OS scheduling, GPU driver,
저장 중 어느 하나만의 원인이라는 증거는 아니다. 특히 `scene`은 전체 native
canvas render 호출이므로 브러시 처리·완료 타일 반영·명령·합성이 모두 포함된다.

### 다음으로 좁힐 지점

`NativeCanvas::apply_materialized_tiles`는 새 입력이 없어도 writer 완료를 받아
CPU tile 교체, GPU upload, history projection을 수행한다. 같은 batch ID 안에서
이 함수의 입출구·처리 타일 수와 `scene.render_viewport` 경계를 측정하여 완료
반영과 합성을 먼저 구분한다. 아직 이 함수를 병목으로 확정하거나 타일 반영을
생략하지 않는다. Live stroke, preview generation, 저장 완료 history 순서는 그대로
보존해야 한다.

초기 실행 정체는 재현되지 않았다. 새 실패 시 첫 socket send→interpreter entry
→return/throw→ACK 중 어디에서 멈추는지 확인하는 것이 정확한 재개 지점이다.

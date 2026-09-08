# 2026-09-08 Sprint 3: 렌더 대기 격리와 합성 비용

문서 기준 시각: 2026-09-08T21:24:30+09:00

기준 소스 `444b286`, 기준 실행 파일은 검증된 alpha.4 stage다.
설치·Save As 수용은 [별도 기록](status-plan-2026-09-08.md)에 있다.
이 작업은 진행 중이며 Sprint 3 전체 완료를 뜻하지 않는다.

## 범위와 담당 경계

- Windows host: GPU surface 작업을 UI 입력 dispatch에서 분리한다. HWND 생존,
  초기화 실패, Save As/activation, geometry publication, 비동기 종료를 함께 다룬다.
- CPU 합성: visible raster의 전체 페이지 임시 버퍼 대신 실제 타일 교차 영역을
  처리한다. group isolation과 기존 정수 alpha 계산은 보존한다.
- 수용 harness: exact child process와 scratch 작품만으로 resize/minimize/restore,
  early Close, 정상 종료와 별도 프로세스 재열기를 검사한다.
- 감독: 변경 전후 동일 4K workload, 전체 그림/PNG 대조, 통합 빌드·검토·문서.

세 작업은 Astra로 나누며 같은 파일을 동시에 수정하지 않는다. 성능 측정 중에는
동시 CPU benchmark와 빌드를 피하도록 슬롯을 나눈다. 다른 사용자 작업/백그라운드
부하는 통제하지 않으므로 실험실 환경이라고 주장하지 않는다.

## 기준선 수집

`target/render-isolation-before/{baseline,export}-{1,2,3}`에 6회 실행을 보존했다.
각각 4K/2레이어, 64px round brush, 32 stroke × 121 sample, 명목 240Hz다.
테스트 창을 활성화하고 screenshot/accessibility 상태를 확인한 뒤 명시적 marker로
시작했다. export 모드는 매 stroke 전 PNG Running을 확인한다. 모든 실행에서
workload 완료와 정상 Close를 기록했다. 6개 프로젝트 모두 별도 프로세스의
deterministic replay에서 전체 tiles/PNG 일치, history 33을 확인했다.
[모든 실행의 분포](measurements/desktop-render-isolation-before-2026-09-08.json)를
보존했다. 변경 후 동일 조건 6회 실행도 완료했으며 아래 비교와 재열기 검증 기록을
함께 확인한다.

환경은 Windows 11 Pro 10.0.26200, Ryzen 7 5800X3D, RTX 3080 DX12/Mailbox,
driver 32.0.15.9621, surface 1353×953/scale 1, DX 0.7.9 release다.
WMI refresh는 60Hz이며 실제 표시 cadence의 측정은 아니다.
fixture 생성/별도 verifier는 debug 실행 파일로 수행하지만 측정 구간은 고정된
release 앱이다. alpha.4 실행 파일 SHA256은
`70eca144a512f7dce3a557119f21c2cbc57521989709211d164626f475fcece8`.

입력은 native bridge 직접 admission이므로 OS dispatch·실제 펜·첫 가시 픽셀을
입증하지 않는다. present API 반환과 디스플레이 표시를 구별하며 p50/p95/p99는
실행별 histogram 상한으로 남긴다. 다른 GPU/이전 Intel 호스트 기록과 합치지 않는다.
기준선의 surface acquire p95는 11~13µs로 짧다. 스레드 격리가 이 머신에서 반드시
지연 개선을 낸다고 미리 단정하지 않는다.

## 완료 조건

1. UI thread에 acquire/configure/present가 남지 않으며 mailbox 잠금 중 GPU/I/O를 하지 않는다.
2. raw surface보다 HWND를 먼저 파괴하지 않고 UI가 live worker를 join하지 않는다.
3. 종료 요청과 geometry epoch가 덮어쓰이거나 잘못된 문서로 새 입력이 들어가지 않는다.
4. 핵심 invariant, fmt/Clippy, DX release, 창 lifecycle와 기존 durability/export/Save As 수용 통과.
5. 전후 4K 비교와 CPU 합성 oracle/measurements를 기록하고 매 작품의 별도 재열기/PNG 대조 통과.

테스트용 alpha.4 설치판을 실험 빌드로 자동 교체하거나 공개 배포하지 않는다.
기능 수용 실패 시 구현을 고치며, 미검증 상태를 완료라고 표시하지 않는다.

## CPU 합성 중간 결과

[ADR-0044](decisions/ADR-0044-tile-intersection-cpu-composite.md)의 타일 교차 행
합성은 CPU oracle과 13개 핵심 tests/Clippy를 통과했다. 4K 희소 8레이어 CPU
합성 p95는 554.055→83.177ms, 조밀 2레이어는 203.397→171.700ms다.
전체 드로잉 지연 측정은 아니다. 변경 후 desktop durability, 창 lifecycle의 별도
재열기/PNG 대조와 실제 Save As 수용까지 통과했으며 CPU 변경은 `26db46c`에
독립적으로 커밋했다. 전체 export-recovery와 렌더 전후 지연 비교는 별도 gate다.

## 렌더 스레드 통합 중간 결과

- [ADR-0045](decisions/ADR-0045-windows-render-actor-lifetime.md): HWND/input은
  UI에 남기고 GPU 생성/configure/acquire/present와 문서 전환은 actor로 옮겼다.
  Dioxus Desktop 0.7.9에 좁은 exit guard를 공급하여 메시지 펌프를 유지하며
  surface가 먼저 정리되도록 했다. 의존성 버전은 올리지 않았다.
- 독립 감사로 pre-present 입력 affine 공개, retirement와 실패 후 reopen 경합,
  guard 없는 기본 LoopDestroyed 생략을 발견하고 수정했다.
- `cargo test --workspace --locked`, 전체 targets/features Clippy `-D warnings`,
  `cargo fmt --all -- --check` 통과. Desktop 핵심 검사는 25 passed / 1 ignored.
- 당시 통합 소스 DX release 빌드 통과. 실행 파일 SHA256:
  `43f09e17e98d868f8de25e3aeec606ff7e9e74ee8acbb98e18f1cec30be43805`.
- `desktop_window_lifecycle_smoke --require-render-worker` 통과.
  `target/window-lifecycle-local-run`에 증거를 보존했다.
  자식 4개 모두 WM_CLOSE 후 exit 0이며 강제 종료 없이 surface-retired →
  canvas-hwnd-destroyed 순서를 확인했다. 4회 resize/minimize/restore와 resize 직후
  Close, first-present 전 초기 Close, 일반 재시작을 포함한다. 별도 verifier 4회에서
  page/tree/history/signed/offpage tiles/PNG가 정확히 일치했다.
  이때 Clippy가 병행됐으므로 응답 시간은 성능 수치로 사용하지 않는다.

기존 durability 수용도 통과했다. 32 stroke/eraser, 입력 discontinuity,
active stroke의 Close/Save 뒤 재열기를 포함한다.
`target/render-isolation-durability.log`에 결과를 남겼다. Invalid nonempty 파일은
오류 창 유지와 원본 보존 확인 뒤 harness가 정확한 scratch 자식만 abort하며,
이 경우를 정상 종료 증거에 포함하지 않는다.

Computer Use로 실제 Save As 대화상자에서 새 한글/공백 경로를 지정했다.
사본 제목 전환 → Alt+F4 → 프로세스 종료 뒤 별도 `desktop_save_as_fixture verify`가
원본 bytes 불변, history 3개와 분기/페이지 밖 타일/metadata/PNG bytes 일치를
확인했다. `target/render-isolation-acceptance/nyatidraw-save-as-scratch/`에
원본·사본·로그를 보존한다. 파일 복사로 UI 동작을 대체하지 않았다.

Export recovery의 첫 실행은 inside/below drag 후 above에서 실패했다.
원본 로그 `target/render-isolation-export-recovery.log`는 보존한다. 재열기 레이어
projection만 기다리면 첫 Fit이 뒤늦게 revision을 바꿔 drag를 올바르게 무효화할
수 있는 경합을 확인했다. Scratch 전용 probe는 첫 present mapping과 그 revision의
DOM 반영을 기다린 뒤 시작하도록 수정했다. 실제 stale drop 거부와 작품 검증은
완화하지 않았다. Probe만 수정한 DX 재빌드/desktop Clippy도 통과했으며 이 빌드의
SHA256은 `c7d44a70a2106765784a0f8a6d4f123700f10f807f8b63b5baeda2240bba0ace`다.
전체 export/recovery 재검사와 변경 후 4K 성능 비교는 진행 중이다.

재검사는 6개 synthetic drag, 67개 history 항목/66 sibling 분기, absent PNG pair를
통과한 뒤 zero-byte pair의 반복 restart에서 first-present timeout을 발견했다.
해당 프로세스는 durable snapshot 2/5 tiles를 읽었으나 child geometry가 1×1에서
갱신되지 않았다. 실패 로그 `target/render-isolation-export-recovery-final.log`도
보존한다. 아직 전체 export/recovery 통과로 세지 않는다. 초기 DOM observer와
WebView mutation/ACK 수명 진단을 먼저 수행하며, 원인을 확정하기 전에 timeout을
늘리거나 임의 재시도로 gate를 통과시키지 않는다.

추가 진단에서 같은 zero-byte snapshot을 12회 연속 재시작한 별도 수용은 통과했다
(`target/render-isolation-repeated-startup-2.log`). 그러나 full 검사에서는 다시
layer-history snapshot 6/20 tiles에서 observer 시작 이전 정체를 관찰했다
(`target/render-isolation-export-recovery-diagnostic.log`). 파일 내용만으로 재현되는
오류라고 결론 내릴 수 없으며 반복 재시작 성공이 이전 실패를 지우지 않는다.

초기 mutation queue/socket/ACK/mount 단계를 구분하는 opt-in
`NAYATI_STARTUP_DIAGNOSTICS`를 추가한 release 빌드
`b2b6d02589c646a096d93fde9f7c2de80b2678389f88cb5fad2e646191a5d742`로
full export/recovery가 2회 연속 통과했다.
`target/render-isolation-export-recovery-ack-hold.log`와 `-ack-hold-2.log`에
PNG pair absent/zero/valid/invalid, same-primary activation, 4개 crash 경계,
superseded export 폐기, failed Close→reopen/save를 기록했다. 실패 후 창을 60초
보존하는 opt-in harness를 사용했지만 두 실행 모두 해당 hold는 발생하지 않았다.
원래 timeout과 실패 판정, artwork 비교를 완화하지 않았다.

**초기화 정체는 원인 미확정 상태다.** 창 활성화 전후의 실패 관찰을 얻지 못했다.
Pinned Dioxus의 Windows 초기화는 숨긴 window config로 interpreter를 만들고
`headless=true`를 전달하므로 첫 mutation ACK는 rAF를 우회한다. 따라서 단순히
가려진 창의 rAF 중단 때문이라는 가설은 이 코드 경로에 맞지 않는다.
진단 로그 추가를 해결책으로 보거나 Sprint 3 startup 안정화 완료로 세지 않는다.
성능 비교는 진단 flag를 끈 정상 완료 실행에서 별도로 수행한다.

## 변경 후 4K campaign

구현은 `fe394e6`에 커밋했다(CPU 변경 `26db46c` 포함). 최종 DX release 빌드는
17.76초에 통과했으며 실행 파일 hash는
`45231e273722b54640e4a433a1a9116c3ccb7cd99e441903de801de29e884134`다.
`target/render-isolation-after/{baseline,export}-{1,2,3}`의 6회 모두 창을 활성화하고
관찰한 뒤 marker로 시작했다. 32×121 sample, 최종 snapshot 33 PNG export,
Alt+F4 정상 종료와 surface-retired→canvas-hwnd-destroyed를 확인했다.
Startup 진단 flag는 끄고 측정했지만 추가 mounted IPC와 tagged geometry 형식은
남아 있으므로 완전히 동일한 진단 없는 경로라고 주장하지 않는다.

[실행별 비교](performance.md#2026-09-08-render-isolation-변경-후-비교)에서 일반
입력 p95는 비슷하고 export-active p99는 3회 중 2회 커졌다. 최대 약 51ms의
outlier도 남아 있다. **렌더 분리 자체를 입력 저지연 gate 통과로 세지 않는다.**
Export CPU 합성 p95는 155.647/139.263/139.263ms에서
93.421/105.169/90.111ms로 줄었다. 전체 변경의 비교이며 원인별 개선율이나
모든 하드웨어에서의 효과를 입증하지 않는다. Export 시간이 줄어 active input
표본 집합도 바뀌었으므로 phase percentile을 같은 입력 시점의 대조로 읽지 않는다.

6개 scratch 프로젝트 모두 별도 프로세스의 deterministic replay와 정확히 일치했다.
각 `verify.log`는 전체 tiles/PNG, history 33과 동일 root
`150107427654205048364740414203780730923`을 확인한다. 모든 입력 sequence는
연속된 32×121이며 모든 export phase에서 dequeue/present 표본 수가 같고 flush의
pending batch는 0이다. Batch 수와 input sample 수는 서로 다른 단위다.
[변경 후 전체 JSON](measurements/desktop-render-isolation-after-2026-09-08.json)에
실행별 분포, hash, 계수와 한계를 보존했다. 기존 alpha.4 설치판·사용자 작품은
교체하지 않았고 이 빌드를 공개 배포하지 않았다.

## 바로 이어가는 작업

아래 진단의 구현·실행 결과는 [후속 기록](status-frame-triage-2026-09-08.md)에
분리한다. 기존 12회 전후 측정은 변경하지 않는다.

- 프레임 ID별 bounded 진단으로 actor batch 관찰, input dequeue, scene,
  surface acquire/present를 연결한다. 별개 분포의 최대값을 같은 프레임이라고
  추측하거나 percentile 차이를 구간 시간으로 해석하지 않는다.
- Startup flag 아래 첫 interpreter mutation의 entry/return/throw를 기록해
  socket 전송→브라우저 적용→ACK 경계를 더 좁힌다. Payload/키/URL은 남기지
  않으며 재시도, timeout, headless/visibility 정책으로 실패를 가리지 않는다.
- 두 작업은 진단이며 아직 원인 해결이나 지연 개선을 뜻하지 않는다. 기존 12회
  전후 측정과 실패 로그는 그대로 보존하고 후속 진단 빌드와 혼합하지 않는다.

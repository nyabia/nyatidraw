# 2026-09-08: 완료 타일 반영의 반복 복사 제거

문서 기준 시각: 2026-09-08T21:24:30+09:00

중복 복사 제거의 전후 측정·독립 픽셀 검증은 완료했고, 전체 재시작 수용은 초기 UI
정체로 열려 있다. 기준은 `4a432c8`이며 [이전 프레임 진단](status-frame-triage-2026-09-08.md)의
무입력 scene tail 후속 작업이다. Sprint 3 전체 완료나 일반적 latency 해결이 아니다.

## 확인된 코드 비용과 미확정 원인

`NativeCanvas::apply_materialized_tiles`는 GPU 반영을 미룬 완료 타일을 매 호출마다
다시 CPU map에 clone/insert한다. 128×128 RGBA8 타일은 64KiB이고 함수는 정상
render에서 두 번 호출된다. 따라서 새 writer completion의 CPU 반영과 deferred
GPU 반영 재시도를 분리할 가치가 있다. 이것이 관측된 17~19ms scene의 주원인인지는
아직 별도 계측이 필요하다.

`upload_closed_tile` 자체에는 tile별 submit이나 texture 생성이 없지만 내부
`write_texture`의 staging 비용은 남는다. Dirty group 합성의 submit을 단순히
묶는 것은 공유 opacity uniform 순서를 바꿔 그림을 훼손할 수 있으므로 이번 작업에서
하지 않는다. 미리보기 세대·live stroke·history 순서를 단축해서 속도를 얻지 않는다.

## 실행 순서

1. 기존 동작 그대로 두고 opt-in trace에 완료 처리 누적시간·호출수·새 completion·
   CPU 복사량·GPU 반영/대기 수와 viewport 경계를 연결한다.
2. 이 진단-only DX release에서 4K baseline/export 각 1회, 정상 종료·독립 tile/PNG
   대조를 수행한다. 측정 중 build나 CPU 검증을 같이 돌리지 않는다.
3. 새 completion의 CPU 반영을 한 번만 수행하고 GPU 대기 항목에는 재반영하지 않는다.
   동일 key의 오래된 deferred/새 completion 순서와 history adoption을 독립 검토한다.
4. 동일 조건으로 최적화 DX release를 측정하고 복사 횟수·CPU 시간·입력 tail을
   각각 비교한다. 제한된 표본의 p99 하나로 인과나 보편적 개선을 주장하지 않는다.
5. 핵심 artwork 불변식만 테스트하고 fmt/Clippy/release build, 저장 후 프로세스 종료와
   재열기 검증, 문서·결과 커밋으로 완료한다.

기존 paced workload는 매 stroke 뒤 history 게시를 기다리므로 deferred 경로를
거의 밟지 않을 수 있다. 이를 기존 비교에 섞지 않고 별도 opt-in overlap 실행을
추가한다. 같은 32×121 샘플과 최종 그림을 유지하되 bounded 소그룹마다 history를
기다려 다음 stroke 진행 중 이전 완료가 도착하게 한다. Export 동시 모드와는
혼용하지 않는다. 양 빌드에서 같은 변형을 한 번씩 별도 비교하며 실제 defer 계수가
0이면 이 실험이 반복 복사 제거를 입증했다고 쓰지 않는다.

기존 설치판·사용자 작품·공개 배포는 변경하지 않는다. 최초 startup 정체, 실제 펜,
가시 픽셀·고주사율·다른 GPU의 증거는 계속 별도 미검증으로 남긴다.

## 구현 및 핵심 검증

계측 기준 소스는 `cd30592`, DX release SHA256은
`b76931c229758ce4e68e2ecc90b461595db02b2a4bda707a899711ef1b18effb`다.
최적화 소스는 `1fe7f21`으로, 이 기준 위 `native_canvas.rs`만 변경했고 SHA256은
`3bc5354e5fb31790898eb763dd43528582a20a5152d8df239ce72913017e9c3e`다.
DX 빌드는 각각 18.68/17.31초에 성공했다. Desktop 핵심 검사는 26 passed,
명시적 registry acceptance 1 ignored이고 전체 targets/features Clippy와 fmt도
통과했다. 기존 vendor warning은 남는다. 이 수치는 runtime 수용을 대신하지 않는다.

`ClosedStrokeCompletion`과 GPU 전용 `DeferredTileUpload`를 분리했다. Fresh FIFO만
CPU map에 반영하고 최신 history를 보관한 뒤, 기존 GPU 대기→fresh 순서로 업로드를
시도한다. Pending GPU 항목은 CPU/history에 재채택되지 않는다. Newer preview와
active layer guard, 성공한 generation만 제거하는 순서, idle 명령 전 drain/revision
검사와 history 게시 시점은 유지했다. 0/1/2 fresh 완료와 빈 재시도에 대한 핵심 표
검사 하나로 동일 signed tile의 CPU/history 최신성·GPU FIFO·복사량을 검증했다.
GPU driver 동작 자체를 mock으로 검증한 것은 아니다.

기준 일반 baseline/export는 모두 defer=0, CPU 복사 1,344회(88,080,384B)였다.
반면 별도 overlap 기준은 defer 재시도 54,178회, CPU 복사 55,522회
(3,638,689,792B)를 실제 기록했다. 32개 fresh completion으로부터 같은 최종 그림을
만들면서도 active stroke 동안 오래된 GPU 대기 항목을 계속 복사하는 비용이다.
최적화 overlap에서도 GPU defer는 53,049회 발생했지만 CPU 복사는 1,344회
(88,080,384B)로 줄었다. Fresh completion은 양쪽 32개, GPU upload 성공은 양쪽
588회다. GPU가 새 preview를 보호하기 위해 반영을 미루는 동작은 유지하면서
불필요한 CPU 재복사를 제거한 것이다. Defer 재시도 횟수는 실제 actor 타이밍에 따라
달라지므로 두 실행이 동일한 scheduling을 가졌다고 주장하지 않는다.

## 실행별 비교

Windows 11 Pro 10.0.26200, Ryzen 7 5800X3D, RTX 3080 (driver 32.0.15.9621),
DX12 Mailbox, 1353×953 scale 1 surface에서 실행했다. 창을 활성화하고 기본 31%
viewport를 관찰한 뒤 marker로 시작했다. Warmup은 제외하지 않고 WMI의 60Hz를
실측 cadence로 취급하지 않는다. 각 측정 구간에는 build/다른 verifier를 겹치지
않았으나 일반 desktop activity는 통제하지 않았다. Debug fixture/독립 검증은
측정한 release 앱의 구간 밖에서 실행했다.

| 변형 | 전 CPU 복사 / bytes | 후 CPU 복사 / bytes | 전 완료 처리 총 ms | 후 완료 처리 총 ms |
|---|---:|---:|---:|---:|
| 일반 baseline | 1,344 / 88,080,384 | 1,344 / 88,080,384 | 69.187 | 65.417 |
| export 동시 | 1,344 / 88,080,384 | 1,344 / 88,080,384 | 61.960 | 68.666 |
| 최대 4획 overlap | 55,522 / 3,638,689,792 | 1,344 / 88,080,384 | 243.990 | 60.737 |

위 수치는 actor lifetime 전체의 계수/누적시간이며 보존된 128개 tail 행만의 합이
아니다. 완료 처리 시간에는 map 삽입·업로드·history 게시도 포함되므로 순수 memcpy
시간으로 읽지 않는다. Overlap 복사 bytes는 약 97.6% 감소했지만 일반 두 변형에는
중복 복사가 없어서 같은 효과를 기대하지 않는다.

Admission→present API 반환 분포(단위 ms, 실행별 histogram percentile 상한):

| 변형 / phase | 전 표본 | 후 표본 | 전 p50/p95/p99 | 후 p50/p95/p99 | 전/후 max |
|---|---:|---:|---|---|---|
| baseline inactive | 3827 | 3816 | 1.407 / 2.303 / 4.863 | 1.343 / 2.303 / 6.655 | 20.968 / 17.539 |
| export inactive | 2452 | 2549 | 1.407 / 2.303 / 3.583 | 1.407 / 2.175 / 3.455 | 16.618 / 16.213 |
| export active | 1378 | 1308 | 1.471 / 2.559 / 12.287 | 1.407 / 2.559 / 11.263 | 24.101 / 16.050 |
| overlap inactive | 3832 | 3838 | 1.407 / 2.431 / 9.727 | 1.343 / 2.431 / 7.679 | 36.713 / 15.217 |

각 변형은 전/후 한 번씩이다. p95가 그대로인 경우가 있고 baseline p99는 커졌다.
이 결과를 일반적인 입력 latency 개선율로 과장하지 않는다. Export phase는 dequeue
시점에 분류하므로 active/inactive는 같은 시점의 표본 집합도 아니다. 물리 펜·GPU
execution·첫 가시 픽셀 latency와는 다른 CPU API 경계다.

### 남아 있는 비용

기준 baseline batch 2316의 scene 19.056ms에는 완료 처리 4.283ms와 viewport
14.760ms가 포함됐다. Export batch 1712도 18.030ms 중 viewport 14.592ms였다.
이처럼 완료 타일 뒤의 재합성이 큰 사례가 있지만, materialized/viewport가 모두
짧은 다른 scene tail도 있다. 현재 수정은 **중복 CPU 복사 제거**이고 모든
무입력 scene 정체의 해결이 아니다. 다음 최적화는 dirty group 합성의 encoder·
uniform 사용 순서를 정확히 보존할 수 있는지 검증한 뒤 별도로 진행해야 한다.

## 작품 재열기와 아직 실패하는 통합 검사

전/후 세 변형, 총 여섯 프로젝트 모두 workload 32×121, PNG 최신 표시, 정상 Close,
surface-retired→HWND 파괴를 확인했다. 각각 별도 verifier 프로세스에서 전체 tiles와
PNG bytes가 deterministic replay와 정확히 일치했다. History는 모두 33이고 루트 ID는
`150107427654205048364740414203780730923`이다. Oracle은 shared brush replay이므로
GPU 화면과 독립 브러시 구현의 정확성까지 입증하는 것은 아니다.

[전 집계](measurements/desktop-completion-before-2026-09-08.json)와
[후 집계](measurements/desktop-completion-after-2026-09-08.json)는 각각 parser
`valid=true runs=3`으로 검증했다. 원본은 `target/completion-before`와
`target/completion-after`의 각 run `out.log`, `err.log`, `verify.log`에 보존했다.
모든 run의 trace retention 생략은 0이다. 성능 측정 후의 통합 검사는 다음과 별개다.

- `target/completion-adoption-export-recovery.log`: 진단 flag OFF의 full 검사에서
  layer/history와 absent PNG pairing은 통과했으나 zero-byte pairing의 초기
  native surface가 1×1, GPU-ready인 채 첫 present를 45초 동안 내지 못해 실패했다.
  실패 확정 후 opt-in 60초 hold로 관찰했다. 활성화 전 accessibility에는 실제 도구
  내용이 없었고, 활성화 뒤에도 제목 아래 검은 빈 창만 보였다. Geometry/첫 present
  로그도 추가되지 않았다. Harness는 원래 실패를 유지하고 exact child를 종료했다.
- `target/completion-adoption-durability.log`: 앞선 raw/closed 획 처리와 재시작은
  진행됐으나 snapshot 38 재시작이 다시 1×1/GPU-ready에서 멈춰 기존 45초 종료
  deadline으로 실패했다. 이 harness에는 hold가 없으며 exact child abort였다.

이 실패들은 정상 Close 성공이나 모든 복구 검사를 통과한 것으로 세지 않는다.
최적화 이전에도 있던 초기 UI 정체와 같은 외형이지만 이번 실패의 내부 원인이
같다고 확정하지는 않는다. 첫 DOM entry/return/ACK 진단을 켠 별도 한 번의 재검사로
범위를 좁히며, flag를 켠 성공이 flag OFF 실패를 해결했다는 증거는 아니다.

그 별도 재검사 `target/completion-adoption-export-recovery-diagnostic.log`는 PASS였다.
50개 primary에서 첫 interpreter entry/return(`headless=true`), ACK, observer까지
이어졌으며 throw/hold는 0이었다. PNG pairing 전체, layer/history, 의도된 crash
4개 경계, supersession과 failed-Close 후 reopen/save를 포함한다. 의도적 crash는
정상 종료 증거로 세지 않는다. 앞선 두 실패는 그대로 미해결이며 같은 바이너리에서
진단 flag가 타이밍을 바꿀 수 있다는 한계도 유지한다.

## 사용자 요청에 따른 마무리 / 다음 재개점

추가 구현·반복 실행은 여기서 중지했다. 모든 측정/검증 앱과 서브에이전트는 종료했다.
이 작업 단위의 CPU 최적화는 구현·핵심 검사·전후 측정·재열기를 마쳤으나 전체
Sprint 3은 초기 UI 정체 때문에 완료 처리하지 않는다. 설치된 alpha.4나 공개 Release는
최신 성능 변경으로 교체하지 않았다.

다음 재개 우선순위는 startup 전달 경계다. 읽기 전용 감사에서는 websocket read
오류 후에도 ACK 성공을 보내는 경로, 재연결 map의 세대 소유권, 주소 갱신 전 알림
순서가 후보 위험으로 보였다. 하지만 이번 실패에 socket 오류/재연결/주소 교체가
있었다는 증거가 없어 원인으로 확정하거나 수정하지 않았다. JS entry 로그 부재도
진단 IPC 전달이 지연된 경우와 구분해야 한다. 최초 mutation 적용 또는 ACK 송신의
throw는 현재 진단으로 구분할 수 있다. rAF/visibility를 추측으로 바꾸지 않는다.

Startup을 먼저 좁힌 뒤 dirty group 합성 비용을 개선한다. CPU 복사 제거에서 얻은
효과와 별개의 작업이며, 공유 uniform의 순서를 보존한 batching 설계가 필요하다.

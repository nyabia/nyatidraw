# 일러스트·게임 아트 확장 검증

현재 작업 트리의 로컬 검증 기록이다. 설치본 교체, GitHub push, 태그 및 배포 기록이 아니다.
기능 범위는 [제품 방향](game-and-illustration.md), 실제 웹 조작은
[애니메이션 통합 확인](animation-acceptance.md)에 별도로 기록한다.

## 구현 범위

| 작업 | 구현 | 보존 계약 |
|---|---|---|
| 일러스트 명암 | Screen/Add/Overlay 합성 | 기존 Normal/Multiply의 wire tag 유지, CPU/GPU 합성 비교 |
| 형태 수정 | 그룹 안 모든 래스터의 공통 affine 변형 | 고정 원본·공통 pivot, 원자적 확정·취소·Undo, 잠금 대상 전체 거절 |
| 자료 활용 | 참조창 드래그 스포이트 | release 때만 색 확정, 투명/여백/취소 보존, 최근 색은 획에서 갱신 |
| 크기 지정 출력 | 별도 축소 PNG | 원본 해상도·픽셀·Undo 및 기본 companion PNG를 바꾸지 않음 |
| 애니메이션 | 프레임 생성·복제·삭제·순서·시간, 재생·어니언 | 전체 cel 소유권을 문서에 유지, 선택한 cel만 편집, 같은 NTDR/복구 형식 |
| 게임 산출 | 고정 격자 스프라이트 시트와 JSON | 프레임 순서·시간·투명도 보존, 원본 read-only, onion 제외 |
| 합성 최적화 | 빈 타일 생략과 Normal 빠른 경로 | 세 fixture의 전후 raw RGBA SHA-256 동일 |

후속 작업에서 래스터 레이어 마스크와 fractional/feathered selection을 구현했다.
[마스크 계약](layer-mask-design.md), [조작](layer-masks.md),
[부드러운 선택](soft-selection.md)에 저장·입력·합성 경계를 기록했다.
애니메이션은 단일 트랙의 layered cel 방식이며 공유 배경 트랙·이름 구간·리깅·영상 출력은 없다.

## 자동 검사

- `cargo test --workspace --all-features --locked --quiet` 통과.
- 이후 전체 선택/마이그레이션 보완에 대해 `nyatidraw-editor`, `nyatidraw-project-redb`,
  `nyatidraw-project-web`, `nyatidraw-web-core`의 all-features 테스트를 다시 통과했다.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` 통과.
  외부 vendored Wry의 기존 lifetime 경고는 별도다.
- Web/Worker WASM all-targets Clippy `-D warnings` 및 DX release web build 통과.
- `cargo fmt --all -- --check` 통과.

로그: ignored `target/animation-workspace-tests.log`, `animation-core-final-tests.log`,
`animation-workspace-clippy-final.log`, `animation-wasm-clippy-final.log`,
`animation-web-build-final.log`, `animation-fmt-final.log`.

핵심 회귀 검사는 프레임 메타데이터만 바뀌는 동일 pixel-root 이력, 128개 이력 유지 후
재열기, 비활성 cel 보존, native/web checkpoint 왕복, 이전 형식 reader 거절,
마이그레이션 원본 byte 보존을 포함한다. UI 모형 테스트를 늘려 통과 수를 만들지 않았다.

## 네이티브·교차 호스트 확인

DX desktop debug 빌드와 실제 native actor의 저장→정상 종료→별도 프로세스 재열기,
두 셀/시간/전체 타일 원본 비교가 통과했다. 실제 창에서 프레임 버튼을 눌러 색과 시간
입력 동기화, 페이지 밖 투명 영역 보존을 확인했다. 상세 경계는
[네이티브 기록](native-animation.md)에 있다. 설치판을 바꾸지는 않았다.

웹에서 저장한 `group-animated.ntdr`를 새 Windows CLI로 읽어 2프레임 시트와
현재 프레임 PNG를 출력했다. 원본 파일 SHA-256은 출력 전후 같았고, 기존 출력 폴더에
다시 쓰려는 요청은 거절되며 기존 PNG도 그대로였다. CLI의 현재 프레임과 시트 두 칸은
웹의 그룹 변형 결과 및 원래 첫 프레임과 raw RGBA 단위로 정확히 일치했다.

- 현재 프레임 raw SHA-256: `11abcb5cd9c3ecb611f2cf7bb5bd6e10f5a96da3628e3eb5fb01a822c1e9d8cf`
- 시트 raw SHA-256: `a3eec0c000f2868cbd16a405f1cfac6b3373c26ff71ecd3f444c19a6a1c522e2`
- 증거: ignored `target/animation-cli-901952305e6a4dcaa83f0765dcd98a57/`.

최종 CPU/GPU 합성 비교는 RTX 3080/Vulkan/release에서 184가지 경우를 통과했다.
raw channel 최대 차이 0, 표시 변환 후 최대 차이 1이다.
로그 `target/animation-gpu-final.log`; 이는 물리 펜/HWND 표시 지연 증거가 아니다.
검증용 Edge·native 창과 8879 루프백 서버는 종료했다.

## 확인 중 발견해 고친 문제

- 그룹 변형 저장 fixture의 Windows 열린 파일 잠금을 수정하고 별도 프로세스 재열기를 확인했다.
- native 어니언 임시 LayerId를 새 실제 레이어가 재사용할 때 GPU 잔상이 남지 않게,
  구조 편집 전에 임시 어니언 surface를 버린다.
- 프레임 전환·Undo 뒤 시간 입력란이 이전 draft 값을 유지하던 것을 수정했다.
- 전체 선택이 할당된 타일의 투명 여백까지 포함해 채우기가 페이지 밖으로 늘어나던 것을
  페이지 + 실제 그림 범위 + 기존 선택으로 수정했다. signed 페이지 밖 그림은 보존한다.
- 반복 웹 시트 출력은 PNG/JSON 쌍마다 고유 이름과 일치하는 manifest 참조를 사용한다.
  브라우저의 두 번째 다운로드 차단 여부는 감지할 수 없으므로 완료를 과장하지 않는다.
- 마이그레이션 table 복사·digest 검증에 animation snapshot을 포함했다.

## 성능과 한계

CPU 측정의 하드웨어·OS·profile·전체 표본·p50/p95/p99는
[측정 기록](measurements/composite-sparse-2026-10-28.md)에 있다.
이것은 펜 지연·실제 표시 완료·대형 애니메이션 성능을 증명하지 않는다.
물리 펜, 다른 OS, 실제 게임 엔진 import, 설치/업데이트 경로는 이번 검사 범위 밖이다.

## 마스크·선택·UX 후속 검증

별도 R8 마스크, 기본 coverage 0/255, 원본 RGBA 보존, 연결 변형, 복제·삭제·Undo,
프레임 소유권과 typed persistence를 Windows/웹 공유 코어에 연결했다. 부드러운 선택은
브러시·채우기·보정·복사/잘라내기·변형에 적용하고, 기록된 획에 byte coverage를 저장한다.
이번 작업의 `cargo test --workspace --all-features --locked --quiet`는 통과했다
(`target/mask-workspace-tests.log`). 130개 이력 뒤의 보존과 별도 프로세스 저장소 재열기,
분수 coverage wire, signed/off-image 마스크 변형과 웹 복구의 출력 픽셀을 포함한다.

`gpu_shading`은 RTX 3080 / Vulkan / release에서 총 344 경우를 통과했다. 원시 합성의
최대 채널 차이는 0, 표시 색 변환 후 차이는 1이다 (`target/mask-gpu-shading.log`).
이는 마스크의 켜기/끄기·삭제·캐시 갱신·clipping·group·blend 비교이며 물리 입력이나
최초 표시 지연의 측정은 아니다.

별도 `gpu_selection_coverage`는 0/1/63/128/192/255 coverage와 겹치는 12개 dab,
round/pencil, paint/recolor/erase를 비교했다 (`target/selection-gpu-probe.log`).
0 영역과 잠긴 alpha는 정확히 보존했다. 이 fixture의 기존 무선택 pencil 누적도
CPU/GPU UNORM 반올림으로 최대 6 차이가 있었다. 부드러운 선택 결과는 같은 조건의
무선택 기준 + 1 또는 기존 4 중 큰 값을 넘지 않았다. 전체 프로젝트의 허용 오차 상수를
올려 통과시키지 않았다. pencil 최소 pigment를 선택 coverage와 곱하기 전에 양자화해
부분 선택에서 생기던 CPU/GPU erase 차이를 줄였다.

UX 변경은 프레임 방향키/Home/End, 시간 입력 Enter/Esc, 레이어 이름 F2/Enter/Esc,
변형 확정·취소, 이미지/마스크 대상 전환이다. 마스크에서 지원하지 않는 이미지 작업은
실행 전에 비활성화하고 이미지 대상으로 돌아가는 동작을 제공한다. 실제
웹 확인 결과는 [브라우저 기록](mask-web-acceptance.md)에 있다. 네이티브 창 결과는
아래 표시 문제의 별도 gate를 따른다. 현재 기록은 설치판 갱신이나
배포를 뜻하지 않는다.

공유 마스크 회색은 화면에서 고른 회색의 밝기를 coverage로 해석한다. 처음 실제 브라우저
검사에서 회색 128이 내부 선형 색상 55로 반영되던 문제를 발견해 수정했다. 검정/중간회색/
흰색과 alpha·opacity·기본 coverage 조합의 핵심 검사를 추가했다. 후속 집중 테스트는
paint-cpu 47개, project-web 17개, web-core 21개를 통과했다
(`target/mask-final-core-tests.log`).

Windows DX debug 앱의 별도 scratch 프로젝트에서 실제 actor를 통해 마스크 획, 중지·재사용,
삭제·Undo와 PNG 저장을 수행했다. 정상 종료 후 새 프로세스에서 source RGBA, R8 coverage,
tree/timeline, 비활성 cel과 합성 픽셀 proof가 일치했다. 위치는 ignored
`target/native-mask-c47ec288368346f09570764c09fe4a0d/`이며 사용자 그림은 사용하지 않았다.
이 단계에서 실제 화면의 재표시 문제를 발견했으므로 파일 재열기 통과를 표시 안정성 통과로
대체하지 않는다. 해당 수정 뒤 별도로 재검증한다.

웹에서 저장한 `feather-mask.ntdr`를 새 Windows CLI로 읽어 출력한 512×512 PNG는 웹에서
재열어 출력한 PNG와 raw RGBA가 정확히 같았다. `.ntdr` 파일의 출력 전후 SHA-256도 같다.
raw RGBA SHA-256: `3411bff5938e9966ad702c6d383a31d415b25209f84c8442c35ed8f783c54f89`.
증거: ignored `output/playwright/masks/feather-native-export.png`.

GPU의 같은 viewport를 반복 렌더하는 비교는 Dx12에서도 464 경우를 통과했다
(`target/mask-gpu-cached-dx12.log`, raw 최대 차이 0, 표시 차이 1). 이것만으로 native
창에서 관찰한 재표시 문제를 해결했다고 판단하지 않는다.


네이티브 마스크 표시 gate는 **미통과**다. 실제 창 캡처에서 메뉴를 열면 마스크 구멍이
사라지는 문제가 남았다. CPU 원본/복구 및 비동기 최종 swapchain 표본은 올바르지만,
FIFO·반복 입력 HWND 영역 설정 생략·Vulkan·`SWP_NOCOPYBITS` 실험 모두 해결하지
못했다. 실제 WebView DOM/PNG는 해당 지점이 투명함을 확인했다.
[진단과 실패한 실험](research/native-mask-presentation.md)에 증거와 다음 경계를 기록한다.
웹 마스크·feather 완료나 파일 재열기 통과로 이 네이티브 gate를 대신하지 않는다.


### 마스크 획 누적 작업량 보완 — 집중 검사 통과

native/web이 공유 `MaskStrokeBudget`으로 Begin부터 End flush/종료 봉인까지의
누적 dab 타일 작업량을 같은 CPU replay 한도로 입장 검사한다. 기본 한도는 단일
128×128 타일 방문65536회에 해당하며, 여러 타일에 걸친 dab은 그만큼 더 소비한다.
종전의 증분 미리보기 허용량과 획 전체 durable replay 한도 불일치를 수정했다.

초과 batch는 채택 전에 거절하고 현재 획 전체를 취소한다. 원본 RGBA/R8와 기존 이력은
보존하며 새 이력을 만들지 않는다. 늦은 End/close가 취소된 획을 저장 큐에 다시 넣거나
전역 workspace 실패로 바꾸지 않고, 다음 정상 Begin부터 다시 그릴 수 있어야 한다.
이 한도는 지연/성능 보장이 아니다.

추가한 다음 핵심 회귀 검사는 모두 **통과**했다 (`target/mask-budget-tests.log`).
해당 로그에서 세 테스트의 개별 `ok`를 확인했다. 네이티브 표시 안정성이나 새 웹 실사용
검사를 뜻하지 않는다.

- paint-cpu: `accepted_mask_batches_fit_replay_and_rejection_is_atomic`
- web-core: `mask_work_rejection_restores_original_and_keeps_history_usable`
- desktop: `mask_work_limit_cancels_before_durability_without_poisoning_the_workspace`


추가 opt-in CompositionVisual 실험도 idle 후 마스크 구멍이 사라지는 캡처를 재현해
제거했다. 기본 raw HWND swapchain 경로를 유지한다. 자세한 실험 범위와 제거한 코드는
[제거된 합성 실험](canvas-composition-trial.md)에 기록한다.

독립 캡처를 위한 Alt+PrintScreen은 기존 Greenshot의 캡처 overlay를 뜻하지 않게 열어
뒤의 일부 오래된 UI 캡처에 간섭했다. 해당 Greenshot을 정상 CLI `/exit`로 종료한 뒤
같은 앱을 다시 실행했다. overlay 정리 **이후에도** 합성 실험의 idle 뒤 마스크 표시
문제가 재현됐으므로 전체 문제를 Greenshot 탓으로 종결하지 않는다. 독립 OS 캡처와
물리 scanout 증거는 확보하지 못했으며, 네이티브 시각 작업 gate는 계속 미통과다.

### 최종 소스 동결 뒤 검사

누적 마스크 작업량 보완과 실패한 합성 실험 제거 후 다시 확인했다.

| 검사 | 결과와 로그 |
|---|---|
| `cargo fmt --all -- --check` | 통과 |
| workspace / all-targets / all-features Clippy | 통과, `target/mask-final-clippy.log`. 기존 vendor WRY lifetime 표기 경고 1개는 남아 있다 |
| paint-cpu와 web-core 전체 단위 검사 | 48개/22개 통과, `target/mask-final-budget-core.log` |
| desktop binary 핵심 검사 | 60개 통과, 별도 장치·registry 검사 5개 ignored, `target/mask-final-desktop-tests.log` |
| DX Windows WebView debug bundle | 통과, `target/mask-final-desktop-build.log` |
| DX web release와 recovery Worker | 통과, `target/mask-final-web-build.log` |
| web / worker WASM all-targets Clippy | 통과, `target/mask-final-wasm-clippy.log` |

새 bundle 빌드 통과는 이전 실제 브라우저 조작을 다시 실행했다는 뜻이 아니다.
최종 웹 entry는 `nyatidraw-web-dxh1aa9758c4fe2c798.js`, Worker entry는
`recovery-7ca0905be182c115ec28.js`다.
마지막 변경은 공유 마스크 입장 한도이며 해당 경계의 취소/복구는 위 핵심 검사로 확인했다.
검증용 native 프로세스는 정상 종료했고, writer drain/join과 GPU surface 퇴거 로그를
확인했다. 검증 창·브라우저·루프백 서버는 종료했으며 scratch 작품과 증거는 보존했다.
이번 작업에서 commit/push/tag, 설치본 교체 또는 배포는 하지 않았다.


### 마스크 대상·프레임 미리보기와 선택 채우기 — 후속 웹 검증

기존 레이어 행의 원본 이미지 옆에 실제 R8 coverage를 읽은 작은 흑백 마스크 미리보기를
추가했다. 선택 대상 테두리, 사용 중지 사선, 흰색 표시/검정 숨김 tooltip을 제공한다.
native/web이 기존 썸네일 캐시를 재사용하며 최대50×56 픽셀만 생성한다. 원본·이력·출력에
넣지 않는 UI 자료이며, native 마스크 획/속성 변경 뒤 소유 레이어의 썸네일도 갱신한다.

마스크의 **선택 전체 채우기**는 명시적 선택이 있고 대상이 표시·잠금 해제·idle일 때
사용할 수 있도록 연결했다. 마스크에서 bucket 경계 옵션은 숨기고, 클릭 채우기는 이미지
대상에서 사용하도록 안내한다. 공유 코어의 별도 원자적 마스크 채우기 경로를 사용한다.

새 실제 웹 검증에서 마스크 이미지가 버튼 높이를 넘던 문제를 발견해 수정했다.
수정 후 이미지와 부모 미리보기 영역 모두30×30으로 측정했고, 라벨은 아래의 별도 행에
배치했다. 모두 흰 마스크에 가짜 검정 여백이나 넘침이 없는 화면은
`output/playwright/masks/followup-mask-thumbnail-fixed.png`다. 앞선 텍스트 대상 UI
화면을 새 썸네일 증거로 재사용하지 않았다.

선택 채우기 결과를 `followup-filled.ntdr`로 저장한 뒤 브라우저 정상 종료→새 실행→
파일 열기를 수행했다. 전후512×512 출력 `followup-filled.png`와
`followup-file-reopened-export.png`의 raw RGBA SHA-256은 모두
`76cfb2f1509bbd2d26bbd7a8e05bf36b6ccb6b046c5a3c8cfaedc6c13fe1bab2`다.
Undo 출력 `followup-undo.png`는 채우기 전 원본 출력과 정확히 같은
`3411bff5938e9966ad702c6d383a31d415b25209f84c8442c35ed8f783c54f89`다.

실제 release 앱의 새 headless Edge 세션에서 두 프레임의36×36 미리보기를 확인했다.
웹의 `host_update` 연결 누락으로 처음에는 생성된 결과가 표시되지 않던 문제를
발견해 수정했다. 마스크를 켜면 첫 프레임만 바뀌고 두 번째는 유지됐으며, Undo는
첫 프레임 이미지를 정확히 복원했다. 시간240→350ms 변경은 라벨만 바꾸고 이미지는
유지했고, 타임라인 접기/펼치기 후 두 이미지가 다시 표시됐다. 증거는
`previews-refresh-fixed.txt`, `previews-enabled.txt`, `previews-undo.txt`,
`previews-duration.txt`, `previews-collapse-reopen.txt`다. 이 로그의 이미지 hash는
raw 픽셀이 아니라 data URI 문자열 SHA-256이다.

프레임 미리보기는 가시 프레임과 가까운 이웃 최대24개를 요청하고 별도 캐시에 최대64개만
유지한다. 웹은 main thread의 지연된 turn마다 한 프레임을 처리한다. 이는 요청/메모리
범위 제한이며 hard millisecond budget이나 대형 작품 성능 보장이 아니다.
[세부 계약과 증거](frame-previews.md)를 따른다. 후속 실제 조작에서 재생/정지 후
미리보기 복원, 정적 문서로 교체했을 때 프레임 목록이 비는 것, 원래 애니메이션을 다시
열었을 때 저장된 이미지 hash 복원, 브라우저 종료/새 실행 뒤 동일 결과를 확인했다.
`previews-stopped.txt`, `previews-project-replaced.txt`, `previews-replaced-back.txt`,
`previews-process-restarted.txt`에 기록했다. 같은 frame ID를 유지한 서로 다른
저장본의 교체 검증도 아래에 추가했다. 네이티브 창의 마스크 표시 문제는
[별도 미통과 gate](research/native-mask-presentation.md)로 남는다.
이번 후속 검증도 설치본 교체, push 또는 배포가 아니다.

### 30프레임 스크롤·네이티브 마스크 채우기 재열기

실제 release 웹에서 2프레임을 30프레임으로 복제하고 마지막 프레임까지 이동했다.
끝 지점은 프레임 30개, 준비된 이미지 15개, `scrollLeft=1872`, `scrollWidth=3312`,
`clientWidth=1440`이었다. 프레임 30을 클릭한 뒤 Home을 누르면 프레임 1이 선택되고
`scrollLeft=8`로 돌아왔다. 그 기록 시점에는 이미지 2개가 준비돼 있었으며 선택된
프레임의 이미지는 양쪽 지점 모두 로드됐다. 가시 요청 최대 24개 한도 안에서 동작했다.
30개를 동시에 렌더했다거나 대형 작품의 지연 한도를 검증했다는 뜻은 아니다.

증거는 `output/playwright/masks/frame-preview-scroll-result.txt`,
`frame-previews-scroll-end.png`, `frame-previews-scroll-start.png`다.
같은 32×32 애니메이션을 복제한 A/B 파일을 A→B→A 순서로 열었다. 프레임 ID는
그대로이며 A의 첫 프레임은 부분 마스크, B는 완전 숨김이다. 첫 미리보기 hash는
서로 달랐고 두 번째 파란 프레임은 같았으며, A 재열기 후 두 hash가 원래 값으로
복원됐다. `replace-a-previews.txt`, `replace-b-previews.txt`,
`replace-restored-previews.txt`와 [값 기록](frame-previews.md)을 따른다.
순차 교체의 실제 증거이며 모든 비동기 경쟁 조건을 검증했다는 뜻은 아니다.

실제 네이티브 scratch `fill-check.ntdr`에서 검정 마스크 채우기 후
`selected_pixels=1024`가 유지됐다. Undo/Redo는 분기 이력에서도 작동했고, 같은 소유
레이어에 마스크가 남아 있는 동안 편집 대상도 마스크로 유지됐다. PNG 저장과 정상
종료 뒤 별도 CLI 프로세스에서 다시 읽어 출력한 32×32 이미지는 완전 투명이었고,
저장한 PNG와 raw RGBA SHA-256이 정확히 같았다.

`ad7facb2586fc6e966c004d7d1d16b024f5805ff7cb47c7a85dabd8b48892ca7`

같은 파일을 새 GUI 프로세스로 열어 snapshot 116,
root `e684cc2a46b4c9ae086d3a2769d343ba`를 확인했다. 첫 프레임은 투명하게 표시됐고
두 번째 프레임의 파란 미리보기도 보였다. 이는 저장·재열기와 해당 조작의 증거다.
네이티브 키보드 포커스는 아래 후속 수정으로 확인했으며, 창 캡처는 물리 화면/펜의 증거가 아니다.
앞서 남은 [네이티브 표시 gate](research/native-mask-presentation.md)는 계속 미통과다.

### 최신 후속 검사 로그

| 검사 | 결과와 로그 |
|---|---|
| workspace / all-targets / all-features 전체 테스트 | 통과, `target/followup-final-workspace-tests.log` |
| workspace / all-targets / all-features Clippy | 통과, `target/followup-final-clippy.log` |
| WASM Clippy | 통과, `target/followup-final-wasm-clippy.log` |
| release 웹 및 recovery Worker 빌드 | 통과, `target/followup-final-web-build.log` |
| Windows native bundle 빌드 | 통과, `target/followup-final-desktop-build.log` |
| 후속 포커스 진단용 native 빌드 | 통과, `target/followup-focus-build.log` |

마지막 행은 진단용 빌드의 성공이며 포커스 문제 해결을 뜻하지 않는다. 이 후속 기록은
로컬 소스·scratch 작품·검증용 앱의 결과이고, push·배포·설치본 교체를 포함하지 않는다.

### 마스크 선택 후 단축키 포커스 복원

Windows에서 마스크 버튼 클릭 뒤 Ctrl+A가 동작하지 않는 문제를 재현했다. 제한된
Win32 포커스 로그에서 `MoveFocus` 성공을 확인했으며, 실제 WebView DOM 관찰에서는
Ctrl+A가 `BODY`에 도착했다. 편집기의 `main.app-shell` 밖에서 발생해 공유 단축키
핸들러로 전달되지 않았다. `native-key-observed.txt`에 입력 문자를 남기지 않고
키의 분류·수정키·대상 요소만 기록했다.

원인은 native 마스크 선택 명령도 비동기 artwork 큐를 거치면서 `edit.busy`가 켜지고,
포커스를 가진 마스크 버튼이 비활성화되는 것이었다. `MaskThumbnail.onclick`에서
명령을 보내기 전에 기존 `focus_editor_shortcuts()`로 편집기 루트에 포커스를 돌린다.
전역 키 재전송이나 Windows 키보드 우회 경로는 추가하지 않았다.

수정된 실제 Windows 앱에서 마스크 클릭→Ctrl+A로 선택 채우기/취소 버튼 활성화,
Esc로 선택 해제와 버튼 비활성화를 확인했다. 프레임 시간 입력란의 Ctrl+A는 숫자만
선택했고 캔버스를 선택하지 않았다. 100→175 입력 뒤 Esc는 100으로 복원했다.
동일한 웹 release UI 검사도 통과했다:
`output/playwright/masks/final-focus-acceptance-result.txt`.

수정 뒤 Windows bundle (`target/followup-focus-fixed-build.log`), workspace
Clippy (`target/followup-focus-fixed-clippy.log`), release 웹과 recovery Worker
(`target/followup-focus-fixed-web-build.log`), `cargo fmt --all -- --check`,
`git diff --check`가 통과했다. 앞선 전체 코어 테스트 후 변경은 opt-in 포커스 진단과
이 UI 포커스 복원뿐이다. 기존 vendor WRY lifetime 표기 경고는 별개로 남아 있다.

이 결과는 마스크 선택 단축키 문제의 해결이다. 메뉴 이후 캡처에서 마스크 구멍이
사라지는 별도 native presentation 문제나 실제 펜/물리 화면 검증의 완료가 아니다.

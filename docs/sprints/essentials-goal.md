# Essentials goal — 러프에서 기본 명암까지

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

기준: [일러스트 워크플로우 W0~W5](../illustration-workflow.md).
이 문서는 다음 자율 구현 범위를 세 묶음으로 제한한다. 기존 Sprint 1~3이나 W 번호를
재정의하지 않는다. 목표는 경쟁 앱의 기능 개수가 아니라 **형태 수정 → 밑색 → 명암**의
필수 조작을 실제 UI까지 연결하는 것이다. W1a/W1b 구현은 유지하고 다시 만들지 않는다.

**상태: E1~E3 구현·자동 검증 완료, 실제 창/물리 펜으로 그리는 수동 확인 대기.**
W0~W4 실사용 합격이나 릴리즈 승인을 의미하지 않는다. 아래 각 단계의 수동 목록을
기준 그림 한 장으로 확인하고, 새로운 고급 기능보다 발견된 작업 중단 문제를 먼저 처리한다.

## E1 — 그리기와 형태 수정 (W1 잔여 + W2 핵심)

- 끄기/강도 조절이 가능한 최소 선 보정. Begin/End/Cancel, 필압, 기존 재생 의미를 보존한다.
  보정으로 입력 유실을 숨기지 않으며, 보정으로 추가되는 끌림과 시스템 지연을 구분한다.
- 페이지 밖까지 사각/올가미 선택. signed 범위와 메모리 상한을 먼저 정의한다.
- 선택 교체/추가/빼기, 전체선택/반전/해제, 선택 픽셀 삭제.
  전체선택/반전의 유한 작업 범위를 명시하고 무한 dense mask를 만들지 않는다.
- 선택된 그림의 직접 이동·임의 각도 회전·크기 조절, 실제 preview, 확정/취소,
  선택 유지·재이동. 기존 붙여넣기도 같은 변형 경로를 재사용한다.
- 확대/회전의 alpha-aware 재샘플링을 제공한다. nearest-only 또는 90도 버튼만으로 완료하지 않는다.

완료 증거: 페이지 밖 팔/머리 조각을 선택·수정·취소·재수정하고 Undo/Redo 후 저장한다.
독립 프로세스 재열기에서 다른 픽셀/레이어와 반투명 가장자리가 보존되어야 한다.
UI 변형 preview는 원본을 누적 변형하지 않고 마지막 확정만 하나의 편집으로 만든다.

## E2 — 선화 아래 밑색 완성 (W3 핵심)

- 기존 참조 레이어 fill을 빈 채색 레이어에서 사용할 수 있게 검증·보완한다.
- 선택 확장/축소, alpha에서 선택 만들기. 연산은 E1의 signed/coverage 모델을 사용한다.
- 최소 틈 닫기, fill 확장과 AA 경계 처리. tolerance/틈 닫기/확장 값의 의미를 분리한다.
- 밑색의 작은 미채움 부분을 선택 추가/빼기와 브러시로 고칠 수 있어야 한다.

완료 증거: 가는 선/굵은 선/AA 선/작은 틈/인접 영역 fixture에서 누출과 흰 테두리를 확인한다.
밝고 어두운 배경으로 경계를 비교하고 저장·별도 프로세스 재열기·PNG 결과를 검증한다.
gap 기능은 모든 열린 선화를 자동 해결한다는 보장이 아니며 적용 한계를 명시한다.

## E3 — 실루엣을 보존하는 명암·재칠 (W4 핵심)

- 투명도 잠금: 기존 alpha를 보존한 재칠. 편집 잠금과 구별하고 지우개/fill/변형과의
  관계를 명시해 우회 편집으로 alpha가 바뀌지 않게 한다.
- 하위 레이어 클리핑: CSP형 사용 모델을 기준으로 연속 stack, 그룹 경계, 숨김,
  opacity, 순서 변경, base 삭제의 의미를 먼저 정한다.
- Normal/Multiply 합성. Screen/Add 및 나머지 blend mode는 이번 필수 범위에 넣지 않는다.
- CPU 출력·GPU 표시·스포이트·미리보기·Undo·저장 재열기의 합성 의미를 일치시킨다.

완료 증거: 머리색·선화색 재칠, 별도 그림자 레이어, base 수정에 따른 clipping 갱신을
검증한다. 선화/밑색/그림자 작업용 레이어는 사용자가 만드는 것이며 초기 세트를 추가하지 않는다.
scratch 작품의 저장 → 종료 → paired PNG 열기 → ntdr 재수정 경로까지 확인한다.

## 실행과 완료 판정

1. E1 → E2 → E3 순으로 진행한다. 독립적인 코어·UI·합성/저장 작업만 파일 소유권을
   나눠 병렬화하고, 같은 문서/선택 모델을 따로 중복 설계하지 않는다.
2. 각 묶음에서 데이터 모델/명령뿐 아니라 실제 UI 연결까지 구현한다. 짧은 ADR에
   의미와 실패 경계를 기록하고 핵심 시험·Clippy·release dx build·가능한 runtime acceptance를 수행한다.
3. 그림을 바꾸는 기능은 scratch 프로젝트에서 저장·프로세스 재시작·재열기 증거가 필요하다.
   자동 시험은 artwork/input/history/recovery 위험에 한정한다.
4. 물리 펜·실제 그림 완성·고주사율 검증을 수행할 수 없으면 미검증으로 기록한다.
   자율 구현 goal의 완료와 W0~W4의 실사용 합격은 별개이며, 코드/합성 입력만으로 후자를 선언하지 않는다.
   가능한 UI 검증은 수행하고 남은 수동 체크리스트를 전달한다.
5. 데이터 손상·입력 유실·크래시는 기능 작업보다 먼저 처리한다. 의미를 결정할 정보나
   새로운 권한이 반드시 필요하면 해당 경계에서 질문한다. 어려움만으로 기능 범위를 몰래 줄이지 않는다.
6. 각 묶음의 검증 결과와 남은 막힘을 이 문서에 추가한다. 세 묶음 완료 후 목표 밖 기능을 자동 추가하지 않는다.

## 이번 목표 밖

- 사용자 요청의 둘러싸서 채우기·미채움 보정·선 넘침 방지 브러시·삐져나온 선 정리는
  [워크플로우 후속 요구](../illustration-workflow.md#5-채색-생산성-후속-요구--계획-기록만-구현-보류)에
  계획만 기록했다. 구현·특허 검토 완료를 뜻하지 않으며 이 목표를 다시 열지 않는다.
- W5의 layer mask·색상 보정·정리용 병합, 자료 창, 고급 fill/선택 도우미.
- 질감/혼색/수채 브러시, 벡터, 텍스트, 애니메이션, PSD 호환, macOS/Linux, Godot addon.
- UI 전체 리디자인, 성능만을 위한 대규모 리팩터링, 무제한 히스토리 재설계.
- 설치본 교체, 버전/날짜 변경, 커밋/푸시/태그/릴리즈. 별도 요청이 있을 때만 수행한다.

## 진행 기록

- **E1a 기반 구현 및 자동 검증 완료, 실사용 확인 대기**: 최소 선 보정의 숫자/슬라이더(0=끄기), signed
  사각/올가미·추가/빼기·전체/반전/해제·선택 픽셀 삭제를 UI/worker에 연결했다.
  GPU 선택/지우개/overlay와 저장 스트로크도 음수 원점을 처리한다.
- E1b의 **임의 회전·alpha-aware 보간·원본 고정 preview·드래그 핸들·확정/취소·선택 유지·paste**를
  worker/actor와 UI에 연결했다. 아래 GPU 실행 증거와 실제 창 수동 확인을 구분한다.
  E1 구현/자동 검증은 진전했지만 W1/W2 실사용 gate를 완료로 판정하지 않는다.
  E2/E3까지 아래 통합 기록으로 진행했다. binary mask가 fractional AA까지 구현한 것은 아니다.
- W1a/W1b의 기존 자동 검증 기록은 [W1 문서](workflow-w1.md)에 있다.
  새 결정: [signed 선택](../decisions/ADR-0053-signed-selection.md),
  [최소 보정](../decisions/ADR-0054-minimal-stroke-smoothing.md).

### E1b 자유 변형 코어 — 최초 검증 기록

- [자유 변형 트랜잭션](../decisions/ADR-0055-free-transform-transaction.md):
  원본 snapshot에서 매번 재계산하고 세대/원본 불일치 시 확정을 거절한다.
  취소는 원본과 history를 바꾸지 않는다. 실패한 최신 preview 대신 이전 결과를 확정하지 않는다.
- 임의 각도·비균등 배율·반전·분수 이동과 premultiplied linear RGBA bilinear 보간을 제공한다.
  signed 선택, 다른 레이어/선택 밖 픽셀 보존, 작업 메모리 상한을 검증했다.
  fractional 선택 coverage와 area-filter 축소는 구현하지 않았다.
- scratch 변형 결과의 Undo/Redo·저장·독립 자식 프로세스 재열기·페이지 PNG 픽셀 일치 통과.
  반복 preview가 원본에 누적되지 않는 것도 검증했다. 실제 GPU preview/드래그 조작 증거는 아니다.
- 최신 `cargo test --workspace --all-features --locked`: **131 passed, 1 ignored**.
  자식 프로세스의 중복 test summary는 합계에서 제외했다.
- workspace Clippy `--all-targets --all-features --locked -- -D warnings`,
  `cargo fmt --all --check`, `git diff --check` 통과. 기존 vendored wry 경고는 남아 있다.
  이 코어 변경 뒤의 `dx` release build와 실제 UI acceptance는 아직 수행하지 않았다.
- 이 시점에 남았던 worker/actor·UI 연결은 다음 통합 기록에서 다룬다.

### E1b 실제 편집 경로 통합

- `TransformWorker`가 원본과 임시 붙여넣기 레이어를 소유하고, `transform_runtime`은
  표시용 snapshot만 GPU에 올린다. 미리보기로 durable CPU 타일·history·PNG를 바꾸지 않는다.
- 선택 이동 도구/변형 패널에서 시작한다. 내부 드래그=이동, 모서리=중심 기준 크기 조절,
  바깥 핸들=회전. 숫자는 소수 이동·축별 배율·임의 각도와 반전을 지원한다.
  숫자는 `미리보기` 후 `확정`, 캔버스 Enter=확정/Escape=취소다. 드래그 End는 확정이 아니다.
- 최신 변형 값 한 개만 worker 뒤에 대기한다. 드래그/미처리 preview가 남으면 확정을 거절한다.
  Cancel은 worker 실행 중에도 보류 가능하다. 입력 단절/viewport 변경은 해당 드래그를 되돌린다.
- 붙여넣기는 임시 레이어 생성→같은 변형 경로→단일 확정이다. 취소 시 임시 레이어도 없어진다.
  확정 후 선택을 유지하고 올가미로 복귀한다. 선택 이동/변형을 다시 선택하면 재변형할 수 있다.
- 일반 편집·도구/레이어/히스토리·Save/Save As는 먼저 확정/취소해야 한다.
  Close/프로젝트 전환은 미확정 preview를 버리고 마지막 확정 상태만 저장한다.
  Alt 임시 스포이트 입력이 변형을 움직이지 않게 차단했다.
- worker 핵심 시험: paste Cancel의 타일/트리/선택/ID 보존, preview 실패/구세대 확정 거절,
  기존 선택의 새 draft 취소, 단일 commit·Undo/Redo, 별도 프로세스의 정확한 root와 PNG 픽셀 일치.
- 최종 `cargo test --workspace --all-features --locked`: **133 passed, 2 ignored**.
  ignored는 기존 scratch-registry 시험과 명시 실행용 GPU acceptance다. GPU 시험은 아래처럼
  별도 실행해 통과했으며 이 합계에 중복 가산하지 않는다. workspace Clippy
  `--all-targets --all-features --locked -- -D warnings`, fmt/diff check도 통과했다.
- 실제 GPU acceptance: **RTX 3080, Windows, release, Vulkan/DX12 모두 통과**.
  실제 `ActiveCanvas`와 writer, 합성 native 샘플을 사용했다. GPU resident 타일 readback과
  preview 일치, Cancel 시 이전 타일 잔상 제거, CPU 원본 보존, 확정/재열기를 확인했다.
  실행: `WGPU_BACKEND=vulkan` 또는 `dx12`로 아래 ignored 시험을 명시 실행한다.
  `cargo test -p nyatidraw-desktop --release --all-features --locked native_canvas::transform_gpu_acceptance::actor_preview_cancel_commit_matches_gpu_and_durable_artwork -- --ignored --exact --nocapture`
  물리 펜/Windows HWND present/지연 p50·p95·p99 측정 증거는 아니다.
- `dx build --release --platform desktop` 성공. 설치본·버전·날짜·커밋·태그·배포는 변경하지 않았다.

### E1b 실제 창 조작 — 미검증

1. 페이지 밖 팔/머리를 선택하고 이동 도구의 내부·모서리·회전 핸들을 펜/마우스로 조작한다.
2. 회전/반전된 보기에서 드래그한 위치와 preview가 일치하는지 확인한다.
3. 숫자 입력→미리보기→확정, Escape 취소, 확정 후 보존된 선택의 재변형을 확인한다.
4. OS clipboard 붙여넣기의 임시 레이어→이동→취소/확정→Undo/Redo를 확인한다.
5. 미확정 상태의 Save/Save As 차단과 Close 시 미확정 폐기 의미를 확인한다.
6. preview 원본은 누적 보간하지 않지만 bilinear 대폭 축소 품질과 CPU 계산/전체 upload의
   반응성은 제한이 있다. 물리 펜·큰 작품의 사용성은 자동 시험으로 합격 처리하지 않는다.

후속 E2/E3 구현과 검증 결과는 아래에 이어진다.

### E1a 실행 증거

- `cargo test --workspace --all-features --locked`: **127 passed, 1 ignored**.
  scratch-registry acceptance만 기존대로 제외했다. UI mock 시험은 추가하지 않았다.
- 보정된 live dab 명령과 저장 sample의 CPU 재생 dab가 일치하고, 별도 프로세스에서
  sample/content root/PNG 디코딩 픽셀을 재확인했다. 실제 GPU 표시 지연 증거는 아니다.
- worker에서 페이지 밖 선택 조합 → 칠하기 → 부분 삭제 → 재칠하기 → 저장 후 별도
  프로세스의 픽셀/페이지 PNG 일치 확인. 선택-only 명령은 artwork history를 만들지 않는다.
- signed 선택 스트로크의 기존 origin-zero wire/hash 보존, origin 변조 거절, capability
  설정의 원자성/metadata upgrade 유지 및 별도 프로세스 replay/reopen 통과.
- release `gpu_selected_stroke`: RTX 3080의 **Vulkan/DX12 각각 8개 경우** 통과.
  원점 0/음수 원점 × brush/eraser × 빈/유효 선택. 선택 내부 최대 차이 brush 2/255,
  eraser 1/255; 선택 외부와 Cancel은 정확히 일치. signed overlay 위치와 원점 0 잔상도 확인했다.
  Windows 11, Ryzen 7 5800X3D. p50/p95/p99 지연·물리 펜·다른 OS를 측정한 것은 아니다.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`,
  `cargo fmt --all --check`, `git diff --check`: 통과. vendored wry lifetime 경고는 남아 있다.
- `apps/desktop`에서 `dx build --release --platform desktop`: 성공, CSS asset 복사 확인.
  산출물은 `target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe`.
  vendored framework의 기존 경고는 남아 있다. 앱 창 실사용·설치본 교체는 하지 않았다.
  버전/날짜/커밋/태그/배포는 변경하지 않았다.

### E1a 실제 조작 — 미검증

1. 보정 0/중간/100으로 필압 선을 긋고 끌림/끝맺음을 비교한다. 도구 전환/펜 뒤집기 후
   기억한 값과 숫자 입력이 맞는지 확인한다. 강한 보정의 마지막 직선 연결은 알려진 한계다.
2. 페이지 밖 팔을 사각/올가미로 선택하고 추가/빼기, 브러시/지우개/삭제를 사용한다.
   반전·회전된 보기에서도 선택 테두리와 실제 제한 픽셀이 맞아야 한다.
3. 전체/반전의 범위와 초과 오류, Ctrl+A/Ctrl+D/Ctrl+Shift+I/Delete를 확인한다.
   텍스트 입력의 동일 키는 artwork 명령으로 전달되면 안 된다.
4. 완성한 scratch를 저장/닫기/paired PNG 열기로 확인한다. 바깥 그림은 ntdr에 남고
   페이지 밖 픽셀은 PNG와 레이어 미리보기에 포함되지 않아야 한다.

### E2 밑색 편집 통합 — 구현/자동 검증 완료, 실사용 확인 대기

- [ADR-0056](../decisions/ADR-0056-flat-fill-and-selection-morphology.md): 빈 채색 레이어에서
  참조 선화의 연결 영역을 채운다. 선택이 있을 때 전체 선택을 칠하던 우회를 제거하고
  선택 외부/구멍을 탐색과 확장의 통과 금지 영역으로 처리했다.
- 허용 오차와 틈 닫기 0~8, 채우기 확장 0~64, AA 경계 받침을 분리해 UI/actor/worker에
  연결했다. 항목별 설정 명령으로 빠른 연속 입력이 다른 항목을 오래된 값으로 덮지 않는다.
  E2 숫자 입력은 편집 중 빈 문자열을 허용하고 focus를 벗어날 때 정규화한다.
- 틈 닫기는 사각 closing 반경이며 모든 열린 선을 해결하는 기능은 아니다. 확장은
  barrier-only 맨해튼 거리다. AA는 최소 1픽셀 받침(`max(expand, AA1)`)이며 fractional
  경계 복원이 아니다. 반투명 한 픽셀 선의 받쳐진 구간이 불투명해질 수 있다.
- 선택 경계의 확장/축소는 Euclidean 원판 반경을 사용한다. `그림에서 선택`은 현재
  raster raw alpha > 0이며 opacity/숨김과 독립적이다. 결과는 binary coverage다.
- 참조 선·다른 레이어·signed 바깥 그림 보존, 선택 clipping, selection-only history 불변,
  명시적 분기 Undo/Redo, 저장→별도 자식 프로세스 재열기→root·PNG 픽셀 일치 통과.
  참조 fixture의 가는/굵은/AA 선, 작은 틈, 인접 영역, 밝고 어두운 배경을 코어에서 검증했다.
- GPU 검증에서 작은 페이지의 경계 타일에 page/sparse 합성 캐시가 같은 유효 표시를
  공유하는 기존 결함을 발견해 분리했다. 페이지 안쪽 갱신이 바깥의 오래된 결과를
  유효하게 만들지 않는다. 두 캐시는 artwork/tree/Solo 변경에 함께 무효화된다.
- 실제 actor/worker GPU acceptance: **RTX 3080, Windows, release, Vulkan/DX12 통과**.
  32px 페이지와 128px 타일을 사용해 페이지 경계를 가로지르는 밑색을 검증했다.
  안쪽/바깥쪽 display texture 픽셀, resident raster readback, Undo 잔상 제거·Redo,
  명시 Save→PNG decoded pixels, 새 actor의 DB 재열기·GPU 재구성을 확인했다.
  독립 프로세스의 재열기는 별도 CPU worker 통합 시험에서 수행한다.
  실제 HWND present/물리 펜/지연 p50·p95·p99 증거는 아니다.
  실행: `WGPU_BACKEND=vulkan` 또는 `dx12`와 아래 시험을 사용한다.
  `cargo test -p nyatidraw-desktop --release --all-features --locked native_canvas::flat_fill_gpu_acceptance::actor_reference_fill_undo_redo_matches_gpu_and_reopened_artwork -- --ignored --exact --nocapture`
- `cargo test --workspace --all-features --locked`: **139 passed, 3 ignored**.
  자식 프로세스 summary는 중복 합산하지 않는다. ignored는 기존 registry와 E1/E2 GPU
  명시 실행 시험이며 E2 GPU는 위 두 backend에서 별도로 통과했다.
- workspace Clippy `--all-targets --all-features --locked -- -D warnings`, fmt/diff check 통과.
  기존 `gpu_layer_viewport` release/Vulkan도 통과해 그룹·부분 무효화·삭제/복원·보기·Solo를
  확인했다. E1 자유 변형 actor GPU 시험도 cache 수정 후 release/DX12 재실행 통과.
  vendored framework의 기존 경고는 남아 있다.
- `apps/desktop`에서 `dx build --release --platform desktop` 성공, CSS asset 복사 확인.
  설치본/버전/날짜/커밋/태그/배포는 변경하지 않았다.

### E2 실제 조작 — 미검증

1. 선화 레이어에 참조를 표시하고 그 아래 빈 raster를 만들어 참조 범위로 채운다.
2. 닫힌 선/작은 틈/AA 선에 tolerance·틈 닫기·확장을 각각 바꿔 누출과 경계를 확인한다.
3. 선택 추가/빼기로 영역을 제한하고 fill, alpha 선택, 확장/축소 후 brush로 미채움을 고친다.
4. 밝고 어두운 배경에서 한 픽셀 반투명 선의 받침 한계와 선 바깥 보존을 확인한다.
5. scratch를 저장/닫고 paired PNG로 다시 열어 수정한다. 실제 작품·펜·큰 그림 반응성을
   자동 fixture 통과로 대신하지 않는다.

### E3 착수 순서

1. raster metadata와 구버전 기본값, 하위 clipping stack/그룹 경계/숨김/opacity/순서·삭제
   의미를 고정한다. 편집 잠금과 alpha lock을 혼동하지 않는다.
2. 공용 타입을 고정한 뒤 CPU 합성·picker·preview, GPU 합성, alpha-lock stroke/replay를
   파일 소유권별로 나눠 진행한다. Multiply에는 투명 backdrop 항도 필요하다.
3. UI/actor와 fill·gradient·삭제·cut·변형의 alpha-lock 우회 경로를 통합한다.
4. 저장 재열기·PNG·GPU 일치와 base 변경/순서/삭제/반투명 경계를 검증한다.
   E3 구현은 아래 진행 기록으로 이어간다.

### E3 명암·재칠 통합 — 구현/자동 검증 완료, 실사용 확인 대기

- [ADR-0057](../decisions/ADR-0057-shading-compositing-contract.md)에 alpha-lock 우회 편집,
  원래 형제 순서의 clipping stack, 그룹/base/opacity/숨김/삭제와 Solo·참조 의존성을 정의했다.
- raster alpha lock와 raster/group clip·Normal/Multiply 속성, layer wire v3·기존 v1/v2 기본값을
  추가했다. metadata/history와 capability 0x800을 원자적으로 저장하고 과거 cursor에서도
  gate를 확인한다. metadata/model/wire scoped 31개 시험과 Clippy 통과.
- layer UI와 actor 명령, Begin의 alpha-lock 고정·별도 GPU painter·worker 재검사를 연결했다.
  stroke wire `NYALP001`/0x1000은 기존 unlocked hash/wire를 유지하며 새 replay 의미를 저장한다.
  CPU/GPU stack, picker/preview/reference fill은 공용 형제 관계를 사용한다.
- release `gpu_shading`: RTX 3080, Windows, Vulkan/DX12 **각각 40개 경우 통과**.
  raw group 합성은 CPU와 정확히 일치하고 display 최대 차이는 1/255다. 32/128px 페이지,
  signed 좌표, 반투명 base, 여러 clip, 그룹, 숨김/opacity/순서/삭제/Undo 복원 상당의 tree
  교체와 dirty base, Solo를 검사했다. 실제 편집 actor의 Undo 증거는 별도다.
- release `gpu_alpha_lock`: 같은 GPU/backend에서 **각각 4개 경우 통과**. hard/soft와
  signed 선택/무선택, alpha 0/1/64/128/255를 검사했다. alpha는 정확히 보존하고 RGB는
  CPU 기준 최대 2/255 차이다. Cancel 원복과 동일 Commit은 정확히 일치한다.
- 실제 edit worker의 투명도 잠금 fill/선택 칠/gradient, 삭제/cut/변형 거절, Undo/Redo,
  별도 프로세스의 metadata/root/alpha/PNG 재열기 시험 통과.
- 독립 감사에서 Solo가 필요한 base 그룹 내부의 숨긴 자식까지 표시할 수 있는 경우를
  발견해 수정했다. 대상/조상 경로와 문맥용 자식을 구분하며, Solo 그룹/root/외부 clip은
  숨긴 내부 base를 드러내지 않고 내부 clip 자체를 Solo했을 때만 필요한 base를 표시한다.
  핵심 불변식과 Vulkan/DX12 40개 합성 경우 재실행 통과.
- 실제 actor GPU acceptance는 **release, RTX 3080, Windows, Vulkan/DX12 모두 통과**.
  UI와 같은 alpha lock/clip/Multiply 명령, native 입력 Begin 고정, stroke 중 metadata 변경
  거절, 다른 레이어/alpha 보존, GPU closed tile·그룹 합성·스포이트, Undo/Redo를 검사했다.
  펜 뒤집기 지우개 거절은 synthetic sample로 검증하며 실제 물리 장치 증거는 아니다.
- Save→writer 종료→별도 프로세스의 **paired PNG 경로 해석→ntdr 재열기→재칠→Undo/Redo→
  재저장/재열기** 통과. raw page 밖 그림과 속성이 보존되고 PNG는 페이지만 포함한다.
  PNG 기대값은 8bit preview의 재인코딩이 아니라 원래 linear RGBA8 페이지와 정확히 비교한다.
  실제 export는 기존 16bit sRGB 경로를 유지하며 허용 오차로 결과를 숨기지 않는다.
  실행: `WGPU_BACKEND=vulkan` 또는 `dx12`에서
  `cargo test -p nyatidraw-desktop --release --all-features --locked native_canvas::shading_gpu_acceptance::actor_shading_alpha_stroke_history_and_fresh_reopen_match -- --ignored --exact --nocapture`.
- 최종 `cargo test --workspace --all-features --locked`: **149 passed, 4 ignored**.
  자식 프로세스 결과는 중복 합산하지 않는다. ignored는 기존 scratch registry와 E1/E2/E3
  GPU 명시 실행 시험이며 GPU는 별도로 수행했다. 새 합성 뒤 기존 E1 actor/DX12,
  E2 actor/Vulkan 및 `gpu_layer_viewport`/Vulkan 회귀 검사도 통과했다.
- workspace Clippy `--all-targets --all-features --locked -- -D warnings`, fmt/diff check,
  `dx build --release --platform desktop` 통과. vendored framework의 기존 경고는 남아 있다.
  실행 산출물: `target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe`.
- HWND present·물리 펜·입력 지연 p50/p95/p99·완성 작품 제작은 미검증이다. 현재 제공된
  UI 제어 도구로 native 창 검증을 수행하지 못했으므로 아래 수동 목록을 남긴다.
  설치본/버전/날짜/커밋/푸시/태그/배포는 변경하지 않았다. E1~E3 범위 밖 자동 확장은 하지 않는다.

### E3 실제 조작 — 미검증

1. 반투명 가장자리가 있는 머리색/선화 raster에서 `α`를 켜고 붓·fill·gradient로 재칠한다.
   지우개(펜 뒤집기 포함)/삭제/cut/변형은 명시적으로 거절되고, 잠금을 풀면 다시 가능해야 한다.
2. 사용자 생성 밑색 위에 그림자 raster를 만들고 `↳`와 `Multiply`를 켠다. 두 번째 clip도
   직전 clip이 아닌 같은 밑색을 base로 쓰는지 확인한다. 초기 Ink/배경 세트는 만들지 않는다.
3. base 숨김/opacity/순서/삭제·복원과 그룹 경계를 조작한다. clipped Solo는 base 문맥을
   포함하지만 문맥용 그룹의 숨긴 자식을 드러내면 안 된다. raw raster 썸네일은 clip 출력이 아니다.
4. 페이지 밖 붓/선택을 포함한 scratch를 Save→닫기→paired PNG 열기로 다시 연다.
   레이어 속성·Undo/Redo·PNG 출력과 재칠을 확인한다. export는 페이지 안쪽만 포함한다.
5. 화면/내비게이터/스포이트의 같은 색을 비교한다. 체크보드와 페이지 테두리는 그림 색이 아니다.
   실제 HWND·물리 펜·완성 작품 제작은 GPU readback이나 합성 sample 시험으로 대신하지 않는다.

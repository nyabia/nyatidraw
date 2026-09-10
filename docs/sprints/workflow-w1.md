# W1 — 기본 드로잉 조작 구현

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

상위 기준: [일러스트 워크플로우](../illustration-workflow.md).
W0의 실제 펜·그림 완성 실습은 미검증이다. 코드 감사에서 확인한 명확한 막힘부터 수정하되,
구현 및 자동 검증을 실사용 합격으로 기록하지 않는다.

## 이번 구현 묶음 W1a

- 크기 필압과 불투명도 필압의 독립 제어 및 최소값.
- 같은 원형 엔진 안에서 hard/soft 경계를 구분하고 CPU/GPU 결과를 일치시킨다.
- 도구별 크기·불투명도·필압·경도 설정을 세션 중 기억한다.
- `[` / `]`로 actor의 현재 크기를 상대 조절한다. 숫자 입력의 키는 캔버스 단축키로 넘기지 않는다.
- 실제 도구 속성 UI에서 설정하고, 스트로크 시작 시 불변 preset으로 고정한다.
- 기존 저장 스트로크의 재생 의미를 보존하고 신규 preset 저장·재열기를 확인한다.

병렬 작업은 brush/paint/persistence와 editor/protocol/session 경계로 분리한다.
감독은 UI 연결, 변경 검토, 통합 검증과 이 기록을 담당한다. 기존 미커밋 변경은 보존한다.
레이어 초기 세트, 단축키 그룹, 문서 픽셀/페이지 의미는 변경하지 않는다.

## W1의 남은 항목

- W1b 임시 스포이트·보기 반전의 실제 조작 확인; 펜 중심 빠른 크기 조절 동작의 실사용 확인.
- E1a의 최소 보정(끄기 포함) 구현 후 실제 끌림·끝맺음 확인. 시스템 지연과 구별한다.
- 실제 펜과 마우스로 러프·선화·부드러운 덧칠, 저장/재시작/PNG 확인.

## 이번 구현 묶음 W1b

- Alt를 누른 상태로 캔버스를 클릭/펜 접촉하면 보이는 그림에서 일회성 색 채취를 한다.
  Begin에서 의도를 고정하고 중간에 Alt를 놓아도 페인트로 바뀌지 않게 한다.
  선택 도구 자체는 바꾸지 않아 종료/취소/포커스 손실 뒤 복귀 명령이 유실될 일이 없게 한다.
- 상단 보기 툴바에 좌우 반전 토글, 기존 각도 표시에 회전 초기화 버튼을 연결한다.
  문서 가로축 반전 후 회전 순서이며 현재 화면 중심의 문서 지점을 유지한다.
- input 역변환, GPU affine, navigator와 선택 overlay를 같은 좌표식으로 연결한다.
  문서 픽셀/Undo/PNG는 보기 조작으로 변경하지 않는다. 화면 맞춤/1:1은 회전과 반전을 초기화한다.
- 보정 알고리즘은 이 묶음 밖이다. 기본 동작의 좌표·입력 안전성부터 확인한다.

상태: W1b 구현 연결 및 핵심 시험 완료. 실제 펜/UI acceptance 전이며 설치·배포하지 않는다.

### W1b 구현 결과와 검증

- 임시 색 채취 End에서 producer가 읽기 장벽을 세운다. `(device_id, End sequence)` 소유권을
  확인해 해제하므로 다른 장치·이전 취소가 새 요청의 장벽을 해제하지 않는다. 이미 승인한 입력은
  버리지 않으며, 결과 전에 시작하려는 새 접촉은 승인하지 않는다. 따라서 아주 빠른 연속 접촉은
  결과를 받은 뒤 다시 시작해야 할 수 있다. 실제 지연과 사용감은 미측정이다.
- 이전 materialization/export 대기 중 읽기는 한 슬롯에 보존해 재시도하고, 최초 Solo를 유지한다.
  읽기가 대기 중이면 명시적 색 변경도 직렬화한다. 종료는 저장할 픽셀 작업과 임시 UI 읽기를 구분한다.
- 중첩 그룹 Solo의 하위 그림 누락을 수정했다. 저장된 가시성이나 PNG 구성을 바꾸지 않는다.
- `cargo test --workspace --all-features --locked`: **120 passed, 1 ignored**.
  마지막 취소 경계 수정 및 임시 display-read 저장 시험 확장 후 desktop 재실행도
  **41 passed, 1 ignored**. 제외 항목은 기존 Windows scratch-registry acceptance다.
- 기존 색 채취 핵심 시험에 임시 `PickDisplayColor` 경로를 포함했다. 원본/선택/history 불변,
  채취 색으로 칠한 결과를 저장하고 **별도 프로세스 재열기·PNG 바이트 일치**를 확인했다.
- release `gpu_layer_viewport`: mirror + rotation, HiDPI checker, 반전 해제 원본 보존,
  중첩 그룹 Solo 및 숨긴 자식 제외 통과. `gpu_signed_workspace`: 반전/회전된 음수 좌표 통과.
  환경: Windows 11 Pro, Ryzen 7 5800X3D, RTX 3080, Vulkan, release.
  픽셀 검증이며 p50/p95/p99 지연, 물리 펜, 다른 OS 증거가 아니다.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: 통과.
  vendored wry의 기존 lifetime 표기 경고는 별도로 남아 있다.
- `apps/desktop`의 `dx build --release --platform desktop`: 성공, CSS 복사 포함.
  산출물은 `target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe`.
  vendored framework의 기존 경고는 남아 있다. 앱 실행·설치본 갱신·태그·배포는 하지 않았다.

결정 근거: [ADR-0052](../decisions/ADR-0052-view-and-temporary-picker.md).

### W1b 실제 조작 체크리스트 — 미실행

1. 서로 다른 색 두 개를 그리고 새 빈 레이어를 만든다. Alt+접촉으로 밑의 색을 가져와
   원래 도구·크기·불투명도를 유지한 채 그린다. 완전 투명 지점은 현재 색을 유지해야 한다.
2. 잠긴 레이어, 그룹 Solo, 그 안의 중첩 그룹에서도 표시된 그림 색만 채취한다.
   Solo 해제와 PNG 출력의 원래 레이어 구성은 변하지 않아야 한다.
3. Alt를 접촉 도중 놓거나, 접촉 중 다른 앱으로 전환한 뒤 돌아온다. 의도하지 않은 선이
   생기거나 스포이트가 고정되지 않아야 한다. 새 접촉은 새 Alt 상태를 따른다.
4. 색 채취 완료 전에 다시 그리려는 입력을 포함해 빠르게 반복한다. 읽기 전에 이전 색으로
   그리지 않아야 하며, 입력 승인 대기와 실제 지연은 물리 펜에서 별도로 평가한다.
5. 캔버스 밖 팔/머리를 포함한 그림을 좌우 반전하고 회전·확대·팬·navigator 이동한다.
   펜 위치/선택 표시가 그림과 일치하고, 각도를 눌러 초기화해도 중앙의 작업 지점이 유지되어야 한다.
6. 반전 상태에서 저장/닫기/PNG 재열기를 한다. 보기 반전이 원본 픽셀이나 PNG를 뒤집으면 안 된다.
   화면 맞춤/1:1은 반전·회전을 해제한다.

## 검증 및 중단 경계

핵심 시험은 브러시 결정성, 기존 포맷 호환, CPU/GPU 픽셀 일치와 저장 복구에 한정한다.
UI는 컴파일/Clippy 및 가능한 실제 조작으로 확인한다. 이번 작업은 로컬 구현이며
버전·태그·시간 변경, 설치본 교체, 커밋·푸시·배포는 하지 않는다.

## W1a 구현 결과

- 크기/농도 필압 독립 토글, 각각의 최소 비율, 경도 숫자·슬라이더를 도구 속성에 연결했다.
- Pen/Eraser hard, Pencil 중간 경도·농도 필압, Brush soft 기본값을 사용한다.
  soft preset은 세부 도구에 `소프트 브러시`로 표시한다. 네 도구의 설정 기억은 세션 한정이다.
- Begin preset 고정 및 펜 뒤집기 시 기억된 Eraser 복사본 사용을 구현했다. 원래 선택 도구와
  다음 그리기 설정은 바꾸지 않는다. 선택/fill/picker에서 뒤집기 override는 아직 없다.
- `[`/`]` 상대 크기 조절은 현재 도구를 명시한다. 같은 도구의 연타만 안전하게 revision을
  재평가하며 다른 도구/미래 revision은 거절한다. 실제 키 연타 조작은 acceptance 대기다.
- 기존 engine1과 신규 engine2의 재생/wire/hash를 분리하고, 신규 저장소 capability bit를
  레이어/page 변경과 Undo에서도 보존한다. [ADR-0051](../decisions/ADR-0051-basic-brush-controls.md).

### 실행한 검증

- `cargo test --workspace --all-features --locked`: **117 passed, 1 ignored**.
  새 시험은 필압/legacy 재생, wire/root 보존, 저장 capability, 뒤집기 snapshot의 핵심 위험에 한정했다.
  제외된 시험은 명시적 Windows scratch-registry acceptance이다.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`: 통과.
  vendored wry의 기존 lifetime 표기 경고는 별도로 남아 있다.
- `cargo fmt --all --check`, `git diff --check`: 통과.
- release `gpu_cpu_diff`: hard/soft paint 8 fixture + hard/soft eraser 통과.
  기존 허용 오차 채널 4/255, 허용 초과 픽셀 0. soft edge 최대 1, dense soft 최대 4,
  eraser 최대 2였다. **bit-exact GPU/CPU 일치는 아니며** 기존 UNORM 기준 안에 든다.
  환경: Windows 11 Pro 10.0.26200, Ryzen 7 5800X3D, RTX 3080, Vulkan, release.
  이는 픽셀 비교이며 지연 p50/p95/p99 측정은 하지 않았다.
- release CLI `diagnostic-smoke`: engine2 soft preset 저장 후 **별도 validate/export 프로세스**로
  재열기 성공. 저장 전 픽셀과 PPM 출력 바이트 일치. 사용자 artwork가 아닌 자체 scratch 사용.
  새 soft preset의 실제 앱 PNG/펜 조작 acceptance를 대체하지 않는다.
- `apps/desktop`에서 `dx build --release --platform desktop`: 성공, CSS asset 복사 확인.
  산출물은 `target/dx/nyatidraw-desktop/release/windows/app/nyatidraw-desktop.exe`.
  vendored framework 경고는 남아 있으며 새 앱 창은 이 검증에서 실행하지 않았다.

### 남은 판정

실제 앱에서 새 속성 입력·도구 전환·물리 펜
굵기/농도·soft 경계·PNG 출력, W0 기준 그림 실습은 미검증이다.
W1a 코드는 연결했지만 **W0 또는 W1 전체를 완료로 판정하지 않는다**.
설치본·태그·공개 릴리즈는 변경하지 않았다.

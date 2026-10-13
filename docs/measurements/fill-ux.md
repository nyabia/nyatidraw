# 채우기 UX 검증

범위: [채우기 작업 흐름](../fill-workflow.md)의 세부도구, 독립 설정, 경계 조절,
선택 전체 채우기와 오류 안내. 사용자 작품이나 설치본은 변경하지 않았다.

## 환경과 빌드

- Windows 11 Pro, build 26200.
- Windows GPU acceptance: NVIDIA GeForce RTX 3080, DirectX 12,
  driver 32.0.15.9621, release 프로필.
- 웹: 격리된 Microsoft Edge 프로필, 1440×960, 로컬 정적 배포의 release WASM 및 Worker.
- Dioxus CLI를 사용하는 `tools/build-web.ps1 -BasePath /` 및 `tools/build-dev.ps1` 성공.
- `tools/build-docs.ps1` 성공. 채우기 가이드를 포함한 8개 페이지와 검색 인덱스 생성.
- 성능 벤치마크가 아니므로 p50/p95/p99 또는 입력 지연 개선을 주장하지 않는다.

## 코어 및 저장 검증

다음 명령의 63개 검사 통과:

```powershell
cargo test -p nyatidraw-paint-cpu -p nyatidraw-editor -p nyatidraw-project -p nyatidraw-project-web -p nyatidraw-web-core --locked
```

기존 경계/틈 닫기/선택 clip/자원 상한/원본 선 보존 검사와 새 독립 설정 및 이전
`NYTOOL01`·`NYWORK03` 복원 검사를 포함한다. 채우기와 마법봉의 참조/허용 오차가
서로 덮이지 않는지 검사했다. UI 배치 전용 자동 테스트는 추가하지 않았다.

추가로 다음 release acceptance를 각각 통과했다:

```powershell
cargo test -p nyatidraw-desktop --release --locked reference_fill_selection_undo_and_fresh_reopen_preserve_artwork -- --nocapture
$env:WGPU_BACKEND='dx12'
cargo test -p nyatidraw-desktop --release --locked actor_reference_fill_undo_redo_matches_gpu_and_reopened_artwork -- --ignored --nocapture
```

- 첫 검사는 별도 프로세스에서 scratch 프로젝트를 다시 열어 픽셀을 대조한다.
- 두 번째는 실제 desktop actor/작업 큐, GPU readback, Undo/Redo, 저장 및 repository/actor
  재생성을 검증한다. CPU 결과와 GPU 결과 및 다시 연 작품이 일치했다.
- GPU 제출/읽기 검증은 창의 첫 표시 픽셀이나 물리 펜 성능을 증명하지 않는다.
- `cargo fmt --all -- --check`, 변경 diff 공백 검사, 변경 코어/UI/web-core의
  all-targets Clippy `-D warnings` 통과. desktop/web 호스트 컴파일도 통과했다.

## 실제 브라우저 조작

Playwright CLI로 별도의 Edge 프로필에서 실제 버튼, 숫자 입력, 슬라이더와 마우스
캔버스 입력을 사용했다. 프로그램 내부 상태를 직접 주입한 UI 테스트가 아니다.

1. 기본 검은 펜으로 닫힌 사각 선화를 그렸다. 새 래스터를 만들고 선화 아래로 이동했다.
2. 채우기를 선택하면 '다른 레이어도 참조', 허용 오차 16이 표시됐다. 붉은색을 골라
   내부를 클릭하자 빈 밑색 레이어의 내부만 채워지고 선화와 바깥 투명 영역은 보존됐다.
3. 채우기 오차를 37로 변경했다. 마법봉의 오차를 97로 바꿨다가 돌아와도 채우기는
   다른 레이어 참조/37을 유지했다.
4. `.ntdr`와 PNG를 내려받았다. 브라우저 프로세스를 종료하고 새 프로세스에서 `.ntdr`를
   가져왔다. 그림과 채우기 설정이 유지됐고 다시 내보낸 PNG의 SHA-256도 같았다.
5. 참조 레이어를 지정하지 않고 '참조 레이어만'을 선택하면 속성에 해결 방법이 나타났다.
   캔버스를 클릭해도 작품을 바꾸지 않고 비치명적 안내를 표시했다.
6. 선 안팎에 걸친 사각 선택을 만든 뒤 '선택 전체 채우기'로 흰색을 칠했다. 연결 영역
   버킷과 달리 선택 전체가 채워졌고 선택과 별도 선화 레이어는 유지됐다. 한 번 Undo 후
   PNG가 최초 출력과 같은 해시로 돌아왔다.
7. 틈 닫기 슬라이더를 0에서 4로 드래그하면 숫자도 즉시 4로 갱신됐다. 좁은 속성 패널은
   가로폭을 모두 쓰고 필요한 경우 내부 스크롤로 하단 명령에 접근할 수 있었다.

최초 출력, 별도 브라우저 프로세스 재열기 후 출력, 선택 채우기 Undo 후 출력의 SHA-256:

```text
9E5C93218A1DC359435BA5A48074C50990CE727051EE7CBDE4DDDD9A0B8EFA25
```

로컬 검증 산출물은 Git에서 제외된 `output/playwright/fill-ux/`에 보관했다.
`reopened.png`, `selection-filled.png`, `final.png`는 실제 앱 화면이다.
콘솔에는 favicon 요청 404 및 WebGPU adapter 선택 관련 브라우저 경고가 있었으며
앱의 JavaScript/WASM 실행 오류는 확인되지 않았다.

## 남은 한계

- 물리 펜, iPad/Safari, 데스크톱 창의 직접 조작은 이번 검증에 포함하지 않았다.
- 이전 검사에서 발견한 활성 레이어 복원 누락은 아래 후속에서 수정했다. 새 레코드가
  없는 과거 파일은 여전히 기존 기본 선택으로 열린다.
- 세밀한 반투명 선/복잡한 완성 일러스트의 품질은 단순 사각 선화 검사로 보증하지 않는다.
- 둘러싸서 자동 채우기, 미채움 보정, 선 넘침 방지 및 정교한 AA는 여전히 후속 범위다.
- 설치본 교체, GitHub push, 공개 배포는 수행하지 않았다.

## 후속: 채색 대상 레이어 복원

- `.ntdr`에 독립적인 프로젝트 세션 레코드를 추가했다. 웹의 복구 Worker도 이를 기록하고
  데스크톱의 bounded 최신값 큐도 같은 레코드를 저장한다. 글로벌 도구 상태에는 포함하지 않는다.
- project-redb 30개, project-web 12개 검사 통과. 삭제된 ID/그룹 ID의 fallback,
  알 수 없는 레코드 보존, 레이어 선택만 저장해도 픽셀과 history cursor 유지,
  별도 프로세스 native/web 재열기에서 같은 대상 복원을 포함한다.
- release desktop 도구 상태 3개 검사 통과. 대기 중 여러 레이어 선택은 마지막 값으로
  저장하고 앱 공통 브러시 설정은 바꾸지 않는다.
- 기존 Save As 충돌/원본 보존 검사에도 활성 레이어 레코드 복사를 추가해 통과했다.
  변경된 저장소·웹 프로젝트·데스크톱 all-targets Clippy 및 포맷/공백 검사도 통과했다.
- release RTX 3080/DX12 actor acceptance에 마지막 대상 변경 후 저장/재열기 검증을
  추가해 통과했다. GPU/CPU 채우기 결과와 PNG, Undo/Redo도 기존 검사대로 일치한다.
- 실제 Edge에서 기존 시험 작품의 Layer 2를 선택하고 다운로드한 `.ntdr`를 새 브라우저
  프로세스에서 다시 열었다. 채색 대상 Layer 2와 허용 오차 37이 복원됐다.
  다시 출력한 `active-layer-export.png`도 위 PNG 해시와 동일하다.
- `active-layer-reopened.png`는 이 재열기 화면이다. 사용자 작품은 사용하지 않았다.
- 웹/Worker와 데스크톱 자산 포함 release 빌드는 완료했으며 설치·커밋·푸시·태그·배포는 하지 않았다.

## 후속: 보이지 않는 대상에 채우기 방지

- 웹/데스크톱 공용 편집 경로에서 숨긴 래스터, 숨긴 부모/조상 그룹의 버킷·선택 전체
  채우기·그라데이션을 거절한다. 픽셀 잠금과 함께 table-driven 코어 검사로 확인했다.
  거절 후 원본 그림·선택·Undo 개수가 그대로이며, 표시한 대상은 정상 수정/Undo된다.
- 관련 editor 5개와 통합 1개, paint-cpu 31개, web-core 8개, project-web 12개로
  총 57개 검사 통과. desktop release의 별도 프로세스 저장/재열기 acceptance도 재실행해 통과했다.
- 실제 Edge에서 Layer 2를 숨긴 채 버킷을 클릭하면 비치명적 안내만 나타났다. 다시
  표시한 뒤 출력한 `hidden-rejected-export.png`는 최초 PNG와 같은 위 해시다.
- 표시를 켠 뒤 같은 영역을 흰색으로 채우면 정상 반영된다. `.ntdr` 저장, 브라우저
  프로세스 종료, 새 프로세스에서 가져오기 후 출력이 아래 해시로 동일했다.
  `visibility-safe.png`, `visibility-safe-reopened.png`, `visibility-reopened-screen.png`에 보관했다.

```text
3761E618B30A9F890AC478A858F11879DA1A2B9C6A215BE073223A4746F56439
```

- 실제 UI에서 참조 선화 숨김, 투명도 잠금, 대상 불투명도 0%를 각각 조작해 속성 안내가
  상태에 맞게 바뀌는지 확인했다. 0%와 투명도 잠금은 작업 금지가 아니라 안내만 한다.
- 변경 코어/공용 UI의 all-targets Clippy `-D warnings`, 포맷과 diff 공백 검사,
  웹/Worker 및 desktop 자산 포함 release 빌드 통과. 기존 third-party 컴파일 경고와
  로컬 favicon 404/WebGPU adapter 경고는 별개이며 물리 펜 검증은 수행하지 않았다.
- 기존 산출물 디렉터리만 사용했다. 사용자 작품·설치본·원격 저장소는 변경하지 않았다.

## 후속: 인접 밑색 영역 보존 코어 검사

```powershell
cargo test -p nyatidraw-paint-cpu --locked flat_fill::tests
```

- debug 프로필에서 4개 검사 통과, 0개 실패. 컴파일 6.36초, 검사 실행 0.17초이며
  성능 벤치마크나 입력 지연 측정은 아니다.
- 새 table-driven 검사는 가는 불투명 선, 굵은 불투명 선, 가는 반투명 선,
  작은 틈을 가진 굵은 반투명 이중 경계 4가지에 참조 레이어/전체 보이는 그림과
  선택 clip 유무를 조합한 16개 사례를 확인한다. 초기 허용 오차 16, 선 밑 확장 1px에서
  빈 영역을 채우고 인접한 기존 밑색, 다른 연결 영역, 바깥 투명 영역과 참조 선 원본을 보존한다.
- 기존 검사도 틈 열린 상태의 누출과 closing 적용 후 차이, 두께 있는 선의 안쪽 AA 받침과
  밝고 어두운 배경 비교, signed 선택 구멍, 원본 불변 및 자원 한도 실패의 원자성을 확인했다.
- 이번 검토에서 재현된 채우기 알고리즘 오류는 없었으며 제품 동작을 변경하지 않았다.
  작은 합성 경계의 무결성 검사이며 실제 피부/머리카락 일러스트 완성과 물리 펜 사용의
  W3 gate를 통과했다고 주장하지 않는다. 가는 반투명 선의 fractional AA 재구성과
  좁은 통로를 의도치 않게 막을 수 있는 square closing 한계는 그대로다.

# 애니메이션 통합 확인

2026-10-28, 현재 작업 트리의 실제 Dioxus 웹 앱을 격리된 Edge에서 확인했다.
아래 웹 결과는 별도 JavaScript 기능 모형이 아니라 실제 앱 버튼, 마우스 획,
파일 선택기, 다운로드 및 IndexedDB/복구 Worker를 사용한 결과다.

## 환경과 증거 경계

- Windows 11 Pro 10.0.26300, Microsoft Edge 156.0.4314.50, WebGPU.
- 1440×1000 CSS viewport, DPR 1. Playwright CLI의 headed Edge, 전용
  `target/animation-web-profile`, `http://127.0.0.1:8879/`.
- 최초 검증 bundle: `nyatidraw-web-dxh49762735a5765181.js`.
- 최종 보완 재검증 bundle: `nyatidraw-web-dxh56762b4aa6616739.js`.
- Playwright CLI close/open으로 브라우저를 종료·재실행해 복구 확인. 그룹 변형 후에도
  다시 종료·재실행했다. CLI 표시 PID10200/5468/15640은 브라우저를 관리하는
  Node 프로세스 ID이며 Edge 주 프로세스 ID로 해석하지 않는다.
- 원본 그림, PNG/JSON, 화면, 실행 스크립트와 결과는 무시되는
  `output/playwright/animation/`에 보관했다. 고정 테스트 문서 크기는 512×512다.
- 이것은 기능/내구성 확인이다. 물리 펜, 120Hz/first-visible-pixel, 타이밍 정밀도,
  모바일/다른 OS, 큰 프로젝트 성능, 외부 게임 엔진 import의 증거가 아니다.
  통합 빌드가 병행되어 성능 측정은 하지 않았다.

## 실제 웹 타임라인

| 작업 | 관찰과 비교 | 결과 |
|---|---|---|
| 시작/독립 셀 | 메뉴에서 애니메이션 시작. 첫 셀에 검은 삼각형, 빈 둘째 셀에 검은 사각형을 실제 mouse stroke로 그림 | 통과 |
| 시간/순서/이력 | 240/480ms 설정. 둘째 프레임 앞으로 이동 → Undo → Redo → Undo에서 순서와 선택 셀/시간 확인 | 통과 |
| 표시 격리 | 둘째 셀에서 첫째 삼각형은 붉은 onion으로만 표시. navigator와 layer thumbnail은 현재 사각형만 표시 | 통과 |
| 재생/정지 | 실제 Play에서 두 프레임을 교대로 관찰. `playback-results.txt`에 DOM 선택 프레임 샘플. Stop은 편집 중이던 둘째 셀로 복귀 | 통과, 프레임 시간 정밀도 벤치마크 아님 |
| 출력 | 메뉴의 PNG, sprite sheet, 작업 파일 저장으로 실제 파일 다운로드. Worker가 PNG/JSON 두 파일 생성 | 통과 |
| 브라우저 프로세스 재시작 | Edge 종료 뒤 같은 격리 profile 재실행. 2개 셀/240·480ms 복구, 두 셀 그림 확인 | 통과 |
| 재열기 후 편집 | 셀 복제/삭제 → 삭제 Undo/Redo. 새 mouse tap → Undo 후 재출력 | 통과 |
| 원본 동일성 | 재시작 전후 시트 RGBA bytes 및 frame/cel IDs·duration 배열 완전 동일 | 통과 |

출력의 첫 셀은 alpha bbox `(67,29,214,142)`, 4151개 비투명 픽셀이고,
둘째 셀은 `(272,233,419,380)`, 5831개다. 모든 비투명 픽셀의 RGB는 검정으로,
붉은 onion은 저장되지 않았다. 현재 PNG 512×512의 raw RGBA는 시트 1024×512의
둘째 512×512 cell과 정확히 같다. PNG 파일 압축 bytes가 아닌 decoded pixels를 비교했다.

주요 증거: `frame1.png`, `frame2-onion.png`, `play-1.png`, `play-2.png`,
`stopped.png`, `reopened-frame1.png`, `reopened-frame2.png`, `duplicated-frame3.png`,
`sheet.png`, `sheet.json`, `reopened-sheet.png`, `reopened-sheet.json`, `two-cels.ntdr`.

초기 sprite 다운로드 이름은 `sheet.png`/`sheet.json`이었다. 최종 빌드에서는 실제
시트 내보내기를 두 번 실행해 서로 다른 UUID 쌍 네 파일을 내려받았다. 각 JSON의
`image`가 해당 PNG의 실제 다운로드 이름과 정확히 같았다. 두 시트의 decoded RGBA는
첫 cell이 보존한 초기 삼각형, 둘째 cell이 그룹 변형 후 그림과 완전히 같았고, 시간도
240/480ms였다. 확인한 쌍의 basename은 다음과 같다.

- `sheet-4ad4e077-f78c-4361-8794-da02ac6e4aae`
- `sheet-8475e909-69d2-45ad-a8c9-e8c5794d3612`

증거: `final-export-results.txt`, `final-pixel-results.txt`, `final-export.png` 및 두 PNG/JSON 쌍.

## 그룹 변형과 셀 격리

실제 레이어 UI에서 그룹을 추가하고 사각형 레이어 thumbnail을 그룹에 드래그했다.
그룹 안에 두 번째 래스터를 추가해 초록 선을 그렸다. 그룹 선택 → 그룹 변형 →
취소를 확인한 뒤 다시 시작하여 X=20px/Y=-10px → **미리보기** → 확정했다.

- 검정 사각형 bbox: `(272,233,419,380)` → `(292,223,439,370)`.
- 초록 선 bbox: `(279,403,405,414)` → `(299,393,425,404)`.
- 두 레이어 합성 결과는 이전 PNG를 `(20,-10)` 이동한 raw RGBA와 완전히 같다.
- Undo 출력은 `group-before.png`와 완전히 같다. Redo 후 저장하고 Edge 프로세스를
  종료·재실행한 출력은 `group-after.png`와 완전히 같다.
- 첫째 애니메이션 셀을 다시 출력한 결과는 초기 시트의 첫 cell과 완전히 같다.

증거: `group-transform-before.png`, `group-transform-preview.png`, `group-before.png`,
`group-after.png`, `group-undo.png`, `group-reopened.png`, `group-reopened.png-export`,
`group-other-cel.png`, `group-animated.ntdr`.

## 참조 색 채취와 크기 지정 출력

참조 패널의 실제 파일 선택기로 RGBA 4구역 fixture `reference-colors.png`를 열었다.
드래그 중 미리보기만 변경되고 release에서만 녹색이 전경색에 적용됐다.
투명 구역·object-fit 여백·Escape·패널 밖 release는 전경색을 바꾸지 않았다.
1.5625배 확대와 40px 수평 pan 후 다른 색 구역을 정확히 채취했다.
원래 펜 도구의 활성 상태는 유지됐고, 채취만으로 최근 색상이 추가되지 않았다.
실제로 초록 획을 그린 뒤에만 최근 색상이 추가됐다.
증거: `reference-preview.png`, `reference-zoomed.png`, `reference-panned.png`,
`reference-results.txt`, `reference-pan-results.txt`.

PNG 크기 지정 dialog에서 50% 입력 후 blur하면 너비·높이가 256으로 갱신됐다.
실제 다운로드는 256×256 RGBA이며 alpha 범위는 0..255다. 이후 512×512 시트의
원본 셀 및 재열기 결과가 그대로인 것도 확인했다.
증거: `scaled-dialog.png`, `scaled256.png`.

## 실패와 제한의 기록

- 첫 웹 빌드에서 둘째 프레임(480ms)에서 첫째(240ms)로 선택을 바꿀 때,
  프레임 버튼의 시간은 240ms인데 시간 입력란이 480으로 남는 현상을 확인했다.
  저장된 timeline/시트 metadata는 올바르다. 공유 FrameDuration draft/props 동기화
  보완을 최종 빌드에서 재검증했다. 첫째 선택 → 입력 240, 둘째 선택 → 480,
  500으로 편집 → Undo 480 → Redo 500 → Undo 480, 프레임 복제 → 480,
  복제 Undo → 2프레임/480을 모두 확인했다. 원래 240/480ms와 두 셀로 되돌린 뒤
  추가 그림 변경 없이 위 최종 시트를 비교했다. 증거: `final-duration-results.txt`,
  `final-duration.png`.

- 자동화의 처음 숫자 입력은 onchange blur를 생략했고, 그룹 변형 숫자 입력 뒤에는
  미리보기 버튼을 생략했다. 각각 기존 값 유지/확정 비활성화라는 올바른 UI 동작이었다.
  해당 `group-failed.yml`은 제품의 변형 실패가 아니다. 올바른 실제 순서로 재검증했다.
- 두 번째 Edge 재실행 때 브라우저가 같은 origin의 탭 두 개를 복원했다. 앱은
  다른 탭이 열렸다는 안내로 둘째 writer를 차단했다. 중복 탭을 닫고 새로고침하여
  저장된 그림을 정상 복구했다. 보호 기능을 우회하지 않았다.
- Console의 favicon 404, Chromium Windows powerPreference 안내,
  autofocus 안내 외에 이번 동작의 기능 오류는 없었다.
- 네이티브 실행과 실제 UI 증거는 담당자의 별도 실행 기록으로 구별한다.
  이 문서의 웹 결과를 네이티브 UI/물리 펜 검증으로 확대하지 않는다.

## 초기 보존 파일 SHA-256

| 파일 | SHA-256 |
|---|---|
| `two-cels.ntdr` | `AB69F21072DE67164462B5FAF35544E8AA6BD54843D9E27FFE805040ECACC74C` |
| `sheet.png` | `95FA8EE93D4867F16E7A29EA866232AF74E8BD809B46CACDA8FA28724970C9A8` |
| `sheet.json` | `BD39B5B727E9D6BA23B940ECE7B985A92CA2F9497AC43829669CE9A0DCC49A20` |
| `group-animated.ntdr` | `EA10ACF8C3853D81A79D1884A4F24EB1ECC3489FC4EE7453EB93E399B38D8353` |

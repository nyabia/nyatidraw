# 마스크·부드러운 선택 실제 웹 확인

2026-10-28, 실제 Dioxus 웹 앱을 격리된 headed Edge에서 조작했다. 아래는
앱 버튼·키보드·마우스 획·파일 선택기·다운로드·복구 Worker를 거친 결과다.
별도 JavaScript 기능 모형이나 UI 단위 테스트의 결과가 아니다.

네이티브에서는 별도의 [화면 재표시 문제](research/native-mask-presentation.md)가
아직 해결되지 않았다. 아래 웹 통과 결과를 Windows 네이티브 표시 안정성 통과로
옮겨 해석하지 않는다.

## 환경

- Windows 11 Pro 10.0.26300, Edge 156.0.4314.50, WebGPU.
- 1440×1000 CSS viewport, DPR 1. Playwright CLI session `mask-accept`,
  전용 profile `target/mask-web-profile`, `http://127.0.0.1:8879/`.
- 최초 bundle `nyatidraw-web-dxh73f06ee8ff133ff1.js`.
- 최종 수정 재검증 bundle `nyatidraw-web-dxhf712973b2b863c9.js`.
- Playwright CLI close/open으로 두 번 브라우저를 종료·재실행해 복구 확인.
  CLI 표시 PID12500/2248/5540은 Node 관리 프로세스 ID다. 마지막 세션의 실제
  Edge 주 프로세스는 Win32_Process에서 PID24924, parent5540으로 확인했다.
- 원본·다운로드·화면·실행 스크립트·결과는 `output/playwright/masks/`에 보관.
  테스트 그림은 512×512 불투명 빨강, 참조는 검정·RGB128 회색·흰색 3구역 PNG다.
- PNG 비교는 압축 파일 bytes가 아닌 decoded RGBA 전체 bytes를 사용했다.
- 물리 펜/필압, 고주사율 지연, 큰 프로젝트 성능, 다른 OS/브라우저,
  native GPU 동작의 증거는 아니다. 빌드가 병행되어 벤치마크하지 않았다.

## 마스크 원본 보존과 내구성

| 실제 조작 | 결과 |
|---|---|
| PNG 열기 → 모두 보이는 마스크 추가 → 검정 Pen80 획 | 누르는 동안 미리보기와 놓은 뒤 확정 모두 그림이 숨겨짐 |
| 마스크 사용 중지 | PNG가 `source-red.png`와 RGBA 전체 동일 |
| 다시 사용 | 초기 masked PNG와 RGBA 전체 동일 |
| 마스크 삭제 → Undo | 삭제는 원본과 동일, Undo는 삭제 전 masked PNG와 동일 |
| Eraser30 획 | 숨긴 영역이 다시 보임; 검사점 alpha0→255 |
| 흰색 참조 채취 → 마스크 Pen 획 | 해당 구역 alpha255 복원, 다른 구역 유지 |
| 작업 파일 저장 → Edge 종료/재시작 | 복구 PNG가 종료 전 `white-painted.png`와 RGBA 전체 동일 |
| 실제 파일 선택기로 `painted-mask.ntdr` 다시 열기 | 동일 그림의 RGBA 전체 일치 |

증거: `black-preview.png`, `black-painted.png`, `black-painted-export.png`,
`mask-disabled.png`, `mask-reenabled.png`, `mask-removed.png`, `mask-remove-undo.png`,
`eraser-revealed.png`, `white-painted.png`, `painted-mask.ntdr`, `restarted.png`,
`restarted-export.png`, `ntdr-reopened.png`, `toggle-results.txt`.

## 부드러운 선택과 기본값 0 마스크

실제 사각형 선택 드래그 → 반경 16 문서 픽셀 → **부드럽게** →
**선택 영역으로 마스크 추가**를 실행했다. 내보낸 PNG는 alpha0부터255까지
256개 값을 포함했다. alpha bbox는 `(117,117,393,396)`이며, y250의 x120/140/
160/180/200/250 alpha는 각각 2/35/136/229/254/255다. 비투명 픽셀의 RGB는 빨강이다.

- 마스크를 끄면 최초 불투명 빨강 원본과 RGBA 전체가 같다.
- `feather-mask.ntdr`를 실제 파일 선택기로 다시 열어 출력한 PNG가 원본 masked
  출력과 정확히 같다.
- 두 번째 브라우저 세션을 닫고 세 번째 세션에서 복구해 출력한 PNG도 정확히 같다.
- 마스크 메뉴 열기/Escape 3회, 참조/내비게이터 탭 전환, 변형 취소 이후에도
  실제 캔버스와 내비게이터에 부드러운 마스크가 남고 출력 RGBA가 같다.
- 초기 즉시 screenshot의 navigator는 갱신 전이므로, 판정에는
  `feather-mask-settled.png`와 `feather-mask-redrawn.png`를 사용했다.

증거: `feather-selection.png`, `feather-mask-settled.png`,
`feather-mask.png.export.png`, `feather-disabled-source.png`, `feather-mask.ntdr`,
`feather-redrawn-export.png`, `feather-file-reopened.png`,
`feather-process-reopened.png`, `feather-restart-results.txt`.

선택 경계 수정 자체의 Undo 이력은 이 작업의 지원/검증 범위가 아니다.

## 대상·키보드 UX

- 마스크 대상에서 Fill의 **선택 전체 채우기**, Transform의 **변형 시작**은
  비활성이다. **이미지 편집으로 돌아가기**를 누르면 실제 이미지 대상이 선택된다.
  도구 전환은 계속 가능했다. `soft-selection-results.txt`의 두 disabled=true,
  imageSelected=true 및 `mask-fill-guidance.png`, `mask-transform-guidance.png`.
- 레이어 이름 F2 → 변경 → Escape는 `Layer 1` 유지. F2 → `Mask artwork` → Enter는
  이름 변경을 확정했다. `reopen-rename-results.txt`, `renamed.png`.
- 마스크가 있는 셀을 실제 타임라인에서 복제했다. 복제 셀 PNG가 원래 masked
  그림과 RGBA 전체 동일했다. 첫째240ms/둘째480ms; 프레임2에서 ArrowLeft는
  프레임1로 초점·선택을 함께 옮기고 시간240, End는 프레임2/480을 표시했다.
  시간999 입력 후 Escape는480을 복원했다. `timeline-keyboard-results.txt`,
  `masked-cel-duplicate.png`, `masked-timeline.png`.
- 복제 셀2의 마스크만 Eraser로 바깥 영역을 드러냈다. 셀2 출력은 바뀌었지만
  셀1 출력은 이전 masked RGBA와 정확히 같았다. Undo 뒤 셀2도 원래와 정확히
  같았다. `copied-mask-edited-frame2.png`, `copied-mask-untouched-frame1.png`,
  `copied-mask-edit-undo.png`, `mask-cel-independence-results.txt`.

## 발견한 두 문제와 최종 재검증

첫 bundle의 RGB128 회색 마스크 획은 alpha55로 기록됐다. 마스크 사용자가
기대하는 50% coverage가 아니므로 display-sRGB scalar로 수정했다.
`gray-painted.png`와 `gray-white-screen.png`는 문제 재현 증거로 보존한다.
이 파일에 이미 기록된55를 잘못된 저장 복구로 해석하지 않는다.

변형 입력 X20 → Enter 미리보기 뒤 초점이 MAIN으로 빠져 두 번째 Enter가 확정되지
않았다. 입력에 다시 초점을 주고 Escape를 누르면 안전하게 취소됐다.
`transform-keyboard-results.txt`, `transform-enter.png`. Preview가 초점을 옮기지
않도록 shared UI를 좁게 수정했다.

최종 bundle에서 두 수정을 실제 조작으로 다시 확인했다.

- 같은 RGB128 참조 색을 채취하고 새 마스크 획을 그렸다. 문서 좌표
  `(80,68)`, `(100,68)`, `(110,68)`의 출력은 모두 `(255,0,0,128)`이며,
  손대지 않은 중앙은 alpha255다. `final-gray128.png`, `final-gray128-screen.png`.
- X20 입력 후 Enter 미리보기는 실제 INPUT `가로 이동`에 초점을 유지했다.
  두 번째 Enter가 확정해 입력 패널이 닫혔다. 경고는 없었다. 이후 Undo 출력은
  변형 전 `final-gray128.png`와 RGBA 전체 동일했다. `final-retest-results.txt`,
  `final-transform-enter.png`, `final-transform-undone.png`.
- 최종 작업 파일 `final-masked-animation.ntdr`를 다운로드했고, 화면은
  `final-ready.png`에 보존했다. 이 최종 파일 자체의 별도 재시작 검사는 반복하지
  않았다. 저장·재시작·동일 파일 다시 열기의 근거는 위 두 기본값 사례다.


## 검증 환경 종료

종료 직전 실제 앱은 브라우저 저장 완료, 경고 없음, 두 프레임240/480ms를 표시했다.
그때 다시 내보낸 `before-cleanup.png`는 `final-transform-undone.png`와 decoded RGBA
전체가 같았다. `final-masked-animation.ntdr`(180224 bytes), 초기·feather NTDR 및
모든 출력/화면 증거를 보존했다. `before-cleanup-results.txt`에 상태를 기록했다.

전용 `mask-accept` Playwright/Edge 세션만 종료했다. PID33704가 정확히
`python -m http.server 8879 --bind 127.0.0.1 --directory
target/dx/nyatidraw-web/release/web/public`임을 다시 확인한 뒤
해당 서버만 종료했고8879의 LISTEN이 없음을 확인했다. profile과 작업 파일은
삭제하지 않았다. 사용자 Edge나 네이티브 앱은 건드리지 않았다.

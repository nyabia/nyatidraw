# 퀵 드로잉 · 입력 · 그림 이동 검증

## alpha.17 로컬 검증

사용자 작품 대신 Windows의 격리 Edge 프로필과 스크래치 문서를 사용했다.
WebGPU 활성화, Dioxus debug Web/Worker 빌드, 1440×900 UI다.
성능 벤치마크 또는 물리 펜 검수 기록이 아니다.

| 항목 | 관찰 결과 |
|---|---|
| PNG 복사 | 실제 클립보드 쓰기 후 다른 로컬 페이지에 Ctrl+V로 PNG 파일 도착 |
| 출력 일치 | 1280×720, 비투명 픽셀 1,824개, 내보낸 PNG와 RGBA 차이 0개 |
| 실패 보존 | clipboard.write의 통제된 NotAllowedError 주입 시 안내 표시; 전후 PNG SHA256 동일 |
| 그림 이동 | 브라우저 마우스 드래그 후 그림 이동, 복구 재열기와 실제 NTDR 다운로드 |
| NTDR | 다운로드 파일과 페이지 밖 이동 fixture를 별도 native/web 프로세스로 열어 artwork root 대조 |
| 흰 배경 | 기존 명령으로 PNG의 전 픽셀 alpha 255, 빈 모서리 RGBA [255,255,255,255] |
| 앱 입력 모드 | 메뉴에서 손가락 선택 후 페이지 재열기에도 유지; 자동으로 복귀 가능 |
| 합성 입력 | 손가락 pressure 1, 펜 선점, 700ms 복귀, 접촉 유지 중 펜 우선, 기존 손바닥 재개 차단 |
| 합성 입력 추가 | 펜 모드의 터치 차단, 손가락 모드의 cooldown 우회, 마우스 유지, Hand 터치 pan, 모드 변경 cancel |

합성 입력은 실제 PointerEvent를 브라우저 핸들러에 전달하되 pointer capture를 대체했다.
OS의 실제 팜 분류·캡처 동작을 검증한 것은 아니다. Windows PT_TOUCH 경로는
컴파일·Clippy와 코드 검토 범위다. 실제 펜·터치·Safari·Misskey 서버·설치 업데이트는 미검증이다.

## 회귀 방지 범위

- 입력 코어의 작은 표 기반 시험은 pen contact/cooldown/mode별 touch begin 허용 규칙을 검사한다.
- 공용 그림 이동 시험은 선택/무선택, signed off-page 이동, 선택 영역의 동반 이동,
  중앙 정렬과 한 번의 Undo를 검사한다. 잠금 거부는 기존 공용 변형의 사전 검사를 재사용한다.
- `worker_reopen_probe`는 이동 후 Undo/Redo와 실제 저장·자식 프로세스 재열기를 실행한다.
- UI 모양이나 단순 wrapper를 위한 단위 시험은 추가하지 않았다.
- 로컬 일회성 브라우저 증거는 무시되는 `output/playwright/alpha17-*`에 보관한다.

## 제품 범위

Misskey 위젯 예제는 링크를 여는 용도다. 인증, 자동 첨부, 자동 게시 기능은 없다.
일반 웹판은 기존 브라우저 작업을 복원하므로 새 그림은 사용자가 직접 시작한다.
기존 SDK의 iframe/popup 반환 기능은 유지한다. 이 문서는 공개 배포 완료 선언이 아니다.

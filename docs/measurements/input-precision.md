# 정밀 입력과 선택적 손떨림 보정 검증

대상은 Windows 데스크톱과 공용 웹 편집기, alpha.23 입력 변경이다.
구현 근거와 독립 구현 경계는 [조사 기록](../research/high-resolution-mouse-input.md)에 있다.

## 입력과 보정의 분리

- Windows 상대 마우스는 기존 raw receiver를 관찰한다. 해당 Down의 raw 증거가 있으면
  한 획의 이동을 raw에 고정하고, 없으면 legacy를 유지한다. 버튼 phase는 legacy가 소유한다.
- 시스템 속도·가속의 독립 근사로 소수 이동을 누적하고 OS 메시지 좌표 각 축의 ±0.5 물리
  픽셀 안으로 제한한다. 이는 커서 정합 조건이며 원본 센서 좌표 복원이나 EPP 완전 일치가 아니다.
- 웹은 첫 실제 이동 이벤트를 기준으로 rawupdate 또는 pointermove를 선택한다.
  병합 샘플을 부모와 중복하지 않고 같은 좌표의 압력 변화도 전달한다.
- 공통 보정 0은 정확한 우회다. 양수일 때 기존 위치 안정화 뒤 cubic 연결을 적용한다.
  2 논리 픽셀 이하의 짧은 구간만 다음 샘플을 기다리고, End/종료는 끝점을 완성한다.
  평탄화 오차 기준은 0.05 논리 픽셀, 출력 상한은 구간 64개/입력 묶음 128개다.
- 평가 샘플을 그대로 기록한다. 저장된 브러시 엔진은 바꾸지 않으며 재열 때 다시 보정하지 않는다.
- UI는 도구 속성에서 불투명도 아래 **손떨림 보정** 슬라이더와 숫자 입력이다.
  범위 0–100, 초기값 0, 기존 설정 복원, 변경값은 다음 획부터 적용한다.

## 핵심 검사

- 입력 코어: 0에서 좌표 bit/phase 보존, 정지 필압·짧은 획·End 보존, 의도한 모서리와 cusp,
  25/50/100/200% 화면 배율과 묶음 분할 불변성, pending 취소·유한 좌표·분할 한도.
- 플랫폼: 소수 누적 유지·OS 정합 상한, 시간 역전/비정상 좌표 거부,
  유효 raw Down이 없을 때 legacy 이동을 차단하지 않는 조건.
- 웹 코어: 보정된 획의 Cancel·Undo와 portable 저장 픽셀 보존.
- 네이티브 코어: live 입력의 평가 샘플 78개를 기록하고 별도 프로세스에서 재열어
  root·PNG 일치를 확인했다. 종료 시 아직 출력하지 않은 꼬리 보존과 취소 획 비복원도 검사한다.
- 브라우저 어댑터의 scratch 합성 harness 18개가 통과했다. raw/move 선택, 중복 억제,
  압력, overflow/cancel/blur/hidden/modal/Esc, 캡처 해제, pan/pick/rotate를 검사했다.
  DOM stub 기반 검사이므로 실제 브라우저나 장치 증거는 아니다.

## 실행 검증

- `cargo test --workspace --all-features --locked`, workspace 전체 대상 Clippy,
  WASM UI/Worker Clippy, fmt, JavaScript 구문 검사와 Windows/웹 release 빌드를 통과했다.
  문서 사이트 빌드와 검색 인덱스 생성도 통과했다. 최초 제한 환경의 scratch hard-link
  저장 검사는 권한 오류였으며, 정상 권한으로 전체 검사를 다시 실행해 통과했다.
- 실행 환경은 Windows NT 10.0.26300.0, NVIDIA GeForce RTX 3080 / Vulkan,
  드라이버 610.88, `Bgra8UnormSrgb` / Mailbox, 화면 배율 1이다. DX release 창에서
  기본 강도 0과 변경한 68로 획을 그리고 Undo/Redo·저장·정상 종료를 수행했다.
  새 프로세스에서 두 획과 강도 68이 복원됐고 다시 정상 종료했다.
- 새 raw 관찰자가 기존 등록을 바꾸지 않고 연결된 것은 로그로 확인했다. UI 자동화 입력은
  물리 raw packet 증거가 없어 두 획 모두 legacy 경로였다. 따라서 이 검사는 실제 raw
  마우스의 정확도·가속 감각·배율별 개선량을 입증하지 않는다.
- 최초 release 실행에서 `GetWindowSubclass`의 기본 DLL export 부재로 진입 전 실패했다.
  해당 import를 제거하고 UI 스레드의 receiver 소유권 guard로 교체했다. 수정한 release
  실행 파일로 위의 시작·종료·재시작 검사를 완료했다.
- 격리한 Edge 프로필의 실제 웹 release에서 강도 0/66, 59%·100%·26% 배율의 곡선과
  짧은 탭을 확인했다. 슬라이더는 드래그 도중 숫자가 갱신됐다. 브라우저 관찰자는
  `pointerrawupdate`와 `pointermove`를 각각 197회 기록했으며, 소수 좌표도 전달됐다.
  이 이벤트는 브라우저 자동화로 생성한 것이며 물리 장치 증거는 아니다.
- 웹에서 PNG·NTDR를 저장하고 브라우저 프로세스를 종료했다. 같은 격리 프로필의 새
  프로세스로 자동 복구한 뒤, 별도로 NTDR를 명시적으로 열어 각각 PNG를 출력했다.
  세 PNG와 네이티브 CLI가 같은 NTDR에서 출력한 PNG는 1280×720 RGBA가 정확히 같다.
  RGBA SHA-256: `e5f713991f95c39400efbfe46d6cda62124f940b5ba5d4597fb9b5822462c252`.
- Windows UI에서 저장한 PNG와 별도 CLI 프로세스가 재열어 출력한 PNG는 1024×768
  RGBA가 정확히 같다. RGBA SHA-256:
  `49970dfb2a9c0a01d49f0208d11139f1a245695e67b61220a7ce2b16e2bc469d`.

모든 실행 검사는 격리 설정과 scratch 작품으로 수행했다. 사용자 작품·설치본을 교체하지 않았다.
원본 로그·스크린샷·출력물은 무시되는 `output/research/input-precision/` 및
`output/playwright/` 아래에 보관하며 공개 소스에는 포함하지 않는다.

## 공개 웹 확인

`54fd7c4`의 [Pages 작업](https://github.com/nyabia/nyatidraw/actions/runs/41178210042)이
성공한 뒤 새 Edge 프로필로 실제 공개 편집기를 열었다. 기본 강도 0, 드래그 중 강도 66
반영, 59%·100%·26% 배율의 획과 짧은 탭을 확인했다. PNG·NTDR를 저장하고 브라우저
프로세스를 종료한 뒤 같은 격리 프로필로 다시 열었을 때 그림과 강도 66이 복원됐다.
복구 전후 PNG의 1280×720 RGBA는 정확히 같다. RGBA SHA-256:
`d135bcfeba4de1451d63a41eb04ee4809c6c8dfe9716da4916b875db3d46cb81`.

홈페이지·편집기·복구 Worker 등 공개 파일 12개는 해당 Pages 산출물과 바이트 단위로
일치했다. 이 첫 게시의 다운로드 안내는 당시 공개 중인 alpha.22를 가리킨다.
브라우저 로그의 favicon 404와 Windows `powerPreference` 무시 경고는 앱 실행 오류와
구분했다. 실제 장치 증거나 Windows 설치 업데이트 증거로 확대하지 않는다.

릴리즈 후 [최종 홈페이지 재게시](https://github.com/nyabia/nyatidraw/actions/runs/41179728944)에서
기본 `release.json`과 실제 Edge 다운로드 버튼의 alpha.23 안내를 확인했다. 최종 공개 파일
12개는 해당 Pages 산출물과 같고, 편집기·Worker 10개 파일은 위 실행 검증 대상과 동일하다.

## 아직 보장하지 않는 것

- 물리 마우스의 다양한 DPI/폴링률, Windows EPP 설정별 품질과 실제 커서 잔차.
- 실제 Windows 펜, iPad Safari/Apple Pencil의 감각과 고주사율 첫 픽셀 지연.
- 브라우저가 제공하지 않은 센서 정밀도. `pointerrawupdate`도 비가속 센서 입력은 아니다.
- 이미 저장된 울퉁불퉁한 픽셀의 자동 복원. 개선은 이후 입력하는 획에 적용된다.

이 작업은 입력/저장 불변식과 실행 검증이며 성능 벤치마크가 아니다.
따라서 측정하지 않은 p50/p95/p99, FPS 또는 물리 입력 지연을 제시하지 않는다.

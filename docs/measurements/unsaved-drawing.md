# 파일 없이 낙서하고 PNG로 복사하기

## 동작 경계

- 새 그림은 이름·경로 선택 없이 시작한다. Windows의 관리되는 복구 문서와 웹의
  IndexedDB 복구는 유지하며, 사용자가 보관할 NTDR 파일 생성은 선택이다.
- 상단과 메뉴에서 새 그림·PNG 복사에 접근한다. PNG 복사는 선택 영역 복사와 달리
  출력 캔버스 전체를 합성한다. UI·체크무늬·캔버스 밖 스케치는 복사하지 않는다.
- 이미 PNG 짝이 있는 기존 Windows 프로젝트는 기존 저장·출력 경로를 유지한다.
- 복사는 작품 수정·Undo 기록·프로젝트 저장·파일 다운로드를 발생시키지 않는다.
  미확정 편집은 완료/취소 후 복사하며, 실패 시 그림을 그대로 유지한다.

## 웹 릴리즈 빌드 검증

Windows의 격리 Edge 프로필, 1600×1000 창, Dioxus release Web/Worker, WebGPU를 사용했다.
사용자 작품 대신 마우스로 만든 검은 선 하나로 검증했다. 물리 펜 검증은 아니다.

1. 새 프로필에서 파일 선택 없이 기본 펜 10px, 투명 래스터 하나로 시작했다.
2. 획을 그리고 NTDR 저장 없이 상단 PNG 복사를 실행했다. 실제 Clipboard API로 읽은
   PNG는 1280×720, 비투명 픽셀 3,740개였다.
3. 선택적으로 내보낸 PNG와 클립보드 PNG의 디코딩 RGBA SHA-256이 같았다.
4. NTDR을 저장하지 않고 브라우저 프로세스를 완전히 종료했다. 같은 프로필로 다시 열어
   자동 복구된 그림을 복사했으며 픽셀 해시가 같았다.
5. Clipboard API의 권한 오류를 주입해 실패 안내를 확인했다. 실제 API로 되돌린 뒤
   다시 복사한 픽셀은 원본과 같았다.
6. 새 그림의 512×512 프리셋에서 기존 그림 교체 확인을 취소해 보존을 확인했다.
   다시 실행해 교체하면 파일 대화상자 없이 투명 래스터 하나, 투명 PNG로 시작했다.

원본·복사·별도 프로세스 자동 복구·복사 실패 후의 RGBA SHA-256:
`e9c7bb251ae3a5e8fc697bac2caae61225497774d33a406d2490013f917352ba`.

스크래치 출력과 화면은 ignored `output/playwright/doodle-*`에 보관했다.
브라우저 콘솔의 favicon 404와 WebGPU power-preference 경고는 비차단 항목이었다.

## Windows 검증 경계

복사 코어는 불변 CPU 타일·레이어 트리·출력 크기를 사용한다. 테스트용 클립보드로
실패·성공을 검증하고 기존 별도 프로세스 복구 검사와 함께 확인한다.
OS 클립보드의 실제 다른 앱 붙여넣기, 설치판 창 조작, 물리 펜, macOS/Linux는
이 기록의 검증 범위가 아니다. 전체 검사·빌드와 게시 결과는 [릴리즈 기록](../releasing.md)에서 구분한다.

## 최종 로컬 검사

- `cargo test --workspace --all-features --locked --quiet`: 성공.
  자식 재열기 verifier를 포함한 로그 집계는 72개 결과, 252 passed, 기존 5 ignored다.
- workspace all-targets/all-features Clippy와 web/Worker WASM Clippy: 성공.
- fmt·diff 공백·JavaScript 문법 검사, DX Windows release와 Web/Worker release: 성공.
- Astro/Starlight 문서와 범용 유저스크립트 빌드: 성공. 기존 i18n/404 안내는 비차단이다.
- Windows 로컬 실행 파일 SHA-256:
  `27a580a4d2ca3e81c2f86e815d4105ead942a392176c67310087e07d554c914c`.
  공개 설치 파일은 이 개발 머신 바이너리가 아니라 CI에서 따로 생성한다.
- 검증용 Edge 프로필과 로컬 서버는 종료했다. 사용자 작품과 설치본은 변경하지 않았다.

## 공개 웹 확인

[Website 실행](https://github.com/nyabia/nyatidraw/actions/runs/38354498430)이 성공한 뒤
공개 HTTPS 주소를 별도 Edge 프로필로 열었다. 파일을 열거나 NTDR을 저장하지 않은 채
마우스로 획을 그리고 상단 PNG 복사를 사용했다. 1280×720, 비투명 픽셀 3,099개가
포함된 PNG였으며, 검증용 붙여넣기 입력 영역에 실제 Ctrl+V로 `image/png` 파일이 전달됐다.
이 수신 영역은 테스트용이며 특정 SNS의 첨부 구현까지 검증한 것은 아니다.

- 공개 그림의 RGBA SHA-256:
  `fdec97238629da6fd2ae6c11d68bad75587731ad203d55887ca92357c4691eae`.
- 본체 WASM:
  `7b60eb4e94f4a74c6f116b5cc026358363481a889631ff4a1b5f2a7232320408`.
- 복구 Worker WASM:
  `52a9a7384ee553263e554a5aadfb33e6dc76423c6f76d328ea69cbfe5cb78c75`.
- 처음 그리기 문서 HTML:
  `e9f0b0e8a55ead460da662b190df938e7986b8c80dae648a9524429465883941`.

본체·Worker·문서의 공개 파일과 CI Pages 산출물이 각각 일치했다.
공개 콘솔의 오류는 루트 favicon 404였으며 앱 실행 오류는 관찰하지 않았다.

# 문서 사이트와 범용 유저스크립트 검증

## 로컬 빌드와 문서

- Node 24.13.0 / npm 11.6.2에서 `npm ci`, Astro 7.3.5 / Starlight 0.42.4 정적 빌드 통과.
- `npm audit --omit=dev`: 취약점 0. 이는 런타임 전체 보안 보증이 아니다.
- `tools/build-site.ps1 -RequireWeb` 조립 통과. 웹 편집기 입력은 alpha.17 Pages 산출물.
- 실제 Edge: 홈페이지와 문서 7개 URL, 내부 링크·자산 24개 HTTP 오류 없음.
- Pagefind `PNG` 검색에서 저장 문서 등 5개 결과 반환.
- 1440×900, 390×844 화면 확인, 모바일 메뉴 열기 및 320px 홈페이지 가로 넘침 없음.
- 기존 `draw/integration/guide.html`은 `docs/guide/save/`로 이동.
- userscript 소스·번들 `node --check`, `git diff --check` 통과.

## 범용 작성창 수락 확인

격리 Edge의 가상 HTTPS 작성창 fixture와 실제 공개 alpha.17 팝업 편집기를 사용했다.
GM storage/menu는 fixture가 제공했다. 실제 Tampermonkey/Greasemonkey 설치 검증은 아니다.

- 미설정 origin에서 추가 버튼 0개.
- 설정 메뉴를 연속 두 번 열어도 설정창 1개.
- 범위 `.composer`, textarea `textarea.draft`, 별도 버튼 위치 `.actions` 저장 후 버튼 1개.
- SPA 작성창 추가 시 버튼 2개; 두 번째 textarea readonly 전환 후 다시 1개.
- 클릭으로 팝업 열기, SDK 초기화, 실제 합성 마우스 획, 완료 후 원래 페이지에 PNG 반환.
- 결과 1024×768 / 22,423 bytes / 비투명 픽셀 6,688개.
- PNG 복사 후 원래 textarea 포커스, Ctrl+V 시 `image/png` 수신.
- 원본/복사본 RGBA SHA256 모두
  `e1ed32b51c802545f783a39b373a588f4ecd6937a685f1c62248e7a542d1a446`.
- 작성 중이던 fixture 텍스트는 변경되지 않았다.
- PNG 다운로드 성공, 사이트 설정 해제 시 저장 규칙과 추가 버튼 제거 확인.

로컬 화면과 일회성 probe는 무시되는 `output/playwright/docs28-*`에 있다.
실제 사이트의 로그인·첨부·업로드·게시, 확장 관리자 sandbox, Firefox/iPad/물리 펜은 미검증이다.
앱 코어·사용자 설치본·작품은 변경하지 않았다.

## 공개 배포

Website workflow와 공개 URL 확인 결과는 배포 후 여기에 기록한다.
Windows 버전은 alpha.17을 유지하며 이번 작업으로 앱 태그를 추가하지 않는다.

첫 Website 실행은 Windows에서 생성한 npm 잠금 파일의 선택 의존성 두 항목 누락으로
설치 단계에서 중단됐다. 기존 node_modules가 없는 디렉터리에서 CI와 같은 npm 11.19.0으로
잠금 파일을 재생성했다. 이미 공개된 사이트는 이 실패로 교체되지 않았다.

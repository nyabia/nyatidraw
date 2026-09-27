# NyatiDraw 작성창 userscript

직접 설정한 HTTPS 사이트의 작성창에 **그림 그리기** 버튼을 넣습니다. 버튼은 NyatiDraw 공개 편집기를 팝업으로 엽니다. 편집기에서 **Done**을 누르면 작은 PNG 미리보기와 **PNG 복사**, **PNG 다운로드**가 표시됩니다. 복사에 성공하면 원래 textarea로 포커스를 돌려주지만, 붙여넣기와 첨부는 사용자가 직접 합니다. 작성창 텍스트를 읽거나 변경하지 않고, 게시·업로드·사이트 API 호출도 하지 않습니다.

## 빌드와 설치

저장소 루트에서 `site-docs` 의존성을 설치한 뒤 실행합니다.

```powershell
node web-integration/userscript/build.mjs
```

빌드는 `site-docs/package.json`의 esbuild를 사용해 `entry.js`와 기존 `../nyatidraw.js` SDK를 classic IIFE로 묶습니다. `metadata.txt`와 MIT 고지를 앞에 붙여 `target/userscript/nyatidraw.user.js`를 생성합니다. 이 파일을 Tampermonkey 또는 Greasemonkey에 설치하세요. 웹사이트 빌드에서는 산출물을 `draw/integration/nyatidraw.user.js`로 복사할 수 있습니다. ES 모듈인 `entry.js`를 직접 설치하지 마세요.

## 사이트 설정

1. 작성하려는 HTTPS 사이트에서 사용자 스크립트 관리자 메뉴의 **NyatiDraw: 이 사이트 설정**을 엽니다.
2. 현재 origin이 고정 표시됩니다. 작성창 범위 CSS 선택자, 범위 안의 `textarea` 선택자, 버튼을 놓을 요소의 선택자를 각각 입력하고 저장합니다. 버튼은 지정한 요소 바로 뒤에 놓입니다.
3. 범위 선택자 하나가 여러 작성창을 가리킬 수 있습니다. 레이아웃이 다른 작성창을 위해 규칙을 최대 20개까지 추가할 수 있습니다. 각 범위에서 textarea와 버튼 위치가 정확히 하나씩만 찾아져야 하며, 모호하거나 비활성·읽기 전용인 textarea는 건너뜁니다. 작성창이 하나인 페이지에서는 범위로 `body`를 쓸 수 있습니다.
4. 해당 origin의 버튼과 저장 규칙을 제거하려면 같은 설정 창에서 **이 사이트에서 사용 안 함**을 선택합니다.

메타데이터의 `@match https://*/*`는 임의의 HTTPS 사이트에서 설정 메뉴를 열기 위한 범위입니다. 현재 `location.origin`과 일치하는 유효한 규칙을 저장하기 전에는 어느 사이트에도 버튼이나 UI를 삽입하지 않습니다. HTTP와 iframe에서는 실행하지 않습니다. 편집기 주소는 `https://nyabia.github.io/nyatidraw/draw/`로 고정되어 있으며 사이트의 인증 정보나 토큰을 요청하거나 저장하지 않습니다.

## 호환 범위

- 사이트가 DOM을 바꾸면 선택자 설정을 수정해야 할 수 있습니다. SPA의 DOM 변경을 따라 버튼을 정리하고 다시 배치합니다. 실제 `<textarea>`만 지원하며 리치 텍스트와 `contenteditable` 편집기는 지원하지 않습니다.
- 사이트의 팝업·COOP 정책 때문에 SDK 연결이 실패하거나 CSP/사용자 스크립트 관리자가 실행 또는 클립보드를 제한할 수 있습니다. 이때 편집기에서 PNG를 직접 다운로드해 첨부할 수 있습니다.
- PNG 클립보드 쓰기는 브라우저 지원과 권한이 필요합니다. 복사해도 사이트에 자동 첨부되지는 않습니다. 일부 작성창은 이미지 붙여넣기를 거부하므로 다운로드 경로를 제공합니다.
- 받은 PNG는 결과 패널을 닫거나 새 결과로 바꿀 때까지 페이지 메모리에만 남습니다. 패널을 닫으면 임시 미리보기 URL을 해제합니다.

# NyatiDraw 사용자 문서

Astro + Starlight 정적 사이트. 홈페이지의 `/nyatidraw/docs/`로 이어지며,
데스크톱 패키지와 별도로 GitHub Pages에 게시한다. 실행 서버는 필요하지 않다.

## 작업

Node.js 22.12 이상, npm 10.8.2 이상. `site-docs`에서 `npm ci` 후 `npm run dev`로 미리본다.
문서는 `src/content/docs/`에 Markdown 또는 MDX로 추가한다.
사용 안내는 `guide/`, 외부 웹 연동은 `integration/`이며 sidebar는 자동 생성된다.
frontmatter `title`, `description`, 필요하면 `sidebar.order`를 지정한다.
상대 문서 링크는 생성되는 trailing-slash URL 기준으로 작성한다.

`tools/build-docs.ps1`은 문서와 범용 유저스크립트를 빌드한다.
`tools/build-site.ps1 -RequireWeb`은 기존 웹 편집기 산출물과 함께 조립한다.
산출물은 `target/site-docs`, `target/userscript`, `target/site`에만 생성한다.
검색은 Starlight의 Pagefind로 빌드 시 생성된다. 공개 경로는 Astro config의 base와
기존 Pages 경로 `/nyatidraw/`에 맞춘다.

개발 설계·상세 API·측정 기록은 기존 `docs/`에 유지하고 여기서 필요한 문서만 연결한다.
빌드 산출물 HTML을 손으로 고치거나 사용자 문서를 `site/*.html`로 늘리지 않는다.
사용 문서에 특정 SNS 전용 섹션을 두지 않으며 범용 유저스크립트는 사이트별 CSS 선택자로 설정한다.

## 배포

`site-docs/**`, `site/**`, `web-integration/**` 변경은 main의 Website workflow를 실행한다.
문서·유저스크립트만 변경하면 앱 버전 태그를 추가하거나 Windows 릴리즈를 만들지 않는다.
배포 후 홈페이지→문서 이동, 검색, 모바일 메뉴, 기존 guide URL 이동, userscript 다운로드와
웹 편집기·Windows 다운로드 링크를 확인한다.

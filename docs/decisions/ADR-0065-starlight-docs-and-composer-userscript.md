# ADR-0065: 정적 Starlight 문서와 범용 작성창 유저스크립트

## 결정

홈페이지 사용자 문서를 `site-docs/src/content/docs/`의 Markdown/MDX로 작성한다.
Astro/Starlight가 목차, 사이드바, 모바일 탐색, Pagefind 검색을 생성한다.
홈페이지는 기존 정적 사이트를 유지하고 `/nyatidraw/docs/`로 연결한다.
개발 설계·상세 API·검증 기록은 기존 `docs/`에 남긴다.
정적 산출물이므로 GitHub Pages 외 별도 서버가 필요하지 않다.

선정 버전: Astro **7.3.5**, Starlight **0.42.4**,
`@astrojs/markdown-remark` **7.3.1**, esbuild **0.28.2**.
정확한 전이 의존성은 `site-docs/package-lock.json`을 따르며 CI는 `npm ci`를 사용한다.
Node 22.12 이상/npm 10.8.2 이상, CI Node 24.

## 연동 경계

Misskey 전용 홈페이지 섹션·위젯 가이드를 제거한다. 예전 공개 guide URL은 문서로 이동한다.
범용 유저스크립트는 정확한 HTTPS origin별로 작성창 범위, textarea 선택자,
버튼을 뒤에 붙일 요소 선택자를 따로 받는다. 초기 규칙은 없으며 기본 DOM 삽입은 없다.
복수 작성창·SPA 교체·disabled/readonly·모호한 선택자를 처리한다.

기존 `openNyatiPopup` SDK를 esbuild로 함께 번들링하여 protocol 검증을 복제하지 않는다.
작품은 완료 시 PNG로 돌아오며 사용자가 복사·다운로드한다. 복사 후 원래 유효한 textarea에
포커스를 되돌릴 뿐 본문을 읽어 전송하거나 변경하지 않는다.
계정 인증·사이트 내부 API·자동 업로드·자동 게시·범용 자동 첨부는 범위 밖이다.
설정 저장은 GM storage, 반환 PNG는 현재 페이지 메모리에만 유지한다.

## 빌드와 배포

문서와 script를 `target/site-docs`, `target/userscript`에 만들고,
홈페이지 조립 시 `target/site/docs`, `target/site/draw/integration/nyatidraw.user.js`로 복사한다.
Website workflow가 빌드하여 Pages에 배포한다. 앱 코드 변경이 아니므로 Windows 태그를 올리지 않는다.

## 검증 경계

[로컬 확인 기록](../measurements/docs-userscript.md).
실제 브라우저 및 실제 공개 편집기와의 PNG 왕복을 확인했지만 GM API는 fixture로 제공했다.
이 확인을 설치된 확장 sandbox·모든 SNS·실제 펜 호환성 증거로 취급하지 않는다.

근거: [Starlight](https://starlight.astro.build/getting-started/),
[Tampermonkey metadata/API](https://www.tampermonkey.net/documentation.php),
[기존 SDK 계약](../web-embedding.md).

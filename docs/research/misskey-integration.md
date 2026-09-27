# Misskey 연동 조사 기록

## 현재 결정

Misskey 전용 홈페이지 섹션과 단독 위젯 가이드는 채택하지 않는다.
사용 문서는 Astro/Starlight의 [문서 사이트](https://nyabia.github.io/nyatidraw/docs/)로 통합한다.
작성창 연결은 특정 SNS에 묶이지 않는 [범용 유저스크립트](../../web-integration/userscript/README.md)로 제공한다.
정확한 origin, 작성창 범위, textarea와 버튼 삽입 위치를 사용자가 따로 설정한다.
기존 팝업 SDK로 PNG를 돌려받고 직접 복사·붙여넣기 또는 다운로드·첨부한다.
계정 정보·MiAuth·별도 업로드 서버·자동 게시는 범위 밖이다.

일반 웹판은 기존 브라우저 작업을 복원한다. 링크를 열었다고 기존 그림을 지우거나
새 그림으로 덮어쓰지 않는다. 필요할 때 사용자가 새 그림을 선택한다.
PNG 복사는 파일 저장이 아니다. 편집 가능한 원본은 `.ntdr`로 따로 보관한다.
복사 성공 뒤에도 그림과 탭을 유지하고, 실패하면 PNG 내려받기를 안내한다.

## 유저스크립트와 자동 첨부는 별개

DOM에 버튼을 추가하는 것과 사이트에 이미지를 첨부하는 것은 다른 계약이다.
upstream 작성창에는 `data-testid="post-form-text"` textarea와 이미지 붙여넣기 처리가 있지만,
이는 모든 포크의 안정된 공개 API가 아니다. 실제 계정에서 첨부·게시는 수행하지 않았다.
범용 스크립트는 호스트 textarea의 본문을 읽어 보내거나 수정하지 않으며,
사이트별 내부 API와 인증 정보를 찾지 않는다. CSP·COOP·관리자 sandbox·DOM 구조에 따른
제한을 문서화하고 수동 PNG 경로를 유지한다.

## 소스 조사와 한계

기준: upstream 2026.9.1. 계정 인증, 플러그인 설치, Drive 업로드, 게시를 수행하지 않았다.

| 경로 | 가능한 범위 | 자동 PNG 첨부에 부족한 부분 |
|---|---|---|
| 설치형 플러그인 | AiScript 실행, 작성창 액션, 외부 링크 | PNG Blob 수신과 브라우저 message 핸들러가 노출되지 않음 |
| 게시 인터럽터 | 기존 Drive fileIds 등 게시 데이터 수정 | 새 PNG 바이트를 얻어 업로드하는 통로는 아님 |
| 외부 확장 설치 | 플러그인·테마 가져오기 | 범용 브라우저 JS 플러그인 실행기가 아님 |
| 공유창/PWA | 텍스트, URL, 기존 Drive fileIds | 기본 share target에 PNG 파일 수신 없음 |
| URL 미리보기 iframe | 외부 콘텐츠 표시 | 기존 작성창 자동 첨부와 별개; NyatiDraw 적용 미검증 |
| 작성창 붙여넣기 | 클립보드 이미지 첨부 | 사용자가 탭을 돌아와 직접 붙여넣어야 함 |

공식 자료:
- [플러그인 구현](https://github.com/misskey-dev/misskey/blob/2026.9.1/packages/frontend/src/plugin.ts)
- [작성창과 이미지 붙여넣기](https://github.com/misskey-dev/misskey/blob/2026.9.1/packages/frontend/src/components/MkPostForm.vue)
- [PWA 공유 설정](https://github.com/misskey-dev/misskey/blob/2026.9.1/packages/backend/src/server/web/manifest.json)
- [AiScript UI API](https://misskey-hub.net/en/docs/for-developers/plugin/plugin-api-reference/)

기존 MiAuth/Drive 업로드 제안은 이번 계획에서 제외한다.
일반 웹사이트가 소유한 iframe/팝업에서 PNG를 직접 돌려받으려면 기존
[SDK 연동](../web-embedding.md)을 사용한다. 두 경로를 혼동하지 않는다.

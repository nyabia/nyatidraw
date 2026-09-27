# Misskey 간편 연결

## 채택한 범위

위젯에 **그림 그리기** 링크를 두고 일반 NyatiDraw Web을 새 탭에서 연다.
그린 뒤 **PNG 복사**를 누르고 원래 Misskey 작성창으로 돌아와 붙여넣는다.
Misskey 계정 정보, MiAuth, 별도 서버, 자동 게시나 자동 첨부는 필요하지 않다.
홈페이지 메인에 Misskey 전용 기능을 배치하지 않는다. 사용법과 예제는
[연동 가이드](../../web-integration/guide.html)에 모은다.

일반 웹판은 기존 브라우저 작업을 복원한다. 링크를 열었다고 기존 그림을 지우거나
새 그림으로 덮어쓰지 않는다. 필요할 때 사용자가 새 그림을 선택한다.
PNG 복사는 파일 저장이 아니다. 편집 가능한 원본은 `.ntdr`로 따로 보관한다.
복사 성공 뒤에도 그림과 탭을 유지하고, 실패하면 PNG 내려받기를 안내한다.

## 위젯 예제

Misskey의 AiScript App 위젯에 다음 MFM 링크를 넣을 수 있다.
서버의 버전 및 포크별 실제 실행은 별도 확인한다.

```aiscript
Ui:render([
  Ui:C:mfm({ text: "[그림 그리기 ↗](https://nyabia.github.io/nyatidraw/draw/)" })
])
```

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

# Misskey 무수정 연동 검토

조사만 수행했다. Misskey 계정 인증·Drive 업로드·노트 게시·플러그인 설치는 하지 않았다.
기준은 upstream 안정판 2026.9.1, commit `7f05994f003a1d84e80aa89aef7de4925880f408`과 공식 문서다.
다른 버전과 포크는 별도 확인이 필요하다.

## 결론

Misskey 소스를 수정하지 않는 연동은 가능하다. 다만 기본 AiScript 플러그인만으로
NyatiDraw의 PNG 반환을 받아 **이미 열려 있는 작성창에 첨부**하는 공식 경로는 없다.
가장 작은 확장은 외부 연동 페이지와 선택적인 실행용 플러그인이다.

| 방법 | 가능한 범위 | 한계 |
|---|---|---|
| AiScript 플러그인만 | 작성창 메뉴에 그림 도구 실행 항목, 외부 URL 열기 | PNG Blob 수신·현재 작성창 파일 첨부 API 없음 |
| Play/AiScript 위젯 | 제한된 UI와 게시 양식 | 임의 iframe/DOM/postMessage 호스트 대체가 아님 |
| 외부 연동 페이지 + MiAuth | PNG 수신 → Drive 업로드 → 첨부된 공유 작성창 | NyatiDraw 쪽 연동 페이지 구현과 사용자 권한 승인 필요 |
| 브라우저 확장/유저스크립트 | 현재 Misskey 화면에 버튼·편집 UI를 덧붙이는 방식 | 공식 플러그인이 아니며 DOM/버전·권한 유지보수 부담 |

## 실제 소스의 제약

- [`MkPostForm.vue` 1236–1247](https://github.com/misskey-dev/misskey/blob/7f05994f003a1d84e80aa89aef7de4925880f408/packages/frontend/src/components/MkPostForm.vue#L1236-L1247):
  플러그인 게시창 액션에는 `text`와 `cw`만 전달하며 rewrite도 두 필드만 반영한다.
- [`plugin.ts` 443–446](https://github.com/misskey-dev/misskey/blob/7f05994f003a1d84e80aa89aef7de4925880f408/packages/frontend/src/plugin.ts#L443-L446):
  `Plugin:open_url`은 `_blank`, `noopener`로 연다. 따라서 그 호출로 NyatiDraw popup 모드를
  직접 열어 기존 게시창의 opener로 결과를 돌려줄 수 없다.
- [공식 AiScript 확장 API](https://misskey-hub.net/en/docs/for-developers/plugin/plugin-api-reference/):
  브라우저 DOM/파일 Blob/postMessage API는 제공하지 않는다. `Ui:C:postForm`은
  Play/위젯용이며 문서화된 초기 필드는 text/cw/visibility/localOnly다.

## 권장 후속안 — 아직 미구현

1. 사용자가 연동 페이지에서 자신의 Misskey 서버를 선택한다.
2. [MiAuth](https://misskey-hub.net/en/docs/for-developers/api/token/miauth/)로 `write:drive`만 요청한다.
3. 연동 페이지가 NyatiDraw iframe을 직접 소유한다. 플러그인은 이 페이지를 여는 바로가기다.
4. 사용자가 완료를 누르면 SDK의 `onComplete`에서 PNG를 받아
   [`drive/files/create`](https://github.com/misskey-dev/misskey/blob/7f05994f003a1d84e80aa89aef7de4925880f408/packages/backend/src/server/api/endpoints/drive/files/create.ts)에 multipart로 올린다.
5. 반환 파일 ID로 **해당 서버의** `/share?fileIds=ID`를 열어 사용자가 글·공개범위를 확인하고 게시한다.
   기존 작성창을 갱신하는 것이 아니라 새 공유 작성창을 여는 방식이다.

[공식 공유 양식](https://misskey-hub.net/en/docs/for-users/features/share-form/)은 `fileIds`를 지원한다.
Misskey Hub 공유 중계는 서버 전용 fileIds를 제거하므로 여기서는 사용하지 않는다.
자동 게시와 `write:notes`는 초기 범위에 넣지 않는다. 이 구조는 정적 프런트엔드로 구현할 수 있으나
대상 서버의 API/CORS·MiAuth·업로드 제한과 브라우저 팝업 정책을 실제로 검증해야 한다.

## 구현 전 안전 조건

- 인증 토큰을 편집기 URL·postMessage·로그·공유 URL에 넣지 않는다. 업로드 호스트만 보유한다.
- 서버 주소는 HTTPS origin으로 검증하고 인증 callback의 일회성 세션을 대조한다.
- 인증 때문에 그림을 잃지 않도록 편집 전 인증하거나, 문서를 명시적으로 보존한다.
- SDK 수락 제한은 12초다. 장시간 업로드를 무조건 그 안에 끝낸다고 가정하지 않는다.
  호스트가 그림을 안전하게 인수한 뒤 업로드를 관리하거나 실패 시 PNG 재다운로드를 제공한다.
- 재시도 중 업로드 중복, 업로드 성공 후 작성창 취소로 남는 Drive 파일을 처리 정책에 포함한다.
- 실제 서버 업로드·게시 검증은 별도 사용자 승인과 시험용 계정/그림으로 수행한다.

---
title: 웹 편집기 SDK
description: iframe이나 팝업에서 새 그림을 그리고 PNG를 호출 페이지로 반환합니다.
sidebar:
  order: 2
---

직접 만드는 사이트라면 JavaScript SDK로 편집기를 넣을 수 있습니다.
사용자가 **완료**를 누르면 PNG를 호출 페이지에 반환합니다. 별도 NyatiDraw 업로드 서버는 필요하지 않습니다.

- **iframe**: 표시할 패널과 초기 캔버스·도구·크기·색을 지정합니다.
- **팝업**: 새 창에서 전체 편집 UI를 사용하고 결과를 돌려받습니다.

[SDK 동작 예제 열기](/nyatidraw/draw/integration/demo.html)

## 팝업 예제

```html
<button id="draw">그림 그리기</button>
<img id="result" alt="완료한 그림" />
<script type="module">
  import { openNyatiPopup } from
    'https://nyabia.github.io/nyatidraw/draw/integration/nyatidraw.js';

  let resultUrl;
  document.querySelector('#draw').addEventListener('click', () => {
    const session = openNyatiPopup({
      editorUrl: 'https://nyabia.github.io/nyatidraw/draw/',
      config: { canvas: { width: 1200, height: 800 } },
      onComplete({ blob }) {
        if (resultUrl) URL.revokeObjectURL(resultUrl);
        resultUrl = URL.createObjectURL(blob);
        document.querySelector('#result').src = resultUrl;
      },
      onError(error) { console.error(error); },
    });
    session.ready.catch(console.error);
  });
</script>
```

이 코드는 반환 이미지를 화면에만 표시합니다. 영구 저장·첨부·게시는 호출한 사이트가 담당합니다.
SDK를 자신의 사이트에서 제공할 수도 있습니다. 이때 `editorUrl`을 명시하세요.

## 저장과 안전 경계

연동 세션은 일반 웹판의 복구 작업과 분리된 새 그림입니다.
완료 전 창을 강제로 닫거나 호출 페이지를 새로고침하면 임시 작업을 잃을 수 있습니다.
v1은 초기 PNG/NTDR 가져오기를 제공하지 않습니다.

`onComplete`은 필수입니다. 결과를 수락한 후에만 완료되어야 합니다.
오류나 시간 초과가 발생하면 편집기는 열린 채로 재시도·직접 다운로드 경로를 제공합니다.

HTTPS 또는 localhost에서 사용하세요. 팝업은 클릭 핸들러 안에서 동기적으로 열어야 합니다.
사이트의 팝업 차단, CSP, COOP 정책에 따라 연동이 제한될 수 있습니다.
SDK는 메시지의 출처·창 참조·세션 ID와 PNG 헤더를 확인합니다.
직접 메시지 수신기를 만들면서 이 검사를 생략하지 마세요.

모든 옵션·수락 제한시간·iframe 예제는
[SDK API 계약](https://github.com/nyabia/nyatidraw/blob/main/docs/web-embedding.md)을 참고하세요.

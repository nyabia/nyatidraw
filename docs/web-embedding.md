# Web embedding (v1)

NyatiDraw Web은 새 빈 그림을 iframe 또는 별도 창에서 열고, 사용자가 **Done**을 누를 때 PNG를 호스트 페이지에 반환할 수 있다. iframe은 지정한 편집 UI만 보인다. 팝업은 전체 편집 UI를 유지한다. [동작 예제](../web-integration/demo.html)와 [ES module SDK](../web-integration/nyatidraw.js)는 빌드된 Web 앱의 `integration/` 경로에도 복사된다.

이 모드는 독립 실행 Web 앱의 작업 공간을 열거나 수정하지 않는다. 매번 새 1024×768 빈 캔버스에서 시작하며, 별도로 설정하지 않으면 Pen, 10px, `#000000`, 불투명도 1이다. v1은 초기 PNG/NTDR 가져오기를 제공하지 않는다. 프레임/팝업을 닫거나 호스트 페이지를 새로고침하면 진행 중인 임시 작업은 사라질 수 있으므로, 사용자가 Done을 누르고 호스트가 반환값을 받아야 결과가 남는다. 자동 업로드·게시·클립보드 복사는 하지 않는다.

## 빠른 통합

SDK와 편집기 파일을 함께 제공하면 SDK의 기본 `editorUrl`은 모듈 위치의 한 단계 위다. 다른 위치의 편집기를 쓰면 `editorUrl`을 명시한다. 둘 다 HTTPS 또는 localhost 개발 URL이어야 한다.

```html
<div id="drawing" style="height: 70vh"></div>
<script type="module">
  import { mountNyatiIframe, openNyatiPopup } from "/draw/integration/nyatidraw.js";

  const options = {
    // editorUrl: "https://example.com/draw/",
    config: {
      canvas: { width: 1200, height: 800 },
      tool: "pen", size: 10, color: "#000000", opacity: 1,
      ui: { layers: false, navigator: false },
    },
    async onComplete({ blob, bytes, width, height }) {
      // Store or process the PNG here. Reject/throw if acceptance fails.
      await saveDrawing(blob, width, height);
    },
    onCancel() { /* The user chose Cancel or Close inside the editor. */ },
    onError(error) { console.error(error); },
    onClose({ reason }) { console.log(reason); },
  };

  const embedded = mountNyatiIframe(document.querySelector("#drawing"), options);
  embedded.ready.catch(error => console.error(error));

  // Call this synchronously inside a user click handler; do not await first.
  document.querySelector("#open-popup").addEventListener("click", () => {
    const popup = openNyatiPopup(options);
    popup.ready.catch(error => console.error(error));
  });
</script>
```

`onComplete`은 필수다. PNG는 `{ blob, bytes, mime: "image/png", width, height, requestId, signal }`로 전달된다. 호스트가 이 콜백에서 저장·수락 작업을 완료하면 SDK가 확인 응답을 보내고 편집기를 닫을 수 있다. 콜백이 예외를 던지거나 반환한 Promise가 거부되거나 12초 안에 끝나지 않으면 편집기는 열린 채로 오류와 재시도/직접 다운로드 경로를 보여준다. 시간 초과 시 `signal`이 abort된다. `onCancel({signal})`도 Promise를 반환할 수 있으며, 거부·시간 초과되면 편집기는 열린다. 호스트가 별도로 구현하지 않는 한 서버 전송은 일어나지 않는다.

시간 초과는 호스트의 비동기 저장이 실제로 취소되었다는 증거가 아니다. 저장 요청에는 `signal`을 전달하고, 이미 제출된 요청이 늦게 완료되거나 사용자가 Done을 다시 누를 수 있으므로 호스트 저장을 자체 그림 ID나 콘텐츠 해시 기준으로 중복 처리 가능하게 설계한다. 늦게 끝난 콜백은 이전 요청에 대한 확인 응답을 보내지 않는다.

`ready`는 초기화 성공 시 resolve한다. `element`(iframe) 또는 `window`(팝업) 속성으로 열린 표면을 얻을 수 있다. `dispose()`는 세션을 명시적으로 폐기하므로, 진행 중인 그림이 있을 수 있다면 사용자에게 확인을 받은 뒤 호출한다. 정상 Done/Cancel 뒤에는 `onClose({reason})`가 `completed`/`cancelled`로 호출된다. 팝업을 브라우저 UI로 직접 닫으면 `window-closed`, `dispose()`는 `disposed`다.

## 설정 범위

| 키 | 허용 범위 | 기본값 |
| --- | --- | --- |
| `canvas.width`, `canvas.height` | 각각 1–4096 정수, 합계 최대 16,777,216 픽셀 | 1024, 768 |
| `tool` | `pen`, `pencil2h`, `pencil2b`, `brush`, `eraser` | `pen` |
| `size` | 0.1–200; 편집기 적용 시 가장 가까운 0.1px로 반올림 | 10 |
| `color` | `#RRGGBB` | `#000000` |
| `opacity` | 0.01–1 | 1 |
| `ui` | 아래 키 각각 boolean | 모두 `true` |

`ui` 키는 `toolbar`, `tools`, `brush`, `colors`, `layers`, `history`, `navigator`다. 지정하지 않은 키는 표시된다. 이 표시 설정은 iframe에서만 적용되며 팝업은 전체 UI다. 초기값은 세션의 새 문서와 편집 도구에 적용된다. 이미 열린 편집기의 설정을 바꾸는 API는 v1에 없다.

전체 패널은 가급적 1280×900 정도의 공간을 제공한다. 720px 안팎의 작은 임베드에는
도구·색상처럼 필요한 패널만 남기거나 캔버스 전용 구성을 권장한다. 패널 숨김은 UI 설정이지
기능 접근 권한이나 보안 경계가 아니다. 실제 확인 범위는 [연동 검증 기록](measurements/web-integration.md)을 참고한다.

## 호스팅과 보안

- WebGPU, JavaScript, secure context가 필요하다. HTTPS를 쓰고, 로컬 개발만 `http://localhost`/`127.0.0.1`/`[::1]`를 쓴다. `file:` 페이지는 지원하지 않는다. 브라우저에서 WebGPU가 불가능하면 편집기의 오류를 그대로 안내한다.
- iframe을 직접 만들 경우 `sandbox="allow-scripts allow-same-origin allow-downloads"`를 포함한다. 호스팅 서버의 `Content-Security-Policy: frame-ancestors`도 실제 호스트를 허용해야 한다. SDK는 sandbox 속성을 설정한다.
- 팝업은 사용자 클릭 핸들러 안에서 동기적으로 열어야 한다. 팝업 차단, `Cross-Origin-Opener-Policy`에 의한 opener 분리, `window.open` 차단은 반환 채널을 끊을 수 있다. 이 경우 호스트에 오류를 표시하고, 편집기에서 직접 PNG/NTDR 다운로드를 사용하게 한다. 호스트와 편집기 서버의 COOP 헤더를 함께 점검한다.
- 호스트와 편집기는 `postMessage`의 정확한 origin, 창 참조, 임의 세션 ID, 프로토콜 버전을 확인한다. `*` 또는 `null` origin을 수신 대상으로 사용하지 않는다. 편집기 URL은 신뢰할 수 있는 배포 주소로 지정한다. 서로 다른 출처에서 SDK 모듈을 직접 가져오면 그 서버가 일반적인 ES module CORS 정책도 충족해야 한다.
- 반환 PNG는 호스트에서 크기·서명·IHDR 치수를 검사한 뒤 콜백에 넘긴다. 이 검사는 이미지 디코딩이나 서버 측 파일 검증을 대신하지 않는다. 호스트가 영구 저장할 경우 자체 파일 검사와 저장 오류 처리가 필요하다.

## 메시지 계약

SDK와 편집기 간 메시지는 `{channel:"nyatidraw",version:1,session,type,...}`다. URL은 `nyatiMode=embed|popup`, `hostOrigin`, `session`을 전달한다. 흐름은 `ready` → `init {config}` → `initialized`다. Done은 `complete {requestId,mime,width,height,bytes:ArrayBuffer}`를 보내며, 호스트의 수락 뒤 `ack {requestId}`, 이어 `closed {requestId}`가 온다. Cancel/Close는 `cancel {requestId}` → `ack` → `closed`다. 수락 실패는 `error {requestId,message}`로 보내며 작업을 보존한다. `ready` 이후 응답이 없거나 팝업 채널이 끊기면 열린 편집기에서 다시 시도하거나 직접 다운로드한다.

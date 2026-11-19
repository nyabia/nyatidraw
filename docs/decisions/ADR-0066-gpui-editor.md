# ADR-0066. Windows와 웹에서 Longbridge GPUI Kit 편집기 공유

## 결정

Windows 실행 파일과 웹 진입점이 같은 `nyatidraw_editor_gpui::EditorView`를 생성한다. 도구, 색상환, 레이어, 도킹, 히스토리, 타임라인, 설정, 캔버스 크기 대화상자의 Rust 소스는 `crates/editor-gpui`에 있다. Longbridge가 배포한 `gpui-kit = 0.10.0`의 `component`와 `assets`를 사용한다. Cargo.lock의 `gpui-pre 0.3.12`는 해당 Kit 릴리스가 선택한 기반 라이브러리다. [공식 저장소](https://github.com/longbridge/gpui-kit)

레이아웃은 기존 파일의 `DockTree`를 읽고 같은 `DockCommand`로 갱신한다. 이를 통해 사용자별 도킹 배치와 상단 도구 모음의 순서가 보존된다. 명령은 그 조작을 표시한 화면의 revision을 기준으로 제출한다. 새 프로젝트를 열면 이전 프로젝트의 입력란과 편집 중 상태를 폐기한다.

## 플랫폼 경계

| 구성 | Windows | 웹 |
|---|---|---|
| 호스트 | `apps/desktop/src/gpui_host.rs` | `apps/gpui-web` 및 `apps/web/src/host.rs` |
| 그리기 | 네이티브 펜·원시 마우스 입력, bounded worker, DirectComposition, wgpu 26.0.1 | Pointer Events, WebGPU, wgpu 30.0.2 |
| 저장 | 기존 프로젝트 writer, 충돌 복구와 종료 처리, sibling PNG export | 기존 정적 Worker, 브라우저 복구 저장, 파일 열기·내려받기 |
| 호스트 UI | 파일 선택, 앱 종료 및 업데이트 | 브라우저 권한, 임베드 완료·취소, PNG 전달 |

펜 샘플은 양쪽에서 UI 명령 큐를 거치지 않는다. 브러시, 작품, 히스토리, 프로젝트 포맷은 기존 엔진을 사용한다. 웹의 GPUI와 드로잉 렌더러가 같은 wgpu 버전을 사용해야 wasm-bindgen이 WebGPU 타입을 한 번만 내보낼 수 있다. 네이티브 렌더러의 버전은 Windows 경로에 고정한다.

## 빌드

Windows 기본 진입점은 `cargo run --locked -p nyatidraw-desktop`이다. `tools/build-dev.ps1`로 release 실행 파일을 만들 수 있다. 개발 설치 및 Velopack 패키징은 이 실행 파일을 사용한다.

`tools/build-web.ps1`은 `target/gpui-web/release/public`에 정적 편집기를 생성한다. GPUI 웹 의존성인 `wasm_thread`에 nightly가 필요하여 웹 UI 빌드에 `nightly-2026-06-09`를 사용한다. 저장 Worker는 기존 도구 모음과 `wasm-bindgen-cli 0.2.127`을 사용한다. 호스팅할 때 편집기 JS, WASM, Worker 및 연동 파일을 같은 빌드의 디렉터리에서 배포한다.

alpha.24부터 Windows 설치 패키지와 웹 배포는 GPUI 편집기를 빌드한다. 이전 Dioxus 화면은 회귀 비교용으로 선택해 빌드할 수 있다.

## 검증 범위

- Windows 릴리스 실행 파일의 실제 창을 Windows Graphics Capture로 검사했다. 시스템 커서와 합성 마우스 버튼으로 획, 실행 취소·다시 실행 및 스포이트 확대경을 확인했다. 임시 NTDR을 저장하고 프로세스를 종료한 뒤 다시 열었을 때 1281×999 캔버스와 작품 루트가 일치했다.
- 별도 프로필의 Edge에서 그림을 그려 NTDR을 내려받았다. 브라우저를 완전히 재시작해 복구한 그림과 파일 열기로 같은 NTDR을 불러온 그림의 캔버스 픽셀이 일치했다. 다른 출처의 iframe에서도 완료 버튼을 누르면 호스트가 그린 내용이 있는 PNG를 받았다.
- 최적화된 웹 빌드에서 색상환으로 빨간색을 선택해 그린 후 다시 열었을 때 714×758 캔버스가 픽셀 단위로 일치했다. 보기 각도를 35°로 입력한 후 0°로 초기화해 같은 픽셀로 돌아오는 것도 확인했다. WebGPU와 WebGL2를 모두 비활성화한 환경에서는 HTML 복구 버튼이 IndexedDB에 보관된 원본과 같은 바이트를 내려받았다.
- GPUI Kit의 웹 UI는 캔버스에 그려진다. 확인한 Edge 접근성 트리에는 패널의 버튼과 슬라이더가 나타나지 않았다. 이전 DOM UI 수준의 스크린 리더 지원에는 GPUI 의미 트리를 브라우저에 전달하고 포커스와 동작을 되돌려 보내는 어댑터가 필요하다. [GPUI Kit 웹 지원 범위](https://gpui-kit.com/docs/webassembly/#web-capabilities-to-plan-for)
- 실제 Windows 펜과 iPad의 Apple Pencil, 기기별 감촉과 화면 표시 지연은 별도 확인 대상이다.

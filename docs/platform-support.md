# 플랫폼 지원 상태

크로스플랫폼 경계를 설계한 것과 해당 플랫폼을 출시 지원하는 것은 같은
뜻이 아니다. 아래 표는 현재 구현·검증 상태를 구분한다.

| 플랫폼 | 상태 | 근거와 공백 |
|---|---|---|
| Windows | Sprint 3 software vertical slice accepted; shell migrated | Dioxus Desktop UI와 child-HWND native WGPU canvas가 빌드되고 DX12 first-present 및 native mouse stroke/durability 로그가 있다. 실제 펜, live DPI/resize, visual checkerboard, high-refresh percentile acceptance는 대기 중이다. |
| Linux | 코어 교차검사 완료 | Linux target에서 `input-platform` 코어 check는 통과했다. 네이티브 Wayland/X11 셸·입력 구현은 없다. |
| wasm/web | 초기 실험판 배포; 공용 UI 통합은 로컬 검증 | Dioxus Web 호스트·공용 CPU 브러시/타일·wgpu WebGPU 합성·IndexedDB 복구. 현재 작업 트리는 Desktop과 `editor-ui`를 공유한다. 기능 동등성은 미완료이며 실제 펜·모바일·전체 브라우저 프로세스 재시작·device loss·성능 acceptance는 미검증이다. [범위와 실행](web-support.md) |
| macOS | 명시적 보류 | 이번 구현 단계에서는 native 지원을 deferred로 한다. AppKit 셸·입력 구현은 없다. |

따라서 현재 문서의 “platform-neutral”은 이후 adapter를 수용할 수 있는
구조를 뜻할 뿐, Linux·macOS 네이티브 앱이나 웹의 정식 출시 지원을 뜻하지 않는다.
웹 실험판의 브라우저 접근 가능성과 해당 OS의 네이티브 창·펜·파일 연결 지원은 별개다.
이번 웹 작업은 이전 웹 보류를 제한된 스파이크 범위에서만 변경하며, Windows 설치본과
Linux/macOS 네이티브 보류는 그대로 유지한다. 웹의 CPU-authoritative 경로를 Windows의
GPU-first 성능과 동등하다고 해석하지 않는다.

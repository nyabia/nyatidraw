# 플랫폼 지원 상태

크로스플랫폼 경계를 설계한 것과 해당 플랫폼을 출시 지원하는 것은 같은
뜻이 아니다. 아래 표는 현재 구현·검증 상태를 구분한다.

| 플랫폼 | 상태 | 근거와 공백 |
|---|---|---|
| Windows | Sprint 3 software vertical slice accepted; shell migrated | Dioxus Desktop UI와 child-HWND native WGPU canvas가 빌드되고 DX12 first-present 및 native mouse stroke/durability 로그가 있다. 실제 펜, live DPI/resize, visual checkerboard, high-refresh percentile acceptance는 대기 중이다. |
| Linux | 코어 교차검사 완료 | Linux target에서 `input-platform` 코어 check는 통과했다. 네이티브 Wayland/X11 셸·입력 구현은 없다. |
| wasm/web | 코어 교차검사 완료 | wasm target에서 `input-platform` 코어 check는 통과했다. 웹 호스트와 입력 구현은 장기 과제다. |
| macOS | 명시적 보류 | 이번 구현 단계에서는 native 지원을 deferred로 한다. AppKit 셸·입력 구현은 없다. |

따라서 현재 문서의 “platform-neutral”은 이후 adapter를 수용할 수 있는
구조를 뜻할 뿐, Linux·macOS·web의 출시 지원을 뜻하지 않는다.

# Sprint 0 — 기술 위험 제거

## 현재 상태

**셸 판정 완료, 하드웨어 gate는 열림.** 최초 Dioxus Native custom-canvas
가설은 실제 crash와 pointer 한계로 철회했다. [ADR-0008](../decisions/ADR-0008-dioxus-desktop-child-canvas.md)의
Dioxus Desktop UI + child-HWND WGPU/input seam으로 전환했고 DX build와 first-present
software evidence를 확보했다. 실제 Windows 펜과 DPI/dock/modal 시각 수용성은
장비 gate로 계속 추적한다.

## 목표

실제 스타일러스 입력과 native WGPU canvas를 안정적으로 호스팅하면서 UI framework를
교체 가능한 셸로 제한할 수 있는지 판정한다.

## 작업

- 최소 Cargo workspace와 의존성 방향 lint 구성.
- Windows 실제 pen recorder: pressure, eraser, barrel button, timestamp, device id.
- winit event loop + Dioxus Native panel + custom wgpu canvas spike.
- 한 device/queue와 resize, DPI change, dock resize, suspend/resume 검증.
- 0바이트 프로젝트 파일 초기화 후보 backend spike.
- startup/input/present tracing 골격과 benchmark JSON schema.
- macOS AppKit, Wayland backend는 compile contract와 fixture format부터 정의.

## 게이트

- 실제 Windows 펜으로 raw sample log를 남긴다.
- canvas 위에서 UI rerender·dock resize 중에도 begin/end가 유실되지 않는다.
- wgpu/winit/raw-window-handle 중복 major를 허용하지 않는다.
- first-window와 first-present를 release build에서 측정한다.
- 빈 파일은 초기화되고 invalid non-empty 파일은 보존된다.

## 판정

- 채택: 입력과 surface ownership은 child HWND로 끌어올리고 Dioxus Desktop은 panel DOM으로 제한한다.
- 철회: Dioxus Native/Blitz custom-paint 셸.
- 실패: `ui-dioxus`만 대체한다. document/brush/project API 설계는 유지한다.

## 제외

레이어 UI, brush preset editor, Vello, production project schema.

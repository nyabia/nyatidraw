# Windows render-thread 격리: 구현 전 소유권 검토

2026-09-06, source `97d9670`. **설계 초안이며 구현·수용 완료가 아니다.**
현재 UI thread surface 대기를 줄이는 다음 작업의 범위와 안전 조건을 정리한다.

## 현재 경로

`desktop_canvas.rs`의 `CanvasWindowState`가 입력 adapter와
`CanvasSurfaceRenderer`를 함께 소유한다. WM_PAINT, resize, activation 및 close
복구에서 렌더 호출이 가능하다. `get_current_texture`, surface configure, blit/present와
`SharedGpuCanvas::activate_project`의 이전 writer drain도 이 UI thread에서 실행된다.
WM_PAINT 병합은 입력보다 먼저 실행되는 불필요한 렌더를 줄였지만 대기 자체를
옮기지는 않았다(ADR-0028). Raw queue와 WebView 분리만으로 격리가 완료되지 않는다.

현재 raw surface의 안전 근거는 HWND가 소유한 state가 WM_NCDESTROY에서 renderer를
먼저 drop한다는 것이다. Renderer만 별도 thread로 이동하면 이 근거는 성립하지 않는다.
또한 [DXGI 지침](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/dxgi-best-practices)은
다른 thread의 DXGI 호출이 창 thread의 메시지 처리를 기다릴 수 있음을 설명한다.
UI가 live render worker를 동기 join하는 설계는 피해야 한다.

## 제안하는 역할

| 소유자 | 담당 | 넘기지 않을 것 |
|---|---|---|
| Windows UI thread | HWND 생성/배치/입력·capture, message pump, modal과 종료 승인 | GPU 실행 중의 mutex 획득이나 live worker join |
| Render worker | surface/device/scene, frame 처리, renderer-side command 반영 | UI 객체나 WndProc state의 mutable reference |
| 기존 writer | CPU materialization, redb, preview와 export FIFO | raw OS message나 HWND |

Raw sample은 기존 bounded primary/retry lane을 그대로 사용한다. 새 render wake는
최대 한 개 pending 상태로 병합하고, 처리 중 도착한 wake는 다음 작업에 남긴다.
Resize는 최신 geometry 값 하나로 병합할 수 있지만 Close, failure recovery와
project activation의 상태 전이는 덮어쓰지 않는다. 이 메시지에 픽셀을 넣지 않는다.
GPU/파일 작업은 mailbox mutex를 해제한 뒤 실행한다.

## 구현 전에 해결할 조건

1. **HWND 생존:** 정상 Close뿐 아니라 WebView 초기화 실패/unwind, component drop,
   parent destruction에서도 worker의 surface drop이 HWND 파괴보다 먼저 일어나야 한다.
   단순 정수 handle 전달이나 `unsafe impl Send`는 이 증명이 아니다. Parent `Arc`
   보존을 택한다면 drop 통지 누락·순환 소유와 event-loop 종료 순서를 함께 검토한다.
   Dioxus on-window callback은 WebView 생성보다 먼저 실행된다는 점을 포함한다.
2. **종료:** 입력/명령 admission 경계를 먼저 닫고, 이전 Save와 stroke를 보존한다.
   Writer 완료와 surface 퇴역을 별도로 확인한다. UI는 계속 메시지를 처리하며,
   render worker가 실제로 끝난 뒤에만 join하고 parent Close를 승인한다.
   Export 실패 시 창과 recovery 경로를 유지해야 한다. `CloseStatus::Ready`만으로
   renderer까지 종료됐다고 가정하면 안 된다.
3. **Resize/admission 순서:** UI가 geometry를 바꾸면 새 Begin admission을 무효화한다.
   Worker가 이전 geometry의 frame을 늦게 마치더라도 이를 새 viewport로 게시하지
   못하도록 geometry epoch와 publication을 함께 검증한다. Active Begin에 고정된
   mapping과 정상 End/Cancel 보존은 별도 계약으로 유지한다.
4. **Activation:** 기존 bounded inbox의 경로를 renderer가 처리하되 HWND의 restore/
   foreground 요청은 UI가 담당한다. Target preflight → old writer drain → 새 project
   채택 또는 이전 project 복구 순서를 유지하고, 전환 중 새 입력을 잘못된 문서에
   입장시키지 않는다.
5. **오류·계측:** worker 실패나 wake 단절을 명시적으로 게시한다. Frame을 건너뛰어도
   history timing을 다른 frame에 붙이지 않는다. Canvas histogram은 실제 render
   thread에서 flush하며 UI input 시각과 present API의 기존 의미를 유지한다.

## 작은 실행 단위와 수용 기준

먼저 scratch-only 실행으로 HWND를 유지한 비동기 surface 퇴역과 message-pump 진행을
확인한다. 생성 실패, 최초 frame 전 Close, resize 중 Close, 종료 실패 후 복구를 포함한다.
그다음 실제 renderer를 연결하고 bounded wake/geometry publication을 검증한다.
자동 테스트는 새 queue 전이·입력 mapping 등 그림/입력 손실 위험의 순수 invariant에
한정한다. Window wrapper나 mock GPU 테스트를 늘리지 않는다.

설치판에서는 4K paced 입력/export campaign과 실제 mouse/keyboard 편집을 각각 수행한다.
Save/정상 Close/일반 restart 뒤 전체 tile/tree/page/history/PNG 비교, resize/minimize/
dock 반복, close failure UI와 process startup failure를 확인한다. UI message 처리와
surface wait가 다른 thread라는 증거, queue counts와 p50/p95/p99를 함께 남긴다.
이 검증 전에는 렌더 격리, physical pen 또는 120Hz 목표 통과를 선언하지 않는다.

현재 컴퓨터 사용 도구의 foreground PID 오류가 해결되기 전에는 이 설계를 설치판
수용 완료로 만들 수 없다. 현재 코드의 소유권을 추측으로 바꾸지 않고, 먼저
ADR-0036의 새 설치판 전체 Undo 측정을 마친 뒤 이 실행 단위를 진행한다.

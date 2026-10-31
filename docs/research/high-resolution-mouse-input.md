# 축소 드로잉의 마우스 정밀도 조사

상태: 소스·공식 문서 조사 후 독립 구현과 자동화·실행 검증을 완료했다. 물리 입력 품질은 미검증이며 사용자 설치본은 변경하지 않았다. 후속 결과는 마지막 절과 [검증 기록](../measurements/input-precision.md)에 있다.
아래 조사의 NyatiDraw 기준은 `750822fbed45d7afcf2ec5f304547cb0b829a7db`이며, 조사 당시 동작과 후속 구현을 구분한다.

## 결론

목표는 **마우스의 미세 이동을 보존하면서 사용자가 설정한 커서 속도와 가속을 유지하는 것**이다.
Raw Input을 고정 감도로 적분하는 방식부터 제품에 넣지 않는다.

- Windows 화면 좌표에서 정밀도가 손실되는 것은 실제 API 제약이다. 그러나 정수 타입 자체가 문제는 아니다. Raw Input도 정수 센서 count를 반환하며 화면 픽셀과 단위가 다르다.
- Krita/Qt, GIMP/GTK, Blender에서도 일반 마우스의 화면 좌표 경로를 확인했다. 타 제품이 모두 소수 센서 좌표로 그린다고 볼 근거는 없다.
- 획을 만드는 처리에는 차이가 있다. Krita의 Basic/Weighted는 Bézier 연결, GIMP는 조건부 Catmull 보간을 사용한다. NyatiDraw의 현재 기본 경로는 입력점을 직선 dab으로 연결한다.
- SDL에는 **raw 이동에 Windows 속도·가속 설정을 적용하고 소수 이동량을 유지하는 구현**이 있다. 고정 감도만 가능한 것은 아니다. 다만 이것은 relative mode의 자체 좌표 처리이며 실제 OS 커서와 완전히 같다는 보장은 아니다.
- 따라서 입력 수집과 획 구성의 영향을 따로 측정하고, SDL 방식의 후보를 실제 커서와 대조하는 순서가 타당하다. 보정 강도를 올리는 것만으로 입력 손실이 해결됐다고 판정하지 않는다.

사용자가 보고한 “축소해서 그린 뒤 확대해도 자글거림이 남는다”는 현상은 표시 필터만의 문제로 보기 어렵다. 입력 양자화가 기여할 수 있지만, 실제 입력 기록을 확보하지 않았으므로 단독 원인으로 확정하지 않았다. 사진에는 연필 질감과 확대 표시도 함께 있으므로 획 중심선의 요철과 의도한 종이 입자를 분리해야 한다.

## 서로 다른 다섯 단계

| 단계 | 보존하거나 판단해야 하는 것 |
|---|---|
| 장치 입력 | 센서 count, 절대 좌표, 장치 종류, 실제 샘플의 순서 |
| 커서 이동 | Windows 속도·가속, DPI, 장치 전환, 좌표의 소수 부분 |
| 문서 변환 | 화면→문서의 소수 좌표, zoom·회전·pan의 일관성 |
| 획 구성 | 입력점 사이 경로, dab 간격, 끝점·의도한 모서리·지연 |
| 픽셀과 표시 | 커버리지 AA, 연필 입자, 축소 필터, 확대 시 픽셀 표현 |

예를 들어 물리 화면 기준 25% 배율에서는 화면 1px가 문서 4px에 해당한다. 화면에서 이미 정수화한 좌표를 나중에 `f64`로 바꾸어도 그 사이의 실제 움직임은 돌아오지 않는다. 곡선 보간은 그럴듯한 경로를 추정하는 것이며 원래 입력의 복원은 아니다. 반대로 더 많은 raw 샘플만 받아도 dab 배치나 래스터 경계가 자동으로 좋아지는 것은 아니다.

Microsoft는 `WM_MOUSEMOVE`의 화면 해상도 제한과 Windows ballistics 적용, `WM_INPUT`의 고해상도 HID 이동과 ballistics 미적용을 구분한다. 해당 문서는 FPS 입력을 중심으로 설명하므로 이를 그대로 그림 도구의 UX 권고로 읽지 않는다. [Microsoft 설명](https://learn.microsoft.com/en-us/windows/win32/dxtecharts/taking-advantage-of-high-dpi-mouse-movement), [RAWMOUSE](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse)

`GetMouseMovePointsEx(GMMP_USE_HIGH_RESOLUTION_POINTS)`의 0…65535 좌표는 고해상도 절대 좌표 장치를 위한 설명이다. 일반 상대 마우스의 센서 정밀도를 그 플래그 하나로 얻는다고 보장하지 않는다. 이벤트 이력을 더 받는 것과 좌표 분해능을 높이는 것도 별개다. [GetMouseMovePointsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmousemovepointsex)

## 다른 제품에서 확인한 것

공개 소스는 고정 버전의 해당 경로를 확인했다. 모든 배포본의 의존성·패치 조합이나 실제 손맛을 검사한 결과는 아니다. Photoshop과 Clip Studio의 내부 Windows 마우스 API는 공개 문서만으로 확정하지 않았다.

| 제품 | 일반 마우스 입력 근거 | 획 처리 / 해석 |
|---|---|---|
| Krita + Qt | 조사한 Qt 5/6 Windows handler는 `GET_X/Y_LPARAM`의 `QPoint`에서 시작. Krita는 `localPos()`를 문서·이미지 좌표로 변환 | Basic/Weighted는 접선 기반 Bézier, None은 직선. Weighted의 선택적 배율 보정은 Basic과 구분 |
| GIMP + GTK | GTK Windows 일반 마우스는 `WM_MOUSEMOVE` 화면 좌표. 조사한 일반 device의 history 함수는 `FALSE` | exact stroke는 이벤트 압축을 끄며, 조건을 만족하면 motion buffer가 Catmull 보간. history API 호출만으로 Windows history 지원을 주장할 수 없음 |
| MyPaint | GDK 이벤트 압축을 끄고 event 좌표를 model 좌표로 변환 | libmypaint는 누적 dab 위상을 유지한 선형 보간과 preset 기반 slow tracking 사용. 압력·tilt spline을 XY spline으로 오해하면 안 됨 |
| Blender | 일반 마우스는 `WM_MOUSEMOVE`. 확인한 Raw Input 등록은 키보드·선택적 NDOF | Continuous Grab은 cursor warp와 이동량 누적. 일반 마우스 페인팅의 raw counts 사용 사례가 아님 |
| Aseprite | LAF 일반 mouse는 `GET_X/Y_LPARAM / scale` | 도구 단계에서 픽셀 좌표를 사용. 픽셀아트의 의도한 정수화는 일러스트 경로 품질과 별개 |
| Photoshop | 공식 문서로 내부 mouse backend를 확인할 수 없음 | Smoothing, 끝점 catch-up, Adjust for Zoom 제공. 공식 최적화 안내의 기본 smoothing은 10% |
| Clip Studio Paint | 고정밀 tablet 설정을 일반 mouse raw 입력의 증거로 사용할 수 없음 | Stabilization, post correction, 커서 지연 표시 옵션 제공. Adjust by scale은 조사한 문서상 post correction에 속함 |

### Krita / Qt 근거

Krita v5.2.9: `ac6cae86d02ce033e81f20f70fdda10aa4fd8dfb`.
Qt 5.15.16: `8ad94db938533e1b7a96f9baddffbcdbae6909d3`.
Qt 6.8.3: `c07c2d5a527a644d36e7853d55132ae38921682f`.

- [Qt5 pointer handler](https://github.com/qt/qtbase/blob/8ad94db938533e1b7a96f9baddffbcdbae6909d3/src/plugins/platforms/windows/qwindowspointerhandler.cpp#L720-L748)와 [legacy handler](https://github.com/qt/qtbase/blob/8ad94db938533e1b7a96f9baddffbcdbae6909d3/src/plugins/platforms/windows/qwindowsmousehandler.cpp#L250-L276)는 일반 마우스의 정수 화면 좌표를 사용한다. 같은 파일의 HIMETRIC 고정밀 경로는 펜용이다.
- [Krita view→document](https://github.com/KDE/krita/blob/ac6cae86d02ce033e81f20f70fdda10aa4fd8dfb/libs/ui/canvas/kis_tool_proxy.cpp#L126), [document→image](https://github.com/KDE/krita/blob/ac6cae86d02ce033e81f20f70fdda10aa4fd8dfb/libs/ui/tool/kis_painting_information_builder.cpp#L117-L118).
- [Bézier / None 분기](https://github.com/KDE/krita/blob/ac6cae86d02ce033e81f20f70fdda10aa4fd8dfb/libs/ui/tool/kis_tool_freehand_helper.cpp#L564-L595), [Weighted의 배율 보정](https://github.com/KDE/krita/blob/ac6cae86d02ce033e81f20f70fdda10aa4fd8dfb/libs/ui/tool/kis_tool_freehand_helper.cpp#L425-L438), [공식 brush smoothing 설명](https://docs.krita.org/en/reference_manual/tools/freehand_brush.html).

### GIMP / GTK / MyPaint 근거

- [GTK 3.24.43 Windows motion](https://github.com/GNOME/gtk/blob/788f4de2799b3f5902d27733ea603e8dfee8d0a6/gdk/win32/gdkevents-win32.c#L2736-L2814), [Win32 history 미지원](https://github.com/GNOME/gtk/blob/788f4de2799b3f5902d27733ea603e8dfee8d0a6/gdk/win32/gdkdevice-win32.c#L31-L40).
- [GIMP 3.0.0 history fallback](https://github.com/GNOME/gimp/blob/9785099e5a0dcdc3011dd26c6c2e9b332d36c035/app/display/gimpdisplayshell-tool-events.c#L897-L960), [이벤트 압축 해제](https://github.com/GNOME/gimp/blob/9785099e5a0dcdc3011dd26c6c2e9b332d36c035/app/display/gimpdisplayshell-tool-events.c#L559-L564), [motion buffer](https://github.com/GNOME/gimp/blob/9785099e5a0dcdc3011dd26c6c2e9b332d36c035/app/display/gimpmotionbuffer.c#L295-L405), [Catmull](https://github.com/GNOME/gimp/blob/9785099e5a0dcdc3011dd26c6c2e9b332d36c035/app/display/gimpmotionbuffer.c#L539-L569). 이 보간은 event-fill과 history 조건, 6 image pixels 이상의 거리를 요구하며 모든 샘플에 항상 적용되는 것은 아니다.
- [MyPaint 2.0.1 압축 해제](https://github.com/mypaint/mypaint/blob/478280f2da4d5377b8185c3cc3e3a0500b93749c/gui/tileddrawwidget.py#L203-L206), [event 좌표](https://github.com/mypaint/mypaint/blob/478280f2da4d5377b8185c3cc3e3a0500b93749c/gui/freehand.py#L370-L414), [압력·tilt 보간](https://github.com/mypaint/mypaint/blob/478280f2da4d5377b8185c3cc3e3a0500b93749c/gui/freehand.py#L770-L793).
- [libmypaint 1.6.1 누적 dab phase](https://github.com/mypaint/libmypaint/blob/2768251dacce3939136c839aeca413f4aa4241d0/mypaint-brush.c#L1425-L1471), [시간 기반 tracking](https://github.com/mypaint/libmypaint/blob/2768251dacce3939136c839aeca413f4aa4241d0/mypaint-brush.c#L1391-L1393), [zoom·dab 기반 tracking](https://github.com/mypaint/libmypaint/blob/2768251dacce3939136c839aeca413f4aa4241d0/mypaint-brush.c#L681-L735).

### Blender / Aseprite / 상용 제품 근거

- Blender `4f97ee24309d76d151432600cb1a175333421053`: [Raw Input 등록 대상](https://github.com/blender/blender/blob/4f97ee24309d76d151432600cb1a175333421053/intern/ghost/intern/GHOST_SystemWin32.cc#L132-L155), [일반 mouse](https://github.com/blender/blender/blob/4f97ee24309d76d151432600cb1a175333421053/intern/ghost/intern/GHOST_SystemWin32.cc#L1992-L2015), [continuous grab](https://github.com/blender/blender/blob/4f97ee24309d76d151432600cb1a175333421053/intern/ghost/intern/GHOST_SystemWin32.cc#L1168-L1251).
- Aseprite `3669f4774d181d77deb3e6db95fe6df8ca347410`와 LAF `67424920043b8bfe895bba4420e51b01e5ccb7d0`: [Windows 입력](https://github.com/aseprite/laf/blob/67424920043b8bfe895bba4420e51b01e5ccb7d0/os/win/window.cpp#L2098-L2166), [도구 좌표](https://github.com/aseprite/aseprite/blob/3669f4774d181d77deb3e6db95fe6df8ca347410/src/app/ui/editor/drawing_state.cpp#L254-L279).
- Photoshop: [stroke smoothing](https://helpx.adobe.com/photoshop/desktop/repair-retouch/clean-restore-images/create-smoother-more-polished-brush-strokes-with-stroke-smoothing.html), [기본값과 성능](https://helpx.adobe.com/photoshop/kb/optimize-painting-and-cloning.html).
- Clip Studio Paint: [Correction](https://help.clip-studio.com/en-us/manual_en/810_subtools/C.htm), [Preferences](https://help.clip-studio.com/en-us/manual_en/720_preferences/Preferences.htm).

## SDL: 가속과 소수 이동량을 함께 다루는 후보

SDL은 드로잉 앱의 채택 사례가 아니라 입력 구현의 참고 사례다. 이 조사 때문에 SDL을 의존성으로 추가하지 않는다.

[SDL3 공식 옵션](https://wiki.libsdl.org/SDL3/SDL_HINT_MOUSE_RELATIVE_SYSTEM_SCALE)은 relative mouse motion에 시스템 가속 곡선을 적용할 수 있다고 설명한다. 고정 커밋 `cb433584b0776f5f9a31ded79c30f4de0888a9d2`의 Windows 구현은 다음을 한다.

1. `SPI_GETMOUSESPEED`, `SPI_GETMOUSE`, `SmoothMouseXCurve/YCurve`에서 속도·EPP·곡선 설정을 읽는다.
2. raw delta에 설정과 display scale을 반영한다.
3. 최종 정수 pixel 반올림을 생략하고 fractional 이동량을 반환한다.
4. relative mode에서 이를 누적해 자체 좌표를 만든다. 기존 absolute OS cursor를 고정밀 API로 바꿔 읽는 것은 아니다.

[Windows scale 구현](https://github.com/libsdl-org/SDL/blob/cb433584b0776f5f9a31ded79c30f4de0888a9d2/src/video/windows/SDL_windowsmouse.c#L737-L825), [설정 읽기](https://github.com/libsdl-org/SDL/blob/cb433584b0776f5f9a31ded79c30f4de0888a9d2/src/video/windows/SDL_windowsmouse.c#L858-L923), [relative mode 적용](https://github.com/libsdl-org/SDL/blob/cb433584b0776f5f9a31ded79c30f4de0888a9d2/src/events/SDL_mouse.c#L754-L785).

이것은 Windows 가속 결과를 반환하는 공식 API가 아니라 설정을 바탕으로 재현한 알고리즘이다. 현재 구현의 scale 함수는 timestamp와 mouse ID를 사용하지 않고 패킷 이동량 및 이전 곡선 구간 상태를 사용한다. display scale은 window 기준이다. 따라서 아래 항목을 측정하기 전에는 OS 커서와 동일하다고 부르지 않는다.

- 느린 이동·빠른 이동·방향 전환에서 실제 커서와의 잔차, 긴 이동 뒤 누적 drift.
- 마우스 DPI·polling rate·배칭과 EPP on/off에 따른 차이.
- 혼합 DPI 모니터, 음수 화면 좌표, 창 이동, 장치 교체와 복수 마우스.
- 획 시작·끝의 기준점, 마지막 raw packet과 button-up의 순서, UI로 복귀할 때의 커서 점프.
- 설정 변경이나 잘못된 곡선 데이터에서 안전하게 기존 경로로 복귀하는 방식.

고정 감도는 비교용 조건으로만 남기고 기본 UX 후보로 확정하지 않는다. 앱 커서를 별도로 그린다면 OS 커서와의 차이와 끝점 처리까지 해결해야 한다. 매 샘플을 정수 OS 좌표에 강제로 맞추면 얻으려던 소수 정밀도를 다시 버릴 수 있다.

## NyatiDraw의 현재 경로

| 코드 | 확인 내용 |
|---|---|
| `crates/input-platform/src/windows.rs` | 일반 mouse history는 `GMMP_USE_DISPLAY_POINTS`; screen `POINT`/`MOUSEMOVEPOINT`를 client 좌표로 변환한 뒤 `f64`에 담음 |
| `crates/input/src/lib.rs` | 화면→문서 변환은 소수 좌표 유지. 입력 단계에서 사라진 정밀도를 복구하지는 못함 |
| `crates/input/src/smoothing.rs` | strength 0은 입력 그대로. 활성화하면 시간 기반 저역 통과 필터이며 곡선 재구성은 아님 |
| `crates/brush/src/lib.rs` | `resample_segment`가 입력점 사이 직선에서 누적 간격을 유지하며 dab 생성 |
| `crates/brush/src/pencil.rs` | 연필 커버리지·입자는 별도 단계. 중심선과 질감을 분리해 평가할 필요가 있음 |
| `crates/paint-gpu/src/viewport.wgsl` | 확대의 pixel 표시와 축소의 filtering은 저장된 획 형상과 구분 |
| `apps/web/src/browser.js` | DOM 좌표에서 canvas 위치를 빼고 coalesced events를 사용. JavaScript의 소수 타입만으로 장치 정밀도를 보장할 수는 없음 |

Windows input adapter만 바꾸는 것과 공용 brush 경로를 개선하는 것은 별도 작업이다. 웹에 Windows ballistics를 적용하지 않으며, 웹/Windows가 공유하는 획 구성·저장/재생 규약은 유지한다.

## 다음 구현을 위한 검증 순서 — 아직 미착수

### 1. 실제 입력과 그림을 나누어 기록

기존 방식의 결과를 유지한 진단 모드에서 raw packet, OS cursor 위치, history 순서, 문서 좌표, 최종 dab 중심을 함께 기록한다. 두 입력 스트림의 이벤트가 항상 1:1 대응한다고 가정하지 않는다. OS 설정은 읽기만 하고 변경하지 않는다.

25/50/100/200% 배율, 단색 round pen과 2H 연필을 분리해 비교한다. 센서·화면·문서 좌표와 저장 PNG의 중심선/가장자리를 나란히 보면 입력 요철, 질감, 표시 aliasing을 분리할 수 있다. 실제 사용자 움직임을 기록하기 전에는 합성 직선만으로 물리 마우스 품질을 통과 처리하지 않는다.

### 2. 같은 샘플로 두 축을 독립 비교

- 입력 후보: 기존 accelerated 화면 history / raw + Windows 설정 기반 fractional ballistics.
- 획 후보: 기존 직선 dab / 끝점과 의도한 모서리를 보존하는 짧은 곡선 연결.

둘을 한꺼번에 교체하지 않는다. 가속 보존, 실제 정보 보존, 곡선 추정의 효과를 구분한다. 곡선은 지연·overshoot·의도한 모서리 훼손도 평가해야 하며, 입력 정밀도가 해결됐다는 증거로 쓰지 않는다.

### 3. 통합·저장·재시작 검사

Raw Input은 한 device class에 process당 마지막 등록 target이 적용되므로 canvas가 임의로 재등록해 기존 Tao 입력을 빼앗으면 안 된다. 현재 Tao의 raw 등록과 Dioxus event-loop 전달 경계를 먼저 확인한다. [Microsoft 등록 계약](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices)

- 펜/터치 승격 mouse 이벤트와 중복 처리하지 않고 장치별 phase를 유지한다.
- bounded queue 초과, capture/focus 상실, viewport 변경, 장치 분리에서 기존 fail-closed 취소 규약을 지킨다.
- button-up에서 OS 정수 끝점으로 갑자기 선을 잇거나 raw/legacy 양쪽으로 중복 획을 만들지 않는다.
- 변경이 저장된 입력·획 재생에 영향을 주면 버전을 구분하고 기존 작품을 다시 다른 선으로 계산하지 않는다.
- 핵심 회귀 검사만 추가하고 scratch 작업의 저장→프로세스 재시작→재열기 픽셀 일치를 확인한다.
- 실제 마우스/펜의 느낌, OS 커서 정합, 고주사율 지연은 하드웨어 확인 전까지 미검증으로 남긴다.

이번 조사에서는 코드 수정, 앱 실행 비교, 테스트 추가, 빌드, 설치, 배포를 하지 않았다. 다음 작업의 우선순위는 **진단으로 원인 분리 → 가속을 보존하는 고정밀 후보와 곡선 후보 비교 → 검증한 경로만 제품에 통합**이다.

## 후속 결정: 방법만 참고하는 독립 구현

사용자는 SDL의 접근 방법을 선택했으며, 코드 이식 없이 독립적으로 구현하고 Windows 커서와 완벽하게 같지 않더라도 충분히 가까운 동작을 원한다.

이미 외부 소스를 조사한 담당자가 구현을 그대로 번역하는 것을 클린룸이라고 부르지 않는다. 기능 명세와 독립 구현을 분리한다. 구현 담당자는 SDL·Krita 등 외부 구현, 이 문서의 소스 구현 상세와 내려받은 사본을 입력으로 삼지 않고, 공개 OS API 계약·관찰 가능한 동작·NyatiDraw 인터페이스와 수용 조건으로 작업한다. 함수 본문, 상수표, 테스트 fixture를 복사하지 않는다. 이번 연결점 조사도 새로운 컨텍스트의 담당자들이 NyatiDraw 코드와 공식 API 문서만 읽도록 분리했다. 실제 구현의 독립성과 동작 품질은 별도로 기록한다.

Windows 구현 명세는 다음 경계로 나눈다.

1. 상대 마우스의 미세 이동을 수집하고 소수 좌표로 누적한다. 절대 장치·펜·터치와 혼합하지 않는다.
2. 사용자 시스템 속도와 가속 설정을 읽어서 이동 감각에 반영한다. 시스템 설정 자체는 바꾸지 않는다.
3. 실제 OS 커서와의 속도별 이동량·누적 오차를 계측한다. 완전 일치를 주장하지 않고 확인한 조건과 오차를 명시한다.
4. 메뉴·툴바는 기존 OS 커서를 사용한다. 그리기 중 자체 위치를 사용할 경우 커서 표시와 획 끝점까지 같은 위치를 쓰고, UI 복귀 시 점프를 검사한다.
5. 한 획의 입력원을 중간에 바꾸지 않는다. 잘못된 장치·설정·불연속을 감지하면 획을 안전하게 취소하고 다음 Begin부터 기존 경로로 복귀한다.

`desktop_shell.rs`의 기존 `Config::with_custom_event_handler`는 연결 후보다. 하지만 Tao의 `DeviceEvent::MouseMotion`만으로 native packet의 절대/상대 flag나 같은 packet의 button 순서를 알 수 있다고 가정하지 않는다. API/구현 계약 확인 후 기존 canvas capture·phase 처리와 연결한다.

## 웹: 같은 획 코어, 다른 입력 수집

일반 웹 페이지에는 Windows의 mouse sensitivity/EPP/registry curve를 읽는 API가 없다. 따라서 Windows 방식의 시스템 가속 재현을 그대로 이식하지 않는다. 일반 Pointer Events로 이미 처리된 커서 좌표에 자체 Windows 가속을 다시 적용해서도 안 된다.

| 경로 | 채택 방향과 한계 |
|---|---|
| Pointer Events의 소수 좌표 | 이미 보존하고 있음. 브라우저가 제공하지 않은 센서 정밀도까지 복원하는 것은 아님 |
| `getCoalescedEvents()` | 이미 사용 중. 부모 이벤트와 병합 전 샘플을 중복 투입하지 않음 |
| `pointerrawupdate` | 지원·secure context를 확인하는 선택적 저지연 경로 후보. 비가속 raw sensor 입력이라는 뜻은 아님 |
| Pointer Lock의 `unadjustedMovement` | 커서를 숨기는 별도 상대 입력 모드이므로 기본 드로잉 UX에는 채택하지 않음. 브라우저 지원·사용자 활성화·iframe 허용과 별도 감도 정책도 필요 |

`pointerrawupdate`와 `pointermove`는 같은 이동을 중복 포함할 수 있으므로 한 획에 이동 입력원 하나만 선택한다. 정지한 좌표에서 압력·기울기·버튼만 바뀌는 샘플도 유효하다. 좌표가 같다는 이유로 제거하지 않는다. `cancel`/capture 상실/blur/숨김/overflow에서는 기존 획 취소를 유지하며 capture 해제도 같은 경계로 정리한다. 예측 샘플은 실제 입력과 구분하고 저장할 확정 획에는 사용하지 않는다.

[W3C Pointer Events](https://www.w3.org/TR/pointerevents3/), [pointerrawupdate](https://developer.mozilla.org/en-US/docs/Web/API/Element/pointerrawupdate_event), [Pointer Lock](https://developer.mozilla.org/en-US/docs/Web/API/Pointer_Lock_API).

iPad/Apple Pencil도 브라우저가 제공하는 펜 좌표·필압·병합 샘플을 사용한다. Windows 상대 마우스 설정을 적용하지 않는다. 웹의 장치별 수집 한계를 넘어 동일 정밀도를 보장하지 않지만, 그 이후의 보정·곡선·dab·저장은 Windows와 같은 Rust 코어를 사용한다.

## 손떨림 보정과 곡선 연결을 함께 완성할 범위

손떨림 보정은 기본 그림 도구의 품질 요구다. 현재 one-pole 필터가 있다는 사실만으로 완성됐다고 보지 않는다. Bézier는 유용한 곡선 표현이지만 손떨림 보정에 필수인 유일한 알고리즘은 아니다. 반대로 입력점을 모두 통과하는 Bézier만 추가하면 입력의 흔들림도 따라갈 수 있다.

제안하는 공통 경로는 다음과 같다.

```text
플랫폼별 정밀 입력 수집
  → 정상화·획 승인
  → 선택적 손떨림 억제
  → 짧은 곡선 연결과 끝점 처리
  → 동일한 평가 샘플을 live 표시와 저장에 전달
  → 거리 기반 dab 생성
```

사용자에게 곡선 수식이나 보정 모드를 선택하게 하지 않는다. 손떨림 보정 UI는 기존처럼 `0…100` range 슬라이더와 우측 정렬한 숫자 입력란을 사용한다. 드래그로 강도를 연속 조절하며 `0 = 꺼짐`, 초기 기본값도 `0`이다. `없음 / 기본 / 안정화` 모드 선택안은 채택하지 않는다. 저장된 사용자 강도는 그대로 복원한다.

0에서는 손떨림 억제와 보정을 위한 곡선 정리를 모두 우회해 승인된 원본 입력을 유지한다. 강도를 올렸을 때만 그 값에 따라 보정을 적용한다. 정밀 입력 수집과 소수 좌표 보존은 보정 강도와 무관하게 동작하며, 보정을 꺼도 입력 정밀도가 낮아지지 않아야 한다. 픽셀 도구·직선 도구·선택·이동에는 브러시 보정을 자동 적용하지 않는다. 설정 변경은 다음 획부터 적용하고, 진행 중인 획의 강도는 Begin 때 고정한다.

구현 시 `StrokeSmoother`와 별도의 bounded curve 단계로 구성한다. 곡선 때문에 생기는 지연과 오버슈트, 의도한 모서리 보존, 빠른 꺾임, 느린 원, 정지 탭, 눌렀다 바로 떼는 획을 함께 검사한다. 필압 보정은 위치 보정과 별개로 둔다.

저장된 brush evaluator를 일괄 변경하면 옛 획 재생 결과가 달라질 수 있다. 따라서 기존 evaluator 앞에서 곡선을 평가 샘플로 바꾸고 그 결과를 기록하는 경계가 우선 후보다. 구현 전 확인할 조건은 다음과 같다.

- 샘플 확장 후 기록 sequence는 엄격히 증가하고 timestamp 순서는 유지할 것.
- 같은 위치의 필압 변화도 보존할 것.
- Begin은 즉시 반응하고 End는 마지막 승인 위치까지 완성할 것.
- Cancel은 보류 중인 곡선을 그리지 않고 버릴 것.
- 앱 종료 때 마지막으로 기록한 점만 사용해 아직 보류 중인 끝부분을 잃지 않을 것.
- 곡선 분할 깊이·출력 수를 제한하고, 초과를 조용한 획 누락으로 처리하지 않을 것.
- 기록한 평가 샘플을 재열 때 다시 보정하지 않을 것.

이 절은 구현 전에 정한 방향과 계약이다. 아래 후속 구현 상태와 구분한다.

## 구현 결과와 검증 경계

Windows는 기존 프레임워크의 raw receiver를 관찰하며 새 receiver로 등록을 빼앗지 않는다.
상대 장치의 raw Down과 legacy Down이 대응할 때만 해당 획을 raw 이동 경로로 고정한다.
그 외에는 기존 경로를 유지한다. Begin/End는 기존 버튼 메시지가 소유하며, 이동을 두 경로에서
중복 입력하지 않는다. 장치 변경·캡처 상실·입력 불연속은 진행 중인 획을 취소한다.

독립적인 속도/가속 근사로 소수 이동량을 누적하되, 각 queued OS 좌표의 축별 ±0.5 물리 픽셀
안으로 제한한다. 따라서 OS 커서와 멀리 어긋나는 누적 drift를 막는다. 이 값은 **정합 상한**이며
실제 센서 좌표를 복원한 오차 보증이 아니다. Windows EPP 곡선을 정확히 재현하지 않으며,
시스템 속도·가속 설정을 변경하지 않는다. 정수 배율의 raw 이동에서는 새로운 소수 정보가
생기지 않을 수 있다. 물리 장치와 여러 감도에서 실제 개선량은 별도 측정이 필요하다.

웹은 secure context와 API 지원을 확인하고, 획에서 먼저 오는 rawupdate 또는 pointermove
한 경로를 선택한다. coalesced 샘플과 부모 이벤트를 중복 사용하지 않으며, 정지한 좌표의
필압 변화도 보존한다. rawupdate는 비가속 센서 입력이라는 뜻이 아니다.

공용 `StrokePath`는 0에서 정확히 우회하고, 양수에서는 위치 안정화 뒤 bounded cubic
연결을 적용한다. 화면 배율을 Begin에서 고정하고 짧은 구간의 lookahead를 최대 2 논리 픽셀로
제한한다. 출력은 한 구간 64개, 입력 한 묶음 128개 이하이며 기존 dab 예산도 묶음 전체에
적용한다. End/종료 시 보류된 끝부분을 마무리하고 Cancel에서는 폐기한다. 평가된 샘플을
그대로 기록하므로 재열 때 보정이 중복 적용되지 않는다. 기존 브러시 evaluator는 변경하지 않는다.

공유 UI는 도구 속성의 불투명도 아래 **손떨림 보정 0–100**으로 정리했다. 초기값은 0이며
슬라이더와 숫자 입력을 함께 제공한다. 저장된 설정은 복원하고 변경은 다음 획부터 적용한다.

검증의 실제 실행 결과와 미검증 범위는 [입력 정밀도 검증](../measurements/input-precision.md)에 기록한다.

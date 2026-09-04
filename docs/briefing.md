# NyatiDraw 아키텍처 브리핑

## 한 문장

NyatiDraw는 **GPU에서 즉시 보이는 잉크**, **불변 스냅샷으로 보존되는 프로젝트**, **Godot 옆에 자동 배치되는 export**를 하나의 짧은 경로로 묶는 것을 목표로 하는 Rust 네이티브 드로잉 앱이다. Desktop Save의 generation-safe sibling PNG export와 실제 worker 완료/오류 UI, CLI 진단 export가 구현돼 있다. Save 직전 raw-input drain, retry와 종료 진행 UI는 후속 단계다.

## 이번 설계의 결론

이 프로그램을 “Dioxus로 만든 그림판”으로 만들지 않는다. 핵심은 UI와 독립된
드로잉·문서·저장 엔진이다. 현재 Dioxus Desktop은 WebView UI 셸만 맡고 Windows
child HWND의 native WGPU canvas가 실시간 입력과 그림을 맡는다.

실시간 스트로크에는 순수 CPU 권위형보다 한 단계 공격적인 구조를 쓴다.

```text
pen samples
  → normalize / coalesce / resample
  → brush evaluation on paint thread
  → GPU working tiles mutate and present immediately
  → stroke closes into immutable commit
  → affected tiles materialize to CPU/storage asynchronously
```

스트로크 중 GPU working set이 화면상의 최신 상태를 맡는다. 스트로크가 끝나면 입력 샘플, 브러시 버전·seed, 변경 전 tile 참조, 변경 후 tile 결과를 하나의 commit으로 봉인한다. 저장과 headless export가 필요로 하는 CPU/storage 객체는 뒤에서 생성하며, Save 또는 Close barrier가 최신 commit의 materialization을 기다린다.

이 모델은 세 목표를 동시에 지킨다.

- 포인터 이벤트가 UI diff나 파일 I/O를 기다리지 않는다.
- Undo는 stroke 재생 대신 tile/root 교체로 즉시 반응할 수 있다.
- 프로젝트 저장, crash recovery, CLI export는 GPU vendor에 종속되지 않는다.

## 제품 우선순위

1. 입력에서 첫 픽셀까지의 지연과 안정적인 stroke cadence
2. 빠른 첫 창, lazy project hydration, idle 시 무의미한 redraw 제거
3. 손실 없는 저장과 최신 export를 보장하는 종료 절차
4. 실제 작업 가능한 래스터·레이어·브러시 경험
5. 벡터, 애니메이션, 웹, Git unpack

## 기술 선택

| 영역 | 기본 방향 | 안전장치 |
|---|---|---|
| 앱 셸 | Dioxus Desktop + native child canvas | WebView에는 저빈도 UI metadata만 전달; raw input과 pixels는 HWND/WGPU에 유지 |
| 실시간 렌더 | wgpu | 하나의 device/queue, dirty tile upload, device-loss 복구 |
| 래스터 브러시 | GPU-first working tiles | CPU reference backend과 golden test 유지 |
| 벡터 | 후속 Vello adapter | 도메인 vector model은 Vello 타입을 저장하지 않음 |
| 프로젝트 | 단일 바이너리 repository | 저장 엔진은 trait 뒤에 격리, wire schema는 명시적 버전 관리 |
| 히스토리 | content-addressed roots + operation DAG | “무한”은 디스크 한도이며 명시적 optimize에서만 GC |
| export | immutable snapshot worker | temp 파일 작성 후 교체, generation check로 역전 방지 |

## 가장 큰 위험

- WebView와 native child canvas의 z-order, DPI, modal 가시성 계약이 어긋날 수 있다.
- 플랫폼별 스타일러스 지원 범위가 다르며, Linux는 display backend별 격차가 크다.
- GPU stroke의 결과를 비동기로 CPU/storage에 옮길 때 readback 비용과 저장 지연이 커질 수 있다.
- 브러시 결과의 GPU 간 완전한 bitwise 재현은 비현실적일 수 있다.

엔진 수직 절단은 이미 확보했다. 활성 첫 스프린트는 이를 사용자가 실제로 쓸 수 있는
**Godot project의 PNG를 Windows에서 NyatiDraw로 열기 → sibling `.ntdr` 확인 →
그리기 → 저장 → sibling PNG 갱신 → 종료 → reopen** 흐름으로 묶는다. Godot editor
addon은 활성 스프린트 밖이다. 게이트를 통과하지 못한 기술은 애착 없이 교체한다.

## 지금 구현하지 않는 것

벡터 편집, 애니메이션 timeline, 필터/보정 레이어, 범용 brush node graph, NyatiDraw
플러그인 API, 협업, WebGL2 fallback, canonical Git unpack과 독립 소프트웨어 공개는
PNG image-open 편집 경로 뒤로 미룬다. 작은 Godot editor addon도 활성 스프린트 밖의
별도 backlog다.

## 첫 브리핑에서 승인할 사항

- 첫 지원 OS는 Windows로 두고, macOS·Wayland를 같은 입력 계약의 후속 backend로 구현한다.
- Dioxus Native 가설은 철회했고, Dioxus Desktop + native WGPU child seam을 채택했다.
- raster live stroke는 GPU-first, persistence는 immutable snapshot 기반이다.
- PNG 자동 export는 프로젝트 저장 성공과 별도 상태로 표시한다.
- 첫 사용자 기능은 “Godot project의 PNG를 NyatiDraw로 열고 paired `.ntdr`을 찾아,
  한 레이어에 압력 round brush로 그리고, undo하고, 저장한 PNG를 재사용하며,
  재실행해서 같은 그림을 보는 것”이다.

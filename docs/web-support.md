# 웹 실험판

상태: 로컬 브라우저에서 기본 드로잉·탭 재개 복구 검증 완료.
배포 파이프라인과 공개 페이지 검증은 진행 중이며, 아직 출시 지원 판정은 아니다.

이번 웹 스파이크는 사용자의 별도 승인을 받은 제한된 구현이다. 이전 문서의 웹 보류를
이 범위에서만 변경한다. Windows 설치판을 교체하거나 Linux·macOS 네이티브 지원을
진행하는 작업은 아니다. 웹과 Windows의 기능·성능이 같다는 의미도 아니다.

## 사용할 수 있는 범위

HTTPS 또는 localhost에서 WebGPU, IndexedDB, Web Locks를 사용할 수 있어야 한다.
지원 API가 없거나 같은 출처의 다른 탭이 작업 저장소를 점유하면 편집을 시작하지 않는다.
Canvas 2D/WebGL 대체 렌더러는 없다. 브라우저 이름만으로 실행 가능성을 보장하지 않는다.

| 범위 | 현재 구현과 제한 |
|---|---|
| 시작 | 1280×720 투명 래스터 한 장. 새 그림에 HD/FHD/정사각형/픽셀 작업 프리셋 |
| 그리기 | 기존 2H 샤프·2B 연필, 펜, 소프트 브러시, 지우개. 크기·불투명도·필압·보정은 공용 코어 사용 |
| 입력 | 직접 DOM Pointer 입력과 coalesced sample. 마우스·펜 경로가 있으며 실제 태블릿 검증은 대기 |
| 보기 | 화면 맞춤, 휠 확대축소, Space/가운데 버튼 드래그 이동. 손가락 입력은 이동으로 처리 |
| 레이어 UI | 래스터 추가·선택·비우기·표시·불투명도. 코어에 있는 모든 레이어 편집 API가 UI에 노출된 것은 아님 |
| 되돌리기 | 세션 내 최대 128개. 메모리 한도에 먼저 닿으면 더 적게 보존하며, 새 편집은 선형 Redo를 지움 |
| 색상 기억 | 도구 설정·전경/배경색은 작업 복구에 포함. 최근 색은 획 Begin 성공 시 등록하며 현재 세션에서만 유지 |
| 파일 | PNG 가져오기/내보내기와 `.nyatidraw-web` 작업 파일. Windows `.ntdr`는 열거나 저장하지 않음 |

선택·채우기·변형·반복·도킹 등 Windows 편집기의 모든 기능을 웹 UI에 옮긴 버전은 아니다.
사용자가 그리지 않은 샘플 레이어나 특수 배경 레이어를 자동 생성하지 않는다.

## 구조와 소유권

```text
DOM Pointer / coalesced samples
  → browser host: capture, viewport → document coordinates
  → web-core: shared brush + smoothing + CPU tile painting
  → CPU-authoritative snapshot + dirty tile keys
  → existing GpuCompositeScene: wgpu / WebGPU composition
  → browser canvas present

Dioxus controls → tool/layer commands → web-core
completed artwork + tool preferences → portable bytes → IndexedDB
```

- `apps/web`: Dioxus Web UI, 브라우저 입력·파일·복구 어댑터, WebGPU presenter.
- `crates/web-core`: `brush`/`input`/`paint-cpu`/`tiles`/`document` 재사용.
  Dioxus, wgpu, redb, Windows API, 데스크톱 프로젝트 형식에 의존하지 않는다.
- `crates/paint-gpu::GpuCompositeScene`: 기존 합성기를 웹 호스트에서도 사용한다.
  새 브러시 엔진이나 Canvas 2D 그림판을 따로 만들지 않았다.

획별 DOM 재렌더를 권위 상태로 사용하지 않고, 입력 콜백은 브러시 코어로 직접 전달한다.
다만 이번 웹 구현은 **메인 WASM 스레드에서 CPU가 획을 그리고 GPU가 표시·합성하는 방식**이다.
Windows의 GPU-first 실시간 경로와 다르며, 고해상도·큰 브러시·고주사율 성능 동등성을
주장하지 않는다. worker/GPU 브러시 이관은 별도 측정과 설계 이후의 일이다.

한 브라우저 이벤트의 coalesced sample이 256개를 넘으면 획을 취소한다.
pointer cancel/capture loss, 포커스 이탈, 비정상 입력과 자원 한도 초과도 부분 획을
완성된 그림으로 받아들이지 않는다. CPU snapshot은 획 전으로 복원하고 화면을 동기화한다.

## 저장·파일 교환

자동 복구는 같은 브라우저·출처의 IndexedDB에 **현재 작업 하나**를 저장한다.
Web Locks로 하나의 편집 탭만 허용하고, 저장 generation과 직렬 쓰기 완료를 구분한다.
진행 중 획은 저장하지 않으며, 이전 저장 완료가 최신 작업의 저장 완료 표시를 지우지 않는다.
미저장 상태에서는 `beforeunload` 경고를 요청하지만, 브라우저 강제 종료·탭 강제 종료·
운영체제 종료 때 저장 완료를 보장하는 장치는 아니다.

복구되는 것은 현재 픽셀·레이어 속성과 순서·페이지·활성 레이어·도구별 설정·색상이다.
Undo/Redo, 진행 중 획, 최근 색 목록은 재개 복구 대상이 아니다.
잘못된 복구 데이터는 자동으로 새 그림으로 덮지 않는다. 가져오기 실패 시 현재 그림도 유지한다.
새 그림과 파일 열기는 앱 내부 확인창을 거친다. 취소·Esc는 현재 그림을 보존하며,
GPU가 시작되지 않는 환경에서도 시작 화면의 ‘저장된 작업 내려받기’로 복구 원본을 꺼낼 수 있다.

`.nyatidraw-web`은 웹 전용 `NYWEB001` 형식이다. 무손실 raw/RGBA-run 타일과
전체 payload BLAKE3를 사용하며, 데스크톱 `.ntdr`의 호환 포맷으로 표시하지 않는다.
자세한 바이트 경계와 코어 API는 [web-core README](../crates/web-core/README.md)에 있다.

PNG는 앱과 웹 사이의 **평면 이미지 교환 경로**이며 레이어를 보존하지 않는다.
PNG 경계는 straight sRGB8, 내부 타일은 linear premultiplied RGBA8이다.
특히 저알파·어두운 색 변환에는 양자화가 있으므로 임의 PNG의 바이트 무손실 왕복을
약속하지 않는다. 웹 작업의 정확한 복구 원본은 작업 파일/IndexedDB 데이터다.

사이트 데이터 삭제, 비공개 모드 종료, 저장 공간 정리로 복구 데이터가 사라질 수 있다.
중요한 작업은 작업 파일로 따로 내려받는다. 현재 기능은 그림을 서버로 업로드하지 않는다.

## 자원 상한

- 페이지는 각 축 최대 4096px, 현재 픽셀은 최대 1024개의 128×128 타일(64MiB), 래스터는 32개.
- 페이지 밖 픽셀을 보존하지만 입력 좌표는 ±32768, 브러시 지름은 0.5~200px로 제한한다.
- 현재 상태와 Undo/Redo의 유지 예산은 128MiB. 취소 가능한 진행 획과 이벤트 임시 타일,
  PNG/파일 바이트, JS 복사본, GPU 메모리는 별도다. 이를 브라우저 전체 RAM 최대값으로 읽으면 안 된다.
- portable payload는 68MiB, 브라우저 파일 선택 단계는 72MiB 이하만 받는다.
  PNG 디코더에도 72MiB 제한을 적용하고, 축당 4096px인지 확인한 뒤 출력 버퍼를 만든다.
- 실제 GPU texture 크기·메모리 제약 때문에 코어 상한 이내라도 표시를 거부할 수 있다.

## 로컬 실행과 배포 경로

프로젝트의 Rust toolchain과 PowerShell을 사용한다. 저장소 루트에서:

```powershell
rustup target add wasm32-unknown-unknown
./tools/setup-web-ci.ps1
./tools/build-web.ps1 -DebugBuild -BasePath /
python -m http.server 8766 --bind 127.0.0.1 --directory target/dx/nyatidraw-web/debug/web/public
```

브라우저에서 `http://127.0.0.1:8766/`을 연다. `file://`로 직접 열지 않는다.
마지막 명령은 정적 파일 확인용 서버이며 종료는 해당 터미널에서 한다.

배포 산출물은 `./tools/build-web.ps1`로 만든다. 기본 URL 경로는 `/nyatidraw/draw/`,
출력은 `target/dx/nyatidraw-web/release/web/public`이다. `tools/build-site.ps1 -RequireWeb`이
웹 산출물을 포함한 사이트를 조립하며, `.github/workflows/pages.yml`은 Linux에서 코어 테스트,
WASM Clippy, 검증된 DX 설치, 웹 빌드와 Pages 배포를 이어서 수행하도록 구성했다.
실제 CI 성공·공개 페이지 실행은 별도 배포 확인 항목이다.

## 현재 검증 근거와 남은 것

| 구분 | 근거/상태 |
|---|---|
| 코어 | 핵심 테스트 3개 통과: 브러시 결정성·취소/Undo, 픽셀/레이어/도구 복구, 손상 파일·자원 제한 |
| 빌드 | 웹 코어와 호스트 WASM 검사 및 WASM Clippy 통과 |
| 로컬 실제 브라우저 | Windows의 Codex 내장 브라우저에서 WebGPU 화면·드로잉·Undo/Redo 확인 |
| 탭 재개 | 탭을 닫고 새 탭으로 열어 그림과 펜/20px 설정 복구. 복구 전후 PNG SHA-256 동일 |
| 동시 탭 | 두 번째 탭 편집 차단 확인. 여러 쓰기 탭이 같은 작업을 덮지 않도록 함 |
| 로컬 배포 빌드 | DX release 빌드 성공. Rust release 최적화는 유지하고 Windows에서 실패한 선택적 wasm-opt 후처리는 비활성화. 디버그 심볼 제외 후 WASM 약 1.30MB |
| 파일 교환 | 시험 PNG 가져오기/재출력 SHA-256 동일. 두 레이어·불투명도 50%·2B/20px 작업을 웹 파일로 저장→열기 후 PNG SHA-256 동일 |
| 실패/취소 | 손상 작업 파일 거부 후 현재 그림 유지, 새 그림 확인창 Esc 취소 후 현재 레이어·설정 유지 확인 |
| 회귀 검사 | Windows workspace 전체 기본 테스트, all-target/all-feature Clippy, DX Windows release 빌드 통과. 설치본은 변경하지 않음. ignored 하드웨어 acceptance는 포함하지 않음 |
| 배치 | 데스크톱과 390px 폭에서 홈페이지와 웹 툴바 확인. 작은 화면에서 페이지·도구 열의 가로 넘침 없음 |
| 진행 중 | Linux CI·Pages 배포·공개 URL의 최종 동작 확인 |
| 미검증 | 실제 펜 필압/버튼, 모바일 터치, GPU device-loss 복구, 전체 브라우저 프로세스 재시작, 장시간·고주사율 성능 |

탭 재시작은 브라우저 프로세스 재시작 검증이 아니며, 마우스와 합성 입력은 실제 펜 증거가 아니다.
한 테스트 그림의 PNG hash 일치는 모든 PNG 입력의 무손실 왕복 보증과도 다르다.

작은 화면에서는 브러시·크기 빠른 선택·색상·파일 컨트롤을 표시하고 레이어/상세 속성은
숨긴다. 이번 편집기의 주 검증 대상은 데스크톱 브라우저이며, 반응형 배치 확인이 모바일
편집 기능 동등성이나 실제 터치·태블릿 검증을 뜻하지 않는다.

### 배포 산출물의 경로 정보

빌드 스크립트는 Rust 소스 경로를 중립 경로로 remap하고 디버그 심볼을 제외한다.
하지만 현재 DX/Manganis의 `asset!` 데이터에는 CSS 원본 절대 경로가 남는다.
따라서 개인 작업 공간에서 만든 WASM을 직접 공개하지 않고 GitHub 호스팅 runner의
비개인 checkout에서 다시 빌드한 결과만 Pages에 배포한다. 자산 데이터는 런타임에
사용되므로 임의 바이트 치환이나 디버그 섹션 삭제로 제거된다고 가정하지 않는다.

설계 결정은 [ADR-0059](decisions/ADR-0059-web-editor-spike.md)를 따른다.

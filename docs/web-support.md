# 웹 실험판

상태: 초기 웹 스파이크는 Pages에 배포했다. 현재 작업 트리는 데스크톱과 같은
공용 Dioxus 화면으로 전환했으며, 이 UI 통합은 로컬 검증 상태이고 아직 배포하지 않았다.
아래 초기 배포 기록과 현재 공용 UI 검증을 구분한다. 아직 정식 출시 지원 판정은 아니다.

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
| 파일 | 데스크톱과 같은 `.ntdr` 읽기/저장, PNG 가져오기/내보내기. 이전 `.nyatidraw-web`은 가져오기만 지원 |

선택·채우기·변형·반복 등 Windows 편집기의 모든 기능을 웹 호스트에 옮긴 버전은 아니다.
패널 너비·높이와 탭은 사용할 수 있지만, 패널 위치를 바꾸는 도킹은 캔버스 수명 보장을
위해 웹에서 비활성화한다. 미지원 기능 때문에 별도의 축소 화면을 만들지 않는다.
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
completed artwork + tool preferences → shared NTDR store → IndexedDB / .ntdr download
```

- `crates/editor-ui`: 데스크톱을 기준으로 한 공용 Dioxus 화면. 툴바·도구·세부 도구·
  속성·크기·내비게이터·색상환·레이어·히스토리·도킹·CSS·아이콘을 단일 소스로 유지한다.
- `apps/desktop`: Dioxus Desktop, 네이티브 캔버스·입력·파일·설정·종료 어댑터.
- `apps/web`: 같은 화면을 Dioxus Web으로 렌더링하는 브라우저 입력·파일·복구 어댑터와
  WebGPU presenter. 웹 전용 CSS는 파일 대화상자·시작 실패·알림 등 호스트 요소만 다룬다.
- `crates/web-core`: `brush`/`input`/`paint-cpu`/`tiles`/`document` 재사용.
  Dioxus, wgpu, redb, Windows API, 데스크톱 프로젝트 형식에 의존하지 않는다.
- `crates/project-web`: 웹 문서와 공용 프로젝트 저장소 사이의 어댑터.
  `project-redb::MemoryProjectDb`로 데스크톱과 같은 redb 파일·타일 압축·메타데이터를 사용한다.
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

복구되는 것은 현재 픽셀·레이어 속성과 순서·페이지·도구별 설정·색상이다.
현재 네이티브 포맷은 활성 레이어를 별도로 기록하지 않으므로 재개 시 유효한 래스터를 선택한다.
웹 UI의 세션 Undo/Redo, 진행 중 획, 최근 색 목록은 재개 복구 대상이 아니다.
파일에는 공용 저장소의 최대 128개 기록을 유지한다.
잘못된 복구 데이터는 자동으로 새 그림으로 덮지 않는다. 가져오기 실패 시 현재 그림도 유지한다.
새 그림과 파일 열기는 앱 내부 확인창을 거친다. 취소·Esc는 현재 그림을 보존하며,
GPU가 시작되지 않는 환경에서도 시작 화면의 ‘저장된 작업 내려받기’로 복구 원본을 꺼낼 수 있다.

새 저장은 모두 데스크톱과 같은 `.ntdr`다. 확장자만 바꾼 웹 형식이 아니라,
같은 `project` 레코드·zstd 타일·`project-redb` 트랜잭션을 사용한다.
데스크톱에서 가져온 DB의 기존 히스토리와 메타데이터는 유지하며 웹의 완료된 수정은
정확한 타일 변경으로 기록한다. 웹 UI의 세션 내 Undo와 기존 데스크톱 히스토리 탐색은
아직 동일하지 않으므로, 파일 호환을 전체 편집 기능의 동등성으로 해석하면 안 된다.

이전 웹 전용 `NYWEB001`/`.nyatidraw-web`은 읽기 마이그레이션 경로만 남긴다.
기존 IndexedDB 이름은 바꾸지 않으며 첫 NTDR 복구 저장 때 이전 바이트를 같은 트랜잭션의
`legacy-before-ntdr` 키에 보존한다. 복구 실패 시 자동 초기화하지 않는다.
웹이 아직 처리하지 못하는 그룹이나 자원 한도를 넘는 파일은 교체 전에 거부한다.
내용을 평탄화하거나, 알 수 없는 도구 설정을 기본값으로 덮어쓰지 않는다.

PNG는 앱과 웹 사이의 **평면 이미지 교환 경로**이며 레이어를 보존하지 않는다.
PNG 경계는 straight sRGB8, 내부 타일은 linear premultiplied RGBA8이다.
특히 저알파·어두운 색 변환에는 양자화가 있으므로 임의 PNG의 바이트 무손실 왕복을
약속하지 않는다. 웹 작업의 정확한 복구 원본은 작업 파일/IndexedDB 데이터다.

사이트 데이터 삭제, 비공개 모드 종료, 저장 공간 정리로 복구 데이터가 사라질 수 있다.
중요한 작업은 작업 파일로 따로 내려받는다. 현재 기능은 그림을 서버로 업로드하지 않는다.

## 자원 상한

- 페이지는 각 축 최대 4096px, 현재 픽셀은 최대 1024개의 128×128 타일(64MiB), 래스터는 32개.
- 페이지 밖 픽셀을 보존하지만 입력 좌표는 ±32768, 브러시 지름은 0.1~200px로 제한한다.
- 현재 상태와 Undo/Redo의 유지 예산은 128MiB. 취소 가능한 진행 획과 이벤트 임시 타일,
  PNG/파일 바이트, JS 복사본, GPU 메모리는 별도다. 이를 브라우저 전체 RAM 최대값으로 읽으면 안 된다.
- NTDR 컨테이너는 128MiB, 이전 portable payload는 68MiB까지 받는다.
  PNG 파일과 디코더에는 72MiB 제한을 적용하고, 축당 4096px인지 확인한 뒤 출력 버퍼를 만든다.
- 압축된 root도 풀기 전에 길이를 확인하며 모든 root의 타일 수를 검사한다.
  히스토리 128개, root 258개, 메타데이터 레코드 128KiB/합계 8MiB 및 획 재생 작업량
  상한을 별도로 검사한다. 작은 파일도 복잡도가 상한을 넘으면 데스크톱에서 열어야 한다.
- 실제 GPU texture 크기·메모리 제약 때문에 코어 상한 이내라도 표시를 거부할 수 있다.

## 로컬 실행과 배포 경로

프로젝트의 Rust toolchain, PowerShell, WASM 타깃을 지원하는 LLVM Clang을 사용한다.
Clang이 PATH에 없으면 `CC_wasm32_unknown_unknown`으로 지정한다. Windows 빌드 스크립트는
기본 LLVM 설치 위치도 확인한다. 저장소 루트에서:

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
실제 CI 성공·공개 페이지 실행은 아래 검증 근거에 별도로 기록한다.

## 공용 UI 통합 로컬 검증

- Windows Desktop all-target Clippy와 DX release 빌드 통과. 네이티브 캔버스 마운트,
  입력·포커스·종료·파일 선택은 기존 호스트에 유지한다. 설치본은 교체하지 않았다.
- 웹 release 화면에서 같은 툴바, 왼쪽 세 패널, 오른쪽 내비게이터·색상·레이어/히스토리,
  캔버스 프리셋·배율 창, 메뉴 덮개를 확인했다.
- 실제 브라우저 조작으로 2B/16px, 색상환, 획 Begin 뒤 최근 색/크기, 레이어 추가,
  불투명도 드래그 53%, 패널 너비·높이 조절, 탭 전환을 확인했다.
- 불투명도 Undo/Redo 후 내비게이터 PNG 데이터가 일치했다. 이는 전체 작품 파일의
  픽셀 동일성을 증명하는 검사가 아니라 화면 연동 확인이다.
- 탭을 닫고 다시 열어 두 레이어·53% 불투명도·2B/16px를 복구했다. 재개 전후
  내비게이터·레이어 미리보기 PNG 데이터가 같고, 펜→연필 전환에서도 2B/16px가 유지됐다.
- 최종 WASM Clippy와 DX web release 빌드 통과. 브라우저 관찰 중 경고·오류 없음.
- API/브러시/웹 문서의 기존 핵심 테스트 8개 통과. UI 단위 테스트는 추가하지 않았다.
- 웹의 스포이트 확대 버블, 정확한 히스토리 작업명·브랜치, 구조 변경 도킹은 남아 있다.
  너비 비율은 세션 한정이고 높이·색상 핀·바깥 배경은 브라우저 설정에 저장한다.

이 통합의 Windows 실제 창·물리 펜 검증과 전체 브라우저 프로세스 재시작은 수행하지
않았다. 외형/소스 공유만으로 기능·성능·저장 호환을 증명하지 않는다.
NTDR 통합의 별도 경계는 [ADR-0061](decisions/ADR-0061-shared-ntdr-browser-storage.md)에 기록한다.

## 공용 NTDR 통합 로컬 검증

- 파일 브리지 핵심 테스트 5개, 메모리 저장소 3개, 웹 코어 3개,
  공용 레코드 코덱 7개, 데스크톱 도구 설정 복구 2개 통과.
- 네이티브→웹→네이티브 파일 재열기에서 정확한 타일·signed 좌표·레이어·필압 설정과
  기존 Undo 뒤 Redo 분기를 보존했다. 이전 웹 형식의 무손실 이전도 핵심 테스트로 확인했다.
- 연속 130회 저장/재열기 후 최신 상태와 공용 128개 기록 제한을 확인했다.
- WASM Clippy, 웹 DX release 빌드, 데스크톱 all-target Clippy와 서식 검사 통과.
  기존 vendored Wry의 lifetime 문법 경고는 남아 있다.
- 별도 localhost 출처에서 네이티브 파일을 열고 웹 획 추가→IndexedDB NTDR 저장→
  탭 닫기/재개를 확인했다. 재개 전후 내비게이터 PNG 데이터가 동일했고 관찰된 오류는 없었다.
- 저장 안내는 캔버스 오른쪽 아래에 표시하며 클릭을 가로채지 않는다.
  캔버스 전체의 `title` 속성은 제거했다.
- 저장 버튼에서 `drawing.ntdr` Blob(시험 파일 135,168 bytes)의 다운로드 클릭이
  취소되지 않고 전달되는 것을 임시 진단으로 확인했다. 진단 코드는 제거했다.
  내장 브라우저의 다운로드 이벤트/완성 파일은 회수하지 못했으므로 실제 다운로드
  산출물의 네이티브 재열기는 미검증이다. 이를 브라우저 제한이나 앱 성공으로 단정하지 않는다.
  위 핵심 파일 왕복 및 실제 IndexedDB 저장/재개와 구분한다.
- 배포 전 workspace 기본/전체 기능 테스트, 전체 타깃·전체 기능 Clippy,
  WASM Clippy 및 서식 검사를 수행한다. 공개 배포 결과는 별도로 기록한다.
- 사용자 작업 출처와 작품 파일은 건드리지 않았으며 설치본은 교체하지 않았다.

## 초기 별도 UI 스파이크의 검증 기록

다음 표는 이미 공개된 초기 스파이크의 기록이다. 현재 공용 UI 변경의 배포·회귀 검증
결과로 재사용하지 않는다.

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
| 공개 배포 | [Pages workflow](https://github.com/nyabia/nyatidraw/actions/runs/35580901885) 빌드·배포 성공. 공개 홈페이지의 alpha.12 다운로드 연결과 웹 편집기의 WebGPU 캔버스 시작 확인. 관찰 중 경고·오류 콘솔 메시지 없음 |
| Windows CI | [Windows workflow](https://github.com/nyabia/nyatidraw/actions/runs/35580901924) 통과. 새 데스크톱 릴리즈 태그나 설치본 교체는 하지 않음 |
| 미검증 | 실제 펜 필압/버튼, 모바일 터치, GPU device-loss 복구, 전체 브라우저 프로세스 재시작, 장시간·고주사율 성능 |

탭 재시작은 브라우저 프로세스 재시작 검증이 아니며, 마우스와 합성 입력은 실제 펜 증거가 아니다.
한 테스트 그림의 PNG hash 일치는 모든 PNG 입력의 무손실 왕복 보증과도 다르다.

초기 웹 스파이크의 작은 화면용 패널 숨김 CSS는 폐기했다. 현재 공용 화면은 데스크톱
배치를 유지한다(기존 최소 너비 1100px). 모바일 전용 재배치는 별도 승인 사항이다.

### 배포 산출물의 경로 정보

빌드 스크립트는 Rust 소스 경로를 중립 경로로 remap하고 디버그 심볼을 제외한다.
하지만 현재 DX/Manganis의 `asset!` 데이터에는 CSS 원본 절대 경로가 남는다.
따라서 개인 작업 공간에서 만든 WASM을 직접 공개하지 않고 GitHub 호스팅 runner의
비개인 checkout에서 다시 빌드한 결과만 Pages에 배포한다. 자산 데이터는 런타임에
사용되므로 임의 바이트 치환이나 디버그 섹션 삭제로 제거된다고 가정하지 않는다.

공개 WASM은 HTTP 200, `application/wasm`, 올바른 WASM 헤더와 1,297,500 bytes를
확인했다. CI와 공개 파일 검사에서 개인 Windows/macOS/Linux 홈 경로 패턴은 검출되지 않았다.
공개 WASM SHA-256:
`FEEA4109A62F509C3F3808DE691262A20794666C201A77A3AE024251D2B13B6E`.
이는 경로 정보에 대한 제한된 검사이며 모든 종류의 민감정보 부재를 증명하는 것은 아니다.

웹 호스트 경계는 [ADR-0059](decisions/ADR-0059-web-editor-spike.md),
공용 화면은 [ADR-0060](decisions/ADR-0060-shared-dioxus-editor-ui.md)을 따른다.

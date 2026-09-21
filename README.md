# NyatiDraw

NyatiDraw는 Godot 프로젝트의 PNG를 빠르게 열고 수정하기 위한 Windows 우선
래스터 드로잉 편집기입니다. 편집 가능한 `.ntdr` 프로젝트를 PNG 옆에 두고,
저장 시 프로젝트의 내구성 있는 스냅샷과 PNG export를 별도 실패 영역으로 처리합니다.

현재 Windows x64 알파판 **0.1.0-alpha.12**를 배포하고 있습니다.
[홈페이지](https://nyabia.github.io/nyatidraw/)에서 설치 파일을 받을 수 있습니다.
별도의 [웹 실험판](https://nyabia.github.io/nyatidraw/draw/)도 제공합니다.
Linux/macOS 설치판·고급 브러시·애니메이션·벡터 기능은 후속 범위입니다.

## 설치와 기본 사용

[Releases](https://github.com/nyabia/nyatidraw/releases)의 게시된 알파판에서
`NyatiDraw-win-Setup.exe`를 실행합니다. 사용자 계정에 설치되며,
시작 메뉴의 **NyatiDraw**로 실행합니다. 현재 Windows 게시자 서명은 없습니다.

- 처음 실행하면 `%LOCALAPPDATA%\NyatiDraw\Sketchbook\작업 중.ntdr`를 엽니다.
  이 기본 그림은 다음 실행에도 다시 열립니다.
- **새 그림**은 새 `.ntdr` 파일을 만듭니다. **열기**는 PNG 또는 `.ntdr`를 엽니다.
- **저장**은 프로젝트를 저장하고 같은 이름의 PNG를 옆에 내보냅니다.
  PNG 옆에 `.ntdr`가 있으면 이후 열기에서 편집 가능한 프로젝트를 우선합니다.
- 새 알파판을 받으면 **저장 후 업데이트**로 적용하고 현재 그림을 다시 엽니다.
  다운로드 실패 시 기존 앱은 계속 사용할 수 있습니다.

alpha.12는 입력·오류 복구·도구 설정 복원과 프로젝트 저장 용량을 개선합니다.
[변경 내용과 남은 검수](docs/releases/0.1.0-alpha.12.md)를 참고하세요.
실제 펜/고주사율 하드웨어 검증과 설치·업데이트 검증은 별도입니다.
프로젝트 X/Y 반복·비파괴 레이어 색상화·메시 변형 등은 아직 구현하지 않았습니다.
자동 업데이트의 운영·검증 범위는 [배포 문서](docs/releasing.md)를 참고하세요.

## 현재 구현 범위

- Dioxus Desktop UI와 Win32 child canvas
- Windows pointer/pen 입력과 WGPU live ink
- 무한 작업공간 좌표와 출력 페이지 crop
- sparse signed tiles, raster layer/group, undo/history
- redb 기반 `.ntdr` 저장·재실행·복구
- colocated PNG import/export와 single-instance activation
- 사용자별 Windows 개발판 설치 및 `.ntdr`/PNG Open With 등록

검증된 범위와 아직 미검증인 하드웨어·플랫폼 증거는
[문서 안내](docs/README.md)와 [플랫폼 지원표](docs/platform-support.md)에 구분해 기록합니다.

## 웹 실험판

[브라우저에서 그려보기](https://nyabia.github.io/nyatidraw/draw/) — WebGPU, IndexedDB,
Web Locks를 지원하는 최신 브라우저가 필요합니다. 데스크톱 브라우저 중심의 제한된 실험판입니다.

- 2H/2B 연필·펜·소프트 브러시·지우개, 기본 레이어와 최대 128단계 되돌리기.
- PNG 가져오기/내보내기, 현재 작업의 브라우저 복구 저장, `.nyatidraw-web` 파일 저장/열기.
- 그림을 서버로 업로드하지 않습니다. 중요한 작업은 별도 파일로 내려받으세요.
- Windows `.ntdr` 호환, 모든 데스크톱 기능 및 동일한 드로잉 성능을 제공하는 버전은 아닙니다.

구조·제한·실행 방법과 실제 검증 범위는 [웹 지원 문서](docs/web-support.md)에 있습니다.

## 개발

프로젝트에 고정된 Rust 1.96.0과 Dioxus CLI 0.7.9가 필요합니다.
setup은 CLI를 저장소의 `.nyatidraw/toolchains/`에 설치합니다.

```powershell
./tools/setup-dev.ps1
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked
./tools/build-dev.ps1
```

개발판 설치와 파일 연결:

```powershell
.\tools\install-dev.ps1
```

자세한 동작과 안전 규칙은 [Windows 개발판 설치](docs/development-install.md)를
참고하세요. 자동 테스트는 작품 손상, 입력 전이 손실, 결정성, 히스토리 및 복구
보장처럼 핵심 데이터 위험이 있는 로직에만 둡니다.

## 프로젝트 상태

- 제품명: `NyatiDraw`
- 프로젝트 확장자: `.ntdr`
- 현재 우선순위: Godot 작업 중 PNG open → draw → save/export → pair reopen
- Godot 전용 addon과 웹 임베드는 후속 범위입니다.

로드맵은 [Godot 우선 스프린트 계획](docs/sprints/README.md)에 있습니다.

## 라이선스

NyatiDraw는 [MIT](LICENSE-MIT) 또는 [Apache-2.0](LICENSE-APACHE) 중
하나를 선택하여 사용할 수 있는 이중 라이선스입니다.
타사 코드는 각자의 라이선스를 따릅니다.

## 피드백

버그 제보와 기능 제안은 [이슈](https://github.com/nyabia/nyatidraw/issues)로 받습니다.
현재 외부 코드 기여(PR/MR)는 받지 않으며, 저장소의 PR 기능을 비활성화했습니다.

## AI 생성 고지

이 프로젝트는 OpenAI Codex 등 생성형 AI를 활용해 개발했으며,
소스 코드와 문서에는 AI가 생성하거나 수정한 내용이 포함되어 있습니다.

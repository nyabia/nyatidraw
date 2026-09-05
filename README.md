# NyatiDraw

NyatiDraw는 Godot 프로젝트의 PNG를 빠르게 열고 수정하기 위한 Windows 우선
래스터 드로잉 편집기입니다. 편집 가능한 `.ntdr` 프로젝트를 PNG 옆에 두고,
저장 시 프로젝트의 내구성 있는 스냅샷과 PNG export를 별도 실패 영역으로 처리합니다.

현재 저장소는 개발 중인 vertical slice입니다. 독립 소프트웨어 공개판이 아니며,
Windows 외 플랫폼·고급 브러시·애니메이션·벡터 기능은 후속 범위입니다.

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

## 개발

stable Rust toolchain과 프로젝트에 고정된 Dioxus CLI가 필요합니다.
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
- Godot 전용 addon과 공개 배포는 현재 스프린트 밖입니다.

로드맵은 [Godot 우선 스프린트 계획](docs/sprints/README.md)에 있습니다.

## 라이선스

아직 공개 라이선스를 지정하지 않았습니다.

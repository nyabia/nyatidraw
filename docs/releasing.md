# Windows 알파판 배포

홈페이지: <https://nyabia.github.io/nyatidraw/>
소스/배포: <https://github.com/nyabia/nyatidraw>

## 자동화 범위

- `Windows CI`: PR/main의 코드 변경에서 기존 핵심 테스트, fmt, Clippy, DX release 빌드.
- `Website`: 홈페이지 변경/릴리스 게시 변경 시 Pages 갱신. 실행 파일은 Releases에서 받는다.
- `Windows alpha release`: `v0.1.0-alpha.1` 형태의 태그 또는 수동 실행으로
  검증→설치 파일/업데이트 피드 생성→GitHub Release 초안까지 진행한다.
  초안은 설치·업데이트 확인 뒤 게시한다. CI 성공만으로 설치 검증을 대신하지 않는다.

모든 workflow는 GitHub 기본 실행 환경을 사용한다. 외부 PR은 읽기 권한이며 배포하지 않는다.
집 머신 runner, 유료 대형 runner, 상시 서버, 매 커밋 바이너리 보관은 사용하지 않는다.
도구 버전: Rust 1.96.0, Dioxus CLI 0.7.9, Velopack SDK/packager 1.2.0.

## 로컬 패키징

```powershell
./tools/setup-ci.ps1
./tools/build-dev.ps1
./tools/package-release.ps1 -Version 0.1.0-alpha.1
```

패키징에는 .NET 8 이상이 필요하지만 앱 사용자에게 .NET SDK를 요구하지 않는다.
설치 프로그램은 WebView2 runtime이 없으면 준비하도록 구성한다.
산출물은 `target/releases/<version>/`에 두며 같은 버전의 기존 산출물을 덮어쓰지 않는다.
배포 대상은 해당 폴더 바로 아래의 파일이다. `stage/`는 포장 전 중간 결과다.

## 게시 전 확인

1. 설치 파일에서 시작 메뉴 실행과 새 그림·PNG/ntdr 열기를 확인한다.
2. 그리기→Save→정상 종료→별도 프로세스 재실행으로 작품과 PNG를 확인한다.
3. 이전 알파판에서 새 알파판 다운로드→저장 후 업데이트→기존 작품 재열기를 확인한다.
4. 오프라인/잘못된 다운로드/저장 실패 시 현재 앱이나 작품을 잃지 않는지 확인한다.
5. Release 초안에 검증 결과, 지원 범위, 알려진 제한을 적고 게시한다.
6. Pages를 다시 배포하고 다운로드 버튼이 게시된 설치판을 가리키는지 확인한다.
   GITHUB_TOKEN으로 생성한 이벤트는 다른 workflow를 자동 실행하지 않을 수 있으므로
   자동 게시 도구를 사용할 때는 Website workflow를 명시적으로 실행한다.

알파판은 `alpha` 업데이트 채널을 사용한다. 처음에는 전체 패키지를 내려받으며
차등 업데이트는 최적화 범위로 남긴다. 실행 중인 앱을 강제 종료해 교체하지 않는다.
업데이트 버튼은 Save를 먼저 접수하고, writer/export가 정상 종료된 뒤에만
Velopack에 적용을 요청한다. 업데이트 후 현재 프로젝트 경로를 다시 전달한다.

패키지의 체크섬은 Windows 게시자 코드 서명과 별개다. 현재 유료 서명 서비스를
설정하지 않았으며, 경고 없는 설치를 보장하지 않는다. 프로젝트는 MIT OR Apache-2.0
이중 라이선스다. 배포 폴더에는 두 라이선스 전문과 수집한 타사 라이선스 고지를 포함한다.
alpha.3은 라이선스 결정 전에 패키징했으므로 두 전문은 Release의 별도 첨부로 제공하며,
다음 패키징부터 설치 파일에도 포함한다.

## 현재 실측 증거

### 2026-09-08 alpha.4 로컬 후보 — 게시 보류

Save As, activation 종료, 파일 연결 소유권 변경의 핵심 검사와 DX release 빌드,
scratch export/recovery·durability 실행 및 실제 저장 대화상자/별도 프로세스 verifier를
통과했다. [전체 실행 기록](status-plan-2026-09-08.md)에 범위와 해시를 기록했다.
산출물은 `target/releases/0.1.0-alpha.4/`에 있고 두 라이선스 전문과 타사 고지를 포함한다.

Codex에서 실행한 설치·업데이트·제거는 파일 시스템 리디렉션 내부에서 성공했다.
외부 WMI에는 일반 설치 경로의 실행 파일이 없고, Codex LocalCache에만 있었다.
실제 사용자 registry의 Open With 명령은 일반 경로를 가리키므로 이 성공을
탐색기 실행 성공으로 확대할 수 없다. 일반 탐색기에서 설치 후 파일 연결로 PNG pair를
열어야 게시 가능하다. 현재 태그/Release 게시/홈페이지 다운로드 전환은 하지 않았다.

### alpha.3 기존 기록과 증거 한계

아래는 당시 실행 기록이다. 09-08에 확인한 Codex 경로 리디렉션 때문에 과거 로컬 설치
역시 일반 탐색기/깨끗한 Windows 설치를 입증한다고 볼 수 없다. GitHub CI·다운로드·
내용 hash·실행 프로세스의 저장/업데이트 결과와 일반 OS 설치 증거를 구별한다.

2026-09-06 `b65892f`의 [Windows CI](https://github.com/nyabia/nyatidraw/actions/runs/34016306285)가
GitHub Windows runner에서 기존 테스트·Clippy·release 빌드까지 통과했다.
[Website 배포](https://github.com/nyabia/nyatidraw/actions/runs/34016306291) 성공 후
공개 홈페이지 HTTP 200과 문서 제목을 확인했다.

2026-09-06 `f602bf7`의 [Windows alpha release](https://github.com/nyabia/nyatidraw/actions/runs/34017520413)는
기존 핵심 테스트·fmt·Clippy·DX release 빌드·패키징·Release 초안 업로드까지 통과했다.
[0.1.0-alpha.3](https://github.com/nyabia/nyatidraw/releases/tag/v0.1.0-alpha.3)을 게시했다.
설치 파일은 12,851,581 bytes, 업데이트 전체 패키지는 8,390,013 bytes다.

- Windows 11 x64의 이 개발 머신에서 alpha.1/alpha.2 설치와 기본 그림의 저장·종료·재열기를 확인했다.
  GitHub 산출물 alpha.3 설치 파일도 종료 코드 0으로 설치됐다. 시작 메뉴 바로가기를 생성한다.
- 게시 전에는 초안에서 받은 패키지를 기존 alpha.2의 패키지 폴더에 준비하여 적용 경로를 확인했다.
  PNG 목적지를 검증용 빈 디렉터리로 막으면 저장 확인 창을 표시하고 alpha.2가 유지됐다.
  프로젝트는 저장됐으며, 방해물을 별도 검증 폴더로 보존 이동하고 PNG를 복원한 뒤
  ‘프로젝트 다시 열기’와 ‘저장 후 업데이트’로 alpha.3 적용/재시작에 성공했다.
- 게시 후 alpha.2를 다시 설치하여 새 패키지가 없는 상태에서 실행했다.
  앱의 인증 없는 GitHub 소스가 alpha.3을 실제로 내려받고 준비 상태를 표시했다.
  다운로드 SHA256은 피드와 동일한 `e722d33b34e27b33f7cf623a5d1ec09f90bf6dd6dbdbafc6f87166dc5720cbd8`.
  저장 후 업데이트로 다시 alpha.3이 실행됐고, 한글/공백을 포함한 현재 프로젝트 경로가 전달됐다.
- 업데이트 전후 프로젝트 내용 root는
  `ffb174f805a9da89f226223d3370fdb0cfe19dfd7b80393427da1272eaf03f33`으로 같았다.
  PNG SHA256은 `7dcb436606b18e15e05fdff8be554269c80754392ba80127db536f0758068dff`로 유지됐다.
- 별도 SDK 실행 검증은 통신 거부와 동일 크기 손상 패키지를 거부했고 설치 manifest를 보존했다.
  앱 프로세스에 임시 프록시를 주는 전체 UI 통신 실패 검증은 실행 정책에 차단되어 수행하지 못했다.

원본 로그/해시는 `target/public-alpha-acceptance/`에 있으며 Git에는 포함하지 않는다.
개발 도구나 WebView2가 없는 깨끗한 Windows 머신, 전원 차단 중 업데이트, 실제 펜·고주사율,
탐색기 Open With는 미검증이다. 이 결과를 해당 환경의 증거로 확대하지 않는다.

Pages 환경의 deployment branch 정책은 `main` 브랜치와 `v*-alpha.*` 태그를 허용한다.
릴리스 이벤트의 실행 ref는 태그이므로 태그 정책이 없으면 checkout을 main으로 지정해도
배포가 거절된다. 첫 릴리스에서 이를 확인하고 알파 태그 정책을 추가했다.

## 간단한 의존성 점검

2026-09-06 cargo-deny **0.20.2**, `cargo deny check`, `deny.toml`의 Windows x64 /
all-features 범위로 점검했다. 라이선스와 출처 검사는 통과했다.
전체 종료 코드는 1이며, 검출된 advisory는 아래 유지보수 중단 2건이다.

- `derivative 2.2.0` (Velopack 경유): [RUSTSEC-2024-0388](https://rustsec.org/advisories/RUSTSEC-2024-0388)
- `paste 1.0.15` (그래픽/이미지 의존성 경유): [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436)

이번 Windows 의존성 그래프에서 취약점 advisory는 검출되지 않았다. 중복 버전 경고는
33건이었다. 가벼운 현황 점검으로 남기며 예외로 숨기거나 의존성을 교체하지 않았다.
원본 결과는 `target/public-alpha-tools/cargo-deny-summary.txt`에 있다.

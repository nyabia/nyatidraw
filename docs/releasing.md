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
설정하지 않았으며, 경고 없는 설치를 보장하지 않는다. 공개 라이선스 선택도 별도로
확정해야 한다. 배포 폴더에는 수집한 타사 라이선스 고지를 포함한다.

## 현재 실측 증거

2026-09-06 `b65892f`의 [Windows CI](https://github.com/nyabia/nyatidraw/actions/runs/34016306285)가
GitHub Windows runner에서 기존 테스트·Clippy·release 빌드까지 통과했다.
[Website 배포](https://github.com/nyabia/nyatidraw/actions/runs/34016306291) 성공 후
공개 홈페이지 HTTP 200과 문서 제목을 확인했다.
설치/업데이트 검증 결과는 통과 후 별도로 추가한다.

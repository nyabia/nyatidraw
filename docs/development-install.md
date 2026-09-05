# Windows 개발판 설치와 파일 연결

## 목적

NyatiDraw 개발판을 매 빌드마다 무거운 설치 프로그램 없이 사용자별 고정 경로에
갱신한다. `.ntdr`은 직접 열고, `.png`에서는 Windows `Open with` 후보로 NyatiDraw를
선택할 수 있어야 한다.

```text
game-project/art/player.png를 NyatiDraw로 열기
  -> sibling player.ntdr 탐색
  -> 있으면 project 열기
  -> 없으면 PNG를 가져와 player.ntdr 복구 원본 즉시 생성
  -> Save 시 최신 player.ntdr 확인 후 PNG export
  -> player.png background export
```

자세한 pair 규칙은 [Godot 작업 폴더와 PNG 연결](godot-integration.md)이 정한다.

## 두 가지 배포 경로

### 빠른 개발 설치

`tools/install-dev.ps1`은 다음을 수행한다.

1. `tools/build-dev.ps1`로 버전을 확인한 뒤 locked release bundle 생성
2. DX가 수집한 app과 assets를 `%LOCALAPPDATA%\Programs\NyatiDraw Development`의
   고정 경로에 설치
3. 현재 사용자 범위에 `.ntdr` open handler와 아이콘 등록
4. NyatiDraw를 `.png`의 `Open with` 후보로 등록하되 기본 PNG 앱은 유지
5. Shell association 변경 알림

관리자 권한은 요구하지 않는다. 설치 갱신은 실행 중인 프로세스를 확인하고 파일을
반쯤 교체하지 않는다.

처음에는 `tools/setup-dev.ps1`을 실행한다. `tools/dioxus-cli.version`의 **0.7.9**를
프로젝트 내부 `.nyatidraw/toolchains`에 설치하며 전역 CLI와 PATH를 변경하지 않는다.
`cargo-binstall`이 있으면 공식 release asset만 사용하고, 없으면 같은 exact 버전을
`cargo install --locked`로 빌드한다. `tools/build-dev.ps1`은 workspace의 Dioxus 버전과
CLI pin이 일치하는지 검사하고, 실제 CLI 버전이 다르면 빌드 전에 중단한다.

2026-09-05에 기존 CLI 0.7.5 거부, 0.7.9 설치, setup 재실행, release bundle 성공을
확인했다. 새 빌드에는 이전 버전 불일치 오류가 없다. 이 확인은 앱 runtime이나
NSIS 설치 검증을 대신하지 않는다.

앱 시작 시 Windows parent window는 현재 커서가 있는 모니터의 작업 영역을 기준으로
최대화를 시도한다. Win32 모니터 조회나 위치 변경이 실패하면 오류를 로그에 남기고
OS 기본 최대화로 계속 실행하므로 설치/업데이트 경로와는 독립적이다. 다중 모니터와
DPI 조합의 실제 수동 검증은 아직 미검증이다.

### 마일스톤 설치

DX CLI가 지원하는 NSIS bundle을 기준으로 한다.

```powershell
dx bundle --release --windows --renderer webview `
  --package nyatidraw-desktop --package-types nsis --locked
```

MSI는 조직 배포 요구가 생길 때 추가한다. 정식 배포 서명, update, clean-VM 공개판
uninstall 검증은 Sprint 8 범위다.

## 파일 형식 등록

### NyatiDraw project

| 항목 | 값 |
|---|---|
| 확장자 | `.ntdr` |
| 표시 이름 | `NyatiDraw Project` |
| versioned ProgID | `NyatiDraw.Project.1` |
| open command | `\"<installed exe>\" \"%1\"` |

### PNG Open With

| 항목 | 값 |
|---|---|
| 대상 | `.png` |
| application key | `Applications\\NyatiDraw.exe` |
| open command | `\"<installed exe>\" \"%1\"` |
| 정책 | supported/open-with 등록만 수행; PNG 기본 handler는 변경하지 않음 |

개발판은 Windows에 정확히 `NyatiDraw`로 표시되고, 기본 등록 위치는 `HKCU\Software\Classes`다. `.ntdr`에는 versioned ProgID,
`DefaultIcon`, `shell\open\command`를 등록한다. PNG에는 NyatiDraw application open
command와 supported type만 등록한다. Windows `UserChoice`는 installer나 앱이
강제로 바꾸지 않는다.

Windows Shell은 HKCU와 HKLM의 `Software\Classes`를 합친 view를 사용하고 사용자
등록을 우선한다. 등록 변경 뒤에는 `SHChangeNotify(SHCNE_ASSOCCHANGED)`를 호출한다.
설치 스크립트는 build나 설치 경로 교체 전에 기존 `.ntdr` handler와 NyatiDraw registry
command의 소유권을 확인해 다른 앱의 등록을 덮어쓰지 않는다. 이미 설치된 과거 개발판이
`NyatiDrawDevOwner` marker가 있는 현재 개발판 등록만 갱신한다. marker가 없거나 네
루트와 명시된 자식 키에 다른 값·자식이 있거나 일부만 존재하면 승계하지 않고 즉시
fail-closed 한다. 이전 무표식 개발판을 자동 채택하는 일회성 호환 분기는 유지하지
않는다.

- [Microsoft: HKEY_CLASSES_ROOT와 사용자별 Classes](https://learn.microsoft.com/en-us/windows/win32/sysinfo/hkey-classes-root-key)
- [Microsoft: File Types 등록](https://learn.microsoft.com/en-us/windows/win32/shell/fa-file-types)
- [Microsoft: ProgID](https://learn.microsoft.com/en-us/windows/win32/shell/fa-progids)
- [Dioxus 0.7: desktop installer와 DX bundle](https://dioxuslabs.com/learn/0.7/guides/deploy/)

## 앱 activation 계약

- 첫 positional argument 하나를 activation path로 받는다.
- `.ntdr` path는 missing/정확히 0-byte일 때만 같은 파일에 새 project로 초기화한다.
- `.png` path는 exact sibling `.ntdr`을 찾고 [PNG activation 규칙](godot-integration.md#png-activation-규칙)을 따른다.
- non-empty invalid `.ntdr`은 오류를 표시하고 절대 덮어쓰거나 untitled로 fallback하지
  않는다.
- 상대 path는 startup 시 canonical absolute path로 고정한다.
- Windows primary process는 `Local\\NyatiDraw.SingleInstance.v1` named mutex를
  보유한다. 두 번째 실행은 새 writer를 만들지 않고 bounded UTF-16 named-pipe
  activation을 primary로 전달한 뒤 종료한다.
- primary는 parent window restore/foreground를 best-effort 요청하고 child WGPU
  canvas UI thread에서 activation을 직렬 처리한다. 기존 writer/export FIFO를
  drain/join해 redb file lock을 먼저 해제한 뒤 새 project를 연다.
- target `.ntdr`의 non-empty validity와 PNG decode는 기존 문서를 닫기 전에
  preflight한다. target open이 그 뒤 실패하면 직전 project를 다시 열고 visible
  activation notice를 보인다. non-empty invalid project는 절대 bootstrap/fallback으로
  덮지 않는다.
- 같은 paired `.ntdr`을 PNG 또는 project path로 다시 활성화하면 storage identity가
  같다고 판정해 reopen하지 않는다. 따라서 competing writer가 생기지 않는다.
- pipe 전달 실패는 secondary process failure다. secondary가 독자 untitled/project
  writer로 fallback하지 않는다.
- 현재 `NAYATI_PROJECT_PATH` 환경변수는 probe 호환용으로만 유지하고 정상 positional
  activation보다 우선하지 않는다.

2026-09-03 설치판 프로세스 acceptance에서는 `sprint1-check.ntdr`을 연 primary가
살아 있는 동안 별도 실행으로 `player.ntdr`을 전달했다. secondary는 code 0으로
종료했고 primary PID 하나만 유지됐으며, CLI에서 첫 project는 즉시 다시 열리고 전달
대상만 `Locked`로 거부됐다. 이는 IPC와 writer lock 교체 증거다. Explorer의 실제
`Open with`, foreground 허용, 다중 모니터 시각 결과는 아직 수동 미검증이다.

## 제거 계약

개발판 제거는 자신이 만든 설치 파일과 자신이 소유한 registry value만 제거한다.
새 설치는 `.nyatidraw-install.json`에 설치 파일의 상대 경로와 SHA-256을 기록한다.
갱신 전에 manifest와 현재 파일이 정확히 일치해야 한다. 설치 폴더에 추가하거나
수정한 파일이 있으면 그대로 보존하고 갱신을 중단한다. 해당 파일을 설치 폴더 밖으로
옮긴 뒤 다시 설치한다. 소유권 manifest가 없는 기존 설치는 자동 승계하지 않는다.

제거는 manifest에 있고 현재 hash도 일치하는 파일만 지운다. 추가 파일과 수정된
파일은 남기며, 비어 있는 폴더만 비재귀적으로 제거한다. 경로 이탈·중복 경로·reparse
point는 파일 제거 전에 거부한다. Registry 기본값 제거는 명시적인 쓰기 handle에서
예상 값을 다시 확인하며, 설치와 제거 모두 Shell association 변경을 알린다.
다음은 절대 삭제하지 않는다.

- 사용자의 `.ntdr` 프로젝트
- sibling PNG와 다른 export
- PNG의 현재 기본 handler와 `UserChoice`
- 다른 NyatiDraw 제품의 등록 정보

uninstall 후에도 프로젝트와 PNG는 일반 파일로 남고, 새 버전 재설치나 `Open with`로
다시 연결할 수 있어야 한다.

## 2026-09-05 설치판 실행 근거

Windows 11 Home 10.0.26200 / Core Ultra 7 155H / Intel Arc integrated / DX12,
DX 0.7.9 release bundle을 실제 사용자 Programs 경로에 설치했다. 새 설치, 반복 갱신,
제거와 재설치가 통과했다. 추가 scratch `.ntdr`과 외부 registry value가 있으면 갱신을
거부했고, 제거 뒤 두 값은 그대로 남고 소유한 binary/등록만 제거됐다. PNG `UserChoice`
ProgID는 전 과정에서 동일했다. scratch 파일과 값만 정리한 뒤 개발판을 다시 설치했다.

독립 scratch 디렉터리에서도 수정된 설치 파일 보존, manifest 경로 이탈 거부,
junction 거부를 확인했다. 처음 실제 제거 실행에서 기존 read-only RegistryKey의
`DeleteValue` 실패를 재현했고 writable handle로 수정한 뒤 위 acceptance를 다시 통과했다.
이는 [Microsoft RegistryKey.DeleteValue 계약](https://learn.microsoft.com/en-us/dotnet/api/microsoft.win32.registrykey.deletevalue)에 따른다.

`NAYATI_DESKTOP_SMOKE_BINARY`로 실제 설치된 `NyatiDraw.exe`를 지정해
`desktop_durability_reopen_smoke`와 `desktop_export_recovery_smoke`를 실행했다.
32-stroke exact root/tile 재실행, active close/deferred Save, invalid bytes 보존,
네 export crash 경계, 구세대 export 폐기, 잠금 실패 후 창 유지·durable reopen·Save
재시도가 통과했다. 로그는 `target/installed-durability.log`,
`target/installed-export-recovery.log`에 남겼다. 새 unit test는 추가하지 않았다.
Explorer Open with 메뉴 직접 조작, Godot reimport, 물리 펜과 시각적 UI 검증은
이 결과에 포함하지 않는다.

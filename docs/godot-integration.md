# Godot 작업 폴더와 PNG 연결 계약

## 현재 통합 방식

Godot editor addon은 활성 스프린트 밖의 backlog다. 당장 사용하는 경로는 Windows에
NyatiDraw를 **PNG를 열 수 있는 프로그램**으로 등록하는 것이다. Godot은 완성된 PNG를
평소처럼 asset으로 사용하고, NyatiDraw는 그 옆의 `.ntdr`을 편집 원본으로 사용한다.

```text
game-project/art/player.png     Godot이 사용하는 asset
game-project/art/player.ntdr    NyatiDraw 편집 원본
```

사용자는 Explorer 또는 Windows의 `Open with`에서 `player.png`를 NyatiDraw로 연다.
Godot 프로젝트 전용 파일 선택기, GDScript addon, GDExtension은 이 흐름에 필요 없다.

## PNG activation 규칙

NyatiDraw가 `player.png`를 첫 positional argument로 받으면 정확히 같은 directory와
stem의 `player.ntdr`만 pair 후보로 사용한다.

1. sibling `.ntdr`이 유효한 non-empty project이면 그것을 연다. PNG는 project의
   export target으로 연결한다.
2. sibling `.ntdr`이 없으면 PNG를 먼저 완전히 decode한 뒤 첫 raster layer로 가져오고,
   복구 가능한 `player.ntdr`을 즉시 만든다. 원래 PNG는 Save 전까지 바꾸지 않는다.
3. sibling `.ntdr`이 정확히 0-byte이면 PNG에서 초기화할 새 project 대상으로 취급한다.
   decode 성공 뒤 `.ntdr` 복구 원본은 즉시 초기화하지만 첫 Save 전까지 원래 PNG를
   바꾸지 않는다.
4. sibling `.ntdr`이 non-empty invalid/corrupt이면 오류를 표시하고 중단한다. PNG만
   새로 가져오는 fallback으로 손상된 pair를 우회하거나 덮어쓰지 않는다.
5. 대소문자나 다른 suffix의 후보를 추측하지 않는다. pair는 path extension을
   `.png`에서 `.ntdr`로 한 번 교체한 결과뿐이다.

`.ntdr` 자체를 열거나 더블클릭하면 기존 project activation 규칙대로 직접 연다.
지원하지 않는 이미지 형식은 등록하거나 조용히 변환하지 않는다. JPEG, WebP, SVG는
각 형식의 alpha/color/import 의미를 정한 후 별도 단계에서 추가한다.

## Save와 Godot 반영

1. 새 PNG activation이면 이미 durable한 imported pixels/history 뒤에 최신 편집을 commit한다.
2. 기존 project이면 최신 immutable snapshot을 `.ntdr`에 durable commit한다.
3. project/export worker가 durable commit과 같은 FIFO 순서로 sibling temporary PNG를
   encode한다.
4. 한 worker가 export를 순서대로 교체하므로 이전 저장이 이후 저장보다 늦게 완료될 수
   없다. 병렬 encoder를 도입할 때는 명시적 generation 검사를 추가한다.
5. Godot의 일반 filesystem watcher/importer가 바뀐 PNG를 감지한다.

Godot의 reimport 성공은 NyatiDraw project 저장의 일부가 아니다. Godot이 닫혀 있거나
갱신이 늦어도 `.ntdr` 저장은 성공할 수 있다. NyatiDraw는 Godot에게 부분 PNG를
노출하지 않는 것까지만 책임진다.

## Windows 등록

- `.ntdr`은 `NyatiDraw.Project.1` ProgID로 NyatiDraw가 소유한다.
- `.png`의 기본 handler나 `UserChoice`는 바꾸지 않는다.
- NyatiDraw를 `Applications\\NyatiDraw.exe` open command와 `.png` supported type으로
  등록해 Windows `Open with` 후보에 나타나게 한다.
- 사용자가 명시적으로 선택한 경우에만 PNG가 NyatiDraw로 열린다.
- uninstall은 NyatiDraw가 만든 registration만 제거하고 PNG 기본 앱과 artwork를
  건드리지 않는다.

관련 Windows 등록 원칙:

- [Microsoft: File Types 등록](https://learn.microsoft.com/en-us/windows/win32/shell/fa-file-types)
- [Microsoft: ProgID](https://learn.microsoft.com/en-us/windows/win32/shell/fa-progids)
- [Microsoft: HKEY_CLASSES_ROOT와 사용자별 Classes](https://learn.microsoft.com/en-us/windows/win32/sysinfo/hkey-classes-root-key)

## 활성 스프린트 acceptance

- 설치된 release app이 Windows `Open with`의 PNG 후보로 보인다.
- `player.png`와 valid `player.ntdr`이 있으면 PNG를 열어 정확히 그 project가 열린다.
- `.ntdr`이 없거나 0-byte이면 PNG pixels를 보존한 새 project가 열리고 복구용 `.ntdr`은
  즉시 생기지만, 첫 Save 전에는 원래 PNG가 변경되지 않는다.
- non-empty invalid sibling은 보존되며 오류 뒤 fallback project를 만들지 않는다.
- 첫 Save 뒤 `.ntdr`을 재실행해 imported pixels/history가 복구되고 PNG는 최신
  snapshot과 일치한다.
- 저장 중인 부분 PNG나 이전 저장 결과가 최신 PNG 뒤에 노출되지 않는다.
- 이미 실행 중인 NyatiDraw에 같은 PNG 또는 `.ntdr`을 다시 열면 second process는
  primary instance로 activation만 전달하고 종료한다. primary가 기존 window를
  foreground 요청하며, same pair는 reopen/second writer 없이 유지한다.

## 2026-09-05 설치 release PNG pair 실행 근거

실제 설치된 release `NyatiDraw.exe`를 positional PNG path로 실행했다.
`desktop_export_recovery_smoke`의 scratch acceptance에 다음 왕복을 연결했다.

- sibling 없음과 0-byte: 최초 import의 `.ntdr` 즉시 생성, Save 전 PNG bytes 보존.
- 정상 sibling: 별도로 바꾼 PNG를 다시 import하지 않고 기존 project를 복원하며,
  Save 시 durable pixels로 PNG를 교체.
- 손상된 non-empty sibling: project와 PNG bytes 모두 보존, import fallback 없음.
- PNG 및 같은 `.ntdr`로 두 번째 프로세스를 실행하면 기존 primary로 전달 후 code 0
  종료, primary에서는 동일 project 재사용.
- 2×2 page 밖 semantic stroke를 Save하고 별도 프로세스로 다시 열어 exact tiles와
  history 복원. PNG에는 page crop만 남음.

이 실행은 작은 PNG의 Fit 배율이 `ViewportTransform` 상한을 넘어서 첫 canvas frame을
그리지 못하는 오류를 발견했다. Fit과 확대 명령을 같은 MIN/MAX zoom 범위로 제한한
후 설치판에서 전체 왕복과 기존 export crash/recovery acceptance를 통과했다.
환경은 Windows 11 Home 10.0.26200 / Core Ultra 7 155H / Intel Arc / DX12,
DX 0.7.9 release이며 로그는 `target/installed-png-pair-fixed.log`다.
이는 positional activation과 프로세스 왕복 증거다. Explorer Open with 직접 선택,
Shell foreground 허용, Godot reimport, 물리 펜 또는 first-visible-pixel 증거는 아니다.

## 스프린트 밖 backlog

Godot FileSystem의 `만들기`, `NyatiDraw로 편집`, 명시적 reimport 기능을 제공하는 editor
addon은 유용하지만 현재 스프린트 완료 조건이 아니다. 기본 Windows image-open 경로가
충분히 안정화된 뒤 별도 통합 작업으로 판단한다. addon 때문에 현재 문서 포맷,
activation CLI 또는 저장/export 경계를 바꾸지 않는다.

## 색상 수정 이후 PNG 계약

`047cf6f`부터 file export는 sRGB tag가 있는 16-bit straight-alpha PNG다.
이는 기존 linear-light RGBA8 타일의 export/import 정밀도를 보존하기 위한
전송 형식이며 project painting bit depth를 바꾸지는 않는다. PNG import는
sRGB/명시적 gamma를 해석하고 미지원 ICC/cICP/색 원색은 오류로 알린다.
색상 정보가 없는 PNG는 sRGB로 가정한다. 임의의 외부 sRGB 색은 최초 linear8
타일 변환 때 양자화되므로 모든 원본 색의 byte-exact 보존을 주장하지 않는다.
원본 PNG는 여전히 첫 Save 전까지 바꾸지 않으며 valid `.ntdr`이 있으면 그
프로젝트를 사용한다. Godot의 16-bit PNG 실제 import/watch 검증은 아직 남아 있다.
[색상 결정과 근거](decisions/ADR-0029-srgb-boundaries.md)를 참조한다.

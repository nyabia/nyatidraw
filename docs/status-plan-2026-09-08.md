# 2026-09-08 다음 Windows 알파판 실행 계획

문서 기준 시각: 2026-09-08T21:24:30+09:00

기준 소스: `b820b3d`. alpha.3의 기존 설치·업데이트 증거는
[배포 문서](releasing.md)에 있으며, 아래 항목은 이번 작업의 완료 조건이다.

## 병렬 작업과 소유권

| 작업 | 수정 범위 | 완료 조건 |
|---|---|---|
| 다른 이름으로 저장 | desktop 파일 명령·문서 전환, 새 Save As 모듈 | 원본과 전체 히스토리 보존, 새 `.ntdr`/PNG 재열기, 충돌·실패 복구 |
| 알파 파일 연결 | Windows association 모듈, Velopack 설치·업데이트·제거 훅 | 안정적인 실행 경로, PNG 기본 앱 보존, 자신이 소유한 등록만 갱신·제거 |
| 종료·전환 감사 | activation pipe, writer/export/업데이트 수명 | 재현 가능한 무한 대기·작품 손실·실패 숨김 수정 |
| 통합 수용 | release 빌드, scratch 실행, 설치 패키지 | 정상 종료·별도 프로세스 복원·전체 PNG 비교와 설치판 사용 흐름 |

코드 변경은 각 담당 파일에서 수행하고 통합 빌드는 한 번에 하나만 실행한다.
자동 테스트는 작품·입력·복구·등록 소유권의 핵심 불변 조건만 다룬다.
실제 펜, 고주사율, 다중 모니터, 깨끗한 Windows VM 증거는 별도로 구분한다.

## 통합 순서

1. 기존 핵심 테스트로 기준선을 확인한다.
2. Save As와 기존 문서 전환의 입력/명령 차단·writer 완료·실패 복구를 검토한다.
3. 알파 설치 훅과 종료 가능한 activation listener를 결합한다.
4. fmt/Clippy/핵심 테스트와 DX release 빌드를 수행한다.
5. scratch 파일로 편집·Save As·실패·일반 종료·별도 프로세스 재열기를 확인한다.
6. 알파 설치·파일 연결·업데이트를 확인하고 다음 설치 패키지와 커밋을 정리한다.

렌더 스레드 개편은 위 통합 결과를 확보한 뒤 별도 실행 단위로 진행할 수 있다.
시간 경과만으로 미완료 작업을 검증 완료 또는 배포 완료로 표시하지 않는다.

## 실행 기록

- 시작: 원격 main과 일치, 로컬 변경 없음.
- 초기 감사: 연결만 열고 frame을 보내지 않는 activation client 때문에
  기존 synchronous pipe read와 종료 join이 무한 대기할 수 있는 경로 발견.
- 초기 감사: 기존 문서 전환이 이전 writer의 실패 latch를 지우고 대기 중인
  semantic command를 버릴 수 있는 경로 발견. Save As와 함께 전환 경계를 수정한다.
- `cargo test --workspace --locked` 통과. 후속 회귀 검사를 포함해 데스크톱 22개 통과,
  실제 registry 대신 scratch registry를 사용하는 수용 검사 1개는 별도 실행했다.
- `cargo test -p nyatidraw-desktop lifecycle_preserves_foreign_associations_and_user_choices --locked -- --ignored`
  통과: 설치·갱신·제거의 외부 등록/기본 앱 보존. 실제 Explorer 표시 증거는 아니다.
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`와
  `cargo fmt --all -- --check` 통과. vendored Wry의 기존 lifetime 경고는 남는다.
- 추가 감사: 실패한 Save As 뒤 원본 PNG 실패 상태가 지워지는 경로를 수정했다.
  원본 writer를 다시 만들면 export generation도 새 세대로 재기준화하여 다음 Save가
  이전 실패 표시를 정상적으로 갱신할 수 있게 했다. table-driven 핵심 검사 통과.
- DX release 빌드 통과. Windows 11 Pro 26200, Ryzen 7 5800X3D,
  RTX 3080 (driver 32.0.15.9621), WGPU DX12/Mailbox 환경.
- 실제 Save As: scratch `source.ntdr`에서 Windows 저장 대화상자를 열어 한글/공백 이름으로
  사본을 생성하고 문서 제목 전환·정상 종료를 확인했다. 이후
  `desktop_save_as_fixture verify`를 별도 프로세스로 실행해 원본 bytes 불변,
  3개 history snapshot/분기/음수 좌표 타일/레이어·페이지 metadata 보존과 PNG bytes 일치 통과.
- 기존 export/recovery smoke의 첫 synthetic drag timeout은 사용자 저장 레이아웃에
  의존하던 probe를 scratch layout으로 격리한 뒤 해소됐다. 최종 전체 실행 통과:
  20개 레이어, 6개 synthetic drag 경로, 66개 history 항목의 페이지 전환,
  PNG import/pair 활성화, 4개 export crash 경계, stale generation, 실패 후 retry.
  synthetic drag를 실제 마우스·펜 하드웨어 증거로 계산하지 않는다.
- 기존 durability smoke는 정상 stroke·input discontinuity·active close/save 재열기까지
  통과했다. invalid project에서 새 오류 확인 창을 유지하므로 예전 자동 종료 기대를
  수정했다. 이 scratch 경우는 오류 유지·bytes 보존 확인 후 정확한 자식 PID를 abort하며,
  정상 종료 증거로 계산하지 않는다.
- 최종 durability smoke 통과: 32개 stroke/8개 eraser, 정상 종료 후 재열기,
  layer protocol, input overflow fail-closed, active close/save, invalid-file 보존.
- alpha.4 설치 경로의 실행 파일에서도 위 durability 실행과 한글/공백 PNG pair 열기,
  정상 종료·별도 verifier 재검사를 통과했다. 단, 아래 설치 격리 한계가 적용된다.

## alpha.4 후보와 남은 설치 게이트

`target/releases/0.1.0-alpha.4/`에 로컬 후보를 생성했다. 아직 게시하지 않았다.
설치 파일은 12,901,252 bytes, 전체 업데이트 패키지는 8,439,684 bytes다.
설치 파일 SHA256:
`e5fded99755ba35081954fc5e380e51a79fec6a0f1774bfadd69d3f5afb043e5`.
검증한 DX 실행 파일 SHA256:
`70eca144a512f7dce3a557119f21c2cbc57521989709211d164626f475fcece8`.
로그와 scratch 작품은 `target/alpha4-acceptance/`에 있으며 Git에 넣지 않는다.

Codex 하위 프로세스에서 alpha.3 → alpha.4 적용, 제거, 재설치는 종료 코드 0이었다.
각 hook의 등록/정리와 기존 PNG 기본 앱·개발판 `.ntdr` 등록·작품 보존도 확인했다.
그러나 실제 파일 handle과 외부 WMI 조회 결과 설치 파일은 Codex 패키지의
`LocalCache/Local/NyatiDraw.Alpha`에 있었다. 일반 `%LOCALAPPDATA%/NyatiDraw.Alpha`
경로에는 외부 WMI가 실행 파일을 찾지 못했다. 반면 ProgID는 실제 사용자 Classes에
기록돼 일반 경로의 `Update.exe`를 가리켰다. 탐색기 Open With에도 앱이 나타나지 않았다.

따라서 위 결과는 **Codex에서 리디렉션된 설치 수명 검증**이지 일반 Windows 설치·
Open With 완료 증거가 아니다. 기존 alpha.3 설치 기록도 이 실행 환경의 가능성을
고려해야 하며 깨끗한 머신 증거로 해석하지 않는다. Codex 전용 경로를 제품 코드에
하드코딩하지 않는다.

남은 게시 게이트는 일반 탐색기에서 Setup 실행 → 외부 WMI에서 실제 설치 경로 확인 →
Open With로 PNG pair 열기 → 정상 종료·재열기다. Windows UI 설치는 사용자 확인 후
진행하며, 보안 경고 우회가 필요하면 사용자가 직접 처리한다. 실제 펜·고주사율·다중
모니터·깨끗한 Windows VM 검증은 여전히 별도다.

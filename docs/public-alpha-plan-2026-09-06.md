# 공개 알파판: 무료 운영과 구현 범위

2026-09-06 사용자 요청에 따른 계획. 이 문서는 배포 완료 증거가 아니다.
성능 최적화는 중단하고, 취미 Godot 개발에서 설치·열기·편집·저장·업데이트가
이어지는 Windows x64 알파판을 다음 목표로 제안한다. 기존 문서의
“공개 배포는 범위 밖”은 이전 스프린트의 경계이며 이번 계획과 구분한다.

## 현재 확인한 출발점

- 기본 편집과 저장·별도 프로세스 재시작·복원 증거가 있다.
- 앱 안의 열기는 비활성이고 실제 Explorer 파일 연결은 미해결이다.
- 미구현/부분 구현 UI가 남아 있어 기능별 사용 가능 범위를 표시해야 한다.
- 개발용 설치 스크립트와 고정 Dioxus CLI 0.7.9는 있다.
- 로컬 저장소에 `.github` 워크플로, 홈페이지, 자동 업데이트 구현은 없다.
- Rust 채널은 `stable`이며 공개 라이선스는 아직 지정하지 않았다.
- 이번 작업은 조사·계획만 수행한다. 저장소 공개, 배포, 라이선스 적용은 하지 않았다.

## 권장 구성

| 역할 | 선택 | 범위 |
|---|---|---|
| 소스·이슈 | 기존 GitHub 저장소 공개 | 새 저장소나 서버 불필요 |
| 홈페이지 | GitHub Pages, 정적 HTML/CSS | 소개·실제 화면·다운로드·짧은 사용법·제한 사항 |
| CI | GitHub Actions 표준 Windows x64 runner | 기존 핵심 테스트, fmt, Clippy, 빌드 |
| 배포 파일 | GitHub Releases | 설치 파일·업데이트 패키지·체크섬·변경 내역 |
| 설치·자동 업데이트 | Velopack 우선 검증 | Rust SDK, 기존 Dioxus 빌드 출력 재사용 |
| 집 머신 | 초기에는 연결하지 않음 | 실제 GPU·펜·설치 검증에 수동 사용 |

Pages 주소는 설정 후 `https://nyabia.github.io/nyatidraw/`를 사용할 수 있다.
아직 해당 주소의 배포 여부는 확인하지 않았다. 도메인은 초기 필수가 아니다.
홈페이지에 설치 파일을 넣지 않고 Releases로 연결한다. 백엔드·DB·로그인도 불필요하다.

공개 저장소의 표준 GitHub runner 실행은 무료이며 대형 runner는 유료다.
Pages는 공개 저장소에서 무료이고 사이트 1 GB, 월 트래픽 100 GB의 soft limit가 있다.
Releases는 파일당 2 GiB 미만이며 문서상 총 크기·대역폭 제한이 없다.
Actions 임시 산출물은 최소화/단기 보관하고 cache는 기본 10 GB 범위로 유지한다.
유료 runner나 유료 저장 한도 확장을 켜지 않는 구성을 기준으로 한다.

## 구현 순서와 완료 기준

1. **공개 준비 및 CI**
   - 공개 대상 파일과 Git 이력을 점검하고 라이선스 선택을 확정한다.
   - 테스트를 늘리는 대신 기존 작품·입력·복구 핵심 테스트를 자동 실행한다.
   - 실제 검증한 Rust 버전과 Dioxus CLI를 고정하고 Cargo.lock을 사용한다.
   - PR/main 변경 시 Windows 검증, 문서만 변경하면 무거운 앱 빌드는 생략한다.
   - 동시 중복 작업 취소와 제한된 캐시를 사용한다. PR에는 배포 권한을 주지 않는다.
   - 완료: 새 GitHub runner에서 검증이 실제 통과한다. YAML 작성만으로 완료하지 않는다.
2. **알파판 사용 흐름과 설치**
   - 파일 열기, 저장 위치, PNG/ntdr 재열기 흐름을 정리한다.
   - 미구현 조작은 비활성·빗금·설명으로 막고 키보드 등 우회 입력도 일치시킨다.
   - 부분 구현은 가능한 조작을 명시한다. 고급 기능을 모두 채우지는 않는다.
   - Velopack의 시작 훅·single-instance·파일 연결·WebView2 준비와 현재 앱 호환성을 검증한다.
   - 기존 개발판에서 새 설치판으로 이동할 때 작품과 설정의 보존을 확인한다.
   - 완료: Rust/개발 도구 없는 환경에서 설치하고 PNG 편집→저장→종료→재열기 성공.
3. **배포와 홈페이지**
   - 버전 태그 또는 수동 실행으로 검증된 커밋의 설치 파일과 업데이트 패키지를 만든다.
   - 첫 공개는 Release 초안에서 설치 확인 후 게시한다. 매 커밋을 배포하지 않는다.
   - 알파 채널을 명시하고 홈페이지 다운로드가 실제 검증된 릴리스를 가리키게 한다.
   - 완료: 외부에서 홈페이지를 열고 다운로드한 설치 파일로 앱을 사용할 수 있다.
4. **자동 업데이트**
   - 첫 공개 설치판부터 같은 업데이트 도구와 앱 식별자를 사용한다.
   - 업데이트 확인·다운로드는 비동기 처리하고 저장 완료 후 재시작 시 적용한다.
   - 검증 실패·오프라인·다운로드 중단 시 현재 앱을 계속 쓸 수 있어야 한다.
   - 이전 앱으로 되돌리기와 프로젝트 파일 형식의 하위 호환성은 별개로 취급한다.
   - 완료: 실제 A판 설치→B판 업데이트→기존 작품 재열기 및 실패 시나리오 확인.

첫 마일스톤은 1~3의 다운로드 가능한 알파판이다. 전체 배포 작업의 완료는 4까지다.
업데이트 알림이나 다운로드 링크만 만든 상태를 자동 업데이트 완료라고 부르지 않는다.

## 이번 범위에서 제외

- 렌더 스레드 개편, 추가 성능 최적화, 모든 mock 기능 완성.
- 웹 편집기/iframe 임베드, macOS/Linux/ARM 배포, 고급 브러시.
- 클라우드 저장·계정·협업·결제·상시 서버, 다중 OS CI 매트릭스.
- CI의 GPU 실행 결과를 실제 펜/표시 지연 증거로 취급하는 것.

집 머신이 필요한지는 GitHub runner의 실제 빌드 시간·디스크 사용을 먼저 측정해 판단한다.
도입하더라도 외부 PR 코드는 집 머신에서 실행하지 않고 신뢰한 릴리스 작업만 격리한다.
무료 호스팅과 Windows의 게시자 신뢰/코드 서명은 별개다. 인증서/서명 서비스와
SmartScreen 동작은 설치 도구 검증 때 확인하며 경고 없는 설치를 아직 약속하지 않는다.
NyatiDraw 라이선스 선택은 미결정이며 이번 계획에서 임의로 적용하지 않는다.

## 공식 자료 (2026-09-06 확인)

- [Actions 요금과 무료 범위](https://docs.github.com/en/billing/concepts/product-billing/github-actions)
- [GitHub runner 사양](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Pages 제한](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits)
- [Releases 배포 및 용량](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)
- [Velopack Rust 지원 및 정적 호스팅](https://docs.velopack.io/)
- [Velopack 소스와 MIT 라이선스](https://github.com/velopack/velopack)
- [Actions 보안](https://docs.github.com/en/actions/reference/security/secure-use)

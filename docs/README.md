# NyatiDraw 설계 문서

이 폴더는 구현 전에 합의할 제품 경계, 엔진 구조, 성능 기준, 저장 규약, 스프린트 실행 순서를 한곳에 모은다. 가장 짧은 진입점은 [브리핑](briefing.md)이며, 실제 첫 PR의 순서는 [구현 시작점](implementation.md)에 있다.

## 문서 지도

| 문서 | 답하는 질문 |
|---|---|
| [2026-09-08 초기 실행·프레임 진단](status-frame-triage-2026-09-08.md) | 간헐적 초기 실행 정지와 입력 tail을 같은 실행 경계로 어떻게 좁혔는가? |
| [2026-09-08 렌더 격리·CPU 합성](status-performance-2026-09-08.md) | Windows 렌더 대기·창 수명·동일 4K 전후 성능을 어떻게 검증하는가? |
| [2026-09-08 다음 알파판](status-plan-2026-09-08.md) | Save As·알파 파일 연결·종료 안정성을 어떻게 통합 검증하는가? |
| [공개 알파판 계획](public-alpha-plan-2026-09-06.md) | 무료 홈페이지·CI·설치·업데이트를 어디까지 구현하는가? |
| [알파판 배포](releasing.md) | GitHub에서 빌드·홈페이지·설치판을 어떻게 운영하는가? |
| [2026-09-05 현황과 실행 계획](status-plan-2026-09-05.md) | 코드 기준으로 무엇이 구현됐고 Sprint 1~3를 어떤 순서로 닫는가? |
| [브리핑](briefing.md) | 무엇을 만들며, 가장 중요한 결정은 무엇인가? |
| [전체 아키텍처](architecture.md) | 입력·드로잉·문서·렌더·저장은 어떻게 분리되는가? |
| [성능 계약](performance.md) | “빠르다”를 무엇으로 측정하고 언제 실패로 판정하는가? |
| [프로젝트 및 저장](project-format.md) | 빈 파일 초기화, 저장, export, 종료는 어떻게 안전하게 연결되는가? |
| [구현 시작점](implementation.md) | 현재 workspace는 무엇을 고정하며 첫 PR은 어떤 순서인가? |
| [편집기 UI 목표](editor-ui-target.md) | 현재 화면의 각 도구·패널·캔버스가 무엇을 의미하며 무엇이 아직 장식인가? |
| [Windows 개발판 설치](development-install.md) | DX 개발 빌드를 어떻게 설치하고 `.ntdr` 더블클릭과 안전한 제거를 연결하는가? |
| [Godot 작업 폴더와 PNG 연결](godot-integration.md) | PNG를 NyatiDraw로 열 때 sibling `.ntdr`을 어떻게 찾고 안전하게 생성하는가? |
| [스프린트 계획](sprints/README.md) | 어떤 순서와 통과 조건으로 구현하는가? |
| [결정 기록](decisions/README.md) | 무엇을 확정했고 무엇을 기술 스파이크에 남겼는가? |
| [플랫폼 지원](platform-support.md) | 어떤 플랫폼이 구현·검증·보류 상태인가? |

## 읽는 순서

1. `briefing.md`로 제품과 위험을 파악한다.
2. `architecture.md`와 `performance.md`로 핫 패스와 비동기 경계를 검토한다.
3. `project-format.md`로 손실·복구·종료 semantics를 확인한다.
4. `sprints/README.md`에서 다음 검증 가능한 작업만 착수한다.

## 문서 규칙

- 목표 수치는 측정값이 아니다. 통과한 벤치마크만 “검증됨”으로 표기한다.
- 라이브러리 버전은 Sprint 0 결과와 `Cargo.lock`이 권위다.
- `document` 계층은 Dioxus, wgpu, Vello, redb 타입을 노출하지 않는다.
- 기능 완료는 UI 존재가 아니라 저장 후 재실행하여 같은 결과가 복구되는 것으로 판정한다.
- 스프린트 범위 밖 아이디어는 현재 스프린트에 끼워 넣지 않고 backlog로 보낸다.

# ADR-0059: 별도 웹 호스트와 bounded CPU-authoritative 실험판

## 상태와 범위

제한된 웹 실험판 채택. 로컬 기본 드로잉·복구 gate는 통과했으며 배포 gate는 진행 중이다.
이번 별도 승인 범위에서 이전 웹 보류 결정을 보완한다. Windows 주력 구현, 네이티브
Linux/macOS 보류, 문서·브러시·타일의 플랫폼 독립 경계는 바꾸지 않는다.

## 배경

브라우저에서 설치 없이 기본 드로잉과 로컬 작업 복구를 확인하되, Windows 셸이나 redb
저장소를 WASM에 끌고 들어오지 않아야 한다. 기존 엔진과 다른 단순 그림판을 만드는 것도
피해야 한다. 한편 Windows의 native input/GPU-first/writer-thread 구조를 그대로 웹에
이식했다고 주장할 만큼의 성능·입력·저장 검증은 없다.

## 결정

### 호스트와 도구 체인

Dioxus CLI `0.7.9`의 `dx new`에서 **Bare-Bones / Web** 구성을 실제로 생성한 뒤
workspace의 `apps/web`에 맞춰 설정을 반영했다. Dioxus를 라이브러리로만 추가하고
어셋·웹 출력 규칙을 새로 추측하지 않는다. DX가 어셋과 WASM 빌드를 담당하고,
`Dioxus.toml`, `tools/build-web.ps1`, CLI 검증 스크립트가 재현 경계를 명시한다.

| 의존성/도구 | 사용 버전 |
|---|---|
| Dioxus CLI / 직접 Dioxus 의존성 | 0.7.9 |
| 잠금 파일의 `dioxus-web` 전이 의존성 | 0.7.10 |
| wgpu | 26.0.1 |
| web-sys / js-sys | 0.3.104 |
| wasm-bindgen | 0.2.127 |
| wasm-bindgen-futures | 0.4.77 |
| png | 0.17.16 |
| base64 (페이지 한정 레이어 미리보기) | 0.22.1 |

직접 의존성은 정확한 버전을 지정하고 `Cargo.lock`을 함께 관리한다. 기존 workspace의
Dioxus Desktop feature는 웹 호스트에 활성화하지 않는다. 현재 CLI 설치 자동화의 검증
대상은 Windows x64와 Linux x64이며 macOS 설치 절차를 검증한 것으로 보지 않는다.

### 드로잉과 표시

`crates/web-core`는 기존 `brush`, `input`, `paint-cpu`, `tiles`, `document`를 재사용한다.
실제 2H/2B 연필 모델·필압·보정·선형 premultiplied 타일 규칙을 공유한다.
브러시는 메인 WASM 스레드에서 CPU 타일에 적용하고, dirty tile을 기존
`GpuCompositeScene`에 업로드하여 wgpu WebGPU로 합성·표시한다.

GPU는 웹 실험판 문서의 유일한 권위 상태가 아니다. Dioxus는 UI와 저빈도 상태를 담당하고
DOM Pointer/coalesced 입력은 브러시 코어에 직접 전달한다. 256개 초과 coalesced sample,
capture loss, 비정상 입력·자원 초과는 부분 획 취소로 처리한다.

Canvas 2D/WebGL fallback, 별도 JS 브러시 엔진, WASM용 데스크톱 redb 포팅은 채택하지 않는다.
Windows GPU-first 경로와의 성능 동등성도 주장하지 않는다. worker나 GPU 브러시가 필요하면
실제 지연·메모리 측정 후 별도 ADR로 바꾼다.

### 작업 복구와 교환

- 브라우저의 현재 작업 하나를 IndexedDB에 저장한다. Web Locks로 하나의 쓰기 탭만 허용한다.
- completed snapshot과 도구 설정을 generation별로 직렬 저장한다. 오래된 완료 응답은
  최신 저장 완료 판정이 될 수 없다. `beforeunload`는 경고 요청이지 fsync/종료 barrier가 아니다.
- `.nyatidraw-web`은 `NYWEB001` portable bytes의 웹 전용 확장자다.
  Windows `.ntdr`로 위장하거나 자동 호환한다고 표시하지 않는다.
- 그림·signed tile·평면 레이어 속성과 순서·페이지·활성 레이어·도구별 설정·색상을 보존한다.
  진행 중 획, RAM Undo/Redo, 세션 최근 색은 저장하지 않는다.
- raw 또는 lossless RGBA-run tile과 BLAKE3 checksum을 사용한다. 임의 크기 JSON 배열이나
  제한 없는 압축 해제를 사용하지 않는다. 해독은 새 문서에서 끝낸 뒤 교체하며 실패한 복구
  데이터를 자동 덮어쓰지 않는다.
- 파일 열기·새 그림은 앱 내부 확인창에서 승인한 뒤에만 현재 작업을 교체한다.
  취소 시 대기 문서만 폐기하며, 확인창 동안 그림 입력·편집 단축키는 차단한다.
  GPU 초기화가 실패해도 저장된 복구 바이트를 내려받을 수 있다.
- PNG는 평면 교환용이다. straight sRGB8 ↔ linear premultiplied RGBA8 변환의 양자화를
  인정하며, 정확한 작업 복구에는 portable bytes를 쓴다.

128단계/128MiB 유지 예산, 현재 1024타일, 4096px 페이지 축, 32래스터 및 68MiB portable
상한을 둔다. 진행 획·이벤트·파일/PNG·JS/GPU 메모리는 별도 상한과 검증 대상이다.
브라우저 전체 RAM을 128MiB로 보장한다는 뜻이 아니다.

## 검증과 아직 닫히지 않은 gate

코어 핵심 테스트 3개와 WASM 검사/Clippy가 통과했다. Windows의 Codex 내장 브라우저에서
실제 WebGPU 표시, 그리기, Undo/Redo, 탭을 닫고 새 탭으로 현재 그림·펜 20px 설정 복구,
두 번째 편집 탭 차단을 확인했다. 복구 전후 테스트 그림 PNG의 SHA-256도 일치했다.

이것은 실제 태블릿, 모바일, 브라우저 프로세스 종료 후 재실행, device loss, 장시간 작업,
고주사율 input-to-visible 성능의 검증이 아니다. 데스크톱 설치본은 변경하지 않았다.

로컬 Windows DX에서 실패한 선택적 wasm-opt 후처리는 비활성화했다. Rust release 최적화는
유지하고 디버그 심볼을 제외한 배포 빌드가 성공했으며 WASM은 약 1.30MB다.
Rust 경로 remap 뒤에도 Manganis 자산 정보에는 CSS 원본 절대 경로가 남으므로,
개인 작업 공간 산출물은 공개하지 않고 GitHub runner에서 만든 산출물만 배포한다.
Linux CI·Pages 배포·공개 페이지 동작은 진행 중이며 최종 결과를 따로 갱신한다.

## 결과와 재검토 조건

엔진 재사용과 브라우저 어댑터 검증을 독립적으로 진행할 수 있지만, 웹은 Windows의 모든
기능과 `.ntdr` 호환성을 갖춘 정식 대체판이 아니다. 저장소가 차단되거나 GPU가 지원되지
않는 경우 조용히 휘발성/저성능 모드로 바꾸지 않고 제한을 표시한다.

장시간 CPU 작업이 입력을 막거나 메모리 예산에 자주 닿는다면 Web Worker/증분 저장/GPU
브러시를 각각 검토한다. 그 전에 실제 측정값과 실패 원자성·취소·복구 검증이 필요하다.
지원 범위 확장은 별도 결정이며 이번 스파이크를 완료한 것처럼 보이게 하려고 끼워 넣지 않는다.

운영·실행 절차와 최신 검증 범위는 [웹 지원 문서](../web-support.md),
바이트/API 계약은 [web-core README](../../crates/web-core/README.md)를 참고한다.

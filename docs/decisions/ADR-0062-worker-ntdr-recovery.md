# ADR-0062: 정적 웹 배포의 NTDR 복구 Worker

## 결정

`apps/web-worker`의 dedicated module Worker가 별도 WASM 인스턴스로 공용
`project-web` 저장 어댑터를 실행한다. 압축·redb 메모리 DB 처리·IndexedDB 쓰기는
Worker가 소유한다. 메인에서 완성 파일을 만들어 넘기는 구조는 사용하지 않는다.

기존 wasm-bindgen `0.2.127`과 같은 버전의 프로젝트 로컬 CLI를 사용한다.
외부 저장 서비스, SharedArrayBuffer, WASM threads, COOP/COEP는 필요하지 않다.
DX 산출물에 content-hash Worker JS/바인딩/WASM/저장 모듈과 manifest를 추가한다.
GitHub Pages 등 일반 HTTPS 정적 배포를 유지한다. 포맷과 확장자는 `.ntdr` 그대로다.

## 경계와 실패 처리

- 메인은 완료된 문서의 metadata, 전체 타일 키 목록, 이전 승인본과 달라진 raw tile만
  전송한다. 첫 요청은 원래 파일도 포함한다. 목록에서 빠진 타일은 삭제다.
- 작업은 실행 중 하나와 최신 대기 상태만 유지한다. 변경 패킷을 무한히 쌓지 않고
  다음 캡처에서 최신 문서 전체 키를 비교하므로 중간 삭제/변경이 누락되지 않는다.
- `epoch / revision / base_revision`이 맞는 요청만 받는다. Writer는 staging과
  accepted 상태를 분리하고 IndexedDB transaction complete 뒤에만 accept한다.
- 손상 패킷, stale base, 실패한 저장은 accepted 상태를 변경하지 않는다. 메인의
  저장 완료 표시는 현재 generation이고 진행 중 획이 없을 때만 갱신한다.
- Worker 실행/응답 오류 및 120초 timeout은 자동 저장 실패로 알린다. 이후 수동 파일
  저장에는 메인 인코딩 구조 경로가 있다. 이 경로는 원래 파일과 현재 그림을 보존하지만
  Worker가 추가한 중간 저장 기록은 포함하지 못할 수 있다. 브라우저 전체 메모리 고갈에
  대한 복구 보장은 아니다.
- 600ms quiet / 최대 3초 대기 묶음, 최대 250ms idle 예약을 사용한다. 미완성 획은
  저장하지 않는다. 강제 종료 전 마지막 묶음까지 보존하는 durable deadline은 아니다.

## 검증과 남은 범위

핵심 검사는 여러 획/타일의 누적 변경, 삭제, Undo, native history 보존, 실패·손상·
stale 요청 거부를 포함한다. `worker_reopen_probe`는 첫 전체 저장과 차분 저장을
각각 별도 프로세스에서 native/web reader로 열고 정확한 content root를 비교한다.

로컬 release 브라우저의 실제 IndexedDB 저장 및 탭 재개를 확인했다. 계측은
`?timings`에서만 동작하고 작품·경로·색상을 기록하지 않는다. 상세 측정과 미검증 경계는
[웹 지원 기록](../web-support.md)을 따른다.

브러시 CPU 래스터화, 미리보기, PNG export, 파일 열기는 여전히 메인이다.
초기 전송과 raw tile 복사는 메인 비용이 있으며 두 WASM 인스턴스의 메모리가 필요하다.
현재 타일/파일/히스토리 상한을 브라우저 전체 메모리 상한으로 해석하지 않는다.
브라우저 강제 종료, IndexedDB quota 고갈, 대형 작품 장시간·물리 펜 시험은 별도다.

# 2026-09-08: 완료 타일 반영의 반복 복사 제거

문서 기준 시각: 2026-09-08T21:24:30+09:00

진행 중. 기준은 `4a432c8`이며 [이전 프레임 진단](status-frame-triage-2026-09-08.md)의
무입력 scene tail 후속 작업이다. 아직 지연 개선이나 최적화 검증 완료를 뜻하지 않는다.

## 확인된 코드 비용과 미확정 원인

`NativeCanvas::apply_materialized_tiles`는 GPU 반영을 미룬 완료 타일을 매 호출마다
다시 CPU map에 clone/insert한다. 128×128 RGBA8 타일은 64KiB이고 함수는 정상
render에서 두 번 호출된다. 따라서 새 writer completion의 CPU 반영과 deferred
GPU 반영 재시도를 분리할 가치가 있다. 이것이 관측된 17~19ms scene의 주원인인지는
아직 별도 계측이 필요하다.

`upload_closed_tile` 자체에는 tile별 submit이나 texture 생성이 없지만 내부
`write_texture`의 staging 비용은 남는다. Dirty group 합성의 submit을 단순히
묶는 것은 공유 opacity uniform 순서를 바꿔 그림을 훼손할 수 있으므로 이번 작업에서
하지 않는다. 미리보기 세대·live stroke·history 순서를 단축해서 속도를 얻지 않는다.

## 실행 순서

1. 기존 동작 그대로 두고 opt-in trace에 완료 처리 누적시간·호출수·새 completion·
   CPU 복사량·GPU 반영/대기 수와 viewport 경계를 연결한다.
2. 이 진단-only DX release에서 4K baseline/export 각 1회, 정상 종료·독립 tile/PNG
   대조를 수행한다. 측정 중 build나 CPU 검증을 같이 돌리지 않는다.
3. 새 completion의 CPU 반영을 한 번만 수행하고 GPU 대기 항목에는 재반영하지 않는다.
   동일 key의 오래된 deferred/새 completion 순서와 history adoption을 독립 검토한다.
4. 동일 조건으로 최적화 DX release를 측정하고 복사 횟수·CPU 시간·입력 tail을
   각각 비교한다. 제한된 표본의 p99 하나로 인과나 보편적 개선을 주장하지 않는다.
5. 핵심 artwork 불변식만 테스트하고 fmt/Clippy/release build, 저장 후 프로세스 종료와
   재열기 검증, 문서·결과 커밋으로 완료한다.

기존 paced workload는 매 stroke 뒤 history 게시를 기다리므로 deferred 경로를
거의 밟지 않을 수 있다. 이를 기존 비교에 섞지 않고 별도 opt-in overlap 실행을
추가한다. 같은 32×121 샘플과 최종 그림을 유지하되 bounded 소그룹마다 history를
기다려 다음 stroke 진행 중 이전 완료가 도착하게 한다. Export 동시 모드와는
혼용하지 않는다. 양 빌드에서 같은 변형을 한 번씩 별도 비교하며 실제 defer 계수가
0이면 이 실험이 반복 복사 제거를 입증했다고 쓰지 않는다.

기존 설치판·사용자 작품·공개 배포는 변경하지 않는다. 최초 startup 정체, 실제 펜,
가시 픽셀·고주사율·다른 GPU의 증거는 계속 별도 미검증으로 남긴다.

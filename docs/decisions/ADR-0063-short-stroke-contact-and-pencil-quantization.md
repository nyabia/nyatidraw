# ADR-0063: 짧은 획의 필압 시작과 연필 양자화 보존

## 결함

round v2는 시작 필압 0 / 최소 크기 0일 때 반경 0 dab만 만들 수 있었다.
그 뒤 압력이 올라와도 이동 거리가 spacing 미만이면 유효한 dab가 없었다.
pencil v3는 작은 2H 탭의 모든 coverage가 paper tooth 또는 RGBA8 반올림으로
0이 될 수 있었다. UI·저장 없는 공용 CPU 재현에서도 발생했다.

## 결정

- 새 round engine v4, pencil engine v5를 사용한다. preset schema는 각각 2/3을
  유지한다. 기존 engine v1/v2/v3의 평가, grain, 직렬화 바이트와 stroke hash는 보존한다.
- v4/v5는 최초 양의 필압이 일반 간격 dab 이전에 도착하면 해당 위치에 한 번 즉시
  찍고 거리 누적을 다시 시작한다. 그 이후에는 기존 거리 기반 간격을 따른다.
  펜을 놓을 때 압력이 0이 되어도 이미 받은 유효 필압을 잃지 않는다.
  크기·불투명도 필압을 모두 끈 경우 같은 위치의 압력 변화로 추가 자국을 만들지 않는다.
- pencil v5의 grain v2는 기존 절차적 종이에 미세 graphite undercoat를 둔다.
  팁의 4×4 coverage가 있는 픽셀에서 양의 opacity/flow는 최소 1/255 dab alpha를
  유지한다. 팁 바깥·opacity 0·flow 0은 계속 투명하다. 여러 자국은 기존처럼 누적되므로
  아주 연한 영역의 질감은 v3보다 약간 촘촘해질 수 있다. 기본 크기는 키우지 않는다.
- GPU instance의 grain tag 상위 비트로 undercoat를 전달한다. CPU/WGSL은 같은
  정수 grain/coverage와 alpha 양자화를 사용한다. 이 필드는 문서에 직접 저장하지 않고
  기록된 engine version으로 재생성한다.
- 의미론적 v4/v5 획은 NTDR capability `0x8000`을 같은 트랜잭션에 기록한다.
  새 의미론을 모르는 기존 writer가 파일을 수정하지 못한다. v5는 기존 pencil capability도
  필요하다. 웹의 baked tile structural 저장은 새 semantic stroke record를 만들지 않는다.

## 증거

- `short_stroke_probe`: 20px/8% pen, pressure 0→1→0에서 0/1px 이동 모두
  유효 픽셀 0→332. 2px 이동은 기존 276을 유지한다.
- 2H 정수 좌표 256개에서 pressure 0/0.1/0.5/1의 완전 투명 탭은
  기존 160/34/4/0 → 새 엔진 0/0/0/0이다. 1/4px 위치까지 4096개 핵심 회귀 검사와
  원본 샘플 재생·다른 패킷 배치·opacity/flow 0 검사를 통과했다.
- `gpu_cpu_diff --release`: RTX 3080, Vulkan, Windows 11 Pro 10.0.26200.
  새 2H/2B signed grain, legacy tap, 새 low/zero/half-pressure tap 및 onset fixture의
  CPU/GPU byte delta는 모두 0이다. 기존 round/eraser fixture는 기존 허용오차 내다.
- native pencil/history core acceptance는 별도 프로세스 재열기를 포함한다.
  Worker 전체/차분 저장도 native/web 별도 프로세스에서 content root가 같다.

이는 실제 장치 필압·드라이버 입력, first-visible-pixel 지연, 모든 GPU backend의
증거가 아니다. 외부 보고자가 사용한 브라우저/장치 조합의 직접 재현도 별도다.

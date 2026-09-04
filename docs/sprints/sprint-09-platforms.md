# Sprint 9 - 플랫폼과 생태계

## 사용자 결과

Windows 밖에서도 같은 `.ntdr` 의미를 유지하고, 필요한 경우 web viewer/editor나
자동화 도구와 연결한다.

## 범위

1. Wayland와 macOS 스타일러스 backend를 각각 capability-detect한다.
2. native redb repository와 같은 의미의 web OPFS/IndexedDB adapter를 구현한다.
3. headless CLI, plugin API, canonical pack/unpack을 안정된 schema 위에 제공한다.
4. backend별 pressure/tilt/buttons, renderer, save/export 지원표를 공개한다.

## 완료 gate

- 각 플랫폼 결과는 실제 hardware/backend 증거로 구분한다.
- synthetic input이나 compile 성공을 physical pen/cross-platform 완료로 세지 않는다.
- 같은 fixture가 플랫폼을 넘어 project 의미와 export 기준을 보존한다.

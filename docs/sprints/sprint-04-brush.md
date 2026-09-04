# Sprint 4 — 브러시다운 브러시

## 목표

Clip Studio를 동작 참고점으로 삼되 범용 node graph 없이 표현력 있는 raster brush preset을 만든다.

## 작업

- bitmap tip, spacing, flow, opacity, density 분리.
- pressure/speed/tilt/direction/distance/random modulators와 curves.
- rotation, scatter, hardness, stabilization.
- build-up와 wash accumulation mode.
- preset schema/versioning, tip resource deduplication.
- synthetic stylus corpus와 golden brush gallery.
- large brush/texture cache stress test.

## 게이트

- 동일 engine version/seed/input은 동일 backend에서 같은 결과를 낸다.
- preset migration round trip이 golden corpus를 보존한다.
- 8K·256px texture brush에서 bounded memory와 latency 결과를 기록한다.
- opacity와 flow가 독립적으로 검증된다.

## 제외

smudge, wet paint simulation, arbitrary brush graph, plugin ABI.

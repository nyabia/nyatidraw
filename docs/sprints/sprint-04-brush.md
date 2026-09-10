# Sprint 4 — 브러시다운 브러시

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

기본 필압 축 분리·hard/soft preset·크기 기억·최소 보정은
[일러스트 워크플로우 W1](../illustration-workflow.md)로 앞당긴다.
아래 목록 전체를 기다려야 기본 펜을 쓸 수 있는 계획으로 해석하지 않는다.

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

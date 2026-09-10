# Sprint 5 — 래스터 편집과 큰 프로젝트

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

기본 선택 조합·직접 변형·채우기 보정·alpha lock/clipping 및 최소 마무리는
[일러스트 워크플로우 W2~W5](../illustration-workflow.md)가 실행 순서를 정한다.
이를 대형 프로젝트 최적화나 전체 고급 필터 완성 뒤로 미루지 않는다.

## 사용자 결과

Sprint 1~2의 빠른 asset 편집을 정밀 선택·변형·필터와 큰 프로젝트, 여러 export
profile까지 확장한다.

## 작업

- selection feather/grow/shrink, mask와 transform 정밀화.
- crop, resize, rotate/flip과 비파괴 adjustment/filter 기반.
- working color space, alpha와 bit-depth 정책 확정.
- layer colorize의 정확한 합성/export 의미 구현.
- 여러 export profile과 해상도 변환.
- flattened tile/mip cache와 background encoder scheduling.
- autosave/checkpoint policy, compaction과 stale temp cleanup.
- export failure/retry UX와 disk-full handling.
- large-project quick-open 및 lazy visible tile hydration.

## 게이트

- selection/mask/transform/filter 결과가 save/restart/reopen과 headless export에서 같다.
- 여러 profile의 generation이 서로의 결과를 덮지 않는다.
- autosave와 explicit save가 같은 history/root 권위를 공유한다.
- compaction 중단 뒤 마지막 valid head를 연다.
- 4K/8K export 중 drawing latency 악화가 기록된 목표 안이다.

## 제외

PSD, background daemon, cloud sync.

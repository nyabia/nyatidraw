# Sprint 5 — 래스터 편집과 큰 프로젝트

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

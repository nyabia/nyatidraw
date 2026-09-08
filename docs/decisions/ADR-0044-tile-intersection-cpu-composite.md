# ADR-0044: CPU raster 합성을 타일 교차 영역으로 제한

문서 기준 시각: 2026-09-08T21:24:30+09:00

- 날짜: 2026-09-08
- 상태: 채택; 핵심 oracle과 desktop 저장/재열기 수용 통과
- 범위: `nyatidraw-paint-cpu`, 파일 export/history용 CPU 합성

## 문제

기존 visible raster 합성은 레이어마다 page 전체 RGBA 버퍼를 만들고 타일을
복사한 뒤, 투명한 빈 영역까지 전체 page를 순회했다. 4K에서 raster 하나당
약 31.6MiB 임시 버퍼와 전체 page 합성 비용이 발생한다. Sparse 문서에서도
레이어 수에 비례해 이 비용을 냈다.

## 결정

Visible raster의 타일과 page가 교차하는 행만 기존 `composite_surface`에
전달한다. 비어 있는 타일은 투명 항등원이므로 방문하지 않는다. Signed tile
좌표 계산은 i64로 하고 page 밖 데이터는 저장 상태에 남기되 export에서 제외한다.

Group은 기존처럼 별도 page surface에서 자식을 합성하고 group opacity를 한 번
적용한다. 정수 source-over 반올림, 순서, visibility/reference 의미론은 바꾸지
않는다. 보이는 레이어의 unsupported mip 오류도 opacity가 0이거나 타일이
page 밖이라는 이유로 숨기지 않는다. 추가 dependency와 GPU 코드 변경은 없다.

레이어별 tile index, group cache, SIMD/parallel 합성은 이번 변경에 포함하지
않는다. 현재 snapshot을 레이어별 순회하는 비용은 남아 있으며 필요하면 별도
측정으로 정당화한다.

## 증거와 한계

Windows 11 Pro / Ryzen 7 5800X3D / Rust 1.96.0 release CPU 실행에서 4K 두
fixture를 변경 전후 각각 warmup 1회 제외 후 20회 측정했다. 단위는 ms,
nearest-rank p50/p95/p99다.

| 장면 | 변경 전 p50 / p95 / p99 | 변경 후 p50 / p95 / p99 |
|---|---:|---:|
| 희소 8레이어, 135 tiles | 512.186 / 554.055 / 560.514 | 75.659 / 83.177 / 99.243 |
| 조밀 2레이어, 1,020 tiles | 192.020 / 203.397 / 204.038 | 151.056 / 171.700 / 172.827 |

[원시 측정과 환경](../measurements/cpu-layer-composite-4k-2026-09-08.json)에 모든
표본과 source hash를 기록했다. 출력/group/raster 임시 할당과 합성을 포함하지만
PNG 인코딩, 저장, 큐, GPU, 입력·표시 시간은 포함하지 않는다. 전체 앱 지연의
개선율이나 export 간섭 gate 통과로 해석하지 않는다.

핵심 table-driven oracle은 이전 full-page 알고리즘과 byte-exact 비교하며
crop 경계, signed 좌표 극값, opacity 반올림, isolated group, hidden/reference,
unsupported mip 오류를 다룬다. crate 13 tests와 focused Clippy가 통과했다.
Desktop 저장·별도 프로세스 재열기·PNG 대조는
[현재 통합 기록](../status-performance-2026-09-08.md)을 따르며 CPU probe만으로
완료를 주장하지 않는다.

## 되돌림 조건

기존 CPU 합성과 artwork/오류 의미론이 달라지거나 대표 장면에서 회귀가 확인되면
기존 crop 기반 raster 분기로 되돌린다. 파일 포맷과 snapshot을 바꾸지 않았으므로
프로젝트 migration은 필요하지 않다.

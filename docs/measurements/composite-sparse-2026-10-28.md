# 희소 CPU 합성: 전후 픽셀 일치와 실행 시간

AMD Ryzen 7 5800X3D, Windows NT 10.0.26300.0, Rust 1.96.0,
`cargo --release`, CPU backend. GPU 표시·펜 입력·PNG 압축은 측정 범위 밖이다.
프로세스별 warmup 1회 뒤 20회를 모두 보존했다. 각 샘플은 3840×2160 합성 한 번이다.
백분위는 정렬한 20개 값의 nearest rank이며 p99는 최댓값이다.
OS 부하·온도는 통제하지 않았고 통계적 신뢰구간을 추정하지 않았다.

| fixture | 전 p50/p95/p99 ms | 후 p50/p95/p99 ms |
|---|---:|---:|
| 희소 8레이어, 135 tiles | 263.059 / 309.314 / 317.013 | 51.318 / 70.055 / 87.631 |
| 전체 2레이어, 1020 tiles | 339.701 / 398.038 / 411.942 | 224.021 / 236.332 / 277.181 |
| 희소 8레이어와 클리핑, 135 tiles | 254.718 / 302.242 / 337.416 | 25.489 / 29.826 / 29.922 |

변경은 없는 타일을 0으로 채운 임시 버퍼로 만들지 않고 생략하며, 없는 clipping base에는
clip을 합성하지 않는 것이다. Normal mode는 같은 정수 반올림 계약을 유지하는
source-over/atop 식을 사용해 픽셀마다 일반 혼합식을 계산하지 않는다. 처음 시도했던
borrowed-tile 버전은 dense p99가 477ms로 나빠져 채택하지 않았다. 최종 수치는 모든
세 fixture에서 개선되었으며 다른 그림·장치에도 같은 배율이 나온다는 뜻은 아니다.

각 fixture의 합성 전체 byte stream SHA256은 전후가 동일했다. 일반/숨김/중첩/색상화/
클리핑 타일 조합은 별도 per-pixel oracle과 대조한다.

| fixture | SHA256 |
|---|---|
| sparse8 | `c3ebcf6c5056fb0e0163da813903018d5bdccf9c02450aeb4b3fca1349ed1089` |
| dense2 | `4e3d15efff8dbeafeaa2d25cfe408fa86829f32766b876624baac62d78b305b2` |
| sparse-clipped8 | `b8bd34886cd8c48a25040dd8c33c617b9599db419b27b5bcc8d4cb60cd627a00` |

재현: `cargo run -p nyatidraw-paint-cpu --example layer_composite_cost --release --locked -- target/composite-check`.
[전체 샘플 JSON](composite-sparse-2026-10-28.json)에 실행 순서의 값과 판정 범위를 남겼다.
GPU blend 검증은 별도 RTX 3080/Vulkan release에서 184 cases, raw 최대오차 0,
display 최대오차 1 byte로 통과했다. 이는 합성 정확도이며 물리 펜/첫 가시 픽셀 증거가 아니다.

# ADR-0050 — 선택 영역 클립보드 경계

문서 정리 기준 시각: 2026-09-09T18:52:14+09:00

## 결정

Windows 그림 교환은 desktop의 `ArtworkClipboard` 어댑터로 격리한다. 픽셀 추출/잘라내기/
붙여넣기는 OS에 의존하지 않는 `paint-cpu::RasterFragment`를 사용한다. 문서 wire schema는
바뀌지 않는다. 기존 `windows = 0.62.2`에 DataExchange/Memory feature만 활성화하며,
PNG는 기존 `png = 0.17.16` 및 `nyatidraw-png-io` 경계를 사용한다. 새 dependency는 없다.

## 형식과 안전성

- `NyatiDraw.RasterFragment.v1`: `NTDRCLP1` 8바이트 magic, little-endian i32 origin x/y,
  u32 width/height, row-major premultiplied linear RGBA8. 24바이트 헤더 뒤에 정확한 픽셀을
  둔다. HGLOBAL allocation padding은 무시한다. 데이터 길이·signed 좌표·premultiplication과
  16 Mi-pixel 상한을 확인한 뒤 픽셀 버퍼를 만든다.
- 등록 형식 `PNG`: 다른 PNG 클립보드 앱과의 교환용, straight 8-bit sRGB+alpha.
  내부 붙여넣기는 전용 형식 우선이므로 낮은 alpha 양자화 round-trip을 회피한다.
  전용 형식이 존재하지만 잘못되었으면 PNG fallback으로 조용히 숨기지 않고 거부한다.
- 읽기 HGLOBAL/PNG 입력은 128 MiB 이하, PNG 이미지도 16 Mi-pixel 이하로 제한한다.
  PNG decoder 자체 기본 64 MiB 한도와 기존 expanded-tile 한도도 유지한다.
- 모든 PNG 인코딩과 HGLOBAL 할당을 clipboard clear 전에 완료한다. 성공한
  `SetClipboardData` 이후 메모리는 OS 소유이며, 실패한 미게시 메모리만 해제한다.
- 잘라내기는 clipboard 게시 성공 후에만 픽셀 트랜잭션을 commit한다. 둘은 하나의
  원자적 시스템이 아니므로 DB 실패 시 클립보드 복사본은 남을 수 있다. 원본은 마지막
  durable 상태로 복구할 수 있고 worker는 fail-stop한다.
- clipboard busy는 무한 재시도하지 않고 사용자에게 재시도를 요청한다. 읽기/인코딩은
  writer에서 수행하고 UI/render/input thread는 해당 작업을 직접 실행하지 않는다.

## Windows 소유권

복사 작업에서 worker가 숨겨진 message-only STATIC 창을 만들고 해당 HWND로 클립보드를
연다. 창은 같은 스레드에서 닫는다. 지연 렌더링은 사용하지 않으며 실제 데이터 핸들을
즉시 게시한다. 읽기에서는 owner가 필요 없고, 바이트 복사 후 잠금을 해제한 뒤 decode한다.

근거: [Microsoft OpenClipboard](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openclipboard)
문서는 NULL owner로 EmptyClipboard한 뒤 SetClipboardData가 실패함을 명시한다.
[Microsoft Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats)는
등록 형식과 표준 비트맵 형식의 구분을 설명한다.

## 보류

DIBV5/DIB/CF_BITMAP 변환, PNG 없는 외부 앱, Linux/macOS, 다중 레이어 복사, floating
paste는 후속이다. 현재 기능은 기본 복붙이며 모든 이미지 앱 호환성을 보장하지 않는다.
실제 OS 클립보드와 외부 앱의 왕복 acceptance는 핵심 memory-adapter 시험과 별개다.

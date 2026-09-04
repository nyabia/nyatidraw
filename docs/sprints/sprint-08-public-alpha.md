# Sprint 8 - 독립 소프트웨어 공개 알파

## 사용자 결과

NyatiDraw를 처음 접한 Windows 사용자가 설치하고, 펜을 확인하고, 작업을 복구하고,
업데이트하거나 제거할 수 있다. Godot가 없어도 일반 드로잉 앱으로 사용할 수 있다.

## 범위

1. signed NSIS/MSI, update channel, file association과 안전한 uninstall을 제공한다.
2. 첫 실행 문서/펜 진단과 최소 onboarding을 제공한다.
3. 키보드 접근성, 고대비/DPI, localization 기반을 점검한다.
4. privacy가 명확한 opt-in diagnostics와 crash recovery 안내를 제공한다.
5. 지원되는 project migration과 이전 버전 rollback/read-only 경로를 검증한다.

## 완료 gate

- clean Windows VM에서 install -> draw -> update -> reopen -> uninstall이 통과한다.
- update/uninstall이 사용자 `.ntdr`, PNG, brush library를 삭제하지 않는다.
- 공개 문서가 구현된 capability와 미지원 범위를 정확히 말한다.

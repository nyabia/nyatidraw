# ADR-0039: 알파판 파일 열기와 새 그림

2026-09-06. Windows UI의 열기/새 그림은 `rfd = 0.17.2` 비동기 대화상자를 사용한다.
Dioxus Desktop이 이미 같은 버전을 사용하므로 새 플랫폼 라이브러리는 늘리지 않았다.
직접 의존성의 default features는 끄고 Windows에만 지정한다.

선택한 경로는 기존 bounded activation inbox에 전달한다. 대화상자에서 저장소를
직접 열거나 UI에 redb/GPU 소유권을 추가하지 않는다. 새 그림에서 기존 프로젝트나
같은 이름의 PNG가 있으면 덮어쓰지 않고 안내한다. 현재 범위는 새로운 이름으로
빈 그림을 만드는 것이며, 기존 작품의 Save As 기능을 의미하지 않는다.

검증: 컴파일, workspace/all-targets/all-features Clippy 통과. 새 자동 테스트 없음.
release 앱 PID 33520에서 새 그림 대화상자로 한글·공백 이름의 scratch `.ntdr` 생성,
마우스 획 1개, Save/PNG 완료, 정상 Close와 writer join. 별도 PID 19288에서
열기 대화상자로 sibling PNG를 열어 동일 프로젝트 snapshot 1과 획 표시를 확인했다.
다시 Save/Close 후 CLI validate의 content root가 동일했고 PNG SHA256도
`602FC7173F5F3A0DA7B7DC9D5EB17D276A283505C994B963DEB93C5E5DF7A394`로 동일했다.
원본 로그/파일은 ignored `target/public-alpha-acceptance/`에 있다.

뭉개기, 레이어 색상화, 실제 브러시 미리보기는 준비 중으로 표시했다.
이 증거는 Windows 설치 프로그램·파일 연결·자동 업데이트의 완료 증거가 아니다.

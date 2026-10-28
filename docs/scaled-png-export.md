# 크기를 지정한 PNG 복사본 출력

W5 산출물과 게임용 이미지에서 원본 해상도를 유지한 채 작은 PNG가 필요한 경로다.
메뉴의 **PNG 크기 지정…**에서 비율(0 초과~100%) 또는 너비/높이를 입력한다.
UI는 원본의 가로세로 비율을 유지하며 정수 픽셀은 반올림하고 최소 1 px로 제한한다.
기본 저장과 PNG companion, 기존 PNG 내보내기, 캔버스 크기 변경은 이 설정을 사용하지 않는다.

## 비교와 범위

[Clip Studio 공식 안내](https://support.clip-studio.com/en-us/faq/articles/20200052)는
출력 크기만 바꾸면 원본을 유지할 수 있고 너비·높이 비율을 유지한다고 설명한다.
[Krita File 메뉴](https://docs.krita.org/ko/reference_manual/main_menu/file_menu.html)의
Export Advanced에도 출력 resize가 있다.
[Photoshop Export 설정](https://helpx.adobe.com/photoshop/desktop/save-and-export/export-files-to-different-formats/export-settings-and-export-location-preferences.html)은
출력 image size와 scale을 별도 설정으로 둔다.
이번 범위는 작은 PNG 복사본이라는 작업만 연결한다. 원본의 파괴적 리샘플링,
확대 보간, batch, sprite sheet, 애니메이션 출력은 포함하지 않는다.

## 그림 보존과 자원 경계

- `png-io`의 공통 encoder는 합성된 premultiplied linear RGBA8을 읽기만 한다.
  정수 겹침 가중치로 각 출력 픽셀의 면적을 적분하므로 축소율이 정수가 아니어도
  모든 원본 픽셀을 반영한다. 알파도 같은 가중치로 계산한 뒤 sRGB로 바꾼다.
  완전히 투명한 픽셀의 RGB를 섞어 가장자리를 어둡게 만들지 않는다.
- 축소 결과를 RGBA8로 다시 양자화하지 않고 straight sRGB16 PNG로 기록한다.
  축소 중 임시 출력 버퍼는 최대 8 KiB다. 원본 합성은 기존 64 Mi-pixel 한도를
  사용하며, PNG 압축기 자체 버퍼와 웹의 파일 바이트 버퍼는 별도다.
  100%는 기존 16-bit PNG encoder 경로를 사용한다.
- 프로젝트 픽셀·레이어·캔버스 크기·선택·Undo/Redo를 바꾸거나 프로젝트 저장을
  대신하지 않는다. 변형·색 보정·획을 끝내야 출력할 수 있다.
- Windows는 기존 프로젝트 writer FIFO에서 실행한다. 크기 지정 출력은 한 번에
  하나만 수락하고 별도 generation gate를 사용해 일반 Save 요청을 취소하지 않는다.
  프로젝트 epoch를 확인하고, 같은 writer의 종료/프로젝트 교체는 기존 join을 따른다.
- 출력은 **새 파일 이름**만 허용한다. 기존 PNG 및 같은 이름의 `.ntdr` companion을
  거절한다. sibling 임시 파일을 완전히 인코딩·sync한 뒤 generation을 확인하고
  배타적으로 새 경로를 게시한다. 실패는 기존 출력·프로젝트를 덮어쓰지 않는다.
  새 파일 게시에 파일시스템 hard link를 사용하므로 이를 지원하지 않는 저장소에서는
  오류를 표시하고 원본을 유지한다.
- 웹은 클릭 시점의 합성 사본을 별도 한 작업 Worker에 전달한다. 리샘플링·PNG
  압축은 Worker에서 수행하고 자동 복구 Worker와 별개다. 프로젝트 교체 뒤 도착한
  출력은 다운로드하지 않는다. 원본 합성 단계는 기존 UI 스레드 경로다.

## CLI

```text
nyatidraw-cli export-png source.ntdr new-output.png 640 360
```

전체 레이어 합성과 현재 history cursor의 캔버스를 읽고 같은 축소 encoder를 쓴다.
CLI에서는 두 출력 크기를 명시하며 UI와 달리 비율 잠금을 강제하지 않는다.
너비·높이 각각 1 px 이상이고 원본 이하이어야 한다. 기존 파일·companion은 거절한다.
기존 단일 레이어 PPM 진단용 `export` 명령은 그대로다.

## 검증 구분

핵심 검사는 투명 빨강 경계, linear-light 흑백 에너지, 비정수 축소율의 양 끝 픽셀,
2차원 체크무늬, 100% 기존 encoder 일치, 입력 픽셀 불변 및 잘못된 크기의 기존
파일 보존을 대상으로 한다. 이 문서는 테스트 실행 성공을 선행 주장하지 않는다.
통합 담당자가 수행하는 Cargo/Clippy, Windows/web 빌드, 실제 메뉴에서 50%와
가로/세로 지정, 저장→프로세스 종료→재열기 후 원본 PNG/Undo 일치 검증 결과는
별도 기록해야 한다. 물리 펜·고주사율·설치판 검증은 이 출력 기능의 자동 검사로
대체하지 않는다.

새 외부 라이브러리는 추가하지 않았다. CLI와 web-worker에 이미 workspace에서
사용 중인 `paint-cpu`/`png-io`/`tiles` 경로 의존성만 연결한다.

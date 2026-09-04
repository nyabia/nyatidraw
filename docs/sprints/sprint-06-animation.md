# Sprint 6 — 애니메이션과 게임 산출물

## 사용자 결과

NyatiDraw 안에서 짧은 2D cel 애니메이션을 그리고 Godot에서 바로 사용할 frame
sequence 또는 sprite sheet로 내보낸다.

## 작업

- `ContentRootId`를 공유하고 수정 시 copy-on-write하는 CelTrack.
- rational frame rate, timeline, cel expose/duplicate와 playback.
- onion skin과 frame/layer 선택 의미.
- PNG sequence, sprite sheet와 metadata export profile.
- timelapse history playback을 작품 animation timeline과 분리.

## 게이트

- cel 추가·복제·수정 뒤 save/restart/reopen해 frame와 content root가 같다.
- playback 순서와 PNG sequence/sprite sheet의 frame hash가 일치한다.
- Godot에서 산출물을 reimport해 frame 경계와 순서를 확인한다.

## 제외

영상 편집, 오디오 timeline, rigging, 협업.

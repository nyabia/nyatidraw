# 이번 UX 피드백 구현 체크리스트

[구현 묶음과 검증 기록](ux-feedback.md) · [브러시 조사](../research/brush-engines.md) ·
[자체 연필 설계](../research/pencil-brush-design.md) ·
[반복 구현 경계](../research/repeat-implementation.md) · [일러스트 작업 기준](../illustration-workflow.md)

이번 UX 피드백과 뒤이어 추가된 최근색·불투명도·선택 액션·스포이트·최소 자체 연필만 대조한다.
프로젝트 전체의 개발 이력을 나열한 문서가 아니다. **현재 소스, 빌드된 실행 파일, 실제 실행한 창을 구별한다.**
근거는 저장소 상대 경로와 함수·타입·CSS 선택자다.

후속 지시로 목록에서 구현 가능한 항목은 모두 진행하며 최소 자체 연필 엔진 개발도 허용됐다.
최초의 브러시 조사 한정을 현재 구현 금지로 재사용하지 않는다. 난이도가 큰 항목은 필요한 범위와
제약을 기록하고, 사용자 동의 없이 승인된 보류나 완료로 바꾸지 않는다.

- **구현됨:** 요청 동작의 구현이 존재한다. 검증 완료를 뜻하지 않으며 근거·검증 열을 함께 읽는다.
- **부분:** 일부 동작만 구현됐거나 알려진 연결 결함·범위 누락이 남아 있다.
- **미구현:** 기능이 아직 없다. 설계·비활성 버튼·조사 결과를 구현으로 세지 않는다.

**최신 Clippy·통합 시험과 DX 릴리즈 빌드가 통과했다. Windows / RTX 3080 / Dx12 / release / scale 1의
scratch 창에서 아래 조작과 저장·프로세스 종료·재열기를 확인했다.**
다만 picker 버블의 hold/drag 중간 표시·추종 감각, Alt/취소·빠른 입력 조합, 다른 DPI·물리 펜까지
검증한 것은 아니다. 기존 설치판과 이번 별도 릴리즈 실행본은 구별한다.

## 1. 브러시·도구 속성

| 요청 | 구현 상태 | 현재 근거 | 검증·남은 범위 |
|---|---|---|---|
| 속성 상단에 실제 엔진의 획 미리보기 | 구현됨 | `apps/desktop/src/main.rs:ToolPropertiesPanel`, `brush_preview.rs:BrushStrokePreview·render`, `native_canvas.rs:preset_for_ui_preview` | 160×48 scratch 예시가 현재 template의 실제 preset을 쓴다. 새 릴리즈 창에서 2H/2B preview와 서로 다른 실제 획을 확인했다. 흑백·합성 필압·큰 크기 축소는 1:1 굵기·물리 펜 감각 증거가 아니다. |
| 경도 설명·주요 옵션 단순화 | 구현됨 | `apps/desktop/src/main.rs:BrushHardnessControl·ToolPropertiesPanel` | 일반 브러시는 가장자리 경도와 0% 부드러움/100% 단단함 툴팁, 추가 옵션을 유지한다. Pencil에서는 round 경도를 숨겨 H/B와 구별한다. 새 릴리즈 창에서 Pencil 경도 숨김을 확인했다. |
| 현재 가족 내부의 세부 도구 | 구현됨 | `apps/desktop/src/main.rs:BrushPanel`, `crates/api/src/protocol.rs:PencilTemplate`, `apps/desktop/src/native_canvas.rs:DrawingConfig::select_tool·remember_current` | 연필은 2H/2B, 다른 가족은 해당 기본형을 표시한다. 2H·2B·펜·브러시·지우개 다섯 세션 슬롯이 마지막 크기·opacity·필압·보정을 보존한다. 새 릴리즈 창에서 2H/2B 선택·표현 전환을 확인했다. 다섯 슬롯의 모든 설정 조합 검수는 남으며 재시작용 프리셋 설정 저장·범용 라이브러리는 아니다. |
| 스포이트 단축키 C | 구현됨 | `apps/desktop/src/main.rs:handle_editor_shortcut·ToolsPanel`, `desktop_canvas.rs:WindowsViewportInput::observe` | UI·native C와 Ctrl+C 복사 유지. 새 릴리즈 창에서 C → 캔버스 drag → End 후 빨강 전경색 확정, 불필요한 획 없음 확인. 버블 중간 표시 검수와는 별개다. |
| 검정 연필로 시작 | 구현됨 | `crates/api/src/protocol.rs:UiProjection::empty·BrushSettings::for_pencil_template`, `apps/desktop/src/native_canvas.rs:DrawingConfig::from_projection·preset` | 새 릴리즈 창에서 검정 2H 샤프·5px·opacity 100% 시작을 확인했다. 도구 전환은 전경색을 초기화하지 않는다. |
| 크기 빠른 선택 패널 분리 | 구현됨 | `apps/desktop/src/main.rs:BrushSizesPanel·ToolPropertiesPanel`, `crates/editor/src/projection.rs:stage_used_brush_size` | 크기 목록과 속성을 분리한다. 최근 크기 네 개는 선택 시점이 아닌 accepted Begin에 기록한다. 새 연필과의 통합 확인은 남는다. |
| Procreate 등을 포함한 엔진 조사·자체 설계 방향 정정 | 구현됨 | `docs/research/brush-engines.md:목적과 결론·제품별로 확인한 표현 모델` | 조사 산출물 작성. Procreate/CSP/Krita/MyPaint/Fresco의 표현 원리를 자체 설계에 참고하며 외부 엔진 채택·코드 복사·C FFI가 목표가 아니다. 이 문서 작성 중 웹 재조사는 하지 않았다. |
| 최소 자체 건식 연필 엔진 | 구현됨 | `crates/brush/src/pencil.rs:pencil_preset·dab_for·pencil_coverage_units`, `docs/research/pencil-brush-design.md` | upright dry-pencil v3의 문서 좌표 종이 결·필압별 침착량을 구현했다. 개별 core/GPU·별도 프로세스 재열기와 통합 Clippy·시험은 통과했다. 새 Dx12 릴리즈 창의 두 연필 획과 저장·재시작 후 픽셀/root/PNG 보존도 확인했다. 물리 펜은 미검수다. |
| 필수 2H 샤프·2B 연필 템플릿 | 구현됨 | `crates/api/src/protocol.rs:PencilTemplate·BrushSettings::for_pencil_template`, `crates/brush/src/pencil.rs:PencilKind·pencil_preset`, `apps/desktop/src/main.rs:BrushPanel` | 고유 preset identity, 서로 다른 size 최소비율·flow·grain deposit 모델과 UI를 연결했다. v1/v2는 기존 분기를 유지한다. template 전환 intent 보완도 반영됐다. 새 창에서 두 template·서로 다른 획·preview와 재열기 픽셀 보존을 확인했다. 물리 펜과 모든 변형 전환 조합은 미검수다. |
| 변형 중 새 template 명령으로 직접 전환 | 구현됨 | `apps/desktop/src/transform_runtime.rs:selected_tool·switch_transform_tool·finish_transform_tool`, `native_canvas.rs:StrokePipeline::pending_pencil_template` | `SelectPencilTemplate`을 전환으로 인식하고 bounded pending 상태에 2H/2B identity를 보존해 draft 확정/취소 뒤 적용한다. 누락 수정은 소스에서 확인했으며 새 실제 창 조합 검수는 남는다. |
| 범용 질감·wet·smudge | 미구현 | `docs/research/brush-engines.md`, `docs/research/pencil-brush-design.md` | 두 건식 연필의 최소 모델을 습식 혼색·범용 재질 엔진 완료로 표시하지 않는다. tilt 물리 모델도 이번 최소 구현에 없다. |

## 2. 색상 패널·최근색

| 요청 | 구현 상태 | 현재 근거 | 검증·남은 범위 |
|---|---|---|---|
| SV 사각형이 색상환 안에 들어감 | 구현됨 | `apps/desktop/assets/colors.css:.compact-color-panel .sv-square` | 변 44%의 대각선 약 62.23%가 안쪽 원 지름 66%보다 작다. 기존 기본 크기 창 확인. 좁은 패널·다른 DPI 검수는 남는다. |
| wheel 아래 최근색·기존 도크 높이 유지 | 구현됨 | `apps/desktop/src/color_panel.rs:ColorPanel·PaletteRow`, `assets/colors.css:.compact-color-panel·.wheel-wrap` | 최근색 22px 한 줄을 예약하고 wheel을 가용 크기에 맞춘다. 기존 기본 크기 확인과 모든 크기 검증은 다르다. |
| HEX·기본 팔레트·중복 설명 제목 제거 | 구현됨 | `apps/desktop/src/color_picker.rs:ColorPicker`, `color_picker.js:render`, `color_panel.rs:ColorPanel` | 상시 HEX와 하드코딩된 기본색·제목 줄을 제거했다. |
| 상단 한 줄·고정 포함 최대 10개·사이드바 공유 | 구현됨 | `apps/desktop/src/color_panel.rs:PaletteRow·Palette::observe·Palette::order_recent_slots`, `assets/colors.css:.palette-row-top` | 같은 10칸, 고정 위치 유지·비고정색 최신순. 새 창에서 색상환 선택만으로 목록이 바뀌지 않고 실제 획 뒤 양쪽에 같은 최근색이 추가됨을 확인했다. |
| 고정 유지·가장 오래된 비고정색만 교체·모두 고정이면 유지 | 구현됨 | `apps/desktop/src/color_panel.rs:Palette::observe·toggle_pin`, `palette_preferences.rs:PalettePreferences::open·save·load·replace` | 작품 Undo와 별도 사용자 설정. 기존 pin 표시·설정 기록 확인. 재시작 pin 복원·10칸 포화·손상 설정의 전체 실사용 검수는 남는다. |
| 전경/배경 표시와 X 교환 | 구현됨 | `apps/desktop/src/color_panel.rs:QuickColors`, `main.rs:handle_editor_shortcut` | 최근색과 별도로 현재 전경색·배경색을 유지한다. |
| 최근색은 실제 그리기 시작에 기록 | 구현됨 | `crates/editor/src/projection.rs:stage_drawing_controls·stage_used_brush_color`, `apps/desktop/src/native_canvas.rs:StrokeDrain::record_brush_begin·StrokePipeline::drain` | accepted painting Begin의 원본 sRGB 색만 bounded 10칸으로 기록한다. 새 창에서 색상환 선택 시 불변 → 실제 획 뒤 양쪽 같은 최근색 추가를 확인했다. C/Alt picker·지우개·Undo/Redo 제외는 코드/core 경계이며 모든 조합의 창 검수까지 완료한 것은 아니다. |
| C/Alt press/drag 확대 픽셀·색상 버블과 End 확정 | 구현됨 | `apps/desktop/src/picker_preview.rs:PATCH_SIZE·PickerRuntime·PickerToken`, `native_canvas.rs:StrokePipeline::drain·ActiveCanvas::adopt_native_edit`, `main.rs:picker_loupe` | 13×13 patch·후보색과 End 확정, Cancel/token/viewport guard 구현·통합 시험 통과. 새 창의 C drag 뒤 빨강 확정·추가 획 없음은 확인했다. **hold/drag 중간 버블 표시는 직접 capture하지 못했고 추종 감각·Alt·취소·빠른 입력 조합은 미검수**다. |

## 3. 버거 메뉴

| 요청 | 구현 상태 | 현재 근거 | 검증·남은 범위 |
|---|---|---|---|
| 불투명 메뉴가 기존 상단 줄을 덮고 옆 UI를 밀지 않음 | 구현됨 | `apps/desktop/src/main.rs:ActionBar`, `assets/styles.css:.menu-line-overlay·.menu-dismiss-scrim`, `src/overlay_focus.js:focusInside·tabInside` | absolute overlay·닫기·Esc·포커스 경계. 기존 창 덮기/닫기 확인. 뒤쪽 native 입력 차단의 모든 조합 검수는 남는다. |

## 4. 프로젝트 X/Y 반복

**전체 미구현이다.** [UX-3](ux-feedback.md#ux-3-반복-반전과-다름)와
[독립 구현 설계](../research/repeat-implementation.md)는 구현 완료나 사용자 승인 보류가 아니다.
저장·Begin 고정 clip·GPU/CPU·wire·hit-test가 함께 필요해 독립 스프린트로 분할한 근거를 기록했다.

| 요청 | 구현 상태 | 현재 근거 | 남은 일 |
|---|---|---|---|
| X/Y 독립 프로젝트 속성·저장 | 미구현 | `crates/api/src/protocol.rs:ProjectCommand`, `crates/project/src/lib.rs:ProjectStore` | `CanvasSpec` 확장 대신 독립 metadata 레코드 제안. 명령·UI·저장·재열기 연결 필요. 현재 production 저장은 redb이며 SQLite는 비교 실험이다. |
| 반복 축의 offpage 원본을 삭제하지 않고 가림·해제 복원 | 미구현 | `crates/paint-gpu/src/compositor.rs:GpuCompositeScene::render_viewport`, `workspace_tiles.wgsl` | signed 원본 보관은 기존 기능이지만 반복 가림은 없다. sparse pass와 source page cache 갱신 필요. |
| 복사본 읽기 전용·꺼진 축 무한 작업공간 유지 | 미구현 | `apps/desktop/src/native_canvas.rs:StrokePipeline`, `crates/stroke/src/lib.rs:StrokeCommit·CpuReplayMaterializer` | 중심점이 아닌 dab footprint 전체 제한, Begin 고정 clip의 hash/wire/replay, selection·fill·transform 보존 경계 필요. |
| PNG는 원본 페이지 한 장 | 미구현 | `crates/paint-cpu/src/lib.rs:flatten_layer_tree_rgba8`, `apps/desktop/src/save_as.rs:copy_closed_project` | 기존 단일 PNG가 반복 모드 검증을 대신하지 않는다. 반복 설정·해제·재열기·Save As와 원본 PNG 대조 필요. |

## 5. 선택·변형·페이지·알림·레이어

| 요청 | 구현 상태 | 현재 근거 | 검증·남은 범위 |
|---|---|---|---|
| Esc 취소, 조작이 없으면 선택 해제 | 구현됨 | `apps/desktop/src/main.rs:app·handle_editor_shortcut`, `native_canvas.rs:ActiveCanvas::apply_editor_command`, `transform_runtime.rs:cancel_native_gesture·enqueue_free_transform` | 열린 UI → 진행 조작 → 선택. key repeat 연쇄 해제 차단. 기존 창 선택 해제 확인과 새 picker Cancel 검증은 구별한다. 초기 포커스·IME 조합은 남는다. |
| G 도구 이름을 변형으로 통일 | 구현됨 | `apps/desktop/src/main.rs:ToolsPanel·BrushPanel·CanvasActions` | 상단과 같은 자유 변형 경로. 현재 선택 영역 필요. |
| 메시 변형 예정 표시 | 구현됨 | `apps/desktop/src/main.rs:BrushPanel` | 비활성 준비 중 표시만 구현. **메시 변형 엔진은 미구현**이며 affine을 메시로 표시하지 않는다. |
| G 순환·변형에서 다른 도구로 전환 | 구현됨 | `apps/desktop/src/transform_runtime.rs:selected_tool·switch_transform_tool·flush_transform_preview·finish_transform_tool` | 미변경 draft 취소, 최신 유효 변경 확정 뒤 전환, Begin/preview 중 의도 보존. actor GPU·Save 후 별도 프로세스 root/PNG 검사를 다시 통과했다. 새 template identity 보완과 실제 창 검수는 구별한다. |
| X/Y·폭/높이 수치 나란히 배치 | 구현됨 | `apps/desktop/src/transform_panel.rs:TransformPanel`, `assets/styles.css:.transform-grid·.transform-actions` | 두 열·수치 우측 정렬. 기존 기본 높이에서 입력·세 버튼이 스크롤 없이 보임을 확인. 다른 폭 검수는 남는다. |
| 선택 픽셀 수 상시 표시 제거 | 구현됨 | `apps/desktop/src/main.rs:app` | 상시 선택 N px 출력만 제거하고 내부 선택 상태·처리 중 표시는 유지한다. |
| 선택 액션이 붙어 보이는 스타일 수정 | 구현됨 | `apps/desktop/src/main.rs:SelectionActions`, `assets/styles.css:.selection-actions·.selection-toolbar·.selection-morph-actions` | 전체 선택·반전·해제·삭제·그림에서 선택의 아이콘 5개, 반경, 확장/축소 두 버튼과 간격을 연결했다. 최신 DX에 스타일을 묶었고 scale 1 창에서 다섯 아이콘·확장/축소 간격을 확인했다. 모든 비활성 상태·다른 DPI 검수는 남는다. |
| 취소 버튼을 상단 페이지 옆·가능할 때 활성화 | 구현됨 | `apps/desktop/src/main.rs:CanvasActions`, `transform_runtime.rs:stage_cancel_availability`, `crates/api/src/protocol.rs:EditProjection::can_cancel` | 비우기·흰 배경·변형·페이지·취소 순서. Begin/preview 취소와 이미 수락한 commit 취소 불가를 구별한다. 새 picker와 빠른 조작 검수는 남는다. |
| 페이지 설정은 속성을 대체하지 않는 dialog | 구현됨 | `apps/desktop/src/main.rs:app·CanvasActions·ToolPropertiesPanel`, `page_panel.rs:PagePanel`, `overlay_focus.js:tabInside` | modal·backdrop·Tab 순환·Esc의 기존 창 확인. native 입력 차단·Resize/Crop 실패 조합은 별도. |
| 계속 남는 알림 닫기 | 구현됨 | `apps/desktop/src/main.rs:TransientNotice·app` | 닫기와 8초 수명, 고정 파일 열기 실패 접두어 제거. 알림을 숨겨도 PNG 재시도·종료 복구 상태는 유지한다. |
| 합성·불투명도 등을 레이어 행 밖으로 이동 | 구현됨 | `apps/desktop/src/main.rs:LayersPanel·LayerStyleControls·LayerRow` | 목록 위 현재 래스터/명시 그룹 설정을 두고 행에는 썸네일·이름·가시성·Solo·상태를 남긴다. 윗줄 mode·잠금·alpha lock·clip·참조, 아래 opacity 배치다. 새 창의 opacity 조절을 확인했으며 래스터·그룹 설정 전체 조합은 미검수다. |
| 레이어 opacity 드래그·수치·주변 아이콘 재배치 | 구현됨 | `apps/desktop/src/main.rs:LayerStyleControls·LayerOpacityControl`, `assets/styles.css:.layer-style-row·.layer-style-flags·.layer-opacity` | 새 창에서 100→48 드래그, Undo 한 번→100, Redo→48과 저장·프로세스 재시작 뒤 48% 보존을 확인했다. 드래그 중 수치만 바뀌고 놓을 때 적용하며 **그림의 실시간 opacity preview는 없다.** |
| Solo·참조 의미 구별 | 구현됨 | `apps/desktop/src/main.rs:LayerRow·LayersPanel`, `crates/api/src/protocol.rs:LayerCommand::ToggleSolo·SetReference` | 기존 Solo는 보기 격리, 참조는 선택·채우기 source membership이다. 배치 변경을 새 합성 기능으로 세지 않는다. |
| 비파괴 레이어 색상화 | 미구현 | `crates/document/src/layers.rs:LayerNode·GroupNode`, `apps/desktop/src/main.rs:LayersPanel` | 준비 중 표시만 있다. reference·색 태그·alpha lock 재칠과 다르다. effect metadata·CPU/GPU·wire/history·export 연결 경계는 반복 설계 문서에 별도로 기록했다. |

## 검증 상태와 실행 파일

이 문서 갱신에서는 Cargo·GUI·웹을 실행하지 않았다. 아래 이전 기록과 담당자의 개별 검사 보고를
구별한다. 아래 최신 결과는 통합 담당자의 빌드·scratch 실행·재열기 확인 보고다.

| 구분 | 확인된 범위 | 아직 증명하지 못한 범위 |
|---|---|---|
| 최신 소스 | accepted Begin 최근색, opacity release 적용·재배치, selection 아이콘 스타일, C/Alt 13×13 preview·End commit, 자체 v3·template UI 반영 | 소스 반영만으로 현재 창이나 새 앱 전체 동작을 증명하지 않음 |
| 최신 통합 컴파일·Clippy | workspace all-target/all-feature Clippy `-D warnings` 통과. 기존 vendored Wry 경고만 별도 출력 | DX 릴리즈 번들·실제 창 표시를 증명하지 않음 |
| 최신 통합 시험 | 총 118개 통과, desktop 50개 통과·4개 ignored. 마지막 brush/native 취소 수정 뒤 brush 3개+desktop 50개 재검사 통과 | ignored 항목과 실제 입력·UI 배치 조합은 기본 시험 통과에 포함하지 않음 |
| 최신 변형 actor GPU | Windows / RTX 3080 / Vulkan / debug에서 실제 GPU actor, Save 후 별도 프로세스 root·PNG 대조 재통과 | 합성 입력·actor 증거이며 물리 펜·새 창 UI 조작 증거가 아님 |
| 최신 DX 빌드 | `tools/build-dev.ps1` 54.32초 성공. `styles.css`·`colors.css` 번들 포함 | 단일 빌드 기록이며 성능 벤치마크·설치판 갱신이 아님 |
| 최신 릴리즈 창 | Windows / RTX 3080 / **Dx12** / release / scale 1. 검정 2H 5px·100%, 2H/2B 획·preview, Pencil 경도 숨김, accepted Begin 최근색, C drag의 End 색 확정·추가 획 없음, 선택 아이콘 간격, opacity 100→48·Undo→100·Redo→48 확인 | 버블 hold/drag 중간 표시를 직접 capture하지 못함. 추종 감각·Alt/Cancel·빠른 입력·다른 DPI·물리 펜은 미검수 |
| 최신 릴리즈 저장·재시작 | 저장 뒤 프로세스 종료·재기동, 두 연필 픽셀·레이어 opacity 48%·artwork root 보존. 재출력 PNG SHA-256도 동일 | 전체 문서·모든 도구 조합으로 일반화하지 않음. 아래 동일 PNG hash 참조 |
| 자체 연필 개별 core/GPU 검사 | 담당자 통과 보고: 새 2H/2B CPU/GPU readback의 엄격한 차이 0, 기존 round 허용차 4 유지, 새 프로세스 저장·재열기 core 시험 통과 | 상세 명령·대상·환경은 담당자가 후속 기록. 앱 UI·native 펜 검증으로 확대하지 않음. workspace 통합 검사는 위 별도 행 참조 |
| 기존 통합 빌드 | workspace check·Clippy, Windows DX 릴리즈·CSS 번들링 통과 기록 | 그 이후 추가된 최신 변경을 포함한 검사·실행 파일이라는 뜻이 아님 |
| 기존 핵심 검사 | API/editor 4개, desktop 기본 49개, 별도 변형 actor/GPU의 취소·전환·Undo·Save 후 새 프로세스 root/history/PNG 대조 기록 | 새 v3/template·picker·opacity UI까지 같은 시험으로 검증하지 않음 |
| 기존 릴리즈 창 | scratch에서 round 검정 연필·preview, 메뉴, page modal/Tab/Esc, 마우스 획·재열기, 변형 G, C, Esc 선택 해제, 색상/pin 양쪽 동기화 | 최신 실행본 증거와 별도 기록. pin 재시작·10칸 포화, 다른 DPI·폭, 모든 그룹/레이어·입력 조합은 남음 |
| 이전 실행 파일·설치판 | 기존 설치·배포는 이 작업으로 갱신하지 않음 | 예전에 켜 둔 창을 최신 소스 실행으로 간주하지 않음. commit/tag/push/release·설치판 갱신 없음 |

재시작 전후 동일 PNG SHA-256:
`31FCC28FFCCA8411DB36944E6D6EAA35C245DEBD1DBE7EB7949C19873B0AA552`.

남은 기능은 X/Y 반복 전체, 비파괴 레이어 색상화, 메시 변형 엔진, 범용 wet·smudge다.
반복·색상화를 사용자 승인 없이 완료 또는 동의된 보류로 바꾸지 않는다.
버블 중간 표시·추종, Alt/Cancel·빠른 입력, 모든 template/변형 전환, 물리 펜·고주사율·다른 DPI 검수는 남는다.
재기동 직후 S 단축키는 한 번 성공했지만 기존 `initial-focus-deferred` 로그가 남아 있어 최초 포커스의
모든 조합까지 해결됐다고 하지 않는다.
검수 창은 종료했고 기존 설치판은 변경하지 않았다.

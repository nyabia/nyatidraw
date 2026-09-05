use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{EditCommand, EditorCommand, UiProjection};

#[derive(Clone, Copy)]
pub(crate) struct PagePanelOpen(pub Signal<bool>);

#[component]
pub(crate) fn PagePanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let crop_ink = live_ink.clone();
    let PagePanelOpen(mut opened) = use_context();
    let mut width = use_signal(|| ui_projection.read().canvas.width_px.to_string());
    let mut height = use_signal(|| ui_projection.read().canvas.height_px.to_string());
    let mut error = use_signal(|| None::<String>);
    let current = ui_projection.read();
    let canvas = current.canvas;
    let busy = current.edit.busy;
    let can_crop = current.edit.has_selection && !busy;
    rsx! {
        section { class: "transform-properties", aria_label: "페이지 크기",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape { opened.set(false); }
            },
            h3 { "페이지" }
            p { "현재 {canvas.width_px} × {canvas.height_px} px" br {} "{canvas.pixels_per_inch} ppi" }
            label { "폭 (px)"
                input { r#type: "number", min: "1", step: "1", value: "{width}", aria_label: "페이지 폭", oninput: move |e| width.set(e.value()) }
            }
            label { "높이 (px)"
                input { r#type: "number", min: "1", step: "1", value: "{height}", aria_label: "페이지 높이", oninput: move |e| height.set(e.value()) }
            }
            p { class: "transform-help", "왼쪽 위를 기준으로 출력 크기를 바꿉니다. 그림 크기와 바깥쪽 그림은 유지됩니다." }
            button { disabled: busy, onclick: move |_| {
                let size = [width().parse::<u32>(), height().parse::<u32>()];
                let [Ok(w), Ok(h)] = size else {
                    error.set(Some("폭과 높이를 양의 정수로 입력하세요.".into())); return;
                };
                if w == 0 || h == 0 {
                    error.set(Some("폭과 높이는 1 이상이어야 합니다.".into())); return;
                }
                let command = EditCommand::ResizePage { size: [w, h] };
                let result = live_ink.push_editor_command(ui_projection.read().revision, EditorCommand::Edit(command));
                println!("desktop-page event=resize size={w}x{h} admission={result:?}");
                if result.is_ok() { opened.set(false); }
                else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
            }, "크기 적용" }
            p { class: "transform-help", "선택 영역의 사각 경계에 페이지를 맞춥니다. 모든 레이어가 함께 이동하며 바깥쪽 그림은 보존됩니다." }
            button { disabled: !can_crop, onclick: move |_| {
                let result = crop_ink.push_editor_command(ui_projection.read().revision, EditorCommand::Edit(EditCommand::CropPageToSelection));
                println!("desktop-page event=crop-selection admission={result:?}");
                if result.is_ok() { opened.set(false); }
                else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
            }, "선택에 맞추기" }
            p { class: "transform-help", "적용하면 선택은 해제됩니다. 실행 취소로 페이지와 그림을 함께 복원할 수 있습니다." }
            button { onclick: move |_| opened.set(false), "닫기" }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
        }
    }
}

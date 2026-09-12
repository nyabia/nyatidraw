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
    let busy = current.edit.busy;
    let can_crop = current.edit.has_selection && !busy;
    rsx! {
        div { class: "page-backdrop", onclick: move |_| opened.set(false),
        section { id: "page-settings-dialog", class: "page-dialog transform-properties", role: "dialog", aria_modal: "true", aria_label: "페이지 크기",
            onmounted: move |_| crate::contain_overlay_focus("page-settings-dialog"),
            onclick: move |event| event.stop_propagation(),
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape { event.prevent_default(); opened.set(false); }
            },
            h3 { "페이지 크기" }
            div { class: "transform-grid",
            label { "폭 (px)"
                input { r#type: "number", min: "1", step: "1", value: "{width}", aria_label: "페이지 폭",
                    oninput: move |e| width.set(e.value()) }
            }
            label { "높이 (px)"
                input { r#type: "number", min: "1", step: "1", value: "{height}", aria_label: "페이지 높이", oninput: move |e| height.set(e.value()) }
            }
            }
            p { class: "transform-help", "왼쪽 위를 기준으로 출력 크기를 바꿉니다. 그림 크기와 바깥쪽 그림은 유지됩니다." }
            div { class: "transform-actions",
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
            button { disabled: !can_crop, title: "선택 경계에 페이지를 맞춥니다. 바깥 그림도 보존하며 함께 이동합니다.", onclick: move |_| {
                let result = crop_ink.push_editor_command(ui_projection.read().revision, EditorCommand::Edit(EditCommand::CropPageToSelection));
                println!("desktop-page event=crop-selection admission={result:?}");
                if result.is_ok() { opened.set(false); }
                else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
            }, "선택에 맞추기" }
            button { onclick: move |_| opened.set(false), "닫기" }
            }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
        }
        }
    }
}

use crate::live_ink::LiveInkBridge;
use dioxus::prelude::*;
use nyatidraw_api::{EditCommand, EditorCommand, RasterTransform, UiProjection};

#[derive(Clone, Copy)]
pub(crate) struct TransformPanelOpen(pub Signal<bool>);

#[component]
pub(crate) fn TransformPanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let TransformPanelOpen(mut opened) = use_context();
    let mut x = use_signal(|| "0".to_owned());
    let mut y = use_signal(|| "0".to_owned());
    let mut width = use_signal(String::new);
    let mut height = use_signal(String::new);
    let mut turns = use_signal(|| 0_u8);
    let mut flip_x = use_signal(|| false);
    let mut flip_y = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let current = ui_projection.read();
    let target = if current.edit.has_selection {
        "선택 영역"
    } else {
        "현재 레이어 전체"
    };
    let layer = current.layers.iter().find(|layer| {
        Some(layer.id)
            == current
                .active_layer
                .map(nyatidraw_api::LayerTreeNodeId::Raster)
    });
    let layer_name = layer
        .map_or("레이어 없음", |layer| layer.name.as_str())
        .to_owned();
    let busy = current.edit.busy || current.active_layer.is_none();
    rsx! {
        section { class: "transform-properties", aria_label: "기본 변형",
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape { opened.set(false); }
            },
            h3 { "변형" }
            p { "{target}" br {} "{layer_name}" }
            label { "가로 이동 (px)"
                input { r#type: "number", step: "1", value: "{x}", aria_label: "가로 이동", oninput: move |e| x.set(e.value()) }
            }
            label { "세로 이동 (px)"
                input { r#type: "number", step: "1", value: "{y}", aria_label: "세로 이동", oninput: move |e| y.set(e.value()) }
            }
            label { "회전"
                select { value: "{turns}", aria_label: "변형 회전", onchange: move |e| {
                    if let Ok(value) = e.value().parse() { turns.set(value); }
                },
                    option { value: "0", "없음" }
                    option { value: "1", "오른쪽 90°" }
                    option { value: "2", "180°" }
                    option { value: "3", "왼쪽 90°" }
                }
            }
            label { class: "transform-check", input { r#type: "checkbox", checked: flip_x(), onchange: move |e| flip_x.set(e.checked()) } "좌우 반전" }
            label { class: "transform-check", input { r#type: "checkbox", checked: flip_y(), onchange: move |e| flip_y.set(e.checked()) } "상하 반전" }
            label { "결과 폭 (px)"
                input { r#type: "number", min: "1", step: "1", placeholder: "자동", value: "{width}", aria_label: "변형 결과 폭", oninput: move |e| width.set(e.value()) }
            }
            label { "결과 높이 (px)"
                input { r#type: "number", min: "1", step: "1", placeholder: "자동", value: "{height}", aria_label: "변형 결과 높이", oninput: move |e| height.set(e.value()) }
            }
            p { class: "transform-help", "반전 → 회전 → 크기 조절 순서. 크기는 둘 다 비우면 유지합니다. 이동은 왼쪽 위 기준입니다. 적용하면 선택은 해제됩니다." }
            button { disabled: busy, onclick: move |_| {
                let parsed = (|| {
                    let offset = [x().parse::<i32>().map_err(|_| "이동값은 정수로 입력하세요.")?,
                        y().parse::<i32>().map_err(|_| "이동값은 정수로 입력하세요.")?];
                    let size = if width().is_empty() && height().is_empty() { None } else {
                        let size = [width().parse::<u32>().map_err(|_| "폭과 높이를 모두 양의 정수로 입력하세요.")?,
                            height().parse::<u32>().map_err(|_| "폭과 높이를 모두 양의 정수로 입력하세요.")?];
                        if size.contains(&0) { return Err("폭과 높이는 1 이상이어야 합니다."); }
                        Some(size)
                    };
                    Ok(RasterTransform { offset, quarter_turns: turns(), flip_x: flip_x(), flip_y: flip_y(), size })
                })();
                let transform = match parsed {
                    Ok(value) => value,
                    Err(message) => { error.set(Some(message.into())); return; }
                };
                let revision = ui_projection.read().revision;
                let result = live_ink.push_editor_command(revision, EditorCommand::Edit(EditCommand::Transform(transform)));
                println!("desktop-transform event=apply transform={transform:?} admission={result:?}");
                if result.is_ok() { opened.set(false); }
                else { error.set(Some("현재 작업이 끝난 뒤 다시 적용하세요.".into())); }
            }, "적용" }
            button { onclick: move |_| opened.set(false), "취소" }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
        }
    }
}

use crate::{UiHost, contain_overlay_focus, send_editor_command};
use dioxus::prelude::*;
use nyatidraw_api::{
    ColorAdjustment, ColorAdjustmentCommand, EditCommand, EditorCommand, UiProjection,
};

fn send(host: &UiHost, command: ColorAdjustmentCommand, error: Signal<Option<String>>) {
    send_editor_command(
        host,
        EditorCommand::Edit(EditCommand::ColorAdjustment(command)),
        error,
    );
}

#[component]
fn CorrectionControl(
    label: &'static str,
    value: i16,
    minimum: i16,
    maximum: i16,
    on_change: EventHandler<i16>,
) -> Element {
    let mut draft = use_signal(|| value.to_string());
    use_effect(use_reactive((&value,), move |(value,)| {
        draft.set(value.to_string());
    }));
    rsx! {
        label { class: "color-adjustment-control",
            span { "{label}" }
            input { r#type: "range", min: minimum, max: maximum, step: 1, value: "{value}",
                aria_label: "{label} 조절", oninput: move |event| {
                    if let Ok(value) = event.value().parse::<i16>() {
                        on_change.call(value.clamp(minimum, maximum));
                    }
                }
            }
            input { class: "property-number", r#type: "number", min: minimum, max: maximum, step: 1,
                value: "{draft}", aria_label: label,
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| {
                    let selected = draft().parse::<i16>().map_or(value, |number| number.clamp(minimum, maximum));
                    draft.set(selected.to_string());
                    on_change.call(selected);
                }
            }
        }
    }
}

#[component]
pub(crate) fn ColorAdjustmentDialog(ui_projection: Signal<UiProjection>) -> Element {
    let host = use_context::<UiHost>();
    let mut values = use_signal(ColorAdjustment::default);
    let error = use_signal(|| None::<String>);
    let preview_host = host.clone();
    use_resource(move || {
        let adjustment = values();
        let host = preview_host.clone();
        async move {
            let mut frame =
                document::eval("await new Promise(requestAnimationFrame); dioxus.send(true);");
            if frame.recv::<bool>().await.is_ok() {
                send(&host, ColorAdjustmentCommand::Preview(adjustment), error);
            }
        }
    });
    let Some(projection) = ui_projection.read().edit.color_adjustment.clone() else {
        return rsx! {};
    };
    let busy = ui_projection.read().edit.busy;
    let matching = projection.adjustment == values();
    let cancel_host = host.clone();
    let keyboard_host = host.clone();
    let commit_host = host.clone();
    rsx! {
        div { class: "color-adjustment-scrim" }
        section {
            id: "color-adjustment-dialog", class: "color-adjustment-dialog", role: "dialog",
            aria_modal: "true", aria_label: "그림 색 보정", tabindex: -1,
            title: "HSV 명도(V) · HSV, 대비, 밝기 순서로 매번 원본에서 계산합니다.",
            onmounted: move |_| contain_overlay_focus("color-adjustment-dialog"),
            onkeydown: move |event| {
                event.stop_propagation();
                if event.key() == Key::Escape {
                    event.prevent_default();
                    send(&keyboard_host, ColorAdjustmentCommand::Cancel, error);
                }
            },
            h2 { "그림 색 보정" }
            p { "현재 레이어 · 선택 영역이 있으면 선택 안만 · 투명도 유지" }
            for (field, label, minimum, maximum, value) in [
                (0, "색상 (°)", -180, 180, values().hue_degrees),
                (1, "채도 (%)", -100, 100, values().saturation_percent),
                (2, "명도 (%)", -100, 100, values().value_percent),
                (3, "밝기 (%)", -100, 100, values().brightness_percent),
                (4, "대비 (%)", -100, 100, values().contrast_percent),
            ] {
                CorrectionControl { label, value, minimum, maximum,
                        on_change: move |value: i16| {
                            let mut next = values();
                            match field {
                                0 => next.hue_degrees = value,
                                1 => next.saturation_percent = value,
                                2 => next.value_percent = value,
                                3 => next.brightness_percent = value,
                                _ => next.contrast_percent = value,
                            }
                            values.set(next);
                        }
                }
            }
            if busy || !matching { p { role: "status", "미리보기 갱신 중…" } }
            if let Some(message) = error() { p { role: "alert", "{message}" } }
            div { class: "color-adjustment-actions",
                button { class: "command", onclick: move |_| {
                    values.set(ColorAdjustment::default());
                }, "초기화" }
                button { class: "command", onclick: move |_| send(&cancel_host, ColorAdjustmentCommand::Cancel, error), "취소" }
                button { class: "command active", disabled: busy || !matching || !projection.can_commit,
                    onclick: move |_| {
                        if projection.adjustment == values() {
                            send(&commit_host, ColorAdjustmentCommand::Commit { generation: projection.generation }, error);
                        }
                    },
                    "확정"
                }
            }
        }
    }
}

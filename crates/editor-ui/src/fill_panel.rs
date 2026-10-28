use super::{EditNumberInput, UiHost, UiIcon, send_editor_command};
use dioxus::prelude::*;
use nyatidraw_api::{
    EditCommand, EditSource, EditorCommand, LayerProjection, LayerTreeNodeId, ToolCommand,
    UiProjection,
};

#[component]
pub(super) fn FillSubtools(ui_projection: Signal<UiProjection>) -> Element {
    let host = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let selected = ui_projection.read().edit_settings.fill_source;
    rsx! {
        for (source, label, hint) in [
            (EditSource::AllVisible, "다른 레이어도 참조", "보이는 그림 전체를 경계로, 현재 레이어에 채웁니다. 러프는 숨겨주세요."),
            (EditSource::ActiveLayer, "현재 레이어만", "현재 레이어의 그림만 경계로 채웁니다."),
            (EditSource::ReferenceLayers, "참조 레이어만", "레이어 패널에서 참조 표시한 선화를 경계로 채웁니다."),
        ] {
            button {
                class: if source == selected { "subtool fill-subtool selected" } else { "subtool fill-subtool" },
                aria_pressed: source == selected, title: hint,
                onclick: { let host = host.clone(); move |_| {
                    send_editor_command(&host, EditorCommand::Tool(ToolCommand::SetFillSource(source)), error);
                } },
                span { class: "subtool-name", "{label}" }
            }
        }
        if let Some(message) = error.read().as_ref() {
            div { class: "command-error", role: "alert", "{message}" }
        }
    }
}

#[component]
pub(super) fn FillProperties(ui_projection: Signal<UiProjection>) -> Element {
    let host = use_context::<UiHost>();
    let tolerance_host = host.clone();
    let gap_host = host.clone();
    let expand_host = host.clone();
    let selection_host = host.clone();
    let clear_host = host.clone();
    let error = use_signal(|| Option::<String>::None);
    let projection = ui_projection.read();
    let settings = projection.edit_settings;
    let has_selection = projection.edit.has_selection;
    let target = projection.active_layer.and_then(|id| {
        projection
            .layers
            .iter()
            .find(|layer| layer.id == LayerTreeNodeId::Raster(id))
    });
    let hidden = target.is_some_and(|layer| {
        layer_path(&projection.layers, layer).any(|ancestor| !ancestor.visible)
    });
    let mask_selected = matches!(
        projection.raster_target,
        Some(nyatidraw_api::RasterEditTarget::Mask { .. })
    );
    let blocked = projection.edit.busy
        || projection.animation.playing
        || projection.edit.transform.is_some()
        || projection.edit.color_adjustment.is_some()
        || hidden
        || target.is_none_or(|layer| layer.locked);
    let alpha_locked = !mask_selected && target.is_some_and(|layer| layer.alpha_locked);
    let target_name = target
        .map_or("레이어를 선택하세요", |layer| layer.name.as_str())
        .to_owned();
    let [red, green, blue, _] = projection.brush_color;
    let color = nyatidraw_tiles::color::srgb8_to_linear_premultiplied(projection.brush_color);
    let notice = if target.is_some_and(|layer| layer.locked) {
        Some("칠할 레이어의 잠금을 해제하세요.")
    } else if hidden {
        Some("칠할 레이어나 상위 그룹의 표시를 켜주세요.")
    } else if target.is_some_and(|layer| {
        layer_path(&projection.layers, layer).any(|ancestor| ancestor.opacity_u16 == 0)
    }) {
        Some("레이어나 상위 그룹의 불투명도가 0%여서 칠한 색이 보이지 않습니다.")
    } else if !mask_selected
        && settings.fill_source == EditSource::ReferenceLayers
        && !projection.layers.iter().any(|layer| {
            layer.reference
                && layer_path(&projection.layers, layer).all(|ancestor| {
                    has_visible_alpha_source(&projection.layers, ancestor, projection.layers.len())
                })
        })
    {
        Some(
            "보이는 선화에 참조 표시를 켜주세요. 클리핑된 선화는 아래 기준 레이어도 표시되고 불투명도가 0%보다 높아야 합니다.",
        )
    } else if alpha_locked {
        Some("투명도 잠금 중입니다. 이미 칠해진 영역만 재칠하며 선 밑 확장은 적용하지 않습니다.")
    } else {
        None
    };
    rsx! {
        div { class: "fill-properties", onkeydown: move |event| { if event.key() != Key::Escape { event.stop_propagation(); } },
            if mask_selected { crate::layer_mask::MaskImageTargetSwitch { ui_projection, selection_fill: true } }
            div { class: "fill-target", title: "현재 색으로 칠할 레이어: {target_name}",
                span { class: "fill-color", style: "background:rgb({red},{green},{blue})" }
                span { class: "fill-target-name", "{target_name}" }
            }
            if let Some(notice) = notice {
                p { class: "fill-notice", role: "status", "{notice}" }
            }
            if has_selection {
                p { class: "fill-selection-note", "선택 영역 안에서만 채웁니다." }
            }
            if !mask_selected {
            FillSlider {
                label: "허용 오차", value: settings.fill_tolerance, max: 255, unit: "",
                hint: "클릭한 색과 얼마나 달라도 같은 영역으로 볼지 정합니다. 높이면 경계를 넘어갈 수 있습니다.",
                on_commit: move |value| send_editor_command(&tolerance_host, EditorCommand::Tool(ToolCommand::SetFillTolerance(value)), error)
            }
            FillSlider {
                label: "틈 닫기", value: settings.fill.gap_close_px, max: 8, unit: "px",
                hint: "0은 끄기. 반경을 높이면 작은 틈을 막지만 의도한 좁은 통로도 막을 수 있습니다. 원래 선은 바꾸지 않습니다.",
                on_commit: move |value| send_editor_command(&gap_host, EditorCommand::Tool(ToolCommand::SetFillGapClose(value)), error)
            }
            FillSlider {
                label: "선 밑으로 확장", value: settings.fill.expand_px, max: 64, unit: "px",
                disabled: alpha_locked,
                hint: "선 아래로 색을 받쳐 흰 틈을 줄입니다. 다른 연결 영역이나 선택 밖으로는 확장하지 않습니다.",
                on_commit: move |value| send_editor_command(&expand_host, EditorCommand::Tool(ToolCommand::SetFillExpansion(value)), error)
            }
            details { class: "brush-extra-options",
                summary { "추가 옵션" }
                label { class: "fill-edge-toggle", title: "확장이 0이어도 선 아래를 최소 1px 받칩니다. 가장자리를 부드럽게 만드는 안티앨리어싱은 아닙니다.",
                    input { r#type: "checkbox", checked: settings.fill.antialias, disabled: alpha_locked,
                        onchange: move |event| send_editor_command(&host, EditorCommand::Tool(ToolCommand::SetFillAntialias(event.checked())), error)
                    }
                    span { "최소 1px 빈틈 보정" }
                }
            }
            }
            div { class: "fill-selection-actions",
                button { disabled: blocked || !has_selection,
                    title: if mask_selected { "선택한 마스크에 현재 명도 채우기 · 검정은 숨김, 흰색은 표시 · 부드러운 선택 유지 · Undo로 복원" } else { "선 경계와 관계없이 선택 전체를 현재 색으로 채웁니다. 되돌리기 한 번으로 취소합니다." },
                    onclick: move |_| send_editor_command(&selection_host, EditorCommand::Edit(EditCommand::FillSelection { color }), error),
                    UiIcon { name: "bucket" }
                    span { "선택 전체 채우기" }
                }
                button { disabled: projection.edit.busy || !has_selection, aria_label: "선택 해제", title: "선택 해제 (Esc)",
                    onclick: move |_| send_editor_command(&clear_host, EditorCommand::Edit(EditCommand::ClearSelection), error),
                    UiIcon { name: "deselect" }
                }
            }
            if let Some(message) = error.read().as_ref() {
                div { class: "command-error", role: "alert", "{message}" }
            }
        }
    }
}

fn layer_path<'a>(
    layers: &'a [LayerProjection],
    layer: &'a LayerProjection,
) -> impl Iterator<Item = &'a LayerProjection> {
    std::iter::successors(Some(layer), |child| {
        layers
            .iter()
            .find(|candidate| candidate.id == LayerTreeNodeId::Group(child.parent))
    })
    .take(layers.len())
}

fn has_visible_alpha_source(
    layers: &[LayerProjection],
    layer: &LayerProjection,
    remaining: usize,
) -> bool {
    if remaining == 0 || !layer.visible || layer.opacity_u16 == 0 {
        return false;
    }
    if layer.clip_to_below {
        // Bind before filtering visibility, matching the original sibling stack.
        let base = layers
            .iter()
            .filter(|candidate| {
                candidate.parent == layer.parent
                    && candidate.index < layer.index
                    && !candidate.clip_to_below
            })
            .max_by_key(|candidate| candidate.index);
        if base.is_none_or(|base| !has_visible_alpha_source(layers, base, remaining - 1)) {
            return false;
        }
    }
    match layer.id {
        LayerTreeNodeId::Raster(_) => true,
        LayerTreeNodeId::Group(group) => layers.iter().any(|child| {
            child.parent == group
                && !child.clip_to_below
                && has_visible_alpha_source(layers, child, remaining - 1)
        }),
    }
}

#[component]
fn FillSlider(
    label: &'static str,
    value: u8,
    max: u8,
    unit: &'static str,
    hint: &'static str,
    #[props(default)] disabled: bool,
    on_commit: EventHandler<u8>,
) -> Element {
    rsx! {
        div { class: "property-row fill-control", title: hint,
            span { "{label}" }
            EditNumberInput { value, min: 0, max, label, on_commit, disabled }
            span { "{unit}" }
            input { class: "property-range", r#type: "range", min: "0", max: "{max}", step: "1", value: "{value}", aria_label: "{label} 조절", disabled,
                oninput: move |event| {
                    if let Ok(value) = event.value().parse::<u8>() {
                        on_commit.call(value.min(max));
                    }
                }
            }
        }
    }
}

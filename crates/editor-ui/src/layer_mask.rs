use dioxus::prelude::*;
use nyatidraw_api::{
    EditorCommand, LayerProjection, LayerTreeNodeId, MaskCommand, RasterEditTarget, UiProjection,
};

use crate::preview::MaskThumbnailFrame;
use crate::{UiHost, UiIcon};
use std::sync::Arc;

#[component]
pub fn MaskThumbnail(
    layer: LayerProjection,
    thumbnail: Option<Arc<MaskThumbnailFrame>>,
    ui_projection: Signal<UiProjection>,
    error: Signal<Option<String>>,
    onselected: EventHandler<()>,
) -> Element {
    let host = use_context::<UiHost>();
    let (LayerTreeNodeId::Raster(id), Some(mask)) = (layer.id, layer.mask) else {
        return rsx! {};
    };
    let state = ui_projection.read();
    let disabled = state.edit.busy
        || state.animation.playing
        || layer.locked
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some()
        || !host.supports_command(&EditorCommand::Mask(MaskCommand::SelectMask(id)));
    let selected = matches!(state.raster_target,
        Some(RasterEditTarget::Mask { layer, mask: selected_mask }) if layer == id && selected_mask == mask.id);
    let thumbnail = thumbnail.filter(|frame| frame.mask == mask.id);
    let ready = thumbnail.is_some();
    let title = if mask.enabled {
        "마스크 편집 · 흰색은 표시, 검정은 숨김 · 페이지 내 미리보기 · 이미지와 함께 변형"
    } else {
        "사용 중지된 마스크 · 저장된 흑백 범위 · 마스크 작업에서 다시 사용"
    };
    rsx! {
        button { class: "mask-thumb", aria_label: "{layer.name} 마스크 편집", aria_pressed: selected,
            "data-enabled": mask.enabled, "data-ready": ready, disabled, title,
            onkeydown: move |event| {
                if event.key() == Key::Enter || event.key() == Key::Character(" ".to_owned()) {
                    event.stop_propagation();
                }
            },
            onclick: move |event| {
                event.stop_propagation();
                crate::focus_editor_shortcuts();
                onselected.call(());
                crate::send_editor_command(&host, EditorCommand::Mask(MaskCommand::SelectMask(id)), error);
            },
            span { class: "mask-thumb-preview",
                if let Some(frame) = thumbnail {
                    img { class: "mask-thumb-image", src: "{frame.data_uri}", draggable: "false",
                        width: "{frame.width}", height: "{frame.height}", alt: "{layer.name} 마스크 미리보기" }
                }
            }
            span { class: "mask-thumb-label", aria_hidden: "true", "마스크" }
        }
    }
}

#[component]
pub fn LayerMaskControls(
    layer: LayerProjection,
    ui_projection: Signal<UiProjection>,
    error: Signal<Option<String>>,
) -> Element {
    let mut opened = use_signal(|| false);
    let LayerTreeNodeId::Raster(id) = layer.id else {
        return rsx! {};
    };
    let state = ui_projection.read();
    let busy = state.edit.busy
        || state.animation.playing
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some();
    let has_selection = state.edit.has_selection;
    let is_active = state.active_layer == Some(id);
    let disabled = busy || layer.locked;
    let mask = layer.mask;
    let toggle_label = if mask.is_some_and(|mask| mask.enabled) {
        "마스크 사용 중지"
    } else {
        "마스크 다시 사용"
    };
    let label = if mask.is_some() {
        "마스크 작업"
    } else {
        "마스크 추가"
    };
    rsx! {
        div { class: "layer-mask-controls",
            if mask.is_some() || is_active {
                button { class: "layer-mask-menu-button", aria_label: "{layer.name} {label}", title: label,
                    disabled: busy, aria_expanded: opened(),
                    onclick: move |event| { event.stop_propagation(); opened.set(true); },
                    if mask.is_some() { "⋯" } else { UiIcon { name: "plus" } span { "마스크" } }
                }
            }
        }
        if opened() {
            div { class: "layer-mask-backdrop", onclick: move |event| { event.stop_propagation(); opened.set(false); },
                div { id: "layer-mask-menu", class: "layer-mask-popover", role: "dialog", aria_modal: "true", aria_label: "{layer.name} 마스크 작업",
                    onmounted: move |_| crate::contain_overlay_focus("layer-mask-menu"),
                    onclick: move |event| event.stop_propagation(),
                    onkeydown: move |event| {
                        event.stop_propagation();
                        if event.key() == Key::Escape { event.prevent_default(); opened.set(false); }
                    },
                    if let Some(mask) = mask {
                        MaskAction { command: MaskCommand::SelectImage(id), label: "이미지 편집", disabled: busy, opened, error }
                        MaskAction { command: MaskCommand::SelectMask(id), label: "마스크 편집", disabled, opened, error }
                        MaskAction { command: MaskCommand::SetEnabled { layer: id, enabled: !mask.enabled }, label: toggle_label, disabled, opened, error }
                        MaskAction { command: MaskCommand::Remove(id), label: "마스크 삭제 · 원본 그림 유지", disabled, opened, error }
                    } else {
                        MaskAction { command: MaskCommand::AddRevealAll(id), label: "모두 보이는 마스크 추가", disabled, opened, error }
                        MaskAction { command: MaskCommand::AddFromSelection(id), label: "선택 영역으로 마스크 추가", disabled: disabled || !has_selection, opened, error }
                    }
                    button { class: "mask-menu-close", onclick: move |_| opened.set(false), "닫기 (Esc)" }
                }
            }
        }
    }
}

#[component]
fn MaskAction(
    command: MaskCommand,
    label: &'static str,
    disabled: bool,
    mut opened: Signal<bool>,
    mut error: Signal<Option<String>>,
) -> Element {
    let host = use_context::<UiHost>();
    let disabled = disabled || !host.supports_command(&EditorCommand::Mask(command));
    rsx! {
        button { disabled, onclick: move |_| {
            match host.push_ui_editor_command(EditorCommand::Mask(command)) {
                Ok(()) => { opened.set(false); error.set(None); crate::focus_editor_shortcuts(); }
                Err(message) => { opened.set(false); error.set(Some(message)); }
            }
        }, "{label}" }
    }
}
#[component]
pub(crate) fn MaskImageTargetSwitch(
    ui_projection: Signal<UiProjection>,
    #[props(default)] selection_fill: bool,
) -> Element {
    let host = use_context::<UiHost>();
    let error = use_signal(|| None::<String>);
    let state = ui_projection.read();
    let Some(RasterEditTarget::Mask { layer, .. }) = state.raster_target else {
        return rsx! {};
    };
    let busy = state.edit.busy
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some()
        || state.animation.playing;
    rsx! {
        div { class: "mask-target-guidance",
            p {
                if selection_fill {
                    "선택 전체를 현재 색의 명도로 마스크에 채웁니다. 클릭 채우기는 이미지 대상에서 사용하세요."
                } else {
                    "이 작업은 이미지 대상에서 사용할 수 있습니다."
                }
            }
            button { disabled: busy, onclick: move |_| {
                crate::send_editor_command(&host, EditorCommand::Mask(MaskCommand::SelectImage(layer)), error);
            }, "이미지 편집으로 돌아가기" }
            if let Some(message) = error.read().as_ref() { p { role: "alert", "{message}" } }
        }
    }
}

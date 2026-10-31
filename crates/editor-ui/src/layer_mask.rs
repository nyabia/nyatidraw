use dioxus::prelude::*;
use nyatidraw_api::{
    EditorCommand, LayerProjection, LayerTreeNodeId, MaskCommand, RasterEditTarget, UiProjection,
};

use crate::UiHost;
use crate::preview::MaskThumbnailFrame;
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
pub(super) fn LayerMaskActions(
    layer: LayerProjection,
    ui_projection: Signal<UiProjection>,
    opened: Signal<Option<u64>>,
    error: Signal<Option<String>>,
) -> Element {
    use crate::layer_context_menu::LayerMenuAction;
    let mut confirm_remove = use_signal(|| false);
    let LayerTreeNodeId::Raster(id) = layer.id else {
        return rsx! {};
    };
    let state = ui_projection.read();
    let busy = state.edit.busy
        || state.animation.playing
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some();
    let disabled = busy || layer.locked;
    let has_selection = state.edit.has_selection;
    rsx! {
        div { class: "layer-menu-section",
            onkeydown: move |event| {
                if event.key() == Key::Escape && confirm_remove() {
                    event.prevent_default(); event.stop_propagation(); confirm_remove.set(false);
                    crate::layer_context_menu::focus_menu();
                }
            },
            if confirm_remove() && layer.mask.is_some() {
                p { class: "layer-menu-confirm", "마스크를 삭제할까요? 원본 그림은 유지됩니다." }
                button { id: "mask-remove-cancel", onmounted: move |_| {
                    let _ = document::eval("document.getElementById('mask-remove-cancel')?.focus();");
                }, onclick: move |_| { confirm_remove.set(false); crate::layer_context_menu::focus_menu(); }, "취소" }
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::Remove(id)), label: "마스크 삭제", disabled, opened, error }
            } else if let Some(mask) = layer.mask {
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::SelectImage(id)), label: "이미지 편집", disabled: busy, opened, error }
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::SelectMask(id)), label: "마스크 편집", disabled, opened, error }
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::SetEnabled { layer: id, enabled: !mask.enabled }), label: "마스크 사용", checked: mask.enabled, disabled, opened, error }
                button { disabled, onclick: move |_| confirm_remove.set(true), "마스크 삭제…" }
            } else {
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::AddRevealAll(id)), label: "모두 보이는 마스크 추가", disabled, opened, error }
                LayerMenuAction { command: EditorCommand::Mask(MaskCommand::AddFromSelection(id)), label: "선택 영역으로 마스크 추가", disabled: disabled || !has_selection, opened, error }
            }
        }
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

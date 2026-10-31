use dioxus::prelude::*;
use nyatidraw_api::{
    EditorCommand, LayerCommand, LayerProjection, LayerTreeNodeId, RasterEditTarget, UiProjection,
};

use crate::{UiHost, layer_mask::LayerMaskActions};

pub(super) fn open_menu(
    host: &UiHost,
    mut opened: Signal<Option<u64>>,
    mut error: Signal<Option<String>>,
) {
    error.set(None);
    opened.set(Some(host.project_epoch()));
}

pub(super) fn node_key(node: LayerTreeNodeId) -> String {
    match node {
        LayerTreeNodeId::Raster(id) => format!("r-{}", id.0),
        LayerTreeNodeId::Group(id) => format!("g-{}", id.0),
    }
}

#[component]
pub(super) fn LayerOptionsButton(
    layer: LayerProjection,
    opened: Signal<Option<u64>>,
    error: Signal<Option<String>>,
) -> Element {
    let host = use_context::<UiHost>();
    let key = node_key(layer.id);
    rsx! {
        button { id: "layer-options-{key}", class: "layer-options-button",
            aria_label: "{layer.name} 레이어 옵션", title: "레이어 옵션 · 오른쪽 클릭 / Shift+F10",
            aria_haspopup: "dialog", aria_expanded: opened() == Some(host.project_epoch()),
            onclick: move |event| { event.stop_propagation(); open_menu(&host, opened, error); },
            "…"
        }
    }
}

#[component]
pub(super) fn LayerContextMenu(
    layer: LayerProjection,
    ui_projection: Signal<UiProjection>,
    mut opened: Signal<Option<u64>>,
    error: Signal<Option<String>>,
    onrename: EventHandler<()>,
) -> Element {
    let host = use_context::<UiHost>();
    let mut deleting = use_signal(|| false);
    let key = node_key(layer.id);
    let anchor = key.clone();
    let dismiss = use_callback(move |()| {
        opened.set(None);
        let _ = document::eval(&format!(
            "const menu = document.getElementById('layer-context-menu');
             const restore = () => document.getElementById('layer-options-{key}')?.focus({{preventScroll:true}});
             if (menu?.isConnected) {{
                 const observer = new MutationObserver(() => {{
                     if (menu.isConnected) return;
                     observer.disconnect(); restore();
                 }});
                 observer.observe(document.body, {{childList:true, subtree:true}});
             }} else {{ restore(); }}"
        ));
    });
    let Some(epoch) = opened() else {
        return rsx! {};
    };
    if epoch != host.project_epoch() {
        return rsx! {};
    }
    let state = ui_projection.read();
    let busy = state.edit.busy
        || state.animation.playing
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some();
    let target = match (layer.id, state.raster_target) {
        (LayerTreeNodeId::Group(_), _) => "그룹",
        (LayerTreeNodeId::Raster(id), Some(RasterEditTarget::Mask { layer, .. }))
            if id == layer =>
        {
            "마스크 편집 중"
        }
        (LayerTreeNodeId::Raster(id), Some(RasterEditTarget::Image(layer))) if id == layer => {
            "이미지 편집 중"
        }
        _ => "선택하지 않은 레이어",
    };
    rsx! {
        div { class: "layer-context-backdrop",
            onclick: move |event| { event.stop_propagation(); dismiss.call(()); },
            oncontextmenu: move |event| { event.prevent_default(); event.stop_propagation(); dismiss.call(()); },
            div { id: "layer-context-menu", class: "layer-context-menu", role: "dialog",
                aria_modal: "true", aria_label: "{layer.name} 레이어 옵션",
                onmounted: move |_| { position_menu(&anchor); crate::contain_overlay_focus("layer-context-menu"); },
                onclick: move |event| event.stop_propagation(),
                oncontextmenu: move |event| { event.prevent_default(); event.stop_propagation(); },
                onkeydown: move |event| {
                    event.stop_propagation();
                    if event.key() == Key::Escape {
                        event.prevent_default();
                        if deleting() { deleting.set(false); focus_menu(); } else { dismiss.call(()); }
                    }
                },
                header { strong { "{layer.name}" } small { "{target}" } }
                if deleting() {
                    p { class: "layer-menu-confirm", "이 레이어를 삭제할까요? 그룹은 안의 레이어도 삭제됩니다. 실행 취소로 복원할 수 있습니다." }
                    button { id: "layer-delete-cancel", onmounted: move |_| {
                        let _ = document::eval("document.getElementById('layer-delete-cancel')?.focus();");
                    }, onclick: move |_| { deleting.set(false); focus_menu(); }, "취소" }
                    LayerMenuAction { command: EditorCommand::Layer(LayerCommand::Delete(layer.id)), label: "레이어 삭제", disabled: busy || layer.locked, opened, error }
                } else {
                    LayerMenuProperties { layer: layer.clone(), solo: state.solo_node == Some(layer.id), busy, opened, error }
                    div { class: "layer-menu-section",
                        button { disabled: busy, onclick: move |_| {
                            if opened() == Some(host.project_epoch()) { opened.set(None); onrename.call(()); }
                        }, "이름 변경" }
                        LayerMenuAction { command: EditorCommand::Layer(match layer.id {
                            LayerTreeNodeId::Raster(id) => LayerCommand::DuplicateRaster(id),
                            LayerTreeNodeId::Group(id) => LayerCommand::DuplicateGroup(id),
                        }), label: "복제", disabled: busy, opened, error }
                    }
                    LayerMaskActions { layer: layer.clone(), ui_projection, opened, error }
                    button { class: "layer-menu-delete", disabled: busy || layer.locked,
                        onclick: move |_| deleting.set(true), "레이어 삭제…" }
                }
                if let Some(message) = error.read().as_ref() { p { class: "command-error", role: "alert", "{message}" } }
                button { class: "layer-menu-close", onclick: move |_| dismiss.call(()), "닫기 (Esc)" }
            }
        }
    }
}

#[component]
fn LayerMenuProperties(
    layer: LayerProjection,
    solo: bool,
    busy: bool,
    opened: Signal<Option<u64>>,
    error: Signal<Option<String>>,
) -> Element {
    let node = layer.id;
    rsx! {
        div { class: "layer-menu-section",
            LayerMenuAction { command: EditorCommand::Layer(LayerCommand::SetVisibility { node, visible: !layer.visible }), label: "표시", checked: layer.visible, disabled: busy, opened, error }
            LayerMenuAction { command: EditorCommand::Layer(LayerCommand::ToggleSolo(node)), label: "이 레이어만 보기", checked: solo, disabled: busy, opened, error }
            if let LayerTreeNodeId::Raster(id) = node {
                LayerMenuAction { command: EditorCommand::Layer(LayerCommand::SetLocked { layer: id, locked: !layer.locked }), label: "픽셀 잠금", checked: layer.locked, disabled: busy, opened, error }
                LayerMenuAction { command: EditorCommand::Layer(LayerCommand::SetAlphaLocked { layer: id, alpha_locked: !layer.alpha_locked }), label: "투명도 잠금", checked: layer.alpha_locked, disabled: busy, opened, error }
                LayerMenuAction { command: EditorCommand::Layer(LayerCommand::SetReference { layer: id, reference: !layer.reference }), label: "참조 레이어", checked: layer.reference, disabled: busy, opened, error }
            }
            LayerMenuAction { command: EditorCommand::Layer(LayerCommand::SetClipToBelow { node, clip_to_below: !layer.clip_to_below }), label: "아래 레이어에 클리핑", checked: layer.clip_to_below, disabled: busy, opened, error }
        }
    }
}

#[component]
pub(super) fn LayerMenuAction(
    command: EditorCommand,
    label: &'static str,
    disabled: bool,
    opened: Signal<Option<u64>>,
    error: Signal<Option<String>>,
    #[props(default)] checked: Option<bool>,
) -> Element {
    let host = use_context::<UiHost>();
    let disabled = disabled || !host.supports_command(&command);
    rsx! {
        button { disabled, title: label, aria_pressed: checked.map(|value| value.to_string()),
            onclick: move |_| dispatch(&host, command.clone(), opened, error),
            span { "{label}" }
            if let Some(value) = checked { span { class: "layer-menu-check", aria_hidden: "true", if value { "✓" } } }
        }
    }
}

fn dispatch(
    host: &UiHost,
    command: EditorCommand,
    mut opened: Signal<Option<u64>>,
    mut error: Signal<Option<String>>,
) {
    if opened() != Some(host.project_epoch()) {
        opened.set(None);
        return;
    }
    match host.push_ui_editor_command(command) {
        Ok(()) => {
            opened.set(None);
            error.set(None);
            crate::focus_editor_shortcuts();
        }
        Err(message) => error.set(Some(message)),
    }
}

pub(super) fn focus_menu() {
    let _ = document::eval(
        "requestAnimationFrame(() => document.querySelector('#layer-context-menu button:not(:disabled)')?.focus({preventScroll:true}));",
    );
}

fn position_menu(anchor: &str) {
    let _ = document::eval(&format!(
        r"
        const menu = document.getElementById('layer-context-menu');
        const anchor = document.getElementById('layer-options-{anchor}');
        if (menu && anchor) {{
            const place = () => {{
                const rect = anchor.getBoundingClientRect();
                const edge = 8;
                menu.style.setProperty('--layer-menu-top', edge + 'px');
                menu.style.left = Math.max(edge, Math.min(rect.right - menu.offsetWidth, innerWidth - menu.offsetWidth - edge)) + 'px';
                const top = Math.max(edge, Math.min(rect.top, innerHeight - menu.offsetHeight - edge));
                menu.style.top = top + 'px';
                menu.style.setProperty('--layer-menu-top', top + 'px');
            }};
            place();
            window.addEventListener('resize', place);
            const cleanup = new MutationObserver(() => {{
                if (menu.isConnected) return;
                window.removeEventListener('resize', place);
                cleanup.disconnect();
            }});
            cleanup.observe(document.body, {{childList: true, subtree: true}});
        }}
    "
    ));
}

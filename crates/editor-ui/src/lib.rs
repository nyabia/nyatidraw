mod animation_panel;
mod ui_icon;
pub use ui_icon::UiIcon;
mod host;
pub use host::{ExportStatus, PalettePins, UiBackend, UiHost, UiSlots};
mod brush_presets;
mod brush_preview;
mod color_adjustment_dialog;
mod color_panel;
mod color_picker;
mod dock_drag;
mod fill_panel;
mod layer_drag;
mod layer_mask;
pub mod layout_codec;
pub mod layout_store;
mod mask_preview;
mod page_panel;
pub mod picker_loupe;
pub mod preview;
pub mod reference_panel;
pub mod scaled_export;
mod transform_panel;
pub mod workspace_appearance;

use dioxus::prelude::*;
use nyatidraw_api::{
    CommandRejectReason, DockAxis, DockCommand, DockNode, DrawingTool, EditorCommand, EditorEvent,
    EventEnvelope, GroupId, HistoryCommand, HistoryOperationLabel, LayerColorEffect, LayerCommand,
    LayerId, LayerProjection, LayerProjectionKind, LayerTreeNodeId, PanelKind, ProjectCommand,
    ToolCommand, UiProjection, ViewportCommand, WorkspaceProjection,
};
use nyatidraw_input::{CanvasInputMode, PressurePreset};
use preview::LayerThumbnailFrame;
use std::{collections::BTreeSet, sync::Arc};

const STYLES: &str = include_str!("../assets/styles.css");
const COLOR_STYLES: &str = include_str!("../assets/colors.css");
const CAPABILITY_STYLES: &str = "[data-dock-rearrange='false'] [data-dock-source], [data-dock-resizable='false'] [data-dock-resize]:not([data-resize-axis='height']) { cursor: default; }";

#[derive(Clone, Copy, Debug)]
struct NavigatorDrag {
    pointer_id: i32,
    origin_x: f64,
    origin_y: f64,
    width: u16,
    height: u16,
    last_sent: Option<[i32; 2]>,
    last_sent_at: f64,
}

#[component]
pub fn EditorChrome(
    ui_projection: Signal<UiProjection>,
    export_status: ExportStatus,
    overlays: Element,
    #[props(default)] host_update: u64,
) -> Element {
    let _ = host_update;
    let live_ink = use_context::<UiHost>();
    color_panel::use_palette_preferences(ui_projection);
    reference_panel::provide_reference();
    dock_drag::use_dock_drag(live_ink.clone());
    use_context_provider(|| transform_panel::TransformPanelOpen(Signal::new(false)));
    let animation_panel::AnimationPanelVisible(animation_visible) =
        use_context_provider(|| animation_panel::AnimationPanelVisible(Signal::new(true)));
    let page_panel::PagePanelOpen(page_open) =
        use_context_provider(|| page_panel::PagePanelOpen(Signal::new(false)));
    let mut page_shortcuts = page_open;
    let mut navigator_drag = use_context_provider(|| Signal::new(Option::<NavigatorDrag>::None));
    let projected_revision = ui_projection.read().revision.0;
    let dock = ui_projection.read().dock.clone();
    let can_resize =
        live_ink.supports_command(&EditorCommand::Dock(DockCommand::Replace(dock.clone())));
    let can_rearrange = live_ink.supports_command(&EditorCommand::Dock(DockCommand::MovePanel {
        panel: PanelKind::Tools,
        target: PanelKind::Navigator,
        position: nyatidraw_api::DockPosition::Right,
    }));
    let dirty = ui_projection.read().dirty;
    let document_title = ui_projection.read().document_title.clone();
    let layout_notice = live_ink.layout_notice();
    let has_layout_notice = layout_notice.is_some();
    let has_animation = !ui_projection.read().animation.frames.is_empty() && animation_visible();
    let edit = ui_projection.read().edit.clone();
    let shortcut_ink = live_ink.clone();
    let navigator_move_ink = live_ink.clone();
    let navigator_up_ink = live_ink.clone();
    let latest_event = live_ink.protocol_snapshot().1;
    let rejection = latest_event.as_ref().and_then(|event| match &event.event {
        EditorEvent::CommandRejected { command, reason } => Some((*command, *reason)),
        _ => None,
    });
    rsx! {
        style { {STYLES} }
        style { {COLOR_STYLES} }
        style { {CAPABILITY_STYLES} }
        main {
            onmounted: move |_| focus_editor_shortcuts(),
            class: if has_layout_notice { "app-shell layout-has-notice" } else { "app-shell" },
            "data-animation": "{has_animation}",
            "data-dock-revision": "{projected_revision}", "data-dock-rearrange": "{can_rearrange}", "data-dock-resizable": "{can_resize}", tabindex: 0,
            onpointermove: move |event| continue_navigator_drag(navigator_drag, &navigator_move_ink, &event.data()),
            onpointerup: move |event| finish_navigator_drag(navigator_drag, &navigator_up_ink, &event.data()),
            onpointercancel: move |event| {
                if navigator_drag.read().as_ref().is_some_and(|drag| drag.pointer_id == event.data().pointer_id()) { navigator_drag.set(None); }
            },
            onkeydown: move |event| {
                let key = event.key().to_string();
                if ui_projection.read().edit.color_adjustment.is_some() {
                    event.prevent_default(); event.stop_propagation();
                    if key == "Escape" {
                        let _ = shortcut_ink.push_ui_editor_command(EditorCommand::Edit(nyatidraw_api::EditCommand::ColorAdjustment(nyatidraw_api::ColorAdjustmentCommand::Cancel)));
                    }
                    return;
                }
                if page_shortcuts() {
                    if key == "Escape" { page_shortcuts.set(false); }
                    event.prevent_default(); event.stop_propagation(); return;
                }
                if key == "Escape" && event.is_auto_repeating() { event.prevent_default(); return; }
                if handle_editor_shortcut(&shortcut_ink, &key, event.modifiers().shift(), event.modifiers().ctrl()) { event.prevent_default(); return; }
                let panel = match key.as_str() {
                    "1" | "F6" => Some(PanelKind::Canvas), "2" => Some(PanelKind::Tools),
                    "3" => Some(PanelKind::Brush), "4" => Some(PanelKind::Navigator),
                    "5" => Some(PanelKind::Color), "6" => Some(PanelKind::Layers),
                    "7" => Some(PanelKind::History),
                    "Escape" => { navigator_drag.set(None); None }, _ => None,
                };
                if let Some(panel) = panel { send_dock_command(&shortcut_ink, DockCommand::ActivatePanel(panel)); }
            },
            ActionBar { ui_projection, export_status }
            if let Some(notice) = layout_notice { div { class: "layout-notice", role: "status", "{notice}" } }
            section {
                class: "workspace", aria_label: "Editor workspace. F6 returns to the drawing canvas.",
                title: "{document_title} · r{projected_revision} · {protocol_status(latest_event.as_ref())}",
                DockNodeView { node: dock.root().clone(), ui_projection }
            }
            if has_animation { animation_panel::AnimationPanel { ui_projection, host_update } }
            if dirty { span { class: "unsaved-dot", title: "저장되지 않은 변경", "•" } }
            if edit.busy { aside { class: "edit-status", role: "status", aria_live: "polite", span { "작업 처리 중…" } } }
            if let Some(reason) = &edit.error { TransientNotice { key: "edit-{reason}", message: format!("편집 실패: {reason}") } }
            if let Some((command, reason)) = rejection { TransientNotice { key: "reject-{command.0}", message: reject_label(reason).to_owned() } }
            if page_open() { page_panel::PagePanel { ui_projection } }
            if edit.color_adjustment.is_some() { color_adjustment_dialog::ColorAdjustmentDialog { ui_projection } }
            {overlays}
        }
    }
}

#[component]
pub fn TransientNotice(message: String) -> Element {
    let mut visible = use_signal(|| true);
    use_future(move || async move {
        let mut timer =
            document::eval("await new Promise(r => setTimeout(r, 8000)); dioxus.send(true);");
        if timer.recv::<bool>().await.is_ok() {
            visible.set(false);
        }
    });
    if !visible() {
        return rsx! {};
    }
    rsx! { aside { class: "status-toast error transient-notice", role: "alert",
        span { "{message}" }
        button { title: "알림 닫기", aria_label: "알림 닫기", onclick: move |_| visible.set(false), "×" }
    } }
}

fn handle_editor_shortcut(live_ink: &UiHost, key: &str, shift: bool, ctrl: bool) -> bool {
    let command = if ctrl && key.eq_ignore_ascii_case("a") {
        Some(EditorCommand::Edit(nyatidraw_api::EditCommand::SelectAll))
    } else if ctrl && key.eq_ignore_ascii_case("d") {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::ClearSelection,
        ))
    } else if ctrl && shift && key.eq_ignore_ascii_case("i") {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::InvertSelection,
        ))
    } else if !ctrl && key == "Delete" {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::DeleteSelectedPixels,
        ))
    } else if ctrl && key.eq_ignore_ascii_case("c") {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::CopySelection,
        ))
    } else if ctrl && key.eq_ignore_ascii_case("x") {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::CutSelection,
        ))
    } else if ctrl && key.eq_ignore_ascii_case("v") {
        Some(EditorCommand::Edit(
            nyatidraw_api::EditCommand::PasteSelection,
        ))
    } else if !ctrl && key == "[" {
        Some(EditorCommand::Tool(ToolCommand::AdjustSizeSteps {
            tool: live_ink.protocol_snapshot().0.drawing_tool,
            steps: -1,
        }))
    } else if !ctrl && key == "]" {
        Some(EditorCommand::Tool(ToolCommand::AdjustSizeSteps {
            tool: live_ink.protocol_snapshot().0.drawing_tool,
            steps: 1,
        }))
    } else if key.eq_ignore_ascii_case("Escape") {
        Some(EditorCommand::Tool(ToolCommand::CancelGesture))
    } else if key.eq_ignore_ascii_case("b") {
        Some(EditorCommand::Tool(ToolCommand::CycleBrushFamily))
    } else if key.eq_ignore_ascii_case("g") {
        Some(EditorCommand::Tool(ToolCommand::CycleSelectionFamily))
    } else if key.eq_ignore_ascii_case("f") {
        Some(EditorCommand::Tool(ToolCommand::CycleFillFamily))
    } else if !ctrl && key.eq_ignore_ascii_case("c") {
        Some(EditorCommand::Tool(ToolCommand::Select(
            DrawingTool::Eyedropper,
        )))
    } else if key.eq_ignore_ascii_case("x") {
        Some(EditorCommand::Tool(ToolCommand::SwapColors))
    } else if key.eq_ignore_ascii_case("e") {
        Some(EditorCommand::Tool(ToolCommand::Select(
            DrawingTool::Eraser,
        )))
    } else if key.eq_ignore_ascii_case("s") {
        Some(EditorCommand::Project(ProjectCommand::Save))
    } else if key.eq_ignore_ascii_case("z") {
        Some(EditorCommand::History(if shift {
            HistoryCommand::Redo
        } else {
            HistoryCommand::Undo
        }))
    } else {
        None
    };
    let Some(command) = command else {
        return false;
    };
    let _ = live_ink.push_ui_editor_command(command);
    true
}

#[component]
fn ActionBar(ui_projection: Signal<UiProjection>, export_status: ExportStatus) -> Element {
    let animation_panel::AnimationPanelVisible(mut animation_visible) = use_context();
    let live_ink = use_context::<UiHost>();
    let reset_layout_ink = live_ink.clone();
    let mut menu_open = use_signal(|| false);
    let mut menu_settings = use_signal(|| false);
    let correction_ink = live_ink.clone();
    let correction_error = use_signal(|| None::<String>);
    let save_ink = live_ink.clone();
    let retry_ink = live_ink.clone();
    let undo_ink = live_ink.clone();
    let redo_ink = live_ink.clone();
    let mut error = use_signal(|| Option::<String>::None);
    let top = ui_projection.read().dock.top().to_vec();

    rsx! {
        nav { class: "commandbar", aria_label: "주요 명령",
            button { class: "command hamburger-button", title: "메뉴", aria_label: "메뉴", aria_expanded: "{menu_open}",
                onclick: move |_| { menu_settings.set(false); menu_open.toggle(); },
                span { class: "hamburger", i {}, i {}, i {} }
            }
            FileButtons {}
            if !ui_projection.read().animation.frames.is_empty() {
                button { class: "command compact", aria_label: "애니메이션 타임라인 표시", aria_expanded: animation_visible(),
                    aria_controls: "animation-timeline", title: if animation_visible() { "타임라인 닫기 · 프레임 유지" } else { "타임라인 열기" },
                    onclick: move |_| { focus_editor_shortcuts(); animation_visible.toggle(); },
                    UiIcon { name: "timeline" }
                }
            }
            if live_ink.is_saving_as() {
                span { class: "export-status", role: "status", "다른 이름으로 저장 중…" }
            }
            button { class: "command", title: "저장 (S)", onclick: move |_| {
                send_editor_command(&save_ink, EditorCommand::Project(ProjectCommand::Save), error);
            }, UiIcon { name: "save" } span { class: "command-label", "저장" } span { class: "shortcut", "S" } }
            if export_status != ExportStatus::Idle {
                if matches!(export_status, ExportStatus::Failed { .. }) {
                    button {
                        class: "command compact export-status-retry",
                        title: "PNG 내보내기 다시 시도",
                        aria_label: "PNG 내보내기 다시 시도",
                        onclick: move |_| {
                            send_editor_command(&retry_ink, EditorCommand::Project(ProjectCommand::Save), error);
                        },
                        "PNG 재시도"
                    }
                } else {
                    span { class: "export-status", role: "status", title: "PNG 내보내기 상태", "{export_status_label(export_status)}" }
                }
            }
            span { class: "divider" }
            button { class: "command compact", title: "실행 취소 (Z)", aria_label: "실행 취소", onclick: move |_| {
                send_editor_command(&undo_ink, EditorCommand::History(HistoryCommand::Undo), error);
            }, UiIcon { name: "undo" } span { class: "shortcut", "Z" } }
            button { class: "command compact fixed-end", title: "다시 실행 (Shift+Z)", aria_label: "다시 실행", onclick: move |_| {
                send_editor_command(&redo_ink, EditorCommand::History(HistoryCommand::Redo), error);
            }, UiIcon { name: "redo" } span { class: "shortcut", "Shift Z" } }

            for panel in top {
                TopToolbar { panel, ui_projection }
            }
            div { class: "toolbar-top-space", "data-dock-top": "" }
            span { class: "document-name", title: "현재 그림: {ui_projection.read().document_title}", "{ui_projection.read().document_title}" }
            if let Some(message) = error.read().as_ref() {
                span { class: "commandbar-error", role: "alert", "{message}" }
            }
            if menu_open() {
                div { class: "menu-dismiss-scrim", aria_hidden: "true", onclick: move |_| menu_open.set(false) }
                div { id: "app-menu-overlay", class: "menu-line-overlay", aria_label: "앱 메뉴",
                    onmounted: move |_| contain_overlay_focus("app-menu-overlay"),
                    onkeydown: move |event| {
                        event.stop_propagation();
                        if event.key() == Key::Escape { event.prevent_default(); menu_open.set(false); }
                    },
                    button { class: "command hamburger-button", autofocus: true, title: "메뉴 닫기", aria_label: "메뉴 닫기",
                        onclick: move |_| menu_open.set(false), "×"
                    }
                    if !menu_settings() {
                        FileButtons { extended: true }
                        span { class: "divider" }
                        ClipboardActions { ui_projection }
                        animation_panel::StartAnimation { ui_projection, onstarted: move |()| menu_open.set(false) }
                        button { class: "command", disabled: ui_projection.read().edit.busy || ui_projection.read().edit.transform.is_some() || ui_projection.read().edit.color_adjustment.is_some(),
                            onclick: move |_| {
                                menu_open.set(false);
                                send_editor_command(&correction_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::ColorAdjustment(nyatidraw_api::ColorAdjustmentCommand::Begin)), correction_error);
                            }, "그림 색 보정…"
                        }
                        span { class: "divider" }
                        button { class: "command", onclick: move |_| menu_settings.set(true), "환경설정" }
                        div { class: "menu-line-spacer" }
                        UpdateControl {}
                    } else {
                        button { class: "command", onclick: move |_| menu_settings.set(false), "← 작업 메뉴" }
                        div { class: "pressure-preset-menu", role: "group", aria_label: "입력 모드",
                            span { "입력" }
                            for (mode, label) in [(CanvasInputMode::Auto, "자동"), (CanvasInputMode::Pen, "펜"), (CanvasInputMode::Finger, "손가락")] {
                                button { class: "command", aria_pressed: live_ink.canvas_input_mode() == mode,
                                    title: match mode { CanvasInputMode::Auto => "펜 우선 · 마지막 펜 입력 700ms 뒤 새 터치 허용", CanvasInputMode::Pen => "캔버스에서 손가락 입력 끄기", CanvasInputMode::Finger => "손가락으로 현재 도구 사용 · 펜도 사용 가능" },
                                    onclick: {
                                        let ink = live_ink.clone();
                                        move |_| {
                                            if let Err(message) = ink.set_canvas_input_mode(mode) { error.set(Some(message)); }
                                        }
                                    }, "{label}"
                                }
                            }
                        }
                        div { class: "pressure-preset-menu", role: "group", aria_label: "펜 필압 감도",
                        span { "펜 필압" }
                        for (preset, label) in [(PressurePreset::Linear, "직선"), (PressurePreset::Soft, "부드럽게"), (PressurePreset::Softer, "더 부드럽게")] {
                            button { class: "command", aria_pressed: live_ink.pressure_preset() == preset,
                                onclick: {
                                    let ink = live_ink.clone();
                                    move |_| {
                                        if let Err(message) = ink.set_pressure_preset(preset) { error.set(Some(message)); }
                                    }
                                }, "{label}"
                            }
                        }
                        }
                        span { class: "divider" }
                        button { class: "command", onclick: move |_| {
                            send_dock_command(&reset_layout_ink, DockCommand::ResetToSafeDefault);
                            menu_open.set(false);
                        }, "기본 화면 배치" }
                    }
                }
            }
        }
    }
}

#[component]
fn ClipboardActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let copy_ink = live_ink.clone();
    let cut_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let projection = ui_projection.read();
    let busy = projection.edit.busy;
    let selected = projection.edit.has_selection && projection.edit.selected_pixels > 0;
    rsx! {
        button { class: "command", disabled: busy || !selected, title: "복사 (Ctrl+C)", onclick: move |_| {
            send_editor_command(&copy_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::CopySelection), error);
        }, UiIcon { name: "copy" } span { "복사" } span { class: "shortcut", "Ctrl C" } }
        button { class: "command", disabled: busy || !selected, title: "잘라내기 (Ctrl+X)", onclick: move |_| {
            send_editor_command(&cut_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::CutSelection), error);
        }, span { "잘라내기" } span { class: "shortcut", "Ctrl X" } }
        button { class: "command", disabled: busy || !live_ink.supports_command(&EditorCommand::Edit(nyatidraw_api::EditCommand::PasteSelection)), title: "새 레이어로 붙여넣기 (Ctrl+V)", onclick: move |_| {
            send_editor_command(&live_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::PasteSelection), error);
        }, span { "붙여넣기" } span { class: "shortcut", "Ctrl V" } }
    }
}

#[component]
fn FileButtons(#[props(default = false)] extended: bool) -> Element {
    let render = use_context::<UiSlots>().file_buttons;
    render(extended)
}

#[component]
fn UpdateControl() -> Element {
    let render = use_context::<UiSlots>().update_control;
    render()
}

#[component]
fn ToolbarContents(panel: PanelKind, ui_projection: Signal<UiProjection>) -> Element {
    match panel {
        PanelKind::CanvasActions => rsx! { CanvasActions { ui_projection } },
        PanelKind::Viewport => rsx! { ViewportActions { ui_projection } },
        PanelKind::QuickColors => rsx! { QuickColors { ui_projection } },
        _ => rsx! {},
    }
}

#[component]
fn CanvasActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let clear_ink = live_ink.clone();
    let transform_ink = live_ink.clone();
    let cancel_ink = live_ink.clone();
    let transform_panel::TransformPanelOpen(mut transform_open) = use_context();
    let page_panel::PagePanelOpen(mut page_open) = use_context();
    let error = use_signal(|| Option::<String>::None);
    let white_ink = live_ink;
    rsx! {
                button { class: "command", title: "현재 레이어 전체 비우기 · 캔버스 밖 포함 · 실행 취소 가능", onclick: move |_| {
                    send_editor_command(&clear_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::ClearActiveLayer), error);
                }, UiIcon { name: "clear-layer" } span { class: "command-label", "비우기" } }
                button { class: "command", title: "흰 배경 추가", disabled: !white_ink.supports_command(&EditorCommand::Layer(LayerCommand::AddWhiteBackground)), onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } span { class: "command-label", "흰 배경" } }
                button { class: if ui_projection.read().edit.transform.is_some() { "command active" } else { "command" }, title: "선택 영역 변형 · 먼저 영역을 선택하세요", disabled: !ui_projection.read().edit.has_selection || ui_projection.read().edit.busy || ui_projection.read().edit.transform.is_some() || matches!(ui_projection.read().raster_target, Some(nyatidraw_api::RasterEditTarget::Mask { .. })), onclick: move |_| {
                    page_open.set(false);
                    transform_open.set(true);
                    focus_editor_shortcuts();
                    send_editor_command(&transform_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::FreeTransform(nyatidraw_api::TransformCommand::Begin)), error);
                },
                    UiIcon { name: "transform" } span { class: "command-label", "변형" }
                }
                button { class: "command", title: "캔버스 크기·프리셋 및 선택 영역에 맞추기", disabled: ui_projection.read().edit.busy || ui_projection.read().edit.transform.is_some(), onclick: move |_| {
                    transform_open.set(false);
                    page_open.set(true);
                }, span { class: "command-label", "캔버스" } }
                button { class: "command", title: "현재 조작 취소 / 선택 해제 (Esc)", disabled: !page_open() && !ui_projection.read().edit.can_cancel && (!ui_projection.read().edit.has_selection || ui_projection.read().edit.busy),
                    onclick: move |_| {
                        if page_open() { page_open.set(false); }
                        else {
                            focus_editor_shortcuts();
                            send_editor_command(&cancel_ink, EditorCommand::Tool(ToolCommand::CancelGesture), error);
                        }
                    }, span { class: "command-label", "취소" } span { class: "shortcut", "Esc" }
                }
                if let Some(message) = error() {
                    TransientNotice { key: "canvas-action-{message}", message }
                }
    }
}

pub fn focus_editor_shortcuts() {
    // Starting/ending an operation can disable or remove its focused button.
    // Put focus on the keyboard scope before that change, not on document.body.
    let _ =
        document::eval("document.querySelector('main.app-shell')?.focus({ preventScroll: true });");
}

pub fn contain_overlay_focus(id: &'static str) {
    // IDs are internal constants; script owns listeners only for the mounted overlay.
    let _ = document::eval(&format!(
        "const overlay = document.getElementById('{id}');\n{}",
        include_str!("overlay_focus.js")
    ));
}

#[component]
fn ViewportActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let fit_ink = live_ink.clone();
    let zoom_out_ink = live_ink.clone();
    let zoom_in_ink = live_ink.clone();
    let rotate_ink = live_ink.clone();
    let reset_ink = live_ink.clone();
    let mirror_ink = live_ink;
    let viewport = ui_projection.read().viewport;
    let zoom = viewport.zoom_ppm / 10_000;
    let rotation = f64::from(
        (viewport.rotation_millidegrees.rem_euclid(360_000) + 180_000).rem_euclid(360_000)
            - 180_000,
    ) / 1_000.0;
    rsx! {
                button { class: "command compact", title: "화면 맞춤", onclick: move |_| {
                    send_editor_command(&fit_ink, EditorCommand::Viewport(ViewportCommand::FitDocument), error);
                }, UiIcon { name: "fit" } }
                button { class: "command compact", title: "축소", onclick: move |_| {
                    send_editor_command(&zoom_out_ink, EditorCommand::Viewport(ViewportCommand::ZoomSteps(-1)), error);
                }, UiIcon { name: "minus" } }
                span { class: "toolbar-value", "{zoom}%" }
                button { class: "command compact", title: "확대", onclick: move |_| {
                    send_editor_command(&zoom_in_ink, EditorCommand::Viewport(ViewportCommand::ZoomSteps(1)), error);
                }, UiIcon { name: "plus" } }
                label { class: "viewport-angle", title: "보기 회전 · 각도 입력 또는 Shift+Space+좌우 드래그",
                    input { r#type: "number", step: "0.1", value: "{rotation}", aria_label: "보기 회전 각도",
                        disabled: !rotate_ink.supports_command(&EditorCommand::Viewport(ViewportCommand::SetRotationMillidegrees(0))),
                        onkeydown: move |event| {
                            event.stop_propagation();
                            if event.key() == Key::Enter {
                                focus_editor_shortcuts();
                            }
                        },
                        onchange: move |event| {
                            if let Ok(degrees) = event.value().parse::<f64>() && degrees.is_finite() {
                                #[allow(clippy::cast_possible_truncation)]
                                let angle = (degrees.rem_euclid(360.0) * 1_000.0).round() as i32;
                                send_editor_command(&rotate_ink, EditorCommand::Viewport(ViewportCommand::SetRotationMillidegrees(angle)), error);
                            }
                        }
                    }
                    span { "°" }
                }
                button { class: "command compact", title: "회전 초기화 · 화면 중심과 확대 배율 유지", aria_label: "보기 회전 0도로 초기화", disabled: !reset_ink.supports_command(&EditorCommand::Viewport(ViewportCommand::ResetRotation)),
                    onclick: move |_| send_editor_command(&reset_ink, EditorCommand::Viewport(ViewportCommand::ResetRotation), error),
                    "0°"
                }
                button { class: "command compact", title: "보기 좌우 반전 · 그림과 PNG는 바뀌지 않습니다", aria_label: "보기 좌우 반전", aria_pressed: "{viewport.mirrored_horizontal}", disabled: !mirror_ink.supports_command(&EditorCommand::Viewport(ViewportCommand::ToggleMirrorHorizontal)),
                    onclick: move |_| send_editor_command(&mirror_ink, EditorCommand::Viewport(ViewportCommand::ToggleMirrorHorizontal), error),
                    UiIcon { name: "mirror-horizontal" }
                }
    }
}

#[component]
fn QuickColors(ui_projection: Signal<UiProjection>) -> Element {
    rsx! { color_panel::QuickColors { ui_projection } }
}

#[component]
fn TopToolbar(panel: PanelKind, ui_projection: Signal<UiProjection>) -> Element {
    rsx! {
        div { class: "toolbar-dock", aria_label: "{panel_label(panel)}", "data-dock-top": "{panel_slug(panel)}",
            span { class: "toolbar-grip", title: "{panel_label(panel)} 이동", "data-dock-source": "{panel_slug(panel)}" }
            ToolbarContents { panel, ui_projection }
        }
    }
}

// Side stacks use content-sized leading panels; only the trailing panel fills.
// Splits containing the canvas retain their geometry ratios.
fn dock_stack_height(node: &DockNode) -> Option<u16> {
    match node {
        DockNode::Panel(panel) => match panel {
            PanelKind::Canvas => None,
            PanelKind::Navigator => Some(200),
            PanelKind::CanvasActions | PanelKind::QuickColors => Some(110),
            PanelKind::Viewport => Some(265),
            PanelKind::Tools => Some(620),
            PanelKind::Brush => Some(130),
            PanelKind::ToolProperties => Some(310),
            PanelKind::BrushSizes | PanelKind::Color | PanelKind::Layers | PanelKind::History => {
                Some(300)
            }
        },
        DockNode::Tabs { panels, .. } => panels
            .iter()
            .map(|panel| dock_stack_height(&DockNode::Panel(*panel)))
            .collect::<Option<Vec<_>>>()
            .and_then(|heights| heights.into_iter().max())
            .map(|height| height + 24),
        DockNode::Split {
            axis,
            first,
            second,
            ..
        } => {
            let first = dock_stack_height(first)?;
            let second = dock_stack_height(second)?;
            Some(if *axis == DockAxis::Vertical {
                first.saturating_add(second).saturating_add(3)
            } else {
                first.max(second)
            })
        }
    }
}

#[component]
fn DockNodeView(node: DockNode, ui_projection: Signal<UiProjection>) -> Element {
    match node {
        DockNode::Panel(panel) => rsx! {
            DockPanel { panel, ui_projection }
        },
        DockNode::Tabs { active, panels } => {
            let active_panel = panels[active];
            rsx! {
                section { class: "dock-tabs", aria_label: "Docked panel tabs",
                    nav { class: "tab-strip", aria_label: "Panel tabs",
                        for (index, panel) in panels.iter().copied().enumerate() {
                            DockTab { panel, selected: index == active }
                        }
                    }
                    DockPanel { panel: active_panel, ui_projection }
                }
            }
        }
        DockNode::Split {
            axis,
            first_per_mille,
            first,
            second,
        } => {
            let class = match axis {
                DockAxis::Horizontal => "dock-split dock-horizontal",
                DockAxis::Vertical => "dock-split dock-vertical",
            };
            if axis == DockAxis::Vertical
                && dock_stack_height(&first).is_some()
                && dock_stack_height(&second).is_some()
            {
                let mut nodes = Vec::new();
                collect_side_stack(*first, &mut nodes);
                collect_side_stack(*second, &mut nodes);
                return rsx! { SideStack { nodes, ui_projection } };
            }
            let first_min = dock_drag::minimum_width(&first);
            let second_min = dock_drag::minimum_width(&second);
            let split_key = dock_drag::split_key(&first, &second);
            let first_style = format!("flex: {first_per_mille} 1 0; min-width: {first_min}px;");
            let second_style = format!(
                "flex: {} 1 0; min-width: {second_min}px;",
                1000_u16.saturating_sub(first_per_mille)
            );
            rsx! {
                section { class: "{class}",
                    div { class: "dock-child", style: "{first_style}",
                        DockNodeView { node: *first, ui_projection }
                    }
                    if axis == DockAxis::Horizontal {
                        div { class: "dock-resizer", role: "separator", tabindex: 0,
                            aria_label: "패널 너비 조절", aria_orientation: "vertical",
                            aria_valuenow: "{first_per_mille}", aria_valuemin: "20", aria_valuemax: "980",
                            "data-dock-resize": "{split_key}", "data-first-min": "{first_min}", "data-second-min": "{second_min}"
                        }
                    }
                    div { class: "dock-child", style: "{second_style}",
                        DockNodeView { node: *second, ui_projection }
                    }
                }
            }
        }
    }
}

fn collect_side_stack(node: DockNode, nodes: &mut Vec<DockNode>) {
    match node {
        DockNode::Split {
            axis: DockAxis::Vertical,
            first,
            second,
            ..
        } => {
            collect_side_stack(*first, nodes);
            collect_side_stack(*second, nodes);
        }
        node => nodes.push(node),
    }
}

#[component]
fn SideStack(nodes: Vec<DockNode>, ui_projection: Signal<UiProjection>) -> Element {
    let heights = use_context::<Signal<layout_store::PanelHeights>>();
    let len = nodes.len();
    let entries: Vec<_> = nodes
        .into_iter()
        .enumerate()
        .map(|(index, node)| {
            let panel = dock_drag::height_panel(&node);
            let height = heights
                .read()
                .get(panel)
                .unwrap_or_else(|| dock_stack_height(&node).unwrap_or(200));
            let is_last = index + 1 == len;
            let style = if index + 1 == len {
                "flex: 1 0 160px; min-height: 160px;".to_owned()
            } else {
                format!("flex: 0 0 {height}px; min-height: 64px;")
            };
            (node, style, panel_slug(panel), is_last, height)
        })
        .collect();
    rsx! {
        section { class: "dock-split dock-vertical side-stack",
            for (node, style, slug, is_last, height) in entries {
                div { class: "dock-child", style,
                    DockNodeView { node, ui_projection }
                }
                if !is_last {
                    div { class: "dock-resizer dock-height-resizer", role: "separator", tabindex: 0,
                        aria_label: "패널 높이 조절", aria_orientation: "horizontal",
                        aria_valuenow: "{height}", aria_valuemin: "64", aria_valuemax: "4096",
                        "data-dock-resize": "{slug}", "data-resize-axis": "height"
                    }
                }
            }
        }
    }
}

#[component]
fn DockTab(panel: PanelKind, selected: bool) -> Element {
    let live_ink = use_context::<UiHost>();
    rsx! {
        button {
            class: if selected { "dock-tab active" } else { "dock-tab" },
            aria_selected: if selected { "true" } else { "false" },
            onclick: move |_| send_dock_command(&live_ink, DockCommand::ActivatePanel(panel)),
            "data-dock-source": "{panel_slug(panel)}",
            "{panel_label(panel)}"
        }
    }
}

#[component]
fn DockPanel(panel: PanelKind, ui_projection: Signal<UiProjection>) -> Element {
    let workspace = ui_projection.read().workspace.clone();
    let panel_title = panel_label(panel);
    let panel_class = format!("dock-panel panel-{}", panel_slug(panel));

    rsx! {
        section { class: "{panel_class}", aria_label: "{panel_title} panel", "data-dock-panel": "{panel_slug(panel)}",
            if panel != PanelKind::Canvas {
                header {
                    class: "panel-header",
                    title: "{panel_title} 패널 이동",
                    "data-dock-source": "{panel_slug(panel)}",
                    span { class: "panel-grip", aria_hidden: "true" }
                    h2 { "{panel_title}" }
                }
            }
            div { class: "panel-content",
                PanelContents { panel, workspace, ui_projection }
            }
        }
    }
}

#[component]
fn PanelContents(
    panel: PanelKind,
    workspace: WorkspaceProjection,
    ui_projection: Signal<UiProjection>,
) -> Element {
    if panel == PanelKind::Canvas {
        return rsx! { CanvasWorkspace { workspace } };
    }
    match workspace {
        WorkspaceProjection::Empty => rsx! {
            div { class: "workspace-state empty",
                strong { "No project is open" }
                p { "Create or open a project to begin drawing." }
            }
        },
        WorkspaceProjection::Loading => rsx! {
            div { class: "workspace-state loading",
                strong { "Opening project…" }
                p { "Panels will receive semantic metadata when loading completes." }
            }
        },
        WorkspaceProjection::Error { .. } => rsx! {},
        WorkspaceProjection::Recovery { summary } => rsx! {
            div { class: "workspace-state recovery",
                strong { "Recovered a safe workspace" }
                p { "{summary}" }
            }
        },
        WorkspaceProjection::Ready => panel_ready_contents(panel, ui_projection),
    }
}

#[component]
fn CanvasWorkspace(workspace: WorkspaceProjection) -> Element {
    rsx! {
        div {
            class: "canvas-panel",
            tabindex: 0,
            role: "application",
            aria_label: "그리기 영역",
            div { class: "canvas-frame", SharedCanvas {} }
            if let WorkspaceProjection::Error { summary } = workspace {
                div { class: "workspace-error-notice", role: "alert",
                    strong { "작업을 계속할 수 없습니다" }
                    p { "저장된 작품은 보존됩니다. 창을 닫으면 저장 상태를 확인할 수 있습니다." }
                    p { "{summary}" }
                }
            }
        }
    }
}

fn panel_ready_contents(panel: PanelKind, ui_projection: Signal<UiProjection>) -> Element {
    match panel {
        PanelKind::Canvas => rsx! {},
        PanelKind::Tools => rsx! { ToolsPanel { ui_projection } },
        PanelKind::Navigator => rsx! { NavigatorPanel { ui_projection } },
        PanelKind::Layers => rsx! { LayersPanel { ui_projection } },
        PanelKind::Brush => rsx! { BrushPanel { ui_projection } },
        PanelKind::ToolProperties => rsx! { ToolPropertiesPanel { ui_projection } },
        PanelKind::BrushSizes => rsx! { BrushSizesPanel { ui_projection } },
        PanelKind::Color => rsx! { ColorPanel { ui_projection } },
        PanelKind::History => rsx! { HistoryPanel { ui_projection } },
        PanelKind::CanvasActions | PanelKind::Viewport | PanelKind::QuickColors => rsx! {
            div { class: "side-toolbar", ToolbarContents { panel, ui_projection } }
        },
    }
}

#[component]
fn HistoryPanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let history = ui_projection.read().history.clone();
    rsx! {
        div { class: "history-panel", aria_label: "현재 브랜치 히스토리",
            if !history.redo_branches.is_empty() || history.redo_page_after.is_some() {
                div { class: "history-branches", aria_label: "다시 실행할 분기 선택",
                    strong { "다시 실행할 분기" }
                    for branch in &history.redo_branches {
                        button {
                            class: "history-branch",
                            key: "{branch.node.0}",
                            onclick: {
                                let live_ink = live_ink.clone();
                                let node = branch.node;
                                move |_| send_editor_command(&live_ink, EditorCommand::History(HistoryCommand::RedoTo(node)), error)
                            },
                            "{history_operation_label(branch.operation)} · 분기 {branch.node.0}"
                        }
                    }
                    div { class: "history-branch-pages",
                        if history.redo_page_after.is_some() {
                            button {
                                onclick: {
                                    let live_ink = live_ink.clone();
                                    move |_| send_editor_command(&live_ink, EditorCommand::History(HistoryCommand::ShowRedoBranches { after: None }), error)
                                },
                                "처음"
                            }
                        }
                        if history.has_more_redo_branches {
                            button {
                                onclick: {
                                    let live_ink = live_ink.clone();
                                    let after = history.redo_branches.last().map(|branch| branch.node);
                                    move |_| send_editor_command(&live_ink, EditorCommand::History(HistoryCommand::ShowRedoBranches { after }), error)
                                },
                                "다음"
                            }
                        }
                    }
                }
            }
            if let Some(message) = error() {
                div { class: "command-error", role: "alert", "{message}" }
            }
            if history.has_older_entries {
                div { class: "history-truncated", "이전 작업은 생략됨" }
            }
            for (index, entry) in history.entries.into_iter().enumerate() {
                div {
                    class: if index == history.current { "history-row active" } else { "history-row" },
                    aria_current: if index == history.current { "true" } else { "false" },
                    span { class: "history-operation", "{history_operation_label(entry.operation)}" }
                    if index == history.current {
                        span { class: "history-current", "현재" }
                    }
                }
            }
        }
    }
}

fn history_operation_label(operation: HistoryOperationLabel) -> &'static str {
    match operation {
        HistoryOperationLabel::Initial => "보관 시작점",
        HistoryOperationLabel::Stroke => "스트로크",
        HistoryOperationLabel::Structural => "구조 변경",
    }
}

#[component]
fn ToolsPanel(ui_projection: Signal<UiProjection>) -> Element {
    let active = ui_projection.read().drawing_tool;
    rsx! {
        nav { class: "tool-list", aria_label: "도구",
            ToolButton { label: "그림 이동", icon: "move-artwork", shortcut: "g", tool: Some(DrawingTool::MoveArtwork), active: active == DrawingTool::MoveArtwork }
            ToolButton { label: "화면 이동", icon: "hand", shortcut: "g", tool: Some(DrawingTool::Move), active: active == DrawingTool::Move }
            ToolButton { label: "변형", icon: "transform", shortcut: "g", tool: Some(DrawingTool::MoveSelection), active: active == DrawingTool::MoveSelection }
            ToolButton { label: "마법봉", icon: "wand", shortcut: "g", tool: Some(DrawingTool::Wand), active: active == DrawingTool::Wand }
            ToolButton { label: "올가미", icon: "lasso", shortcut: "g", tool: Some(DrawingTool::Lasso), active: active == DrawingTool::Lasso }
            ToolButton { label: "사각형 선택", icon: "rectangle-selection", shortcut: "g", tool: Some(DrawingTool::RectangleSelection), active: active == DrawingTool::RectangleSelection }
            span { class: "tool-separator" }
            ToolButton { label: "연필", icon: "pencil", shortcut: "b", tool: Some(DrawingTool::Pencil), active: active == DrawingTool::Pencil }
            ToolButton { label: "펜", icon: "pen", shortcut: "b", tool: Some(DrawingTool::Pen), active: active == DrawingTool::Pen }
            ToolButton { label: "브러시", icon: "brush", shortcut: "b", tool: Some(DrawingTool::Brush), active: active == DrawingTool::Brush }
            span { class: "tool-separator" }
            ToolButton { label: "지우개", icon: "eraser", shortcut: "e", tool: Some(DrawingTool::Eraser), active: active == DrawingTool::Eraser }
            span { class: "tool-separator" }
            ToolButton { label: "채우기", icon: "bucket", shortcut: "f", tool: Some(DrawingTool::Fill), active: active == DrawingTool::Fill }
            ToolButton { label: "그라데이션", icon: "gradient", shortcut: "f", tool: Some(DrawingTool::Gradient), active: active == DrawingTool::Gradient }
            span { class: "tool-separator" }
            ToolButton { label: "스포이트", icon: "eyedropper", shortcut: "c", tool: Some(DrawingTool::Eyedropper), active: active == DrawingTool::Eyedropper }
        }
    }
}

#[component]
fn ToolButton(
    label: &'static str,
    icon: &'static str,
    shortcut: &'static str,
    tool: Option<DrawingTool>,
    #[props(default = false)] active: bool,
) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let title = match tool {
        Some(DrawingTool::Move) => "화면 이동 (G) · 보기만 이동 · 그림 픽셀 유지".to_owned(),
        Some(DrawingTool::MoveArtwork) => "그림 이동 (G) · 현재 레이어의 픽셀 이동".to_owned(),
        Some(DrawingTool::MoveSelection) => {
            "변형 (G) · 선택 영역 또는 현재 레이어의 크기·회전".to_owned()
        }
        Some(DrawingTool::Eyedropper) => {
            "스포이트 (C) · Alt+클릭: 보이는 그림에서 임시 색 채취".to_owned()
        }
        _ => format!("{label} ({shortcut})"),
    };
    rsx! {
        button {
            class: if active { "tool active" } else { "tool" },
            disabled: tool.is_none_or(|tool| !live_ink.supports_command(&EditorCommand::Tool(ToolCommand::Select(tool)))),
            title,
            aria_label: "{label}, 단축키 {shortcut}",
            onclick: move |_| {
                if let Some(tool) = tool {
                    send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::Select(tool)), error);
                }
            },
            UiIcon { name: icon }
            span { class: "tool-key", "{shortcut}" }
        }
    }
}

#[component]
fn BrushPanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let tool = ui_projection.read().drawing_tool;
    let pencil_template = ui_projection.read().pencil_template;
    let current_brush = nyatidraw_api::BrushPresetValues::from_projection(&ui_projection.read());
    let named_active = ui_projection
        .read()
        .brush_presets
        .iter()
        .any(|p| p.values.matches(current_brush));
    let name = match tool {
        DrawingTool::Pencil => "기본 연필",
        DrawingTool::Pen => "기본 펜",
        DrawingTool::Brush => "소프트 브러시",
        DrawingTool::Eraser => "기본 지우개",
        DrawingTool::MoveSelection => "자유 변형",
        DrawingTool::Move => "화면 이동",
        DrawingTool::MoveArtwork => "그림 이동",
        DrawingTool::Wand => "마법봉",
        DrawingTool::Lasso => "올가미",
        DrawingTool::RectangleSelection => "사각형 선택",
        DrawingTool::Fill => "영역 채우기",
        DrawingTool::Gradient => "선형 그라데이션",
        DrawingTool::Eyedropper => "스포이트",
    };
    rsx! {
        div { class: "subtool-list",
            if tool == DrawingTool::Pencil {
                for (template, name) in [
                    (nyatidraw_api::PencilTemplate::Mechanical2H, "2H 샤프펜슬"),
                    (nyatidraw_api::PencilTemplate::Graphite2B, "2B 연필"),
                ] {
                    button { class: if template == pencil_template && !named_active { "subtool selected" } else { "subtool" },
                        title: name, aria_pressed: template == pencil_template && !named_active,
                        onclick: { let ink = live_ink.clone(); move |_| send_editor_command(&ink, EditorCommand::Tool(ToolCommand::SelectPencilTemplate(template)), error) },
                        span { class: "subtool-name", "{name}" }
                    }
                }
            } else if tool == DrawingTool::Fill {
                fill_panel::FillSubtools { ui_projection }
            } else {
            SubtoolButton { name, tool: Some(tool), active: !named_active }
            }
            if tool == DrawingTool::MoveSelection {
                button { class: "subtool unavailable", disabled: true, "메시 변형 · 준비 중" }
            }
            if matches!(tool, DrawingTool::Pencil | DrawingTool::Pen | DrawingTool::Brush | DrawingTool::Eraser) {
                brush_presets::BrushPresets { ui_projection }
            }
        }
    }
}

#[component]
fn ToolPropertiesPanel(ui_projection: Signal<UiProjection>) -> Element {
    if ui_projection.read().edit.transform.is_some()
        || ui_projection.read().drawing_tool == DrawingTool::MoveSelection
    {
        return rsx! { transform_panel::TransformPanel { ui_projection } };
    }
    if ui_projection.read().drawing_tool == DrawingTool::MoveArtwork {
        return rsx! { MoveArtworkPanel { ui_projection } };
    }
    if ui_projection.read().drawing_tool == DrawingTool::Fill {
        return rsx! { fill_panel::FillProperties { ui_projection } };
    }
    if ui_projection.read().drawing_tool.is_edit() {
        return rsx! { EditToolPanel { ui_projection } };
    }
    let current_size = ui_projection.read().brush_size_tenths;
    let opacity =
        (u32::from(ui_projection.read().brush_opacity_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let settings = ui_projection.read().brush_settings;
    let tool = ui_projection.read().drawing_tool;
    let opacity_u16 = ui_projection.read().brush_opacity_u16;
    let pencil_template = ui_projection.read().pencil_template;
    rsx! {
        div { class: "tool-properties", onkeydown: move |event| { if event.key() != Key::Escape { event.stop_propagation(); } },
            brush_preview::BrushStrokePreview { tool, pencil_template, size_tenths: current_size, opacity_u16, settings }
            BrushSizeControl { size_tenths: current_size }
            OpacityControl { percent: opacity }
            BrushSmoothingControl { strength: settings.smoothing }
            details { class: "brush-extra-options",
                summary { "추가 옵션" }
                if tool != DrawingTool::Pencil { BrushHardnessControl { hardness_u16: settings.hardness_u16 } }
                BrushPressureControl { size_axis: true, enabled: settings.size_pressure, minimum_u16: settings.size_minimum_u16 }
                BrushPressureControl { size_axis: false, enabled: settings.opacity_pressure, minimum_u16: settings.opacity_minimum_u16 }
            }
        }
    }
}

#[component]
fn MoveArtworkPanel(ui_projection: Signal<UiProjection>) -> Element {
    let ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let has_selection = ui_projection.read().edit.has_selection;
    let busy = ui_projection.read().edit.busy;
    rsx! {
        div { class: "edit-tool-properties",
            p { if has_selection { "선택한 그림을 끌어 이동합니다. 손을 떼면 확정됩니다." } else { "현재 레이어의 그림을 끌어 이동합니다. 페이지 밖 그림도 함께 이동합니다." } }
            div { class: "selection-morph-actions", role: "group", aria_label: "캔버스 중앙 정렬",
                button { disabled: busy, onclick: { let ink = ink.clone(); move |_| send_editor_command(&ink, EditorCommand::Edit(nyatidraw_api::EditCommand::CenterArtwork { horizontal: true, vertical: false }), error) }, "가로 중앙" }
                button { disabled: busy, onclick: move |_| send_editor_command(&ink, EditorCommand::Edit(nyatidraw_api::EditCommand::CenterArtwork { horizontal: false, vertical: true }), error), "세로 중앙" }
            }
            if let Some(message) = error.read().as_ref() { div { class: "command-error", role: "alert", "{message}" } }
        }
    }
}

#[component]
fn BrushPressureControl(size_axis: bool, enabled: bool, minimum_u16: u16) -> Element {
    let live_ink = use_context::<UiHost>();
    let minimum_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let percent = (u32::from(minimum_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let mut draft = use_signal(|| percent.to_string());
    use_effect(use_reactive((&percent,), move |(value,)| {
        draft.set(value.to_string());
    }));
    let axis = if size_axis { "크기" } else { "불투명도" };
    rsx! {
        div { class: "pressure-row",
            label { class: "pressure-toggle", title: "{axis}에 필압 적용",
                input { r#type: "checkbox", checked: enabled, aria_label: "{axis} 필압",
                    onchange: move |event| {
                        let command = if size_axis { ToolCommand::SetSizePressure(event.checked()) }
                            else { ToolCommand::SetOpacityPressure(event.checked()) };
                        send_editor_command(&live_ink, EditorCommand::Tool(command), error);
                    },
                }
                span { "{axis}에 필압 적용" }
            }
            label { class: "pressure-minimum", title: "필압이 가장 약할 때의 {axis} 비율",
                span { "최소 {axis}" }
                input { class: "property-number", r#type: "number", min: "0", max: "100", step: "1",
                    disabled: !enabled || !minimum_ink.supports_command(&EditorCommand::Tool(if size_axis { ToolCommand::SetSizeMinimumU16(minimum_u16) } else { ToolCommand::SetOpacityMinimumU16(minimum_u16) })), value: "{draft}", aria_label: "{axis} 최소 필압 비율 %",
                    oninput: move |event| draft.set(event.value()),
                    onchange: move |_| {
                        if let Ok(value) = draft().parse::<u32>() {
                            let percent = value.min(100);
                            let value = u16::try_from((percent * u32::from(u16::MAX) + 50) / 100)
                                .expect("bounded pressure ratio");
                            let command = if size_axis { ToolCommand::SetSizeMinimumU16(value) }
                                else { ToolCommand::SetOpacityMinimumU16(value) };
                            send_editor_command(&minimum_ink, EditorCommand::Tool(command), error);
                            draft.set(percent.to_string());
                        } else { draft.set(percent.to_string()); }
                    },
                }
                span { "%" }
            }
        }
    }
}

#[component]
fn BrushHardnessControl(hardness_u16: u16) -> Element {
    let live_ink = use_context::<UiHost>();
    let slider_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let percent = (u32::from(hardness_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let mut draft = use_signal(|| percent.to_string());
    use_effect(use_reactive((&percent,), move |(value,)| {
        draft.set(value.to_string());
    }));
    rsx! {
        label { class: "property-row", title: "0%는 부드러운 경계, 100%는 단단한 경계",
            span { "가장자리 경도" }
            input { class: "property-number", r#type: "number", min: "0", max: "100", step: "1", value: "{draft}", aria_label: "브러시 가장자리 경도 %",
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| {
                    if let Ok(value) = draft().parse::<u32>() {
                        let percent = value.min(100);
                        let value = u16::try_from((percent * u32::from(u16::MAX) + 50) / 100)
                            .expect("bounded hardness");
                        send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetHardnessU16(value)), error);
                        draft.set(percent.to_string());
                    } else { draft.set(percent.to_string()); }
                },
            }
            span { "%" }
            input { class: "property-range", r#type: "range", min: "0", max: "100", value: "{percent}", aria_label: "브러시 경도 슬라이더",
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<u32>() {
                        let value = u16::try_from((value.min(100) * u32::from(u16::MAX) + 50) / 100)
                            .expect("bounded hardness");
                        send_editor_command(&slider_ink, EditorCommand::Tool(ToolCommand::SetHardnessU16(value)), error);
                    }
                },
            }
        }
    }
}

#[component]
fn BrushSmoothingControl(strength: u8) -> Element {
    let live_ink = use_context::<UiHost>();
    let slider_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let mut draft = use_signal(|| strength.to_string());
    use_effect(use_reactive((&strength,), move |(value,)| {
        draft.set(value.to_string());
    }));
    rsx! {
        label { class: "property-row", title: "0은 끄기 · 강도가 높을수록 선이 부드럽게 따라옵니다 · 다음 선부터 적용",
            span { "선 보정" }
            input { class: "property-number", r#type: "number", min: "0", max: "100", step: "1", value: "{draft}", aria_label: "선 보정 강도 (0: 끄기)",
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| {
                    if let Ok(value) = draft().parse::<u8>() {
                        let value = value.min(100);
                        send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetSmoothing(value)), error);
                        draft.set(value.to_string());
                    } else { draft.set(strength.to_string()); }
                },
            }
            span { class: "property-unit", aria_hidden: "true" }
            input { class: "property-range", r#type: "range", min: "0", max: "100", step: "1", value: "{strength}", aria_label: "선 보정 강도 슬라이더",
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<u8>() {
                        send_editor_command(&slider_ink, EditorCommand::Tool(ToolCommand::SetSmoothing(value.min(100))), error);
                    }
                },
            }
        }
    }
}

#[component]
fn BrushSizesPanel(ui_projection: Signal<UiProjection>) -> Element {
    let current_size = ui_projection.read().brush_size_tenths;
    rsx! {
            div { class: "brush-sizes",
                div { class: "recent-sizes", aria_label: "최근 브러시 크기",
                    span { class: "recent-size-label", "최근" }
                    for size in ui_projection.read().recent_brush_sizes.iter().copied() {
                        BrushSizeButton { key: "{size}", size_tenths: size, dot: brush_dot(i32::from(size) / 10), recent: true, active: current_size == size }
                    }
                    for slot in ui_projection.read().recent_brush_sizes.len()..4 {
                        span { key: "empty-{slot}", class: "recent-size empty", aria_hidden: "true" }
                    }
                }
                div { class: "size-list",
                    for size in [1_u16, 2, 3, 5, 8, 10, 15, 20, 30, 40, 80, 120] {
                        BrushSizeButton { size_tenths: size * 10, dot: brush_dot(i32::from(size)), recent: false, active: current_size == size * 10 }
                    }
                }
            }
    }
}

#[component]
fn BrushSizeControl(size_tenths: u16) -> Element {
    let live_ink = use_context::<UiHost>();
    let slider_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let mut draft = use_signal(|| format!("{:.1}", f32::from(size_tenths) / 10.0));
    use_effect(use_reactive((&size_tenths,), move |(size,)| {
        draft.set(format!("{:.1}", f32::from(size) / 10.0));
    }));
    let mut commit = move || {
        if let Ok(value) = draft().parse::<f32>()
            && value.is_finite()
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let size = (value.clamp(0.1, 200.0) * 10.0).round() as u16;
            send_editor_command(
                &live_ink,
                EditorCommand::Tool(ToolCommand::SetSizeTenths(size)),
                error,
            );
            draft.set(format!("{:.1}", f32::from(size) / 10.0));
            return;
        }
        draft.set(format!("{:.1}", f32::from(size_tenths) / 10.0));
    };
    rsx! {
        label { class: "property-row", title: "브러시 크기 · [ 작게 / ] 크게", span { "크기" }
            input { class: "property-number", r#type: "number", min: "0.1", max: "200", step: "0.1", value: "{draft}", aria_label: "브러시 크기 px",
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| commit(),
            }
            span { "px" }
            input { class: "property-range", r#type: "range", min: "1", max: "2000", step: "1", value: "{size_tenths}", aria_label: "브러시 크기 슬라이더",
                onchange: move |event| {
                    if let Ok(size) = event.value().parse::<u16>() {
                        send_editor_command(&slider_ink, EditorCommand::Tool(ToolCommand::SetSizeTenths(size.clamp(1, 2000))), error);
                    }
                },
            }
        }
    }
}

#[component]
fn SubtoolButton(name: &'static str, tool: Option<DrawingTool>, active: bool) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        button {
            class: if active { "subtool selected" } else { "subtool" },
            disabled: tool.is_none_or(|tool| !live_ink.supports_command(&EditorCommand::Tool(ToolCommand::Select(tool)))),
            title: if tool.is_none() { "준비 중 · 아직 사용할 수 없습니다" } else { name },
            onclick: move |_| {
                if let Some(tool) = tool {
                    send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::Select(tool)), error);
                }
            },
            span { class: "subtool-name", "{name}", if tool.is_none() { small { " 준비 중" } } }
        }
    }
}

#[component]
fn BrushSizeButton(size_tenths: u16, dot: i32, recent: bool, active: bool) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let size = f32::from(size_tenths) / 10.0;
    rsx! {
        button {
            class: match (recent, active) {
                (true, true) => "recent-size active",
                (true, false) => "recent-size",
                (false, true) => "size-option active",
                (false, false) => "size-option",
            },
            title: "브러시 크기 {size:.1}",
            aria_label: "브러시 크기 {size:.1}",
            aria_pressed: "{active}",
            onclick: move |_| send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetSizeTenths(size_tenths)), error),
            span { class: "size-preview", span { class: "size-dot", style: "width:{dot}px;height:{dot}px" } }
            span { class: "size-number", if size_tenths % 10 == 0 { "{size:.0}" } else { "{size:.1}" } }
        }
    }
}

#[component]
fn EditToolPanel(ui_projection: Signal<UiProjection>) -> Element {
    use nyatidraw_api::EditSource;
    let live_ink = use_context::<UiHost>();
    let tolerance_ink = live_ink.clone();
    let tolerance_number_ink = live_ink.clone();
    let mode_ink = live_ink.clone();
    let gradient_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let settings = ui_projection.read().edit_settings;
    let tool = ui_projection.read().drawing_tool;
    let source = match settings.source {
        EditSource::ActiveLayer => "active",
        EditSource::ReferenceLayers => "reference",
        EditSource::AllVisible => "visible",
    };
    let help = match tool {
        DrawingTool::Wand => "클릭한 픽셀과 연결된 영역을 선택합니다.",
        DrawingTool::Lasso => "끌어서 영역을 둘러싸세요. 손을 떼면 닫힙니다.",
        DrawingTool::RectangleSelection => "끌어서 사각형 영역을 선택합니다.",
        DrawingTool::MoveSelection => "끌어 변형을 미리 보고 Enter로 확정, Esc로 취소합니다.",
        DrawingTool::Eyedropper => "그림에서 색을 가져옵니다. 투명한 픽셀은 현재 색을 유지합니다.",
        DrawingTool::Fill => "클릭한 연결 영역을 채웁니다. 선택이 있으면 그 안에서만 채웁니다.",
        _ => "끌어 두 색을 연결합니다. 선택이 없으면 출력 페이지 전체에 칠합니다.",
    };
    rsx! {
        div { class: "edit-tool-properties", "aria-description": help,
            if tool == DrawingTool::Gradient {
                label { "색 연결"
                    select { aria_label: "그라데이션 색 연결",
                        value: match settings.gradient_mode {
                            nyatidraw_api::GradientMode::ForegroundToTransparent => "transparent",
                            nyatidraw_api::GradientMode::ForegroundToBackground => "background",
                        },
                        onchange: move |event| {
                            let mode = if event.value() == "background" { nyatidraw_api::GradientMode::ForegroundToBackground }
                                else { nyatidraw_api::GradientMode::ForegroundToTransparent };
                            send_editor_command(&gradient_ink, EditorCommand::Tool(ToolCommand::SetGradientMode(mode)), error);
                        },
                        option { value: "transparent", "전경색 → 투명" }
                        option { value: "background", "전경색 → 배경색" }
                    }
                }
                p { class: "transform-help", "선택 안에 칠하기 · 선택이 없으면 페이지 전체. 배경색은 색 패널에서 X로 교환해 바꿉니다." }
            }
            if matches!(tool, DrawingTool::Lasso | DrawingTool::RectangleSelection | DrawingTool::Wand) {
                select { aria_label: "선택 조합",
                    value: match settings.selection_mode {
                        nyatidraw_api::SelectionMode::Replace => "replace",
                        nyatidraw_api::SelectionMode::Add => "add",
                        nyatidraw_api::SelectionMode::Subtract => "subtract",
                    },
                    onchange: move |event| {
                        let mode = match event.value().as_str() {
                            "add" => nyatidraw_api::SelectionMode::Add,
                            "subtract" => nyatidraw_api::SelectionMode::Subtract,
                            _ => nyatidraw_api::SelectionMode::Replace,
                        };
                        send_editor_command(&mode_ink, EditorCommand::Tool(ToolCommand::SetSelectionMode(mode)), error);
                    },
                    option { value: "replace", "새 선택" }
                    option { value: "add", "추가 (+)" }
                    option { value: "subtract", "빼기 (−)" }
                }
            }
            if matches!(tool, DrawingTool::Lasso | DrawingTool::RectangleSelection | DrawingTool::Wand | DrawingTool::MoveSelection) {
                SelectionActions { ui_projection }
            }
            if matches!(tool, DrawingTool::Wand | DrawingTool::Eyedropper) {
                label { "참조 범위"
                    select { value: source, aria_label: "선택 참조 범위",
                        onchange: move |event| {
                            let source = match event.value().as_str() { "reference" => EditSource::ReferenceLayers, "visible" => EditSource::AllVisible, _ => EditSource::ActiveLayer };
                            send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetEditSource(source)), error);
                        },
                        option { value: "active", "현재 레이어" }
                        option { value: "reference", "참조 표시 레이어" }
                        option { value: "visible", "보이는 모든 레이어" }
                    }
                }
                if tool != DrawingTool::Eyedropper { label { class: "property-row",
                    span { "허용 오차" }
                    EditNumberInput { value: settings.tolerance, min: 0, max: 255, label: "색상 허용 오차 수치",
                        on_commit: move |value| {
                            send_editor_command(&tolerance_number_ink, EditorCommand::Tool(ToolCommand::SetEditTolerance(value)), error);
                        }
                    }
                    span { "/255" }
                    input { class: "property-range", r#type: "range", min: "0", max: "255", value: "{settings.tolerance}", aria_label: "색상 허용 오차",
                        onchange: move |event| { if let Ok(value) = event.value().parse::<u8>() {
                            send_editor_command(&tolerance_ink, EditorCommand::Tool(ToolCommand::SetEditTolerance(value)), error);
                        } }
                    }
                } }
            }
        }
    }
}

#[component]
fn SelectionActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let error = use_signal(|| Option::<String>::None);
    let mut radius = use_signal(|| 1_u8);
    let state = ui_projection.read();
    let busy = state.edit.busy
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some()
        || state.animation.playing;
    let has_selection = ui_projection.read().edit.has_selection;
    rsx! { div { class: "selection-actions",
        div { class: "selection-toolbar", role: "group", aria_label: "선택 영역 명령",
        for (label, icon, title, needs_selection, command) in [
            ("전체 선택", "rectangle-selection", "전체 선택 (Ctrl+A) · 페이지와 현재 레이어·선택의 유한 범위", false, nyatidraw_api::EditCommand::SelectAll),
            ("선택 반전", "invert-selection", "선택 반전 (Ctrl+Shift+I)", false, nyatidraw_api::EditCommand::InvertSelection),
            ("선택 해제", "deselect", "선택만 해제 (Esc)", true, nyatidraw_api::EditCommand::ClearSelection),
            ("선택 픽셀 삭제", "trash", "선택 픽셀 삭제 (Delete) · 현재 레이어만", true, nyatidraw_api::EditCommand::DeleteSelectedPixels),
            ("그림에서 선택", "select-alpha", "현재 이미지 알파와 사용 중인 마스크에서 선택 · 레이어 불투명도·그룹 표시는 반영하지 않음", false, nyatidraw_api::EditCommand::SelectLayerAlpha),
        ] {
            button { title, aria_label: label, disabled: busy || (needs_selection && !has_selection), onclick: {
                let ink = live_ink.clone();
                move |_| send_editor_command(&ink, EditorCommand::Edit(command.clone()), error)
            }, UiIcon { name: icon } }
        }
        }
        label { class: "property-row", title: "선택 확장·축소·부드럽게 반경, 문서 픽셀 단위",
            span { "선택 경계" }
            EditNumberInput { value: radius(), min: 1, max: 64, label: "선택 경계 반경 문서 픽셀",
                on_commit: move |value| radius.set(value)
            }
            span { "px" }
        }
        div { class: "selection-morph-actions",
        for (label, icon, title, command) in [
            ("확장", "plus", "선택 영역 확장", nyatidraw_api::EditCommand::GrowSelection { radius: radius() }),
            ("축소", "minus", "선택 영역 축소", nyatidraw_api::EditCommand::ShrinkSelection { radius: radius() }),
            ("부드럽게", "gradient", "선택 경계를 부드럽게 · 반경은 문서 픽셀 · 반복 적용", nyatidraw_api::EditCommand::FeatherSelection { radius: radius() }),
        ] {
            button { title, disabled: busy || !has_selection || !live_ink.supports_command(&EditorCommand::Edit(command.clone())), onclick: {
                let ink = live_ink.clone();
                move |_| send_editor_command(&ink, EditorCommand::Edit(command.clone()), error)
            }, UiIcon { name: icon } span { "{label}" } }
        }
        }
    } }
}

#[component]
fn EditNumberInput(
    value: u8,
    min: u8,
    max: u8,
    label: String,
    on_commit: EventHandler<u8>,
    #[props(default)] disabled: bool,
) -> Element {
    let mut draft = use_signal(|| value.to_string());
    let mut editing = use_signal(|| false);
    use_effect(use_reactive((&value,), move |(value,)| {
        if !*editing.peek() {
            draft.set(value.to_string());
        }
    }));
    rsx! {
        input { class: "property-number", r#type: "number", min: "{min}", max: "{max}", step: "1", value: "{draft}", aria_label: "{label}", disabled,
            onfocus: move |_| editing.set(true),
            oninput: move |event| draft.set(event.value()),
            onblur: move |_| {
                if disabled {
                    draft.set(value.to_string());
                    editing.set(false);
                    return;
                }
                let parsed = draft().trim().parse::<i64>().unwrap_or_else(|error| match error.kind() {
                    std::num::IntErrorKind::PosOverflow => i64::MAX,
                    std::num::IntErrorKind::NegOverflow => i64::MIN,
                    _ => i64::from(value),
                });
                let normalized = u8::try_from(parsed.clamp(i64::from(min), i64::from(max))).unwrap_or(value);
                draft.set(normalized.to_string());
                editing.set(false);
                on_commit.call(normalized);
            }
        }
    }
}

#[component]
fn OpacityControl(percent: u32) -> Element {
    let live_ink = use_context::<UiHost>();
    let slider_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let mut draft = use_signal(|| percent.to_string());
    use_effect(use_reactive((&percent,), move |(value,)| {
        draft.set(value.to_string());
    }));
    rsx! {
        label { class: "property-row",
            span { "불투명도" }
            input { class: "property-number", r#type: "number", min: "1", max: "100", step: "1", value: "{draft}", aria_label: "브러시 불투명도 %",
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| {
                    if let Ok(value) = draft().parse::<u32>() {
                        let value = value.clamp(1, 100);
                        let scaled = (value * u32::from(u16::MAX) + 50) / 100;
                        send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetOpacityU16(u16::try_from(scaled).expect("bounded opacity"))), error);
                        draft.set(value.to_string());
                    } else { draft.set(percent.to_string()); }
                },
            }
            span { "%" }
            input {
                class: "property-range",
                aria_label: "브러시 불투명도 슬라이더",
                r#type: "range",
                min: "1",
                max: "100",
                value: "{percent}",
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<u32>() {
                        let scaled = ((value.min(100) * u32::from(u16::MAX)) / 100).max(1);
                        if let Ok(opacity) = u16::try_from(scaled) {
                            send_editor_command(&slider_ink, EditorCommand::Tool(ToolCommand::SetOpacityU16(opacity)), error);
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NavigatorPanel(ui_projection: Signal<UiProjection>) -> Element {
    let reference = use_context::<reference_panel::ReferenceState>();
    let show_reference = reference.show_reference();
    rsx! {
        div { class: "navigator-subviews",
            div { class: "navigator-tabs",
                button { aria_pressed: "{!show_reference}", onclick: move |_| reference.select(false), "내비게이터" }
                button { aria_pressed: "{show_reference}", onclick: move |_| reference.select(true), "참조" }
            }
            if show_reference { reference_panel::ReferencePanel { ui_projection } }
            else { DocumentNavigatorPanel { ui_projection } }
        }
    }
}

#[component]
fn DocumentNavigatorPanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let fit_ink = live_ink.clone();
    let actual_ink = live_ink.clone();
    let zoom_in_ink = live_ink.clone();
    let zoom_out_ink = live_ink.clone();
    let center_ink = live_ink.clone();
    let leave_ink = live_ink.clone();
    let mut navigator_drag = use_context::<Signal<Option<NavigatorDrag>>>();
    let error = use_signal(|| Option::<String>::None);
    let viewport = ui_projection.read().viewport;
    let zoom = viewport.zoom_ppm / 10_000;
    let navigator = live_ink.navigator_snapshot();
    let frame = navigator.frame;
    let navigator_dragging = navigator_drag.read().is_some();
    let canvas_style = frame.as_ref().map(|frame| {
        format!(
            "width:{}px;height:{}px;aspect-ratio:{} / {};",
            frame.width,
            frame.height,
            u32::from(frame.width),
            u32::from(frame.height)
        )
    });
    let viewport_style = navigator.viewport.map(|viewport| {
        let points = viewport
            .quad_page_10k
            .iter()
            .map(|point| {
                format!(
                    "{}% {}%",
                    f64::from(point[0]) / 100.0,
                    f64::from(point[1]) / 100.0
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!("clip-path:polygon({points});")
    });
    rsx! {
        div { class: "navigator-panel",
            div { class: "navigator-preview",
                div {
                    class: if navigator_dragging {
                        "navigator-canvas dragging"
                    } else {
                        "navigator-canvas"
                    },
                    style: canvas_style,
                    title: "클릭한 위치를 작업영역 중앙으로 이동",
                    onpointerdown: move |event| {
                        let Some(frame) = frame.as_ref() else { return };
                        if event.data().trigger_button()
                            != Some(dioxus::html::input_data::MouseButton::Primary)
                        {
                            return;
                        }
                        event.prevent_default();
                        let point = event.data().element_coordinates();
                        let client = event.data().client_coordinates();
                        let page = [
                            navigator_coordinate_10k(point.x, frame.width),
                            navigator_coordinate_10k(point.y, frame.height),
                        ];
                        let sent = send_navigator_center(&center_ink, page);
                        navigator_drag.set(Some(NavigatorDrag {
                            pointer_id: event.data().pointer_id(),
                            origin_x: client.x - point.x,
                            origin_y: client.y - point.y,
                            width: frame.width,
                            height: frame.height,
                            last_sent: sent.then_some(page),
                            last_sent_at: center_ink.now_millis(),
                        }));
                    },
                    onpointerleave: move |event| {
                        finish_navigator_drag(navigator_drag, &leave_ink, &event.data());
                    },
                    if let Some(ref frame) = frame {
                        img { class: "navigator-image", src: "{frame.data_uri}", alt: "현재 페이지 미리보기" }
                    }
                    if let Some(style) = viewport_style {
                        span { class: "viewport-box", style: "{style}", aria_hidden: "true" }
                    }
                }
            }
            div { class: "navigator-actions",
                button { class: "nav-mode", title: "1:1 크기", onclick: move |_| send_editor_command(&actual_ink, EditorCommand::Viewport(ViewportCommand::ActualPixels), error), "1:1" }
                button { title: "화면 맞춤", onclick: move |_| send_editor_command(&fit_ink, EditorCommand::Viewport(ViewportCommand::FitDocument), error), UiIcon { name: "fit" } }
                span { class: "mini-divider" }
                button { title: "축소", onclick: move |_| send_editor_command(&zoom_out_ink, EditorCommand::Viewport(ViewportCommand::ZoomSteps(-1)), error), UiIcon { name: "minus" } }
                span { class: "nav-value", "{zoom}%" }
                button { title: "확대", onclick: move |_| send_editor_command(&zoom_in_ink, EditorCommand::Viewport(ViewportCommand::ZoomSteps(1)), error), UiIcon { name: "plus" } }
            }
        }
    }
}

#[allow(clippy::cast_possible_truncation)]
fn navigator_coordinate_10k(coordinate: f64, extent: u16) -> i32 {
    if !coordinate.is_finite() || extent == 0 {
        return 0;
    }
    (coordinate / f64::from(extent) * 10_000.0)
        .round()
        .clamp(0.0, 10_000.0) as i32
}

fn send_navigator_center(live_ink: &UiHost, page: [i32; 2]) -> bool {
    live_ink
        .push_ui_editor_command(EditorCommand::Viewport(ViewportCommand::CenterPageAt {
            page_x_10k: page[0],
            page_y_10k: page[1],
        }))
        .is_ok()
}

fn navigator_drag_page(data: &dioxus::html::events::PointerData, drag: NavigatorDrag) -> [i32; 2] {
    let client = data.client_coordinates();
    [
        navigator_coordinate_10k(client.x - drag.origin_x, drag.width),
        navigator_coordinate_10k(client.y - drag.origin_y, drag.height),
    ]
}

fn continue_navigator_drag(
    mut navigator_drag: Signal<Option<NavigatorDrag>>,
    live_ink: &UiHost,
    data: &dioxus::html::events::PointerData,
) {
    let Some(mut drag) = navigator_drag.read().as_ref().copied() else {
        return;
    };
    if data.pointer_id() != drag.pointer_id {
        return;
    }
    let now = live_ink.now_millis();
    if now - drag.last_sent_at < 8.0 {
        return;
    }
    let page = navigator_drag_page(data, drag);
    drag.last_sent_at = now;
    if send_navigator_center(live_ink, page) {
        drag.last_sent = Some(page);
    }
    navigator_drag.set(Some(drag));
}

fn finish_navigator_drag(
    mut navigator_drag: Signal<Option<NavigatorDrag>>,
    live_ink: &UiHost,
    data: &dioxus::html::events::PointerData,
) {
    let Some(drag) = navigator_drag.read().as_ref().copied() else {
        return;
    };
    if drag.pointer_id != data.pointer_id() {
        return;
    }
    let page = navigator_drag_page(data, drag);
    if drag.last_sent != Some(page) {
        let _ = send_navigator_center(live_ink, page);
    }
    navigator_drag.set(None);
}

#[component]
fn ColorPanel(ui_projection: Signal<UiProjection>) -> Element {
    rsx! { color_panel::ColorPanel { ui_projection } }
}

#[component]
fn LayersPanel(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<UiHost>();
    let add_raster_ink = live_ink.clone();
    let duplicate_ink = live_ink.clone();
    let add_group_ink = live_ink.clone();
    let white_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let group_transform_ink = live_ink.clone();
    let transform_panel::TransformPanelOpen(mut transform_open) = use_context();
    let state = ui_projection.read();
    let layer_busy = state.edit.busy
        || state.edit.transform.is_some()
        || state.edit.color_adjustment.is_some()
        || state.animation.playing;
    let layers = state.layers.clone();
    let active = ui_projection.read().active_layer;
    let mut group_settings = use_signal(|| None::<GroupId>);
    use_effect(use_reactive((&active,), move |_| group_settings.set(None)));
    let settings_node = group_settings()
        .map(LayerTreeNodeId::Group)
        .or_else(|| active.map(LayerTreeNodeId::Raster));
    let settings_layer = layers
        .iter()
        .find(|layer| Some(layer.id) == settings_node)
        .cloned();
    let settings_id = settings_layer.as_ref().map(|layer| layer.id);
    let active_locked = layers.iter().any(|layer| {
        layer.id == LayerTreeNodeId::Raster(active.unwrap_or(LayerId(0))) && layer.locked
    });
    let solo_node = ui_projection.read().solo_node;
    let error = use_signal(|| Option::<String>::None);
    let collapsed_groups = use_signal(BTreeSet::<GroupId>::new);
    layer_drag::use_layer_pointer_drag(error);
    let collapsed_snapshot = collapsed_groups.read().clone();
    let thumbnails = live_ink.layer_thumbnail_snapshot();
    let mut hidden_below = None;
    let visible_layers: Vec<_> = layers
        .into_iter()
        .filter(|layer| {
            if let Some(depth) = hidden_below {
                if layer.depth > depth {
                    return false;
                }
                hidden_below = None;
            }
            if let LayerTreeNodeId::Group(group) = layer.id
                && collapsed_snapshot.contains(&group)
            {
                hidden_below = Some(layer.depth);
            }
            true
        })
        .collect();
    rsx! {
        div { class: "layers-panel",
            div { class: "layer-actions",
                button { title: "래스터 레이어 추가", aria_label: "래스터 레이어 추가", disabled: layer_busy, onclick: move |_| {
                    send_editor_command(&add_raster_ink, EditorCommand::Layer(LayerCommand::AddRaster), error);
                }, UiIcon { name: "plus" } }
                button { title: "선택 레이어 또는 그룹 전체 복제", aria_label: "선택 레이어 또는 그룹 전체 복제", disabled: layer_busy || settings_id.is_none(), onclick: move |_| {
                    if let Some(node) = settings_id {
                        let command = match node {
                            LayerTreeNodeId::Raster(layer) => LayerCommand::DuplicateRaster(layer),
                            LayerTreeNodeId::Group(group) => LayerCommand::DuplicateGroup(group),
                        };
                        send_editor_command(&duplicate_ink, EditorCommand::Layer(command), error);
                    }
                }, UiIcon { name: "copy" } }
                button { title: "그룹 추가", aria_label: "레이어 그룹 추가", disabled: layer_busy || !add_group_ink.supports_command(&EditorCommand::Layer(LayerCommand::AddGroup)), onclick: move |_| {
                    send_editor_command(&add_group_ink, EditorCommand::Layer(LayerCommand::AddGroup), error);
                }, UiIcon { name: "folder-plus" } }
                button { title: "흰 배경 추가", aria_label: "흰 배경 레이어 추가", disabled: layer_busy || !white_ink.supports_command(&EditorCommand::Layer(LayerCommand::AddWhiteBackground)), onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } }
                if group_settings().is_none() { layer_drag::LayerMoveButtons { projection: ui_projection, error } }
            }
            if let Some(layer) = settings_layer {
                if group_settings().is_some() {
                    div { class: "group-settings-label",
                        span { "그룹 설정 · {layer.name}" }
                        button {
                            title: "그룹 전체 변형 · 선택 영역이 있으면 그 안의 그림만 변형",
                            disabled: layer_busy || matches!(ui_projection.read().raster_target, Some(nyatidraw_api::RasterEditTarget::Mask { .. })),
                            onclick: move |_| {
                                if let Some(group) = group_settings() {
                                    transform_open.set(true);
                                    send_editor_command(&group_transform_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::FreeTransform(nyatidraw_api::TransformCommand::BeginGroup(group))), error);
                                }
                            },
                            "그룹 변형"
                        }
                        button { title: "그리는 레이어 설정으로 돌아가기", onclick: move |_| group_settings.set(None), "×" }
                    }
                }
                LayerStyleControls { layer, error }
            }
            div { class: "layer-list",
                for layer in visible_layers {
                    {
                        let thumbnail = match layer.id {
                            LayerTreeNodeId::Raster(layer_id) => thumbnails.frame(layer_id),
                            LayerTreeNodeId::Group(_) => None,
                        };
                        rsx! { LayerRow { key: "{layer.id:?}", layer, thumbnail, active, solo_node, error, collapsed_groups, group_settings, ui_projection } }
                    }
                }
            }
            if let Some(message) = error.read().as_ref() { div { class: "command-error", role: "alert", "{message}" } }
            div { class: "layers-footer", button { disabled: layer_busy || settings_id.is_none() || (group_settings().is_none() && active_locked), title: "설정 대상 레이어/그룹 삭제 (Undo로 복원)", aria_label: "설정 대상 레이어 또는 그룹 삭제", onclick: move |_| {
                if let Some(node) = settings_id {
                    send_editor_command(&delete_ink, EditorCommand::Layer(LayerCommand::Delete(node)), error);
                }
            }, UiIcon { name: "trash" } } }
        }
    }
}

#[component]
fn LayerRow(
    layer: LayerProjection,
    thumbnail: Option<Arc<LayerThumbnailFrame>>,
    active: Option<LayerId>,
    solo_node: Option<LayerTreeNodeId>,
    error: Signal<Option<String>>,
    mut collapsed_groups: Signal<BTreeSet<GroupId>>,
    mut group_settings: Signal<Option<GroupId>>,
    ui_projection: Signal<UiProjection>,
) -> Element {
    let live_ink = use_context::<UiHost>();
    let active_ink = live_ink.clone();
    let visibility_ink = live_ink.clone();
    let thumbnail_ink = live_ink.clone();
    let keyboard_ink = live_ink.clone();
    let solo_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let id = layer.id;
    let is_active = matches!(id, LayerTreeNodeId::Raster(layer_id) if active == Some(layer_id));
    let is_group = layer.kind == LayerProjectionKind::Group;
    let layer_has_mask = layer.mask.is_some();
    let mask_thumbnail = thumbnail.as_ref().and_then(|frame| frame.mask.clone());
    let image_selected = ui_projection.read().raster_target
        == active.map(nyatidraw_api::RasterEditTarget::Image)
        && is_active;
    let is_collapsed =
        matches!(id, LayerTreeNodeId::Group(group) if collapsed_groups.read().contains(&group));
    let opacity = (u32::from(layer.opacity_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let indent = u32::from(layer.depth.saturating_sub(1)) * 9;
    let visibility_icon = if layer.visible { "eye" } else { "eye-off" };
    let layer_key = match id {
        LayerTreeNodeId::Raster(id) => format!("r-{}", id.0),
        LayerTreeNodeId::Group(id) => format!("g-{}", id.0),
    };
    rsx! {
        div { class: if is_group { "layer-row group" } else if is_active { "layer-row active" } else { "layer-row" }, style: "padding-left:{indent}px",
            "data-layer-key": "{layer_key}", "data-parent": "{layer.parent.0}", "data-index": "{layer.index}",
            "data-depth": "{layer.depth}",
            "data-has-mask": layer_has_mask,
            button { class: "layer-eye", title: if layer.visible { "레이어 숨기기" } else { "레이어 표시" }, aria_label: "{layer.name} 표시", aria_pressed: layer.visible, onclick: move |_| {
                let visible = !layer.visible;
                send_editor_command(&visibility_ink, EditorCommand::Layer(LayerCommand::SetVisibility { node: id, visible }), error);
            }, span { class: if layer.visible { "eye-mark" } else { "eye-mark hidden" }, UiIcon { name: visibility_icon } } }
            button {
                class: if solo_node == Some(id) { "layer-solo active" } else { "layer-solo" },
                disabled: !solo_ink.supports_command(&EditorCommand::Layer(LayerCommand::ToggleSolo(id))),
                title: if solo_node == Some(id) { "레이어만 보기 해제" } else { "이 레이어만 보기" },
                aria_label: "{layer.name}만 보기", aria_pressed: solo_node == Some(id),
                onclick: move |_| send_editor_command(
                    &solo_ink,
                    EditorCommand::Layer(LayerCommand::ToggleSolo(id)),
                    error,
                ),
                "S"
            }
            if let LayerTreeNodeId::Group(group) = id {
                button { class: "layer-tree", title: if is_collapsed { "그룹 펼치기" } else { "그룹 접기" }, onclick: move |_| {
                    let mut next = collapsed_groups.read().clone();
                    if !next.insert(group) {
                        next.remove(&group);
                    }
                    collapsed_groups.set(next);
                },
                    if is_collapsed {
                        UiIcon { name: "chevron-right" }
                    } else {
                        UiIcon { name: "chevron-down" }
                    }
                }
            } else {
                span { class: "layer-tree", if layer.depth > 1 { "└" } else { "" } }
            }
            div { class: "layer-targets",
            span { class: if is_group { "layer-thumb group-thumb" } else { "layer-thumb raster-thumb" },
                role: "button", tabindex: "0", aria_label: "{layer.name} 이미지 선택", aria_pressed: image_selected,
                onkeydown: move |event| {
                    if event.key() != Key::Enter && event.key() != Key::Character(" ".to_owned()) { return; }
                    event.prevent_default(); event.stop_propagation();
                    match id {
                        LayerTreeNodeId::Raster(layer) => {
                            group_settings.set(None);
                            send_editor_command(&keyboard_ink, image_target_command(layer, layer_has_mask), error);
                        }
                        LayerTreeNodeId::Group(group) => group_settings.set(Some(group)),
                    }
                },
                title: "이미지 선택 · 레이어 순서 변경은 오른쪽 이동 핸들",
                onclick: move |_| {
                    if let LayerTreeNodeId::Raster(layer_id) = id {
                        group_settings.set(None);
                        send_editor_command(&thumbnail_ink, image_target_command(layer_id, layer_has_mask), error);
                    } else if let LayerTreeNodeId::Group(group) = id {
                        group_settings.set(Some(group));
                    }
                },
                if let Some(thumbnail) = thumbnail {
                    img {
                        class: "layer-thumb-image",
                        draggable: "false",
                        src: "{thumbnail.data_uri}",
                        width: "{thumbnail.width}",
                        height: "{thumbnail.height}",
                        alt: "{layer.name} 레이어 미리보기",
                    }
                } else if is_group {
                    UiIcon { name: "folder" }
                }
            }
            if layer_has_mask {
                layer_mask::MaskThumbnail { layer: layer.clone(), thumbnail: mask_thumbnail, ui_projection, error,
                    onselected: move |()| group_settings.set(None) }
            }
            }
            div { class: "layer-meta", onclick: move |_| {
                if let LayerTreeNodeId::Raster(layer_id) = id {
                    group_settings.set(None);
                    send_editor_command(&active_ink, EditorCommand::Layer(LayerCommand::SetActive(layer_id)), error);
                } else if let LayerTreeNodeId::Group(group) = id {
                    group_settings.set(Some(group));
                }
            },
                LayerNameInput { node: id, name: layer.name.clone(), error }
                span { class: "layer-detail",
                    if layer.locked { span { title: "픽셀 잠김", UiIcon { name: "lock" } } }
                    if layer.reference { "참조 · " }
                    if layer.alpha_locked { span { title: "투명도 잠금", UiIcon { name: "alpha-lock" } } }
                    if layer.clip_to_below { span { title: "아래 레이어에 클리핑", UiIcon { name: "clip" } } }
                    if layer.blend_mode != nyatidraw_api::LayerBlendMode::Normal { "{blend_mode_option(layer.blend_mode).1} · " }
                    if layer.color_effect.is_some_and(|effect| effect.enabled) { "색상화 · " }
                    if is_group { "그룹 · " }
                    "{opacity}%"
                }
                layer_mask::LayerMaskControls { layer: layer.clone(), ui_projection, error }
            }
            div { class: "layer-row-actions",
            button {
                class: "layer-delete", title: "삭제 (Undo로 복원)", aria_label: "{layer.name} 삭제",
                disabled: layer.locked,
                onclick: move |_| send_editor_command(&delete_ink, EditorCommand::Layer(LayerCommand::Delete(id)), error),
                UiIcon { name: "trash" }
            }
            span { class: "layer-reorder-handle", "data-layer-reorder-handle": "true",
                role: "img", aria_label: "{layer.name} 순서 이동 핸들",
                title: "끌어서 레이어 순서 또는 그룹 변경",
                UiIcon { name: "grip" }
            }
            }
        }
    }
}

fn image_target_command(layer: LayerId, masked: bool) -> EditorCommand {
    if masked {
        EditorCommand::Mask(nyatidraw_api::MaskCommand::SelectImage(layer))
    } else {
        EditorCommand::Layer(LayerCommand::SetActive(layer))
    }
}

#[component]
fn LayerNameInput(
    node: LayerTreeNodeId,
    name: String,
    mut error: Signal<Option<String>>,
) -> Element {
    let host = use_context::<UiHost>();
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(|| name.clone());
    use_effect(use_reactive((&name,), move |(name,)| {
        if !*editing.peek() {
            draft.set(name);
        }
    }));
    let original = name.clone();
    let commit = use_callback(move |()| {
        if !editing() {
            return;
        }
        editing.set(false);
        let value = draft();
        if value.trim().is_empty() {
            draft.set(original.clone());
            error.set(Some("레이어 이름은 비워둘 수 없습니다".to_owned()));
        } else if value != original {
            if let Err(message) =
                host.push_ui_editor_command(EditorCommand::Layer(LayerCommand::Rename {
                    node,
                    name: value,
                }))
            {
                error.set(Some(message));
                draft.set(original.clone());
            } else {
                error.set(None);
            }
        }
    });
    rsx! {
        input { class: "layer-name", aria_label: "레이어 이름",
            title: "선택 · 더블클릭/F2 이름 변경 · Enter 확정 · Esc 취소",
            readonly: !editing(), value: "{draft}", maxlength: "128", spellcheck: "false",
            ondoubleclick: move |_| editing.set(true),
            oninput: move |event| draft.set(event.value()),
            onblur: move |_| commit.call(()),
            onkeydown: move |event| {
                event.stop_propagation();
                match event.key() {
                    Key::F2 => { event.prevent_default(); editing.set(true); }
                    Key::Enter => {
                        event.prevent_default();
                        if editing() { commit.call(()); crate::focus_editor_shortcuts(); }
                        else { editing.set(true); }
                    }
                    Key::Escape => {
                        event.prevent_default(); editing.set(false); draft.set(name.clone());
                        error.set(None); crate::focus_editor_shortcuts();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn blend_mode_option(mode: nyatidraw_api::LayerBlendMode) -> (&'static str, &'static str) {
    use nyatidraw_api::LayerBlendMode;
    match mode {
        LayerBlendMode::Normal => ("normal", "표준"),
        LayerBlendMode::Multiply => ("multiply", "곱하기"),
        LayerBlendMode::Screen => ("screen", "스크린"),
        LayerBlendMode::Add => ("add", "더하기"),
        LayerBlendMode::Overlay => ("overlay", "오버레이"),
    }
}

#[component]
fn LayerStyleControls(layer: LayerProjection, error: Signal<Option<String>>) -> Element {
    let live_ink = use_context::<UiHost>();
    let alpha_ink = live_ink.clone();
    let clip_ink = live_ink.clone();
    let reference_ink = live_ink.clone();
    let lock_ink = live_ink.clone();
    let color_ink = live_ink.clone();
    let id = layer.id;
    let color_effect = layer.color_effect.unwrap_or_default();
    let color_enabled = layer.color_effect.is_some_and(|effect| effect.enabled);
    let color_hex = layer_color_hex(color_effect.dark_rgb);
    rsx! { div { class: "layer-style-controls", aria_label: "{layer.name} 합성 설정",
        div { class: "layer-style-row",
        select { class: "layer-blend", disabled: !live_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetBlendMode { node: id, blend_mode: layer.blend_mode })), aria_label: "레이어 합성 모드", title: "{layer.name} 합성 모드",
            value: blend_mode_option(layer.blend_mode).0,
            onkeydown: move |event| event.stop_propagation(),
            onchange: move |event| {
                use nyatidraw_api::LayerBlendMode;
                let blend_mode = match event.value().as_str() {
                    "normal" => LayerBlendMode::Normal,
                    "multiply" => LayerBlendMode::Multiply,
                    "screen" => LayerBlendMode::Screen,
                    "add" => LayerBlendMode::Add,
                    "overlay" => LayerBlendMode::Overlay,
                    _ => return,
                };
                send_editor_command(&live_ink, EditorCommand::Layer(LayerCommand::SetBlendMode { node: id, blend_mode }), error);
            },
            option { value: "normal", "표준" }
            option { value: "multiply", "곱하기" }
            option { value: "screen", "스크린" }
            option { value: "add", "더하기" }
            option { value: "overlay", "오버레이" }
        }
        div { class: "layer-style-flags", role: "group", aria_label: "레이어 속성",
        if let LayerTreeNodeId::Raster(layer_id) = id {
            button { title: if layer.locked { "픽셀 잠금 해제" } else { "픽셀 잠금" }, aria_label: "픽셀 잠금", disabled: !lock_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetLocked { layer: layer_id, locked: !layer.locked })), aria_pressed: "{layer.locked}",
                onclick: move |_| send_editor_command(&lock_ink, EditorCommand::Layer(LayerCommand::SetLocked { layer: layer_id, locked: !layer.locked }), error),
                if layer.locked { UiIcon { name: "lock" } } else { UiIcon { name: "unlock" } }
            }
            button { title: "투명도 잠금 · 이미 그려진 픽셀 안에서만 칠하기", aria_label: "투명도 잠금", disabled: !alpha_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetAlphaLocked { layer: layer_id, alpha_locked: !layer.alpha_locked })), aria_pressed: "{layer.alpha_locked}",
                onclick: move |_| send_editor_command(&alpha_ink, EditorCommand::Layer(LayerCommand::SetAlphaLocked { layer: layer_id, alpha_locked: !layer.alpha_locked }), error), UiIcon { name: "alpha-lock" }
            }
        }
        button { title: "아래 레이어에 클리핑", aria_label: "아래 레이어에 클리핑", disabled: !clip_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetClipToBelow { node: id, clip_to_below: !layer.clip_to_below })), aria_pressed: "{layer.clip_to_below}",
            onclick: move |_| send_editor_command(&clip_ink, EditorCommand::Layer(LayerCommand::SetClipToBelow { node: id, clip_to_below: !layer.clip_to_below }), error), UiIcon { name: "clip" }
        }
        if let LayerTreeNodeId::Raster(layer_id) = id {
            button { title: "선택·채우기 참조 레이어 (PNG에는 영향 없음)", aria_label: "참조 레이어", disabled: !reference_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetReference { layer: layer_id, reference: !layer.reference })), aria_pressed: "{layer.reference}",
                onclick: move |_| send_editor_command(&reference_ink, EditorCommand::Layer(LayerCommand::SetReference { layer: layer_id, reference: !layer.reference }), error),
                UiIcon { name: "reference" }
            }
        }
        if let LayerTreeNodeId::Raster(layer_id) = id {
            button { title: if color_enabled { "레이어 색상화 끄기 · 원본 픽셀 보존" } else { "레이어 색상화 · 검정/흰색을 지정색으로 표시" },
                aria_label: "레이어 색상화", aria_pressed: "{color_enabled}",
                disabled: !color_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetColorEffect { layer: layer_id, effect: Some(color_effect) })),
                onclick: move |_| send_editor_command(&color_ink, EditorCommand::Layer(LayerCommand::SetColorEffect {
                    layer: layer_id, effect: Some(LayerColorEffect { enabled: !color_enabled, ..color_effect }),
                }), error),
                span { class: "layer-color-chip", style: "background:{color_hex}" }
            }
        } else {
            button { disabled: true, title: "레이어 색상화는 래스터 레이어에서 사용할 수 있습니다", aria_label: "레이어 색상화 · 래스터 전용", span { class: "layer-color-chip" } }
        }
        }
        }
        LayerOpacityControl { key: "{id:?}", node: id, opacity_u16: layer.opacity_u16, error }
        if color_enabled && let LayerTreeNodeId::Raster(layer_id) = id {
            LayerColorSwatches { layer: layer_id, effect: color_effect, error }
        }
    } }
}

#[component]
fn LayerColorSwatches(
    layer: LayerId,
    effect: LayerColorEffect,
    error: Signal<Option<String>>,
) -> Element {
    let dark_ink = use_context::<UiHost>();
    let light_ink = dark_ink.clone();
    let disabled =
        !dark_ink.supports_command(&EditorCommand::Layer(LayerCommand::SetColorEffect {
            layer,
            effect: Some(effect),
        }));
    let dark_hex = layer_color_hex(effect.dark_rgb);
    let light_hex = layer_color_hex(effect.light_rgb);
    rsx! {
        div { class: "layer-color-swatches", role: "group", aria_label: "레이어 색상화 색",
            label { "진한 색"
                input { r#type: "color", value: "{dark_hex}", disabled,
                    aria_label: "레이어 색상화 진한 색", title: "검정 픽셀에 적용할 색",
                    onkeydown: move |event| event.stop_propagation(),
                    onchange: move |event| {
                        if let Some(dark_rgb) = parse_layer_color_hex(&event.value()) {
                            send_editor_command(&dark_ink, EditorCommand::Layer(LayerCommand::SetColorEffect {
                                layer, effect: Some(LayerColorEffect { dark_rgb, ..effect }),
                            }), error);
                        }
                    },
                }
            }
            label { "밝은 색"
                input { r#type: "color", value: "{light_hex}", disabled,
                    aria_label: "레이어 색상화 밝은 색", title: "흰색 픽셀에 적용할 색",
                    onkeydown: move |event| event.stop_propagation(),
                    onchange: move |event| {
                        if let Some(light_rgb) = parse_layer_color_hex(&event.value()) {
                            send_editor_command(&light_ink, EditorCommand::Layer(LayerCommand::SetColorEffect {
                                layer, effect: Some(LayerColorEffect { light_rgb, ..effect }),
                            }), error);
                        }
                    },
                }
            }
        }
    }
}

fn layer_color_hex([red, green, blue]: [u8; 3]) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}

fn parse_layer_color_hex(value: &str) -> Option<[u8; 3]> {
    if value.len() != 7 {
        return None;
    }
    let [_, red, green, blue] = u32::from_str_radix(value.strip_prefix('#')?, 16)
        .ok()?
        .to_be_bytes();
    Some([red, green, blue])
}

#[component]
fn LayerOpacityControl(
    node: LayerTreeNodeId,
    opacity_u16: u16,
    error: Signal<Option<String>>,
) -> Element {
    let live_ink = use_context::<UiHost>();
    let slider_ink = live_ink.clone();
    let percent = (u32::from(opacity_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let mut draft = use_signal(|| percent.to_string());
    use_effect(use_reactive((&percent,), move |(value,)| {
        draft.set(value.to_string());
    }));
    rsx! {
        div { class: "layer-opacity", role: "group", aria_label: "레이어 불투명도",
            input { class: "layer-opacity-slider", r#type: "range", min: "0", max: "100", step: "1",
                title: "불투명도 · 드래그 후 놓으면 적용", aria_label: "레이어 불투명도 드래그 조절",
                value: "{draft}",
                onkeydown: move |event| { if event.key() != Key::Escape { event.stop_propagation(); } },
                oninput: move |event| draft.set(event.value()),
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<u32>() {
                        let value = value.min(100);
                        if value != percent {
                            let opacity_u16 = u16::try_from((value * 65_535 + 50) / 100).expect("bounded opacity");
                            send_editor_command(&slider_ink, EditorCommand::Layer(LayerCommand::SetOpacity { node, opacity_u16 }), error);
                        }
                        draft.set(value.to_string());
                    }
                }
            }
            input { r#type: "number", min: "0", max: "100", step: "1",
                class: "layer-opacity-number", title: "레이어 불투명도 수치",
                aria_label: "레이어 불투명도 %", value: "{draft}",
                onkeydown: move |event| { if event.key() != Key::Escape { event.stop_propagation(); } },
                oninput: move |event| draft.set(event.value()),
                onchange: move |_| {
                    if let Ok(value) = draft().parse::<i64>() {
                        let value = u32::try_from(value.clamp(0, 100)).expect("bounded percent");
                        if value != percent {
                            let opacity_u16 = u16::try_from((value * 65_535 + 50) / 100).expect("bounded opacity");
                            send_editor_command(&live_ink, EditorCommand::Layer(LayerCommand::SetOpacity { node, opacity_u16 }), error);
                        }
                        draft.set(value.to_string());
                    } else { draft.set(percent.to_string()); }
                }
            }
            span { "%" }
        }
    }
}

fn send_editor_command(
    live_ink: &UiHost,
    command: EditorCommand,
    mut error: Signal<Option<String>>,
) {
    error.set(live_ink.push_ui_editor_command(command).err());
}

#[component]
fn SharedCanvas() -> Element {
    let render = use_context::<UiSlots>().canvas;
    render()
}

fn panel_label(panel: PanelKind) -> &'static str {
    match panel {
        PanelKind::Canvas => "Canvas",
        PanelKind::Tools => "도구",
        PanelKind::Navigator => "내비게이터",
        PanelKind::Layers => "레이어",
        PanelKind::Brush => "세부 도구",
        PanelKind::ToolProperties => "도구 속성",
        PanelKind::BrushSizes => "크기 빠른 선택",
        PanelKind::Color => "색상",
        PanelKind::History => "히스토리",
        PanelKind::CanvasActions => "캔버스 작업",
        PanelKind::Viewport => "보기",
        PanelKind::QuickColors => "현재 색상과 최근 색상",
    }
}

fn panel_slug(panel: PanelKind) -> &'static str {
    match panel {
        PanelKind::Canvas => "canvas",
        PanelKind::Tools => "tools",
        PanelKind::Navigator => "navigator",
        PanelKind::Layers => "layers",
        PanelKind::Brush => "brush",
        PanelKind::ToolProperties => "tool-properties",
        PanelKind::BrushSizes => "brush-sizes",
        PanelKind::Color => "color",
        PanelKind::History => "history",
        PanelKind::CanvasActions => "canvas-actions",
        PanelKind::Viewport => "viewport",
        PanelKind::QuickColors => "quick-colors",
    }
}

fn brush_dot(size: i32) -> i32 {
    match size {
        1 => 3,
        3 => 5,
        5 => 7,
        10 => 9,
        20 => 12,
        40 => 15,
        80 => 18,
        _ => 21,
    }
}

fn send_dock_command(live_ink: &UiHost, command: DockCommand) {
    if live_ink
        .push_ui_editor_command(EditorCommand::Dock(command))
        .is_err()
    {
        eprintln!("native-ui event=dock-command-rejected reason=queue-full");
    }
}

fn protocol_status(event: Option<&EventEnvelope>) -> String {
    match event.map(|envelope| &envelope.event) {
        None => "Starting".to_owned(),
        Some(EditorEvent::CommandAccepted { command, revision }) => {
            format!("Accepted #{} at r{}", command.0, revision.0)
        }
        Some(EditorEvent::CommandRejected { command, reason }) => {
            format!("Rejected #{}: {}", command.0, reject_label(*reason))
        }
        Some(EditorEvent::ProjectionChanged { revision }) => {
            format!("Projection r{}", revision.0)
        }
        Some(EditorEvent::LayoutRecovered { revision, .. }) => {
            format!("Layout recovered at r{}", revision.0)
        }
    }
}

fn export_status_label(status: ExportStatus) -> String {
    match status {
        ExportStatus::Idle => String::new(),
        ExportStatus::Waiting { .. } => "저장 요청 대기".to_owned(),
        ExportStatus::Queued { .. } => "PNG 대기".to_owned(),
        ExportStatus::Running { .. } => "PNG 저장 중".to_owned(),
        ExportStatus::Current { .. } => "PNG ✓".to_owned(),
        ExportStatus::Failed { .. } => "PNG 실패".to_owned(),
    }
}

fn reject_label(reason: CommandRejectReason) -> &'static str {
    match reason {
        CommandRejectReason::StaleProjection { .. } => "화면 상태가 바뀌었습니다. 다시 시도하세요",
        CommandRejectReason::UnknownLayer => "레이어를 찾을 수 없습니다",
        CommandRejectReason::LayerLocked => "잠긴 레이어가 있습니다. 먼저 잠금을 해제하세요",
        CommandRejectReason::InvalidLayerMove => "그 위치로 레이어를 옮길 수 없습니다",
        CommandRejectReason::InvalidLayout => "패널 배치를 적용할 수 없습니다",
        CommandRejectReason::InvalidViewport => "보기 위치를 적용할 수 없습니다",
        CommandRejectReason::CommandQueueBusy => "저장 작업 중입니다. 잠시 후 다시 시도하세요",
        CommandRejectReason::WorkspaceFailed => "작업공간 오류로 명령을 적용하지 못했습니다",
        CommandRejectReason::UnsupportedCommand => "현재 상태에서는 사용할 수 없는 명령입니다",
        CommandRejectReason::RevisionExhausted => "상태 번호 한도에 도달했습니다",
    }
}

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod artwork_clipboard;
mod brush_preview;
#[cfg(windows)]
mod canvas_host;
mod color_panel;
mod color_picker;
#[cfg(windows)]
mod desktop_canvas;
mod desktop_shell;
mod dock_drag;
mod edit_gesture;
mod edit_worker;
#[cfg(windows)]
mod file_associations;
mod layer_drag;
mod layout_store;
mod live_ink;
mod native_canvas;
mod page_panel;
mod performance;
mod performance_probe;
mod performance_workload;
mod preview;
#[cfg(windows)]
mod render_host;
mod save_as;
#[cfg(windows)]
mod single_instance;
mod transform_gesture;
mod transform_panel;
mod transform_worker;
#[cfg(windows)]
mod updates;

use std::time::{Duration, Instant};
use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

use dioxus::prelude::*;
use live_ink::{CloseStatus, ExportStatus, LiveInkBridge};
use nyatidraw_api::{
    CommandRejectReason, DockAxis, DockCommand, DockNode, DrawingTool, EditorCommand, EditorEvent,
    EventEnvelope, GroupId, HistoryCommand, HistoryOperationLabel, LayerCommand, LayerId,
    LayerProjection, LayerProjectionKind, LayerTreeNodeId, PanelKind, ProjectCommand, ToolCommand,
    UiProjection, ViewportCommand, WorkspaceProjection,
};
use preview::LayerThumbnailFrame;

const INK_LAYER: LayerId = LayerId(1);
const ROOT_GROUP: GroupId = GroupId(100);

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
const STYLES: &str = include_str!("../assets/styles.css");
const STYLESHEET: Asset = asset!("/assets/styles.css");
const COLOR_STYLES: &str = include_str!("../assets/colors.css");
const COLOR_STYLESHEET: Asset = asset!("/assets/colors.css");

#[derive(Clone, Copy, Debug)]
struct NavigatorDrag {
    pointer_id: i32,
    origin_x: f64,
    origin_y: f64,
    width: u16,
    height: u16,
    last_sent: Option<[i32; 2]>,
    last_sent_at: Instant,
}

fn main() {
    #[cfg(windows)]
    velopack::VelopackApp::build()
        .on_after_install_fast_callback(|_| file_associations::register())
        .on_after_update_fast_callback(|_| file_associations::register())
        .on_before_uninstall_fast_callback(|_| file_associations::unregister())
        .set_auto_apply_on_startup(false)
        .run();
    PROCESS_START.get_or_init(Instant::now);
    println!(
        "native-shell event=launch profile={} lifecycle={:?}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        nyatidraw_editor::LifecycleState::Starting,
    );

    #[cfg(windows)]
    match single_instance::acquire_or_route() {
        Ok(single_instance::InstanceDisposition::Primary(guard)) => {
            desktop_shell::launch(app, guard);
        }
        Ok(single_instance::InstanceDisposition::SecondaryRouted) => {}
        Err(error) => {
            eprintln!("native-shell event=single-instance-failed error={error}");
            std::process::exit(1);
        }
    }

    #[cfg(not(windows))]
    desktop_shell::launch(app);
}

#[allow(clippy::too_many_lines)]
fn app() -> Element {
    let startup_diagnostics = use_hook(|| {
        let enabled = std::env::var_os("NAYATI_STARTUP_DIAGNOSTICS").is_some();
        if enabled {
            println!("desktop-startup event=root-initial-render");
        }
        enabled
    });
    let live_ink = use_context::<LiveInkBridge>();
    let notifier_ink = live_ink.clone();
    use_hook(move || notifier_ink.set_ui_notifier(dioxus_core::schedule_update()));
    #[cfg(windows)]
    {
        let canvas = try_use_context::<desktop_canvas::DesktopCanvasHandle>();
        use_hook(move || {
            if let Some(canvas) = canvas {
                canvas.start_render_worker();
            }
        });
    }
    #[cfg(windows)]
    {
        let mut update_status = use_context_provider(|| Signal::new(updates::status()));
        let current = updates::status();
        if *update_status.peek() != current {
            update_status.set(current);
        }
    }
    let initial_dock = live_ink.protocol_snapshot().0.dock;
    let mut ui_projection = use_signal(move || {
        let mut initial = initial_ui_projection();
        initial.dock = initial_dock;
        initial
    });
    color_panel::use_palette_preferences(ui_projection);
    let (authoritative, latest_event) = live_ink.protocol_snapshot();
    if latest_event.is_some()
        && authoritative.revision >= ui_projection.peek().revision
        && *ui_projection.peek() != authoritative
    {
        ui_projection.set(authoritative);
    }
    dock_drag::use_dock_drag(live_ink.clone());
    use_context_provider(|| transform_panel::TransformPanelOpen(Signal::new(false)));
    let page_panel::PagePanelOpen(page_open) =
        use_context_provider(|| page_panel::PagePanelOpen(Signal::new(false)));
    let mut page_shortcuts = page_open;
    let mut navigator_drag = use_context_provider(|| Signal::new(Option::<NavigatorDrag>::None));
    let projected_revision = ui_projection.read().revision.0;
    let dock = ui_projection.read().dock.clone();
    let dirty = ui_projection.read().dirty;
    let document_title = ui_projection.read().document_title.clone();
    let export_status = live_ink.export_status_snapshot();
    let close_status = live_ink.close_status();
    let activation_notice = live_ink.activation_notice_snapshot();
    let layout_notice = live_ink.layout_notice();
    let has_layout_notice = layout_notice.is_some();
    let edit = ui_projection.read().edit.clone();
    let picker = live_ink.picker_snapshot();
    let shortcut_ink = live_ink.clone();
    let navigator_move_ink = shortcut_ink.clone();
    let navigator_up_ink = shortcut_ink.clone();
    let rejection = latest_event.as_ref().and_then(|event| match &event.event {
        EditorEvent::CommandRejected { command, reason } => Some((*command, *reason)),
        _ => None,
    });

    rsx! {
        // The native shell keeps the inline fallback below because its private
        // network provider does not guarantee dioxus:// asset fetches.
        document::Link { rel: "stylesheet", href: STYLESHEET }
        style { {STYLES} }
        document::Link { rel: "stylesheet", href: COLOR_STYLESHEET }
        style { {COLOR_STYLES} }
        main {
            onmounted: move |_| {
                focus_editor_shortcuts();
                if startup_diagnostics {
                    println!("desktop-startup event=root-dom-mounted");
                }
            },
            class: if has_layout_notice { "app-shell layout-has-notice" } else { "app-shell" },
            "data-dock-revision": "{projected_revision}",
            tabindex: 0,
            onpointermove: move |event| {
                continue_navigator_drag(navigator_drag, &navigator_move_ink, &event.data());
            },
            onpointerup: move |event| {
                finish_navigator_drag(navigator_drag, &navigator_up_ink, &event.data());
            },
            onpointercancel: move |event| {
                if navigator_drag
                    .read()
                    .as_ref()
                    .is_some_and(|drag| drag.pointer_id == event.data().pointer_id())
                {
                    navigator_drag.set(None);
                }
            },
            onkeydown: move |event| {
                let key = event.key().to_string();
                if page_shortcuts() {
                    if key == "Escape" { page_shortcuts.set(false); }
                    event.prevent_default();
                    event.stop_propagation();
                    return;
                }
                if key == "Escape" && event.is_auto_repeating() {
                    event.prevent_default();
                    return;
                }
                if handle_editor_shortcut(&shortcut_ink, &key, event.modifiers().shift(), event.modifiers().ctrl()) {
                    event.prevent_default();
                    return;
                }
                let panel = match key.as_str() {
                    "1" | "F6" => Some(PanelKind::Canvas),
                    "2" => Some(PanelKind::Tools),
                    "3" => Some(PanelKind::Brush),
                    "4" => Some(PanelKind::Navigator),
                    "5" => Some(PanelKind::Color),
                    "6" => Some(PanelKind::Layers),
                    "7" => Some(PanelKind::History),
                    "Escape" => {
                        navigator_drag.set(None);
                        None
                    }
                    // F6 reveals the keyboard-reachable canvas panel;
                    // native drawing input has its own focus route.
                    _ => None,
                };
                if let Some(panel) = panel {
                    send_dock_command(&shortcut_ink, DockCommand::ActivatePanel(panel));
                }
            },
            ActionBar { ui_projection, export_status }
            if let Some(notice) = layout_notice {
                div { class: "layout-notice", role: "status", "{notice}" }
            }
            section {
                class: "workspace",
                aria_label: "Editor workspace. F6 returns to the drawing canvas.",
                title: "{document_title} · r{projected_revision} · {protocol_status(latest_event.as_ref())}",
                DockNodeView { node: dock.root().clone(), ui_projection }
            }
            if dirty { span { class: "unsaved-dot", title: "저장되지 않은 변경", "•" } }
            if edit.busy {
                aside { class: "edit-status", role: "status", aria_live: "polite",
                    span { "작업 처리 중…" }
                }
            }
            if let Some(reason) = &edit.error {
                TransientNotice { key: "edit-{reason}", message: format!("편집 실패: {reason}") }
            }
            if let Some((command, reason)) = rejection {
                TransientNotice { key: "reject-{command.0}", message: reject_label(reason).to_owned() }
            }
            if let Some(notice) = activation_notice {
                TransientNotice { key: "notice-{notice}", message: notice }
            }
            if page_open() { page_panel::PagePanel { ui_projection } }
            if let Some(snapshot) = picker { {picker_loupe(snapshot)} }
            CloseProgress { status: close_status }
        }
    }
}

/// Read-only feedback: raw input and release/cancel remain in the native actor.
fn picker_loupe(snapshot: native_canvas::PickerSnapshot) -> Element {
    let [x, y] = snapshot.cursor_css;
    let top = if y >= 190.0 { y - 180.0 } else { y + 26.0 };
    let position = format!(
        "left:clamp(6px,{}px,calc(100vw - 138px));top:clamp(6px,{top}px,calc(100vh - 160px))",
        x - 66.0
    );
    let candidate = snapshot.frame.as_ref().and_then(|frame| frame.candidate);
    let swatch = candidate.map_or_else(String::new, |[r, g, b, _]| {
        format!("background:rgb({r} {g} {b})")
    });
    let status = if snapshot.pending {
        "…"
    } else if snapshot.error.is_some() && snapshot.frame.is_none() {
        "채취 실패"
    } else if candidate.is_none() {
        "투명"
    } else {
        ""
    };
    rsx! {
        aside { class: "picker-loupe", style: position, aria_label: "스포이트 확대 미리보기",
            div { class: "picker-pixels",
                if let Some(frame) = snapshot.frame {
                    img { src: frame.data_uri.to_string(), alt: "커서 주변 픽셀", draggable: "false", "data-pixel-size": "{frame.size}" }
                }
                span { class: "picker-center" }
            }
            div { class: "picker-candidate",
                span { class: "picker-swatch", style: swatch }
                span { "{status}" }
            }
        }
    }
}

/// Dismiss presentation only. Save/export failures retain their retry/recovery state.
#[component]
fn TransientNotice(message: String) -> Element {
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

#[component]
fn CloseProgress(status: CloseStatus) -> Element {
    #[cfg(windows)]
    let host = try_use_context::<desktop_canvas::DesktopCanvasHandle>();
    #[cfg(windows)]
    let reopen_host = host.clone();
    if matches!(status, CloseStatus::Open | CloseStatus::Ready) {
        return rsx! {};
    }
    let title = match &status {
        CloseStatus::Exporting => "PNG 저장을 마친 뒤 종료합니다",
        CloseStatus::Failed { .. } => "저장을 확인해주세요",
        _ => "작품을 저장하고 있습니다",
    };
    rsx! {
        div { class: "close-backdrop",
            section { class: "close-dialog", role: "dialog", aria_modal: "true", aria_label: "{title}",
                onmounted: move |_| println!("native-shell event=close-dialog-mounted"),
                h2 { "{title}" }
                if let CloseStatus::Failed { project_saved, project_path } = status {
                    p { role: "alert",
                        if project_saved { "프로젝트는 저장됐지만 PNG 저장에 실패했습니다. 프로젝트를 다시 열고 저장을 재시도할 수 있습니다." }
                        else { "일부 변경이 저장되지 않았을 수 있습니다. 다시 열면 마지막으로 저장된 작품을 확인할 수 있습니다." }
                    }
                    p { class: "recovery-path", "{project_path}" }
                    button { autofocus: true, onclick: move |_| {
                        #[cfg(windows)]
                        if let Some(host) = &reopen_host { host.reopen_after_close_failure(); }
                    }, "프로젝트 다시 열기" }
                    button { onclick: move |_| {
                        #[cfg(windows)]
                        if let Some(host) = &host { host.confirm_failed_close(); }
                    }, "오류를 확인하고 종료" }
                } else {
                    p { role: "status", aria_live: "polite", "저장이 끝나면 자동으로 종료됩니다." }
                    progress { aria_label: "저장 중" }
                }
            }
        }
    }
}

fn handle_editor_shortcut(live_ink: &LiveInkBridge, key: &str, shift: bool, ctrl: bool) -> bool {
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

fn initial_ui_projection() -> UiProjection {
    let mut projection = UiProjection::empty();
    projection.document_title = "Untitled".into();
    projection.workspace = WorkspaceProjection::Ready;
    projection.active_layer = Some(INK_LAYER);
    projection.layers = vec![LayerProjection {
        alpha_locked: false,
        clip_to_below: false,
        blend_mode: nyatidraw_api::LayerBlendMode::Normal,
        id: LayerTreeNodeId::Raster(INK_LAYER),
        parent: ROOT_GROUP,
        index: 0,
        depth: 1,
        kind: LayerProjectionKind::Raster,
        name: "Layer 1".into(),
        visible: true,
        reference: false,
        locked: false,
        opacity_u16: u16::MAX,
    }];
    projection
}

#[component]
fn ActionBar(ui_projection: Signal<UiProjection>, export_status: ExportStatus) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let reset_layout_ink = live_ink.clone();
    let mut menu_open = use_signal(|| false);
    let save_ink = live_ink.clone();
    let retry_ink = live_ink.clone();
    let undo_ink = live_ink.clone();
    let redo_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let top = ui_projection.read().dock.top().to_vec();

    rsx! {
        nav { class: "commandbar", aria_label: "주요 명령",
            button { class: "command hamburger-button", title: "메뉴", aria_label: "메뉴", aria_expanded: "{menu_open}",
                onclick: move |_| menu_open.toggle(),
                span { class: "hamburger", i {}, i {}, i {} }
            }
            FileButtons {}
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
            button { class: "command compact", title: "실행 취소 (Z)", onclick: move |_| {
                send_editor_command(&undo_ink, EditorCommand::History(HistoryCommand::Undo), error);
            }, UiIcon { name: "undo" } span { class: "shortcut", "Z" } }
            button { class: "command compact fixed-end", title: "다시 실행 (Shift+Z)", onclick: move |_| {
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
                    FileButtons { extended: true }
                    span { class: "divider" }
                    ClipboardActions { ui_projection }
                    span { class: "divider" }
                    button { class: "command", onclick: move |_| {
                        send_dock_command(&reset_layout_ink, DockCommand::ResetToSafeDefault);
                        menu_open.set(false);
                    }, "기본 화면 배치" }
                    div { class: "menu-line-spacer" }
                    UpdateControl {}
                }
            }
        }
    }
}

#[component]
fn ClipboardActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
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
        button { class: "command", disabled: busy, title: "새 레이어로 붙여넣기 (Ctrl+V)", onclick: move |_| {
            send_editor_command(&live_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::PasteSelection), error);
        }, span { "붙여넣기" } span { class: "shortcut", "Ctrl V" } }
    }
}

#[component]
fn UpdateControl() -> Element {
    #[cfg(windows)]
    {
        let host = try_use_context::<desktop_canvas::DesktopCanvasHandle>();
        let status = use_context::<Signal<updates::Status>>().read().clone();
        let (label, title, disabled) = match &status {
            updates::Status::Development => return rsx! {},
            updates::Status::Checking => ("업데이트 확인 중", "업데이트 확인 중".to_owned(), true),
            updates::Status::Downloading => (
                "업데이트 받는 중",
                "작업을 계속할 수 있습니다".to_owned(),
                true,
            ),
            updates::Status::Ready(version) => (
                "저장 후 업데이트",
                format!("{version} 적용 후 현재 그림을 다시 엽니다"),
                false,
            ),
            updates::Status::Current(version) => {
                ("업데이트 확인", format!("현재 버전 {version}"), false)
            }
            updates::Status::Failed(error) => ("업데이트 재시도", error.clone(), false),
        };
        return rsx! { button { class: "command compact", title, disabled,
            onclick: move |_| {
                if matches!(updates::status(), updates::Status::Ready(_)) {
                    if let Some(host) = &host { host.request_close(); }
                } else { updates::check(); }
            }, "{label}"
        } };
    }
    #[cfg(not(windows))]
    rsx! {}
}

#[component]
fn FileButtons(#[props(default = false)] extended: bool) -> Element {
    #[cfg(windows)]
    {
        let host = try_use_context::<desktop_canvas::DesktopCanvasHandle>();
        let live_ink = use_context::<LiveInkBridge>();
        let mut busy = use_signal(|| false);
        let window = dioxus_desktop::window().window.clone();
        return rsx! {
            for file_action in (0_u8..=2).filter(|action| extended || *action == 0) {
                button {
                    class: "command", disabled: busy() || host.is_none(),
                    title: match file_action { 1 => "새 그림의 저장 위치 선택", 2 => "현재 그림과 모든 실행 취소 기록을 다른 이름으로 저장", _ => "PNG 또는 NyatiDraw 프로젝트 열기" },
                    onclick: {
                        let host = host.clone();
                        let live_ink = live_ink.clone();
                        let window = window.clone();
                        move |_| {
                            let host = host.clone();
                            let live_ink = live_ink.clone();
                            let window = window.clone();
                            async move {
                                if busy() { return; }
                                busy.set(true);
                                let dialog = rfd::AsyncFileDialog::new().set_parent(window.as_ref());
                                let selected = if file_action != 0 {
                                    dialog.set_title(if file_action == 2 { "다른 이름으로 저장" } else { "새 그림 저장 위치" }).add_filter("NyatiDraw 프로젝트", &["ntdr"])
                                        .set_file_name("새 그림.ntdr").save_file().await
                                } else {
                                    dialog.set_title("그림 열기").add_filter("그림 / 프로젝트", &["png", "ntdr"])
                                        .pick_file().await
                                };
                                if let Some(file) = selected {
                                    let mut path = file.path().to_path_buf();
                                    if file_action != 0 && path.extension().is_none() { path.set_extension("ntdr"); }
                                    let result = if file_action != 0 && !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("ntdr")) {
                                        Err("새 그림은 .ntdr 확장자로 저장해주세요".into())
                                    } else if file_action != 0 && (path.exists() || path.with_extension("png").exists()) {
                                        Err("같은 이름의 그림이 있습니다. 다른 이름을 선택하거나 기존 그림을 열어주세요".into())
                                    } else if let Some(host) = host {
                                        if file_action == 2 { host.save_as(path) } else { host.open_path(path) }
                                    } else { Err("캔버스가 아직 준비되지 않았습니다".into()) };
                                    if let Err(error) = result { live_ink.publish_activation_notice(error); }
                                }
                                busy.set(false);
                            }
                        }
                    },
                    if file_action == 2 { span { "다른 이름으로 저장" } }
                    else if file_action == 1 { span { "새 그림" } }
                    else { UiIcon { name: "folder" } span { "열기" } }
                }
            }
        };
    }
    #[cfg(not(windows))]
    rsx! { button { disabled: true, "열기 · Windows 전용" } }
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
    let live_ink = use_context::<LiveInkBridge>();
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
                button { class: "command", title: "흰 배경 추가", onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } span { class: "command-label", "흰 배경" } }
                button { class: if ui_projection.read().edit.transform.is_some() { "command active" } else { "command" }, title: "선택 영역 변형 · 먼저 영역을 선택하세요", disabled: !ui_projection.read().edit.has_selection || ui_projection.read().edit.busy || ui_projection.read().edit.transform.is_some(), onclick: move |_| {
                    page_open.set(false);
                    transform_open.set(true);
                    focus_editor_shortcuts();
                    send_editor_command(&transform_ink, EditorCommand::Edit(nyatidraw_api::EditCommand::FreeTransform(nyatidraw_api::TransformCommand::Begin)), error);
                },
                    UiIcon { name: "transform" } span { class: "command-label", "변형" }
                }
                button { class: "command", title: "페이지 크기 및 선택 영역에 맞추기", disabled: ui_projection.read().edit.busy || ui_projection.read().edit.transform.is_some(), onclick: move |_| {
                    transform_open.set(false);
                    page_open.set(true);
                }, span { class: "command-label", "페이지" } }
                button { class: "command", title: "현재 조작 취소 / 선택 해제 (Esc)", disabled: !page_open() && !ui_projection.read().edit.can_cancel && (!ui_projection.read().edit.has_selection || ui_projection.read().edit.busy),
                    onclick: move |_| {
                        if page_open() { page_open.set(false); }
                        else {
                            focus_editor_shortcuts();
                            send_editor_command(&cancel_ink, EditorCommand::Tool(ToolCommand::CancelGesture), error);
                        }
                    }, span { class: "command-label", "취소" } span { class: "shortcut", "Esc" }
                }
    }
}

fn focus_editor_shortcuts() {
    // Starting/ending an operation can disable or remove its focused button.
    // Put focus on the keyboard scope before that change, not on document.body.
    let _ =
        document::eval("document.querySelector('main.app-shell')?.focus({ preventScroll: true });");
}

fn contain_overlay_focus(id: &'static str) {
    // IDs are internal constants; script owns listeners only for the mounted overlay.
    let _ = document::eval(&format!(
        "const overlay = document.getElementById('{id}');\n{}",
        include_str!("overlay_focus.js")
    ));
}

#[component]
fn ViewportActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    let fit_ink = live_ink.clone();
    let zoom_out_ink = live_ink.clone();
    let zoom_in_ink = live_ink.clone();
    let rotate_ink = live_ink.clone();
    let reset_ink = live_ink.clone();
    let mirror_ink = live_ink;
    let viewport = ui_projection.read().viewport;
    let zoom = viewport.zoom_ppm / 10_000;
    let rotation = viewport.rotation_millidegrees / 1_000;
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
                button { class: "command compact", title: "45도 회전", onclick: move |_| {
                    send_editor_command(&rotate_ink, EditorCommand::Viewport(ViewportCommand::RotateQuarterSteps(1)), error);
                }, UiIcon { name: "rotate" } }
                button { class: "command compact toolbar-value", title: "회전 초기화 · 화면 중심과 확대 배율 유지", aria_label: "회전 초기화, 현재 {rotation}도",
                    onclick: move |_| send_editor_command(&reset_ink, EditorCommand::Viewport(ViewportCommand::ResetRotation), error),
                    "{rotation}°"
                }
                button { class: "command compact", title: "보기 좌우 반전 · 그림과 PNG는 바뀌지 않습니다", aria_label: "보기 좌우 반전", aria_pressed: "{viewport.mirrored_horizontal}",
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
    let live_ink = use_context::<LiveInkBridge>();
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
        WorkspaceProjection::Error { summary } => rsx! {
            div { class: "workspace-state error", role: "alert",
                strong { "Project could not be opened" }
                p { "{summary}" }
            }
        },
        WorkspaceProjection::Recovery { summary } => rsx! {
            div { class: "workspace-state recovery",
                strong { "Recovered a safe workspace" }
                p { "{summary}" }
            }
        },
        WorkspaceProjection::Ready => panel_ready_contents(panel, ui_projection),
    }
}

fn panel_ready_contents(panel: PanelKind, ui_projection: Signal<UiProjection>) -> Element {
    match panel {
        PanelKind::Canvas => rsx! {
            div {
                class: "canvas-panel",
                tabindex: 0,
                role: "application",
                aria_label: "Drawing canvas. Use F6 to reveal this panel, then Tab to this focus target.",
                div { class: "canvas-frame", SharedCanvas {} }
            }
        },
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        button {
            class: if active { "tool active" } else { "tool" },
            disabled: tool.is_none(),
            title: if tool == Some(DrawingTool::Eyedropper) { "스포이트 (C) · Alt+클릭: 보이는 그림에서 임시 색 채취" } else { "{label} ({shortcut})" },
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
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    let tool = ui_projection.read().drawing_tool;
    let pencil_template = ui_projection.read().pencil_template;
    let name = match tool {
        DrawingTool::Pencil => "기본 연필",
        DrawingTool::Pen => "기본 펜",
        DrawingTool::Brush => "소프트 브러시",
        DrawingTool::Eraser => "기본 지우개",
        DrawingTool::MoveSelection => "자유 변형",
        DrawingTool::Move => "화면 이동",
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
                    button { class: if template == pencil_template { "subtool selected" } else { "subtool" },
                        title: name, aria_pressed: template == pencil_template,
                        onclick: { let ink = live_ink.clone(); move |_| send_editor_command(&ink, EditorCommand::Tool(ToolCommand::SelectPencilTemplate(template)), error) },
                        span { class: "subtool-name", "{name}" }
                    }
                }
            } else {
            SubtoolButton { name, tool: Some(tool), active: true }
            }
            if tool == DrawingTool::MoveSelection {
                button { class: "subtool unavailable", disabled: true, "메시 변형 · 준비 중" }
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
fn BrushPressureControl(size_axis: bool, enabled: bool, minimum_u16: u16) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
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
                    disabled: !enabled, value: "{draft}", aria_label: "{axis} 최소 필압 비율 %",
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        button {
            class: if active { "subtool selected" } else { "subtool" },
            disabled: tool.is_none(),
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
    let tolerance_ink = live_ink.clone();
    let tolerance_number_ink = live_ink.clone();
    let mode_ink = live_ink.clone();
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
        _ => "선택 영역에서 끌어 현재 색 → 투명 그라데이션을 만듭니다.",
    };
    rsx! {
        div { class: "edit-tool-properties", "aria-description": help,
            if matches!(tool, DrawingTool::Lasso | DrawingTool::RectangleSelection) {
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
            if matches!(tool, DrawingTool::Wand | DrawingTool::Fill | DrawingTool::Eyedropper) {
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
            if tool == DrawingTool::Fill { FillBoundaryControls { ui_projection } }
        }
    }
}

#[component]
fn FillBoundaryControls(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let gap_ink = live_ink.clone();
    let expand_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let fill = ui_projection.read().edit_settings.fill;
    rsx! {
        label { class: "property-row", title: "참조 경계의 형태학적 닫기 반경입니다. 0은 끄기이며, 같은 폭의 모든 틈을 막는다는 뜻은 아닙니다.",
            span { "틈 닫기" }
            EditNumberInput { value: fill.gap_close_px, min: 0, max: 8, label: "틈 닫기 반경 문서 픽셀",
                on_commit: move |value| {
                    send_editor_command(&gap_ink, EditorCommand::Tool(ToolCommand::SetFillGapClose(value)), error);
                }
            }
            span { "px" }
        }
        label { class: "property-row", title: "선 경계 안쪽으로 맨해튼 거리 기준으로 받쳐 칠합니다. 다른 연결 영역이나 현재 선택 밖으로는 넘어가지 않습니다.",
            span { "채우기 확장" }
            EditNumberInput { value: fill.expand_px, min: 0, max: 64, label: "채우기 확장 문서 픽셀",
                on_commit: move |value| {
                    send_editor_command(&expand_ink, EditorCommand::Tool(ToolCommand::SetFillExpansion(value)), error);
                }
            }
            span { "px" }
        }
        label { style: "display:flex;align-items:center;gap:5px", title: "참조 선 아래를 최소 한 픽셀 받쳐 칠해 반투명 경계의 빈틈을 줄입니다. 초과 샘플링은 하지 않습니다.",
            input { r#type: "checkbox", style: "width:auto", checked: fill.antialias, aria_label: "채우기 경계 안티앨리어싱",
                onchange: move |event| {
                    send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetFillAntialias(event.checked())), error);
                }
            }
            "AA 경계 받침"
        }
    }
}

#[component]
fn SelectionActions(ui_projection: Signal<UiProjection>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    let mut radius = use_signal(|| 1_u8);
    let busy = ui_projection.read().edit.busy;
    let has_selection = ui_projection.read().edit.has_selection;
    rsx! { div { class: "selection-actions",
        div { class: "selection-toolbar", role: "group", aria_label: "선택 영역 명령",
        for (label, icon, title, needs_selection, command) in [
            ("전체 선택", "rectangle-selection", "전체 선택 (Ctrl+A) · 페이지와 현재 레이어·선택의 유한 범위", false, nyatidraw_api::EditCommand::SelectAll),
            ("선택 반전", "invert-selection", "선택 반전 (Ctrl+Shift+I)", false, nyatidraw_api::EditCommand::InvertSelection),
            ("선택 해제", "deselect", "선택만 해제 (Esc)", true, nyatidraw_api::EditCommand::ClearSelection),
            ("선택 픽셀 삭제", "trash", "선택 픽셀 삭제 (Delete) · 현재 레이어만", true, nyatidraw_api::EditCommand::DeleteSelectedPixels),
            ("그림에서 선택", "select-alpha", "현재 레이어의 불투명 픽셀 선택 · 레이어 불투명도와 다른 레이어는 반영하지 않음", false, nyatidraw_api::EditCommand::SelectLayerAlpha),
        ] {
            button { title, aria_label: label, disabled: busy || (needs_selection && !has_selection), onclick: {
                let ink = live_ink.clone();
                move |_| send_editor_command(&ink, EditorCommand::Edit(command.clone()), error)
            }, UiIcon { name: icon } }
        }
        }
        label { class: "property-row", title: "선택 확장·축소 반경, 문서 픽셀 단위",
            span { "선택 경계" }
            EditNumberInput { value: radius(), min: 1, max: 64, label: "선택 확장 축소 반경 문서 픽셀",
                on_commit: move |value| radius.set(value)
            }
            span { "px" }
        }
        div { class: "selection-morph-actions",
        for (label, icon, grow) in [("확장", "plus", true), ("축소", "minus", false)] {
            button { title: "선택 영역 {label}", disabled: busy || !has_selection, onclick: {
                let ink = live_ink.clone();
                move |_| {
                    let command = if grow { nyatidraw_api::EditCommand::GrowSelection { radius: radius() } }
                        else { nyatidraw_api::EditCommand::ShrinkSelection { radius: radius() } };
                    send_editor_command(&ink, EditorCommand::Edit(command), error);
                }
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
) -> Element {
    let mut draft = use_signal(|| value.to_string());
    let mut editing = use_signal(|| false);
    use_effect(use_reactive((&value,), move |(value,)| {
        if !*editing.peek() {
            draft.set(value.to_string());
        }
    }));
    rsx! {
        input { class: "property-number", r#type: "number", min: "{min}", max: "{max}", step: "1", value: "{draft}", aria_label: "{label}",
            onfocus: move |_| editing.set(true),
            oninput: move |event| draft.set(event.value()),
            onblur: move |_| {
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
    let live_ink = use_context::<LiveInkBridge>();
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
    let live_ink = use_context::<LiveInkBridge>();
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
                            last_sent_at: Instant::now(),
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

fn send_navigator_center(live_ink: &LiveInkBridge, page: [i32; 2]) -> bool {
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
    live_ink: &LiveInkBridge,
    data: &dioxus::html::events::PointerData,
) {
    let Some(mut drag) = navigator_drag.read().as_ref().copied() else {
        return;
    };
    if data.pointer_id() != drag.pointer_id {
        return;
    }
    let now = Instant::now();
    if now.duration_since(drag.last_sent_at) < Duration::from_millis(8) {
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
    live_ink: &LiveInkBridge,
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
    let live_ink = use_context::<LiveInkBridge>();
    let add_raster_ink = live_ink.clone();
    let duplicate_ink = live_ink.clone();
    let add_group_ink = live_ink.clone();
    let white_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let layers = ui_projection.read().layers.clone();
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
                button { title: "래스터 레이어 추가", onclick: move |_| {
                    send_editor_command(&add_raster_ink, EditorCommand::Layer(LayerCommand::AddRaster), error);
                }, UiIcon { name: "plus" } }
                button { title: "선택 래스터 레이어 복제", disabled: active.is_none() || group_settings().is_some(), onclick: move |_| {
                    if let Some(layer) = active {
                        send_editor_command(&duplicate_ink, EditorCommand::Layer(LayerCommand::DuplicateRaster(layer)), error);
                    }
                }, UiIcon { name: "copy" } }
                button { title: "그룹 추가", onclick: move |_| {
                    send_editor_command(&add_group_ink, EditorCommand::Layer(LayerCommand::AddGroup), error);
                }, UiIcon { name: "folder" } }
                button { title: "흰 배경 추가", onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } }
                if group_settings().is_none() { layer_drag::LayerMoveButtons { projection: ui_projection, error } }
            }
            if let Some(layer) = settings_layer {
                if group_settings().is_some() {
                    div { class: "group-settings-label",
                        span { "그룹 설정 · {layer.name}" }
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
                        rsx! { LayerRow { key: "{layer.id:?}", layer, thumbnail, active, solo_node, error, collapsed_groups, group_settings } }
                    }
                }
            }
            if let Some(message) = error.read().as_ref() { div { class: "command-error", role: "alert", "{message}" } }
            div { class: "layers-footer", button { disabled: settings_id.is_none() || (group_settings().is_none() && active_locked), title: "설정 대상 레이어/그룹 삭제 (Undo로 복원)", onclick: move |_| {
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
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let active_ink = live_ink.clone();
    let visibility_ink = live_ink.clone();
    let thumbnail_ink = live_ink.clone();
    let rename_ink = live_ink.clone();
    let mut rename_error = error;
    let mut renaming = use_signal(|| false);
    let solo_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let id = layer.id;
    let is_active = matches!(id, LayerTreeNodeId::Raster(layer_id) if active == Some(layer_id));
    let is_group = layer.kind == LayerProjectionKind::Group;
    let is_collapsed =
        matches!(id, LayerTreeNodeId::Group(group) if collapsed_groups.read().contains(&group));
    let opacity = (u32::from(layer.opacity_u16) * 100 + 32_767) / u32::from(u16::MAX);
    let indent = u32::from(layer.depth.saturating_sub(1)) * 9;
    let layer_key = match id {
        LayerTreeNodeId::Raster(id) => format!("r-{}", id.0),
        LayerTreeNodeId::Group(id) => format!("g-{}", id.0),
    };
    rsx! {
        div { class: if is_group { "layer-row group" } else if is_active { "layer-row active" } else { "layer-row" }, style: "padding-left:{indent}px",
            "data-layer-key": "{layer_key}", "data-parent": "{layer.parent.0}", "data-index": "{layer.index}",
            "data-depth": "{layer.depth}",
            button { class: "layer-eye", title: "보기/숨기기", onclick: move |_| {
                let visible = !layer.visible;
                send_editor_command(&visibility_ink, EditorCommand::Layer(LayerCommand::SetVisibility { node: id, visible }), error);
            }, span { class: if layer.visible { "eye-mark" } else { "eye-mark hidden" }, UiIcon { name: "eye" } } }
            button {
                class: if solo_node == Some(id) { "layer-solo active" } else { "layer-solo" },
                title: if solo_node == Some(id) { "레이어만 보기 해제" } else { "이 레이어만 보기" },
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
            span { class: if is_group { "layer-thumb group-thumb" } else { "layer-thumb raster-thumb" },
                title: "페이지 내 원본 픽셀 미리보기 · 끌어서 레이어 순서 또는 그룹 변경",
                onclick: move |_| {
                    if let LayerTreeNodeId::Raster(layer_id) = id {
                        group_settings.set(None);
                        send_editor_command(&thumbnail_ink, EditorCommand::Layer(LayerCommand::SetActive(layer_id)), error);
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
            div { class: "layer-meta", onclick: move |_| {
                if let LayerTreeNodeId::Raster(layer_id) = id {
                    group_settings.set(None);
                    send_editor_command(&active_ink, EditorCommand::Layer(LayerCommand::SetActive(layer_id)), error);
                } else if let LayerTreeNodeId::Group(group) = id {
                    group_settings.set(Some(group));
                }
            },
                input {
                    class: "layer-name",
                    aria_label: "레이어 이름",
                    title: "끌어서 이동 · 더블클릭하여 이름 변경",
                    readonly: !renaming(),
                    ondoubleclick: move |_| renaming.set(true),
                    onblur: move |_| renaming.set(false),
                    value: "{layer.name}",
                    maxlength: "128",
                    spellcheck: "false",
                    onkeydown: move |event| event.stop_propagation(),
                    onchange: move |event| {
                        let name = event.value();
                        if name.trim().is_empty() {
                            rename_error.set(Some("레이어 이름은 비워둘 수 없습니다".into()));
                        } else {
                            send_editor_command(
                                &rename_ink,
                                EditorCommand::Layer(LayerCommand::Rename { node: id, name }),
                                rename_error,
                            );
                        }
                    }
                }
                span { class: "layer-detail",
                    if layer.locked { span { title: "픽셀 잠김", UiIcon { name: "lock" } } }
                    if layer.reference { "참조 · " }
                    if layer.alpha_locked { "α · " }
                    if layer.clip_to_below { "↳ · " }
                    if layer.blend_mode == nyatidraw_api::LayerBlendMode::Multiply { "곱하기 · " }
                    if is_group { "그룹 · " }
                    "{opacity}%"
                }
            }
            button {
                class: "layer-delete", title: "삭제 (Undo로 복원)", aria_label: "{layer.name} 삭제",
                disabled: layer.locked,
                onclick: move |_| send_editor_command(&delete_ink, EditorCommand::Layer(LayerCommand::Delete(id)), error),
                "×"
            }
        }
    }
}

#[component]
fn LayerStyleControls(layer: LayerProjection, error: Signal<Option<String>>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let alpha_ink = live_ink.clone();
    let clip_ink = live_ink.clone();
    let reference_ink = live_ink.clone();
    let lock_ink = live_ink.clone();
    let id = layer.id;
    rsx! { div { class: "layer-style-controls", aria_label: "{layer.name} 합성 설정",
        div { class: "layer-style-row",
        select { class: "layer-blend", aria_label: "레이어 합성 모드", title: "{layer.name} 합성 모드",
            value: if layer.blend_mode == nyatidraw_api::LayerBlendMode::Multiply { "multiply" } else { "normal" },
            onkeydown: move |event| event.stop_propagation(),
            onchange: move |event| {
                let blend_mode = if event.value() == "multiply" { nyatidraw_api::LayerBlendMode::Multiply } else { nyatidraw_api::LayerBlendMode::Normal };
                send_editor_command(&live_ink, EditorCommand::Layer(LayerCommand::SetBlendMode { node: id, blend_mode }), error);
            },
            option { value: "normal", "표준" }
            option { value: "multiply", "곱하기" }
        }
        div { class: "layer-style-flags", role: "group", aria_label: "레이어 속성",
        if let LayerTreeNodeId::Raster(layer_id) = id {
            button { title: if layer.locked { "픽셀 잠금 해제" } else { "픽셀 잠금" }, aria_label: "픽셀 잠금", aria_pressed: "{layer.locked}",
                onclick: move |_| send_editor_command(&lock_ink, EditorCommand::Layer(LayerCommand::SetLocked { layer: layer_id, locked: !layer.locked }), error),
                if layer.locked { UiIcon { name: "lock" } } else { UiIcon { name: "unlock" } }
            }
            button { title: "투명도 잠금", aria_label: "투명도 잠금", aria_pressed: "{layer.alpha_locked}",
                onclick: move |_| send_editor_command(&alpha_ink, EditorCommand::Layer(LayerCommand::SetAlphaLocked { layer: layer_id, alpha_locked: !layer.alpha_locked }), error), "α"
            }
        }
        button { title: "아래 레이어에 클리핑", aria_label: "아래 레이어에 클리핑", aria_pressed: "{layer.clip_to_below}",
            onclick: move |_| send_editor_command(&clip_ink, EditorCommand::Layer(LayerCommand::SetClipToBelow { node: id, clip_to_below: !layer.clip_to_below }), error), "↳"
        }
        if let LayerTreeNodeId::Raster(layer_id) = id {
            button { title: "선택·채우기 참조 레이어 (PNG에는 영향 없음)", aria_label: "참조 레이어", aria_pressed: "{layer.reference}",
                onclick: move |_| send_editor_command(&reference_ink, EditorCommand::Layer(LayerCommand::SetReference { layer: layer_id, reference: !layer.reference }), error),
                UiIcon { name: "reference" }
            }
        }
        button { class: "unavailable", disabled: true, title: "레이어 색상화 · 준비 중", aria_label: "레이어 색상화 · 준비 중", span { class: "layer-color-chip" } }
        }
        }
        LayerOpacityControl { key: "{id:?}", node: id, opacity_u16: layer.opacity_u16, error }
    } }
}

#[component]
fn LayerOpacityControl(
    node: LayerTreeNodeId,
    opacity_u16: u16,
    error: Signal<Option<String>>,
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
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
    live_ink: &LiveInkBridge,
    command: EditorCommand,
    mut error: Signal<Option<String>>,
) {
    if live_ink.push_ui_editor_command(command).is_err() {
        error.set(Some(
            "캔버스가 처리 중입니다. 잠시 후 다시 시도하세요".into(),
        ));
    } else {
        error.set(None);
    }
}

#[component]
fn SharedCanvas() -> Element {
    let startup_diagnostics = use_hook(|| std::env::var_os("NAYATI_STARTUP_DIAGNOSTICS").is_some());
    let observer_id = use_hook(|| {
        static NEXT_OBSERVER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_OBSERVER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if startup_diagnostics {
            println!("desktop-layout event=canvas-component-created observer={id}");
        }
        id
    });
    use_drop(move || {
        if startup_diagnostics {
            println!("desktop-layout event=canvas-component-dropped observer={observer_id}");
        }
    });
    #[cfg(windows)]
    let canvas_host = use_context::<desktop_canvas::DesktopCanvasHandle>();
    #[cfg(windows)]
    let desktop = dioxus_desktop::use_window();

    #[cfg(windows)]
    use_effect(move || {
        use dioxus_desktop::wry::WebViewExtWindows as _;
        let canvas_host = canvas_host.clone();
        let input_hwnd = desktop.webview.composition_input_hwnd();
        spawn(async move {
            if startup_diagnostics {
                println!("desktop-layout event=observer-started observer={observer_id}");
            }
            let Some(input_hwnd) = input_hwnd else {
                eprintln!("desktop-layout event=composition-host-missing");
                canvas_host.hide();
                return;
            };
            let router = match canvas_host::windows::CanvasInputRouter::new(
                canvas_host.input_router_hwnd(),
                input_hwnd,
            ) {
                Ok(router) => router,
                Err(error) => {
                    eprintln!("desktop-layout event=input-router-failed error={error}");
                    canvas_host.hide();
                    return;
                }
            };
            let mut observer = document::eval(include_str!("canvas_host/observe.js"));
            let mut first_message = true;
            let mut first_anomaly = true;
            let mut last_bounds = None;
            loop {
                match observer
                    .recv::<(String, [f64; 5], bool, Vec<[f64; 4]>)>()
                    .await
                {
                    Ok((kind, [x, y, width, height, scale], connected, ui_regions)) => {
                        let anomaly = kind != "geometry" || !connected;
                        if startup_diagnostics && (first_message || (anomaly && first_anomaly)) {
                            println!(
                                "desktop-layout event=observer-message observer={observer_id} kind={kind} connected={connected} rect=({x},{y},{width},{height}) scale={scale}"
                            );
                            first_message = false;
                            first_anomaly &= !anomaly;
                        }
                        if kind == "geometry" && connected {
                            let layout = canvas_host::HostLayout {
                                canvas: [x, y, width, height, scale],
                                ui_regions,
                            };
                            if let Err(error) = canvas_host::windows::apply_input_layout(
                                input_hwnd, &router, &layout,
                            ) {
                                eprintln!(
                                    "desktop-layout event=composition-layout-failed error={error}"
                                );
                                break;
                            }
                            // Overlays only change hit testing. Do not resize or hide
                            // the GPU child when a close dialog changes UI regions.
                            if last_bounds != Some(layout.canvas) {
                                canvas_host.set_geometry(x, y, width, height, scale);
                                last_bounds = Some(layout.canvas);
                            }
                            canvas_host.repaint_underlay();
                        } else {
                            break;
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "desktop-layout event=observer-failed observer={observer_id} error={error}"
                        );
                        break;
                    }
                }
            }
            canvas_host.hide();
            canvas_host::windows::restore_input(&router);
        });
    });

    rsx! {
        div {
            id: "shared-gpu-canvas",
            onmounted: move |_| {
                if startup_diagnostics {
                    println!("desktop-layout event=canvas-dom-mounted observer={observer_id}");
                }
            },
            aria_label: "Persistent native WGPU live ink surface"
        }
    }
}

#[component]
fn UiIcon(name: &'static str) -> Element {
    let paths: &[&str] = match name {
        "folder" => &["M3 6h6l2 2h10v10H3z"],
        "save" => &["M5 3h12l2 2v16H5z", "M8 3v6h8V3", "M8 21v-7h8v7"],
        "undo" => &["M9 7 4 12l5 5", "M5 12h8a6 6 0 0 1 6 6"],
        "redo" => &["m15 7 5 5-5 5", "M19 12h-8a6 6 0 0 0-6 6"],
        "white" => &["M5 4h14v16H5z", "M8 8h8v8H8z"],
        "transform" => &[
            "M4 9V4h5",
            "M15 4h5v5",
            "M20 15v5h-5",
            "M9 20H4v-5",
            "M8 8h8v8H8z",
        ],
        "fit" => &["M8 3H3v5", "M16 3h5v5", "M21 16v5h-5", "M8 21H3v-5"],
        "minus" => &["M5 12h14"],
        "plus" => &["M12 5v14", "M5 12h14"],
        "clear-layer" => &[
            "M12 2v4",
            "M12 18v4",
            "M2 12h4",
            "M18 12h4",
            "m5 5 3 3",
            "m16 16 3 3",
            "m5 19 3-3",
            "m16 8 3-3",
        ],
        "copy" => &["M8 8h12v12H8z", "M16 8V4H4v12h4"],
        "eyedropper" => &[
            "m14 4 6 6",
            "m15 5 3-3 4 4-3 3",
            "m16 8-9 9-4 1 1-4 9-9",
            "M3 21h4",
        ],
        "lock" => &["M5 10h14v11H5z", "M8 10V6a4 4 0 0 1 8 0v4", "M12 14v3"],
        "unlock" => &["M5 10h14v11H5z", "M8 10V6a4 4 0 0 1 8 0", "M12 14v3"],
        "rotate" => &["M20 11a8 8 0 1 0-2 6", "M20 4v7h-7"],
        "mirror-horizontal" => &["M12 3v3m0 4v4m0 4v3", "M3 18 8 6v12z", "m21 18-5-12v12z"],
        "move" => &[
            "M3 3v15l4-4 4 7 3-2-4-7h6z",
            "M15 5h7",
            "m17 3-2 2 2 2",
            "m20 3 2 2-2 2",
        ],
        "hand" => &[
            "M8 12V6a2 2 0 0 1 4 0v5",
            "M12 9V4a2 2 0 0 1 4 0v7",
            "M16 8a2 2 0 0 1 4 0v8c0 4-3 6-6 6h-2c-2 0-4-1-5-3l-4-6a2 2 0 0 1 3-2l2 2",
        ],
        "rectangle-selection" => &[
            "M3 7V3h4",
            "M11 3h2",
            "M17 3h4v4",
            "M21 11v2",
            "M21 17v4h-4",
            "M13 21h-2",
            "M7 21H3v-4",
            "M3 13v-2",
        ],
        "invert-selection" => &[
            "M3 3h18v18H3z",
            "M3 21 21 3",
            "M3 8h5",
            "M3 13h10",
            "M3 18h15",
        ],
        "deselect" => &[
            "M7 3H3v4",
            "M17 3h4v4",
            "M21 17v4h-4",
            "M7 21H3v-4",
            "m8 8 8 8",
            "m16 8-8 8",
        ],
        "select-alpha" => &[
            "M7 3H3v4",
            "M17 3h4v4",
            "M21 17v4h-4",
            "M7 21H3v-4",
            "M12 6c-2 3-5 5-5 8a5 5 0 0 0 10 0c0-3-3-5-5-8z",
        ],
        "wand" => &[
            "m4 20 10-10",
            "m12 2 1 3",
            "m5 2 3 1",
            "m-1 7 3 1",
            "m-7 6 1 3",
        ],
        "lasso" => &["M5 15c-4-3 0-10 7-10s11 7 6 11c-3 3-10 2-10-2 0-2 5-2 5 1"],
        "pencil" => &["m4 20 4-1 11-11-3-3L5 16z", "m14-16 2 2"],
        "pen" => &["m5 19 3-8 8-8 5 5-8 8z", "m8-5-3-3", "M5 19h7"],
        "brush" => &[
            "m14 4 6 6-8 8",
            "M12 18c-2 3-7 2-8 2 2-1 1-6 4-8 3-2 7 1 4 6z",
        ],
        "eraser" => &["m4 15 8-11 8 7-7 9H8z", "M11 20h10"],
        "bucket" => &["m5 12 7-7 7 7-7 7z", "M3 20h18"],
        "gradient" => &["M4 5h16v14H4z", "M9 5v14", "M14 5v14"],
        "layer" => &["m4 8 8-4 8 4-8 4z", "m4 4 8 4 8-4", "m4 4 8 4 8-4"],
        "reference" => &["M4 4h16v16H4z", "M8 8h8v8H8z", "m4-4 4 4"],
        "trash" => &[
            "M5 7h14",
            "M9 7V4h6v3",
            "m-8 0 1 14h10l1-14",
            "M10 11v6",
            "M14 11v6",
        ],
        "eye" => &[
            "M2 12s4-6 10-6 10 6 10 6-4 6-10 6S2 12 2 12",
            "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6",
        ],
        "chevron-right" => &["m9 5 7 7-7 7"],
        "chevron-down" => &["m5 9 7 7 7-7"],
        _ => &["M5 5h14v14H5z"],
    };
    rsx! {
        svg { class: "icon", view_box: "0 0 24 24",
            for path in paths { path { d: "{path}" } }
        }
    }
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

fn send_dock_command(live_ink: &LiveInkBridge, command: DockCommand) {
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

pub(crate) fn elapsed_since_launch() -> u128 {
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

#[cfg(windows)]
mod desktop_canvas;
mod desktop_shell;
mod edit_gesture;
mod edit_worker;
mod layer_drag;
mod live_ink;
mod native_canvas;
mod preview;
#[cfg(windows)]
mod single_instance;

use std::time::{Duration, Instant};
use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

use dioxus::prelude::*;
use live_ink::{CloseStatus, ExportStatus, LiveInkBridge};
use nyatidraw_api::{
    CommandRejectReason, DockAxis, DockCommand, DockNode, DockPosition, DrawingTool, EditorCommand,
    EditorEvent, EventEnvelope, GroupId, HistoryCommand, HistoryOperationLabel, LayerCommand,
    LayerId, LayerProjection, LayerProjectionKind, LayerTreeNodeId, PanelKind, ProjectCommand,
    ToolCommand, UiProjection, ViewportCommand, WorkspaceProjection,
};
use preview::LayerThumbnailFrame;

const BACKGROUND_LAYER: LayerId = LayerId(2);
const INK_LAYER: LayerId = LayerId(1);
const ROOT_GROUP: GroupId = GroupId(100);
const INK_GROUP: GroupId = GroupId(10);

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
const STYLES: &str = include_str!("../assets/styles.css");
const STYLESHEET: Asset = asset!("/assets/styles.css");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DockDrop {
    target: PanelKind,
    position: DockPosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DockDrag {
    source: PanelKind,
    hovered: Option<DockDrop>,
}

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
    let live_ink = use_context::<LiveInkBridge>();
    let notifier_ink = live_ink.clone();
    use_hook(move || notifier_ink.set_ui_notifier(dioxus_core::schedule_update()));
    let mut ui_projection = use_signal(initial_ui_projection);
    let (authoritative, latest_event) = live_ink.protocol_snapshot();
    if latest_event.is_some()
        && authoritative.revision >= ui_projection.peek().revision
        && *ui_projection.peek() != authoritative
    {
        ui_projection.set(authoritative);
    }
    let mut dock_drag = use_signal(|| Option::<DockDrag>::None);
    let mut navigator_drag = use_context_provider(|| Signal::new(Option::<NavigatorDrag>::None));
    let projected_revision = ui_projection.read().revision.0;
    let dock = ui_projection.read().dock.clone();
    let dirty = ui_projection.read().dirty;
    let document_title = ui_projection.read().document_title.clone();
    let export_status = live_ink.export_status_snapshot();
    let close_status = live_ink.close_status();
    let activation_notice = live_ink.activation_notice_snapshot();
    let edit = ui_projection.read().edit.clone();
    let clear_edit_ink = live_ink.clone();
    let dock_drop_ink = live_ink.clone();
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
        main {
            class: "app-shell",
            tabindex: 0,
            onmouseup: move |_| {
                let Some(drag) = *dock_drag.read() else { return };
                if let Some(drop_target) = drag.hovered {
                    send_dock_command(
                        &dock_drop_ink,
                        DockCommand::MovePanel {
                            panel: drag.source,
                            target: drop_target.target,
                            position: drop_target.position,
                        },
                    );
                }
                dock_drag.set(None);
            },
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
                if handle_editor_shortcut(&shortcut_ink, &key, event.modifiers().shift()) {
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
                        dock_drag.set(None);
                        navigator_drag.set(None);
                        None
                    }
                    // Dioxus Native does not expose supported programmatic
                    // mounted-element focus, so F6 reveals the normal
                    // keyboard-reachable Canvas focus target instead.
                    _ => None,
                };
                if let Some(panel) = panel {
                    send_dock_command(&shortcut_ink, DockCommand::ActivatePanel(panel));
                }
            },
            ActionBar { ui_projection, export_status }
            section {
                class: if dock_drag.read().is_some() { "workspace dock-dragging" } else { "workspace" },
                aria_label: "Editor workspace. F6 returns to the drawing canvas.",
                title: "{document_title} · r{projected_revision} · {protocol_status(latest_event.as_ref())}",
                DockNodeView { node: dock.root().clone(), ui_projection, dock_drag }
            }
            if dirty { span { class: "unsaved-dot", title: "저장되지 않은 변경", "•" } }
            if edit.busy || edit.has_selection || edit.error.is_some() {
                aside { class: "edit-status", role: "status", aria_live: "polite",
                    if edit.busy { span { "선택·채우기 처리 중…" } }
                    else {
                        span { "선택 {edit.selected_pixels} px" }
                        button { onclick: move |_| {
                            let (current, _) = clear_edit_ink.protocol_snapshot();
                            let _ = clear_edit_ink.push_editor_command(current.revision,
                                EditorCommand::Edit(nyatidraw_api::EditCommand::ClearSelection));
                        }, "선택 해제" }
                    }
                    if let Some(reason) = &edit.error { span { role: "alert", "편집 실패: {reason}" } }
                }
            }
            if let Some((command, reason)) = rejection {
                span { class: "status-toast error", role: "alert", "명령 #{command.0}: {reject_label(reason)}" }
            }
            if let Some(notice) = activation_notice {
                span { class: "status-toast error activation-notice", role: "alert", "파일 열기 실패: {notice}" }
            }
            CloseProgress { status: close_status }
        }
    }
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

fn handle_editor_shortcut(live_ink: &LiveInkBridge, key: &str, shift: bool) -> bool {
    let command = if key.eq_ignore_ascii_case("Escape") {
        Some(EditorCommand::Tool(ToolCommand::CancelGesture))
    } else if key.eq_ignore_ascii_case("b") {
        Some(EditorCommand::Tool(ToolCommand::CycleBrushFamily))
    } else if key.eq_ignore_ascii_case("g") {
        Some(EditorCommand::Tool(ToolCommand::Select(DrawingTool::Move)))
    } else if key.eq_ignore_ascii_case("w") {
        Some(EditorCommand::Tool(ToolCommand::Select(DrawingTool::Wand)))
    } else if key.eq_ignore_ascii_case("l") {
        Some(EditorCommand::Tool(ToolCommand::Select(DrawingTool::Lasso)))
    } else if key.eq_ignore_ascii_case("f") {
        Some(EditorCommand::Tool(ToolCommand::Select(if shift {
            DrawingTool::Gradient
        } else {
            DrawingTool::Fill
        })))
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
    projection.layers = vec![
        LayerProjection {
            id: LayerTreeNodeId::Group(INK_GROUP),
            parent: ROOT_GROUP,
            index: 1,
            depth: 1,
            kind: LayerProjectionKind::Group,
            name: "Ink group".into(),
            visible: true,
            reference: false,
            opacity_u16: u16::MAX,
        },
        LayerProjection {
            id: LayerTreeNodeId::Raster(INK_LAYER),
            parent: INK_GROUP,
            index: 0,
            depth: 2,
            kind: LayerProjectionKind::Raster,
            name: "Ink".into(),
            visible: true,
            reference: false,
            opacity_u16: u16::MAX,
        },
        LayerProjection {
            id: LayerTreeNodeId::Raster(BACKGROUND_LAYER),
            parent: ROOT_GROUP,
            index: 0,
            depth: 1,
            kind: LayerProjectionKind::Raster,
            name: "Background".into(),
            visible: true,
            reference: false,
            opacity_u16: u16::MAX,
        },
    ];
    projection
}

#[component]
fn ActionBar(ui_projection: Signal<UiProjection>, export_status: ExportStatus) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let save_ink = live_ink.clone();
    let retry_ink = live_ink.clone();
    let undo_ink = live_ink.clone();
    let redo_ink = live_ink.clone();
    let white_ink = live_ink.clone();
    let fit_ink = live_ink.clone();
    let zoom_out_ink = live_ink.clone();
    let zoom_in_ink = live_ink.clone();
    let rotate_ink = live_ink.clone();
    let error = use_signal(|| Option::<String>::None);
    let viewport = ui_projection.read().viewport;
    let zoom = viewport.zoom_ppm / 10_000;
    let rotation = viewport.rotation_millidegrees / 1_000;
    let current = ui_projection.read().brush_color;
    let current_color = format!("#{:02X}{:02X}{:02X}", current[0], current[1], current[2]);

    rsx! {
        nav { class: "commandbar", aria_label: "주요 명령",
            button { class: "command hamburger-button", title: "메뉴 (후속 구현)", aria_label: "메뉴", disabled: true,
                span { class: "hamburger", i {}, i {}, i {} }
            }
            button { class: "command", title: "열기 (후속 구현)", disabled: true, UiIcon { name: "folder" } span { class: "shortcut", "Alt O" } }
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

            div { class: "toolbar-dock", aria_label: "캔버스 작업",
                span { class: "toolbar-grip", aria_hidden: "true" }
                button { class: "command", title: "흰 배경 추가", onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } span { class: "command-label", "흰 배경" } }
                button { class: "command", title: "변형 (후속 구현)", disabled: true,
                    UiIcon { name: "transform" } span { class: "command-label", "변형" }
                }
            }
            div { class: "toolbar-dock", aria_label: "보기",
                span { class: "toolbar-grip", aria_hidden: "true" }
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
                span { class: "toolbar-value", "{rotation}°" }
            }
            div { class: "toolbar-dock color-toolbar", aria_label: "현재 색상과 최근 색상",
                span { class: "toolbar-grip", aria_hidden: "true" }
                div { class: "color-stack", span { class: "color-chip back" } span { class: "color-chip front", style: "background:{current_color}" } }
                div { class: "quick-colors",
                    for (color, rgba) in [("#e83030", [232,48,48,255]), ("#20b94a", [32,185,74,255]), ("#267be0", [38,123,224,255]), ("#f1c94a", [241,201,74,255]), ("#ffffff", [255,255,255,255]), ("#171717", [23,23,23,255]), ("#ef8ea0", [239,142,160,255]), ("#8f61c9", [143,97,201,255])] {
                        ToolbarColorButton { color, rgba }
                    }
                }
            }
            if let Some(message) = error.read().as_ref() {
                span { class: "commandbar-error", role: "alert", "{message}" }
            }
        }
    }
}

#[component]
fn ToolbarColorButton(color: &'static str, rgba: [u8; 4]) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        button { class: "quick-color", style: "background:{color}", title: "{color}", aria_label: "최근 색상 {color}", onclick: move |_| send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetColor(rgba)), error) }
    }
}

#[component]
fn DockNodeView(
    node: DockNode,
    ui_projection: Signal<UiProjection>,
    dock_drag: Signal<Option<DockDrag>>,
) -> Element {
    match node {
        DockNode::Panel(panel) => rsx! {
            DockPanel { panel, ui_projection, dock_drag }
        },
        DockNode::Tabs { active, panels } => {
            let active_panel = panels[active];
            rsx! {
                section { class: "dock-tabs", aria_label: "Docked panel tabs",
                    nav { class: "tab-strip", aria_label: "Panel tabs",
                        for (index, panel) in panels.iter().copied().enumerate() {
                            DockTab { panel, selected: index == active, dock_drag }
                        }
                    }
                    DockPanel { panel: active_panel, ui_projection, dock_drag }
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
            let first_style = format!("flex: {first_per_mille} 1 0;");
            let second_style = format!("flex: {} 1 0;", 1000_u16.saturating_sub(first_per_mille));
            rsx! {
                section { class: "{class}",
                    div { class: "dock-child", style: "{first_style}",
                        DockNodeView { node: *first, ui_projection, dock_drag }
                    }
                    div { class: "dock-child", style: "{second_style}",
                        DockNodeView { node: *second, ui_projection, dock_drag }
                    }
                }
            }
        }
    }
}

#[component]
fn DockTab(panel: PanelKind, selected: bool, dock_drag: Signal<Option<DockDrag>>) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    rsx! {
        button {
            class: if selected { "dock-tab active" } else { "dock-tab" },
            aria_selected: if selected { "true" } else { "false" },
            onclick: move |_| send_dock_command(&live_ink, DockCommand::ActivatePanel(panel)),
            onmousedown: move |_| dock_drag.set(Some(DockDrag { source: panel, hovered: None })),
            "{panel_label(panel)}"
        }
    }
}

#[component]
fn DockPanel(
    panel: PanelKind,
    ui_projection: Signal<UiProjection>,
    dock_drag: Signal<Option<DockDrag>>,
) -> Element {
    let workspace = ui_projection.read().workspace.clone();
    let panel_title = panel_label(panel);
    let panel_class = format!("dock-panel panel-{}", panel_slug(panel));
    let is_source = dock_drag.read().is_some_and(|drag| drag.source == panel);

    rsx! {
        section { class: if is_source { "{panel_class} dock-drag-source" } else { "{panel_class}" }, aria_label: "{panel_title} panel",
            if panel != PanelKind::Canvas {
                header {
                    class: "panel-header",
                    title: "{panel_title} 패널 이동",
                    onmousedown: move |_| dock_drag.set(Some(DockDrag { source: panel, hovered: None })),
                    span { class: "panel-grip", aria_hidden: "true" }
                    h2 { "{panel_title}" }
                }
            }
            div { class: "panel-content",
                PanelContents { panel, workspace, ui_projection }
            }
            if dock_drag.read().is_some_and(|drag| drag.source != panel) {
                DockDropOverlay { target: panel, dock_drag }
            }
        }
    }
}

#[component]
fn DockDropOverlay(target: PanelKind, dock_drag: Signal<Option<DockDrag>>) -> Element {
    rsx! {
        div { class: "dock-drop-overlay", aria_label: "{panel_label(target)} 주변 도킹 위치",
            DockDropZone { target, position: DockPosition::Left, class: "dock-zone zone-left", dock_drag }
            DockDropZone { target, position: DockPosition::Top, class: "dock-zone zone-top", dock_drag }
            DockDropZone { target, position: DockPosition::Right, class: "dock-zone zone-right", dock_drag }
        }
    }
}

#[component]
fn DockDropZone(
    target: PanelKind,
    position: DockPosition,
    class: &'static str,
    dock_drag: Signal<Option<DockDrag>>,
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let active = dock_drag
        .read()
        .is_some_and(|drag| drag.hovered == Some(DockDrop { target, position }));
    let indicator = if matches!(position, DockPosition::Left | DockPosition::Right) {
        "dock-insert vertical"
    } else {
        "dock-insert horizontal"
    };
    rsx! {
        div {
            class: if active { "{class} active" } else { "{class}" },
            onmousemove: move |_| {
                let current = *dock_drag.read();
                if let Some(mut drag) = current {
                    drag.hovered = Some(DockDrop { target, position });
                    dock_drag.set(Some(drag));
                }
            },
            onmouseup: move |_| {
                let current = *dock_drag.read();
                if let Some(drag) = current {
                    send_dock_command(
                        &live_ink,
                        DockCommand::MovePanel { panel: drag.source, target, position },
                    );
                    dock_drag.set(None);
                }
            },
            span { class: "{indicator}" }
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
        PanelKind::Color => rsx! { ColorPanel { ui_projection } },
        PanelKind::History => rsx! { HistoryPanel { ui_projection } },
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
        HistoryOperationLabel::Initial => "시작",
        HistoryOperationLabel::Stroke => "스트로크",
        HistoryOperationLabel::Structural => "구조 변경",
    }
}

#[component]
fn ToolsPanel(ui_projection: Signal<UiProjection>) -> Element {
    let active = ui_projection.read().drawing_tool;
    rsx! {
        nav { class: "tool-list", aria_label: "도구",
            ToolButton { label: "이동", icon: "move", shortcut: "g", tool: Some(DrawingTool::Move), active: active == DrawingTool::Move }
            ToolButton { label: "마법봉", icon: "wand", shortcut: "w", tool: Some(DrawingTool::Wand), active: active == DrawingTool::Wand }
            ToolButton { label: "올가미", icon: "lasso", shortcut: "l", tool: Some(DrawingTool::Lasso), active: active == DrawingTool::Lasso }
            span { class: "tool-separator" }
            ToolButton { label: "연필", icon: "pencil", shortcut: "b", tool: Some(DrawingTool::Pencil), active: active == DrawingTool::Pencil }
            ToolButton { label: "펜", icon: "pen", shortcut: "b", tool: Some(DrawingTool::Pen), active: active == DrawingTool::Pen }
            ToolButton { label: "브러시", icon: "brush", shortcut: "b", tool: Some(DrawingTool::Brush), active: active == DrawingTool::Brush }
            span { class: "tool-separator" }
            ToolButton { label: "지우개", icon: "eraser", shortcut: "e", tool: Some(DrawingTool::Eraser), active: active == DrawingTool::Eraser }
            span { class: "tool-separator" }
            ToolButton { label: "채우기", icon: "bucket", shortcut: "f", tool: Some(DrawingTool::Fill), active: active == DrawingTool::Fill }
            ToolButton { label: "그라데이션", icon: "gradient", shortcut: "Shift f", tool: Some(DrawingTool::Gradient), active: active == DrawingTool::Gradient }
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
            title: "{label} ({shortcut})",
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
    if ui_projection.read().drawing_tool.is_edit() {
        return rsx! { EditToolPanel { ui_projection } };
    }
    let current_size = ui_projection.read().brush_size_tenths;
    let display_size = f32::from(current_size) / 10.0;
    let opacity = u32::from(ui_projection.read().brush_opacity_u16) * 100 / u32::from(u16::MAX);
    rsx! {
        div { class: "brush-panel",
            div { class: "subtool-list",
                SubtoolButton { name: "마커펜", tool: Some(DrawingTool::Brush), active: ui_projection.read().drawing_tool == DrawingTool::Brush }
                SubtoolButton { name: "G펜", tool: Some(DrawingTool::Pen), active: ui_projection.read().drawing_tool == DrawingTool::Pen }
                SubtoolButton { name: "뭉개기", tool: None, active: false }
            }
            div { class: "tool-properties",
                div { class: "brush-sample", span { class: "stroke-sample wide" } }
                div { class: "property-row", span { "브러시 크기" } span { class: "property-value", "{display_size:.1}" } }
                OpacityControl { percent: opacity }
            }
            div { class: "brush-sizes",
                div { class: "section-caption", "브러시 크기" }
                div { class: "recent-sizes", aria_label: "최근 브러시 크기",
                    for (size, dot) in [(70_u16, 7), (110, 11), (200, 20), (80, 8)] { BrushSizeButton { size_tenths: size, dot, recent: true, active: current_size == size } }
                }
                div { class: "size-list",
                    for size in [1_u16, 3, 5, 10, 20, 40, 80, 120] {
                        BrushSizeButton { size_tenths: size * 10, dot: brush_dot(i32::from(size)), recent: false, active: current_size == size * 10 }
                    }
                }
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
            onclick: move |_| {
                if let Some(tool) = tool {
                    send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::Select(tool)), error);
                }
            },
            span { class: "stroke-sample" }
            span { class: "subtool-name", "{name}" }
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
            class: if recent { "recent-size" } else if active { "size-option active" } else { "size-option" },
            title: "브러시 크기 {size:.1}",
            onclick: move |_| send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetSizeTenths(size_tenths)), error),
            span { class: "size-dot", style: "width:{dot}px;height:{dot}px" }
            if !recent { span { "{size:.0}" } }
        }
    }
}

#[component]
fn EditToolPanel(ui_projection: Signal<UiProjection>) -> Element {
    use nyatidraw_api::EditSource;
    let live_ink = use_context::<LiveInkBridge>();
    let tolerance_ink = live_ink.clone();
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
        DrawingTool::Fill => {
            "선택 영역을 현재 색으로 채웁니다. 선택이 없으면 클릭한 연결 영역을 채웁니다."
        }
        _ => "선택 영역에서 끌어 현재 색 → 투명 그라데이션을 만듭니다.",
    };
    rsx! {
        div { class: "edit-tool-properties",
            p { "{help}" }
            if matches!(tool, DrawingTool::Wand | DrawingTool::Fill) {
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
                label { "허용 오차 {settings.tolerance} / 255"
                    input { r#type: "range", min: "0", max: "255", value: "{settings.tolerance}", aria_label: "색상 허용 오차",
                        onchange: move |event| { if let Ok(value) = event.value().parse::<u8>() {
                            send_editor_command(&tolerance_ink, EditorCommand::Tool(ToolCommand::SetEditTolerance(value)), error);
                        } }
                    }
                }
            }
        }
    }
}

#[component]
fn OpacityControl(percent: u32) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        div { class: "property-row",
            span { "불투명도" }
            span { class: "property-value", "{percent}" }
            input {
                r#type: "range",
                min: "1",
                max: "100",
                value: "{percent}",
                onchange: move |event| {
                    if let Ok(value) = event.value().parse::<u32>() {
                        let scaled = ((value.min(100) * u32::from(u16::MAX)) / 100).max(1);
                        if let Ok(opacity) = u16::try_from(scaled) {
                            send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetOpacityU16(opacity)), error);
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
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    let current = ui_projection.read().brush_color;
    let current_hex = format!("#{:02X}{:02X}{:02X}", current[0], current[1], current[2]);
    rsx! {
        div { class: "color-panel",
            div { class: "wheel-wrap",
                div { class: "color-wheel", aria_label: "색상환",
                    span { class: "wheel-inner" }
                    span { class: "sv-square" }
                    span { class: "wheel-picker" }
                    input {
                        class: "color-wheel-input",
                        r#type: "color",
                        title: "색상 선택",
                        value: "{current_hex}",
                        oninput: move |event| {
                            if let Some(rgba) = parse_html_color(&event.value()) {
                                send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetColor(rgba)), error);
                            }
                        }
                    }
                }
                span { class: "value-strip" }
            }
            span { class: "color-value", "{current_hex}" }
            div { class: "recent-colors",
                for (color, rgba) in [("#171717", [23,23,23,255]), ("#ffffff", [255,255,255,255]), ("#e83030", [232,48,48,255]), ("#20b94a", [32,185,74,255]), ("#267be0", [38,123,224,255]), ("#f1c94a", [241,201,74,255]), ("#8f61c9", [143,97,201,255]), ("#ef8c35", [239,140,53,255])] {
                    RecentColorButton { color, rgba }
                }
            }
            if let Some(message) = error.read().as_ref() { div { class: "command-error", role: "alert", "{message}" } }
        }
    }
}

fn parse_html_color(value: &str) -> Option<[u8; 4]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(hex, 16).ok()?;
    Some([
        u8::try_from((rgb >> 16) & 0xff).ok()?,
        u8::try_from((rgb >> 8) & 0xff).ok()?,
        u8::try_from(rgb & 0xff).ok()?,
        u8::MAX,
    ])
}

#[component]
fn RecentColorButton(color: &'static str, rgba: [u8; 4]) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let error = use_signal(|| Option::<String>::None);
    rsx! {
        button { style: "background:{color}", title: "{color}", aria_label: "최근 색상 {color}", onclick: move |_| send_editor_command(&live_ink, EditorCommand::Tool(ToolCommand::SetColor(rgba)), error) }
    }
}

#[component]
fn LayersPanel(ui_projection: Signal<UiProjection>) -> Element {
    layer_drag::use_layer_drag_probe();
    let live_ink = use_context::<LiveInkBridge>();
    let add_raster_ink = live_ink.clone();
    let add_group_ink = live_ink.clone();
    let white_ink = live_ink.clone();
    let reference_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let layers = ui_projection.read().layers.clone();
    let active = ui_projection.read().active_layer;
    let active_reference = layers.iter().any(|layer| {
        layer.id == LayerTreeNodeId::Raster(active.unwrap_or(LayerId(0))) && layer.reference
    });
    let solo_node = ui_projection.read().solo_node;
    let error = use_signal(|| Option::<String>::None);
    let collapsed_groups = use_signal(BTreeSet::<GroupId>::new);
    let mut layer_drag = use_signal(|| Option::<layer_drag::LayerDrag>::None);
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
            ondragend: move |_| layer_drag.set(None),
            onkeydown: move |event| {
                if event.key() == Key::Escape { layer_drag.set(None); }
            },
            div { class: "layer-actions",
                button { title: "래스터 레이어 추가", onclick: move |_| {
                    send_editor_command(&add_raster_ink, EditorCommand::Layer(LayerCommand::AddRaster), error);
                }, UiIcon { name: "layer" } }
                button { title: "그룹 추가", onclick: move |_| {
                    send_editor_command(&add_group_ink, EditorCommand::Layer(LayerCommand::AddGroup), error);
                }, UiIcon { name: "folder" } }
                button { title: "흰 배경 추가", onclick: move |_| {
                    send_editor_command(&white_ink, EditorCommand::Layer(LayerCommand::AddWhiteBackground), error);
                }, UiIcon { name: "white" } }
                layer_drag::LayerMoveButtons { projection: ui_projection, error }
            }
            div { class: "layer-settings",
                button { disabled: true, title: "레이어 색상화 (후속 구현)", span { class: "layer-color-chip" } "색상화" }
                button {
                    disabled: active.is_none(), aria_pressed: "{active_reference}",
                    title: "선택·채우기 참조 레이어 표시 (일반 PNG에는 영향 없음)",
                    onclick: move |_| {
                        if let Some(layer) = active {
                            send_editor_command(&reference_ink, EditorCommand::Layer(LayerCommand::SetReference {
                                layer, reference: !active_reference,
                            }), error);
                        }
                    },
                    UiIcon { name: "reference" } "참조"
                }
            }
            div { class: "layer-list",
                for layer in visible_layers {
                    {
                        let thumbnail = match layer.id {
                            LayerTreeNodeId::Raster(layer_id) => thumbnails.frame(layer_id),
                            LayerTreeNodeId::Group(_) => None,
                        };
                        rsx! { LayerRow { key: "{layer.id:?}", layer, thumbnail, active, solo_node, error, collapsed_groups, layer_drag, ui_projection } }
                    }
                }
            }
            if let Some(message) = error.read().as_ref() { div { class: "command-error", role: "alert", "{message}" } }
            div { class: "layers-footer", button { disabled: active.is_none(), title: "선택 레이어 삭제 (Undo로 복원)", onclick: move |_| {
                if let Some(layer) = active {
                    send_editor_command(&delete_ink, EditorCommand::Layer(LayerCommand::Delete(LayerTreeNodeId::Raster(layer))), error);
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
    mut layer_drag: Signal<Option<layer_drag::LayerDrag>>,
    ui_projection: Signal<UiProjection>,
) -> Element {
    let live_ink = use_context::<LiveInkBridge>();
    let active_ink = live_ink.clone();
    let visibility_ink = live_ink.clone();
    let opacity_ink = live_ink.clone();
    let rename_ink = live_ink.clone();
    let mut rename_error = error;
    let solo_ink = live_ink.clone();
    let delete_ink = live_ink.clone();
    let id = layer.id;
    let is_active = matches!(id, LayerTreeNodeId::Raster(layer_id) if active == Some(layer_id));
    let is_group = layer.kind == LayerProjectionKind::Group;
    let is_collapsed =
        matches!(id, LayerTreeNodeId::Group(group) if collapsed_groups.read().contains(&group));
    let opacity = u32::from(layer.opacity_u16) * 100 / u32::from(u16::MAX);
    let indent = u32::from(layer.depth.saturating_sub(1)) * 9;
    let layer_key = match id {
        LayerTreeNodeId::Raster(id) => format!("r-{}", id.0),
        LayerTreeNodeId::Group(id) => format!("g-{}", id.0),
    };
    rsx! {
        div { class: if is_group { "layer-row group" } else if is_active { "layer-row active" } else { "layer-row" }, style: "padding-left:{indent}px",
            "data-layer-key": "{layer_key}", "data-parent": "{layer.parent.0}", "data-index": "{layer.index}",
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
                draggable: "true", title: "끌어서 레이어 순서 또는 그룹 변경",
                ondragstart: move |_| {
                    let current = ui_projection.read();
                    if let Some(source) = current.layers.iter().find(|layer| layer.id == id) {
                        layer_drag.set(Some(layer_drag::LayerDrag::begin(source, current.revision)));
                    }
                },
                ondragend: move |_| layer_drag.set(None),
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
                    send_editor_command(&active_ink, EditorCommand::Layer(LayerCommand::SetActive(layer_id)), error);
                }
            },
                input {
                    class: "layer-name",
                    aria_label: "레이어 이름",
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
                    if layer.reference { "참조 · " }
                    if is_group { "그룹" } else { "{opacity}%" }
                }
            }
            button { class: "layer-opacity", title: "불투명도 전환", onclick: move |_| {
                let opacity_u16 = if layer.opacity_u16 == u16::MAX { 32_768 } else { u16::MAX };
                send_editor_command(&opacity_ink, EditorCommand::Layer(LayerCommand::SetOpacity { node: id, opacity_u16 }), error);
            }, "{opacity}%" }
            button {
                class: "layer-delete", title: "삭제 (Undo로 복원)", aria_label: "{layer.name} 삭제",
                onclick: move |_| send_editor_command(&delete_ink, EditorCommand::Layer(LayerCommand::Delete(id)), error),
                "×"
            }
            layer_drag::LayerDropTargets { layer: layer.clone(), projection: ui_projection, drag: layer_drag, error }
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
    #[cfg(windows)]
    let canvas_host = use_context::<desktop_canvas::DesktopCanvasHandle>();

    #[cfg(windows)]
    use_effect(move || {
        let canvas_host = canvas_host.clone();
        spawn(async move {
            let mut observer = document::eval(
                r"
                const element = document.getElementById('shared-gpu-canvas');
                if (!element) {
                    throw new Error('native canvas placeholder missing');
                }
                const publish = () => {
                    const rect = element.getBoundingClientRect();
                    dioxus.send([
                        rect.left,
                        rect.top,
                        rect.width,
                        rect.height,
                        window.devicePixelRatio || 1
                    ]);
                };
                const resizeObserver = new ResizeObserver(publish);
                resizeObserver.observe(element);
                window.addEventListener('resize', publish);
                publish();
                await new Promise(() => {});
                ",
            );
            while let Ok([x, y, width, height, scale]) = observer.recv::<[f64; 5]>().await {
                canvas_host.set_geometry(x, y, width, height, scale);
            }
            canvas_host.hide();
        });
    });

    rsx! {
        div {
            id: "shared-gpu-canvas",
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
        "rotate" => &["M20 11a8 8 0 1 0-2 6", "M20 4v7h-7"],
        "move" => &["M12 2v20", "m8-4 4-4-4-4", "M2 12h20", "m4-8-4 4 4 4"],
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
        PanelKind::Color => "색상",
        PanelKind::History => "히스토리",
    }
}

fn panel_slug(panel: PanelKind) -> &'static str {
    match panel {
        PanelKind::Canvas => "canvas",
        PanelKind::Tools => "tools",
        PanelKind::Navigator => "navigator",
        PanelKind::Layers => "layers",
        PanelKind::Brush => "brush",
        PanelKind::Color => "color",
        PanelKind::History => "history",
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

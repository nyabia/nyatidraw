#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod artwork_clipboard;
#[cfg(windows)]
mod canvas_host;
#[cfg(windows)]
mod desktop_canvas;
mod desktop_shell;
mod edit_gesture;
mod edit_worker;
#[cfg(windows)]
mod file_associations;
mod layout_store;
mod live_ink;
mod native_canvas;
mod performance;
mod performance_probe;
mod performance_workload;
mod preview;
#[cfg(windows)]
mod recent_projects;
#[cfg(windows)]
mod render_host;
mod save_as;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod sketchbook;
mod transform_gesture;
mod transform_worker;
#[cfg(windows)]
mod updates;
mod workspace_appearance;

mod editor_ui_host;
mod palette_preferences;

use dioxus::prelude::*;
use live_ink::{CloseStatus, LiveInkBridge};
use nyatidraw_editor_ui::{EditorChrome, TransientNotice, UiIcon, UiSlots};
use std::{sync::OnceLock, time::Instant};

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
#[cfg(windows)]
#[derive(Clone, Copy)]
struct RecentProjectList(Signal<Result<Vec<std::path::PathBuf>, String>>);
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

fn app() -> Element {
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
        let mut update_status = use_context_provider(|| Signal::new(updates::status()));
        let current = updates::status();
        if *update_status.peek() != current {
            update_status.set(current);
        }
        let RecentProjectList(mut recent) =
            use_context_provider(|| RecentProjectList(Signal::new(recent_projects::list())));
        let current = recent_projects::list();
        if *recent.peek() != current {
            recent.set(current);
        }
    }
    let ui_ink = live_ink.clone();
    let host = use_hook(move || editor_ui_host::create(ui_ink, dioxus_core::schedule_update()));
    use_context_provider(move || host);
    use_context_provider(|| UiSlots {
        canvas: || rsx! { SharedCanvas {} },
        file_buttons: |extended| rsx! { FileButtons { extended } },
        update_control: || rsx! { UpdateControl {} },
    });
    let authoritative = live_ink.protocol_snapshot().0;
    let mut ui_projection = use_signal(|| authoritative.clone());
    if *ui_projection.peek() != authoritative {
        ui_projection.set(authoritative);
    }
    let export_status = editor_ui_host::export_status(live_ink.export_status_snapshot());
    let close_status = live_ink.close_status();
    let activation_notice = live_ink.activation_notice_snapshot();
    let picker = live_ink.picker_snapshot();
    let render_serial = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(0_u64)));
    let host_update = render_serial.get().wrapping_add(1);
    render_serial.set(host_update);
    rsx! {
        EditorChrome { ui_projection, export_status, host_update,
            overlays: rsx! {
                if let Some(notice) = activation_notice { TransientNotice { key: "notice-{notice}", message: notice } }
                if let Some(snapshot) = picker { {picker_loupe(snapshot)} }
                CloseProgress { status: close_status }
            }
        }
    }
}
use nyatidraw_editor_ui::picker_loupe::picker_loupe;

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
                    title: match file_action { 1 => "저장 위치를 정하지 않고 새 그림 시작", 2 => "현재 그림과 모든 실행 취소 기록을 다른 이름으로 저장", _ => "PNG 또는 NyatiDraw 프로젝트 열기" },
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
                                if file_action == 1 {
                                    if let Some(host) = &host && let Err(error) = host.new_drawing() {
                                        live_ink.publish_activation_notice(error);
                                    }
                                    return;
                                }
                                busy.set(true);
                                let dialog = rfd::AsyncFileDialog::new().set_parent(window.as_ref());
                                let selected = if file_action != 0 {
                                    dialog.set_title("다른 이름으로 저장").add_filter("NyatiDraw 프로젝트", &["ntdr"])
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
            if extended {
                RecentProjects { disabled: busy() }
            }
        };
    }
    #[cfg(not(windows))]
    rsx! { button { disabled: true, "열기 · Windows 전용" } }
}

#[cfg(windows)]
#[component]
fn RecentProjects(disabled: bool) -> Element {
    let host = try_use_context::<desktop_canvas::DesktopCanvasHandle>();
    let live_ink = use_context::<LiveInkBridge>();
    let recent = use_context::<RecentProjectList>().0.read().clone();
    let title = recent
        .as_ref()
        .err()
        .map_or("최근 그림", String::as_str)
        .to_owned();
    let paths = recent.unwrap_or_default();
    let entries: Vec<_> = paths
        .iter()
        .map(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
            format!("{name} — {}", parent.display())
        })
        .collect();
    rsx! {
        select { class: "command recent-projects", style: "max-width: 180px", aria_label: "최근 그림", title, value: "",
            disabled: disabled || host.is_none() || paths.is_empty(),
            onchange: move |event| {
                if let Ok(index) = event.value().parse::<usize>()
                    && let Some(path) = paths.get(index)
                {
                    let result = if !path.is_file() {
                        Err("그림 파일을 찾지 못했습니다. 이동한 파일은 열기에서 다시 선택해주세요.".into())
                    } else if let Some(host) = &host { host.open_path(path.clone()) }
                    else { Err("캔버스가 아직 준비되지 않았습니다.".into()) };
                    if let Err(error) = result { live_ink.publish_activation_notice(error); }
                }
            },
            option { value: "", disabled: true, "최근 그림" }
            for (index, label) in entries.iter().enumerate() {
                option { value: "{index}", "{label}" }
            }
        }
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
        let desktop = desktop.clone();
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
                std::rc::Rc::new(move || {
                    let _ = desktop.webview.evaluate_script(
                        "document.activeElement?.blur?.(); document.querySelector('main.app-shell')?.focus({ preventScroll: true });",
                    );
                }),
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

pub(crate) fn elapsed_since_launch() -> u128 {
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

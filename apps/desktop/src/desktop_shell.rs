use dioxus::prelude::{Element, VirtualDom};
use dioxus_desktop::{Config, WindowBuilder};
use nyatidraw_api::LayerId;

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use crate::live_ink::LiveInkBridge;

const INPUT_QUEUE_CAPACITY: usize = 512;
const INITIAL_ACTIVE_LAYER: LayerId = LayerId(1);

/// Launches the ordinary Dioxus Desktop `WebView` shell.
///
/// The `WebView` owns only editor chrome. On Windows, `with_on_window` creates
/// a sibling child HWND which owns the WGPU surface and receives raw pen and
/// mouse messages without crossing the DOM or IPC boundary.
#[cfg(windows)]
pub(crate) fn launch(app: fn() -> Element, instance: crate::single_instance::PrimaryInstance) {
    launch_with_instance(app, Some(instance));
}

#[cfg(not(windows))]
pub(crate) fn launch(app: fn() -> Element) {
    launch_with_instance(app, None);
}

fn launch_with_instance(
    app: fn() -> Element,
    #[cfg(windows)] mut instance: Option<crate::single_instance::PrimaryInstance>,
    #[cfg(not(windows))] _instance: Option<()>,
) {
    let live_ink = LiveInkBridge::with_capacity(INPUT_QUEUE_CAPACITY, INITIAL_ACTIVE_LAYER);
    #[cfg(windows)]
    crate::updates::initialize(live_ink.clone());
    live_ink.enable_layout_persistence();
    let context_ink = live_ink.clone();
    #[cfg(windows)]
    let exit_host = std::sync::Arc::new(std::sync::Mutex::new(
        None::<crate::render_host::RenderHost>,
    ));
    #[cfg(windows)]
    let window_exit_host = exit_host.clone();
    let window = WindowBuilder::new()
        .with_title("NyatiDraw")
        .with_inner_size(dioxus_desktop::LogicalSize::new(1280.0, 800.0))
        .with_maximized(true);
    #[cfg(windows)]
    let window = parent_presentation_trial(window);
    let config = Config::new()
        // Windows' native file-drop handler intercepts HTML layer drag/drop.
        .with_disable_drag_drop_handler(true)
        .with_window(window)
        .with_icon(
            dioxus_desktop::tao::window::Icon::from_rgba(
                include_bytes!("../assets/nyatidraw.rgba").to_vec(),
                64,
                64,
            )
            .expect("checked-in NyatiDraw icon is 64 by 64 RGBA"),
        )
        .with_menu(None::<dioxus_desktop::muda::Menu>)
        .with_parent_composition(cfg!(windows))
        .with_background_color(if cfg!(windows) {
            (0, 0, 0, 0)
        } else {
            (24, 24, 24, 255)
        })
        .with_on_window(move |window, vdom: &mut VirtualDom| {
            vdom.insert_any_root_context(Box::new(context_ink.clone()));

            #[cfg(windows)]
            {
                use dioxus_desktop::tao::platform::windows::WindowExtWindows as _;

                crate::window_placement::place_on_pointer_monitor(HWND(
                    window.hwnd() as *mut c_void
                ));

                let instance = instance
                    .take()
                    .expect("Windows launch creates exactly one primary desktop window");
                match crate::desktop_canvas::DesktopCanvasHandle::create(
                    HWND(window.hwnd() as *mut c_void),
                    Some(std::sync::Arc::downgrade(
                        &(window.clone() as std::sync::Arc<dyn Send + Sync>),
                    )),
                    &context_ink,
                    instance,
                    Some(&window_exit_host),
                ) {
                    Ok(canvas) => vdom.insert_any_root_context(Box::new(canvas)),
                    Err(error) => {
                        eprintln!("desktop-shell event=canvas-host-failed error={error}");
                        context_ink.publish_workspace_error(error);
                    }
                }
            }
        });

    #[cfg(windows)]
    let config = config.with_exit_guard(move || {
        let host = exit_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        host.is_none_or(|host| host.exit_ready())
    });

    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(app);
}

#[cfg(windows)]
fn parent_presentation_trial(window: WindowBuilder) -> WindowBuilder {
    use dioxus_desktop::tao::platform::windows::WindowBuilderExtWindows as _;

    let enabled = crate::desktop_canvas::composition_host_enabled();
    if enabled {
        println!(
            "desktop-shell event=composition-host-trial parent_no_redirection=true canvas_target=parent-bottom web_target=parent-top"
        );
    }
    window.with_no_redirection_bitmap(enabled)
}

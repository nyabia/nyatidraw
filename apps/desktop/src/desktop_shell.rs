use dioxus::prelude::{Element, VirtualDom};
use dioxus_desktop::{Config, WindowBuilder};
use nyatidraw_api::LayerId;

#[cfg(windows)]
use std::ffi::c_void;

#[cfg(windows)]
use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    UI::WindowsAndMessaging::{
        GetCursorPos, SW_MAXIMIZE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, ShowWindow,
    },
};

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
    let config = Config::new()
        // Windows' native file-drop handler intercepts HTML layer drag/drop.
        .with_disable_drag_drop_handler(true)
        .with_window(
            WindowBuilder::new()
                .with_title("NyatiDraw")
                .with_inner_size(dioxus_desktop::LogicalSize::new(1280.0, 800.0))
                .with_maximized(true),
        )
        .with_menu(None::<dioxus_desktop::muda::Menu>)
        .with_background_color((24, 24, 24, 255))
        .with_on_window(move |window, vdom: &mut VirtualDom| {
            vdom.insert_any_root_context(Box::new(context_ink.clone()));

            #[cfg(windows)]
            {
                use dioxus_desktop::tao::platform::windows::WindowExtWindows as _;

                place_on_pointer_monitor(window.hwnd());

                let instance = instance
                    .take()
                    .expect("Windows launch creates exactly one primary desktop window");
                match crate::desktop_canvas::DesktopCanvasHandle::create(
                    window.hwnd(),
                    &context_ink,
                    instance,
                ) {
                    Ok(canvas) => vdom.insert_any_root_context(Box::new(canvas)),
                    Err(error) => {
                        eprintln!("desktop-shell event=canvas-host-failed error={error}");
                        context_ink.publish_workspace_error(error);
                    }
                }
            }
        });

    dioxus::LaunchBuilder::desktop()
        .with_cfg(config)
        .launch(app);
}

/// Positions the restored window on the monitor containing the cursor before
/// asking Windows to maximize it. `WindowBuilder::with_maximized(true)` only
/// requests the OS default monitor, which is not necessarily the monitor the
/// user is currently working on. This is deliberately best-effort: a failed
/// probe must not prevent the `WebView` or native canvas from starting.
#[cfg(windows)]
fn place_on_pointer_monitor(parent: isize) {
    let hwnd = HWND(parent as *mut c_void);
    let mut cursor = POINT::default();
    // SAFETY: `cursor` is writable storage owned by this call.
    if let Err(error) = unsafe { GetCursorPos(&raw mut cursor) } {
        eprintln!(
            "desktop-shell event=pointer-monitor-probe-failed stage=get-cursor-pos error={error}"
        );
        return;
    }

    // SAFETY: the point is supplied by the OS and the nearest-monitor fallback
    // also handles a transient cursor position outside a monitor work area.
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        eprintln!("desktop-shell event=pointer-monitor-probe-failed stage=monitor-from-point");
        return;
    }

    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>())
            .expect("MONITORINFO size fits in u32"),
        ..Default::default()
    };
    // SAFETY: `monitor` is a live HMONITOR returned by MonitorFromPoint and
    // `info` has its required cbSize initialized.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        eprintln!("desktop-shell event=pointer-monitor-probe-failed stage=get-monitor-info");
        return;
    }
    let work = info.rcWork;
    let width = work.right.saturating_sub(work.left).max(1);
    let height = work.bottom.saturating_sub(work.top).max(1);

    // A maximized window's restored rectangle determines which monitor
    // Windows chooses. Restore first, place that rectangle on the target work
    // area, then request maximization. No activation or z-order change is
    // needed.
    // SAFETY: the parent HWND is live and owned by the Dioxus Desktop window.
    let _ = unsafe { ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_RESTORE) };
    // SAFETY: `hwnd` is the live tao parent from this callback and all
    // rectangle values came from the OS.
    if let Err(error) = unsafe {
        SetWindowPos(
            hwnd,
            None,
            work.left,
            work.top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
    } {
        eprintln!(
            "desktop-shell event=pointer-monitor-position-failed stage=set-window-pos error={error}"
        );
        // We restored the window immediately before positioning it. Reapply
        // the builder's requested fallback state even when placement fails.
        // SAFETY: the parent HWND remains live for this callback.
        let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
        return;
    }

    // SAFETY: the parent HWND is live and owned by the Dioxus Desktop window.
    let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
    println!(
        "desktop-shell event=pointer-monitor-selected cursor=({}, {}) work_area=({}, {}, {}, {})",
        cursor.x, cursor.y, work.left, work.top, work.right, work.bottom,
    );
}

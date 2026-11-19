use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
    UI::WindowsAndMessaging::{
        GetCursorPos, SW_MAXIMIZE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, ShowWindow,
    },
};

/// Maximize on the monitor containing the cursor, retaining the default
/// monitor when placement information is unavailable.
pub(crate) fn place_on_pointer_monitor(hwnd: HWND) {
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
    // SAFETY: the application owns this parent window.
    let _ = unsafe { ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_RESTORE) };
    // SAFETY: `hwnd` is the live parent from this callback and all
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

    // SAFETY: the application owns this parent window.
    let _ = unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
    println!(
        "desktop-shell event=pointer-monitor-selected cursor=({}, {}) work_area=({}, {}, {}, {})",
        cursor.x, cursor.y, work.left, work.top, work.right, work.bottom,
    );
}

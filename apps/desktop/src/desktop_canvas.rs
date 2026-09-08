//! Windows child-HWND host for the native WGPU drawing surface.
//!
//! Dioxus Desktop owns the parent `WebView` and editor chrome. This child window
//! sits above the `WebView` only inside the canvas rectangle, receives raw
//! `WM_POINTER` and mouse messages, and presents the existing GPU drawing engine.

use std::{
    ffi::c_void,
    num::NonZeroIsize,
    sync::{Arc, Mutex},
};

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, Win32WindowHandle, WindowsDisplayHandle,
};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient},
        System::LibraryLoader::GetModuleHandleW,
        UI::HiDpi::GetDpiForWindow,
        UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_SPACE},
        UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        UI::WindowsAndMessaging::{
            CREATESTRUCTW, CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CreateWindowExW, DefWindowProcW,
            DestroyWindow, GWLP_USERDATA, GetClientRect, GetMessagePos, GetMessageTime, GetParent,
            GetWindowLongPtrW, HWND_TOP, IDC_ARROW, LoadCursorW, MSG, PostMessageW, RegisterClassW,
            SW_HIDE, SW_SHOWNA, SWP_NOACTIVATE, SetTimer, SetWindowLongPtrW, SetWindowPos,
            ShowWindow, WINDOW_EX_STYLE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE, WM_ERASEBKGND,
            WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
            WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_NCDESTROY, WM_PAINT,
            WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE, WM_SIZE,
            WM_TIMER, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
        },
    },
    core::w,
};

use crate::{
    elapsed_since_launch,
    live_ink::{CanvasViewportSnapshot, CloseStatus, LiveInkBridge},
    native_canvas::SharedGpuCanvas,
    render_host::{Geometry, RenderHost, RenderLifetime},
    single_instance::{ActivationInbox, PrimaryInstance},
};

const WM_NAYATI_REDRAW: u32 = WM_APP + 0x4e1;
// 0x4e2 belongs to the single-instance activation inbox.
const WM_NAYATI_CLOSE: u32 = WM_APP + 0x4e3;
const WM_NAYATI_REOPEN: u32 = WM_APP + 0x4e4;
const WM_NAYATI_SAVE_AS: u32 = WM_APP + 0x4e5;
const CLOSE_SUBCLASS: usize = 0x4e59;
const CLOSE_TIMER: usize = 0x4e59;

#[derive(Clone)]
pub(crate) struct DesktopCanvasHandle {
    hwnd: HWND,
    live_ink: LiveInkBridge,
    activation: ActivationInbox,
    lifetime: Arc<RenderLifetime>,
}

impl DesktopCanvasHandle {
    pub(crate) fn create(
        parent_window: &Arc<dioxus_desktop::tao::window::Window>,
        live_ink: &LiveInkBridge,
        instance: PrimaryInstance,
        exit_host: &Arc<Mutex<Option<RenderHost>>>,
    ) -> Result<Self, String> {
        use dioxus_desktop::tao::platform::windows::WindowExtWindows as _;
        let parent = HWND(parent_window.hwnd() as *mut c_void);
        register_canvas_class()?;

        // WM_NCCREATE takes this Option exactly once. If creation later fails,
        // WM_NCDESTROY owns the taken box; if NCCREATE never ran, we still own it.
        let mut transfer = Some(Box::new(CanvasWindowState::new(live_ink.clone(), instance)));
        let instance = module_instance()?;
        // SAFETY: `transfer` stays live throughout synchronous window creation.
        // The registered `canvas_wnd_proc` takes its box only at WM_NCCREATE;
        // adopted state belongs to the HWND until WM_NCDESTROY reclaims it.
        let hwnd = match unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("NyatiDrawWgpuCanvas"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(instance),
                Some((&raw mut transfer).cast_const().cast()),
            )
        } {
            Ok(hwnd) => hwnd,
            Err(error) => {
                return Err(format!("create child canvas HWND: {error}"));
            }
        };

        // SAFETY: the HWND is live and its user data was installed by
        // WM_NCCREATE before CreateWindowExW returned.
        let state = unsafe { canvas_state(hwnd) }
            .ok_or_else(|| "child canvas state was not installed".to_owned())?;
        let host = RenderHost::new(parent_window, hwnd.0 as isize, live_ink.clone());
        state.host = Some(host.clone());

        state.activation.attach_windows(parent, hwnd);
        let close = Box::into_raw(Box::new(CloseWindowState {
            child: hwnd,
            live_ink: live_ink.clone(),
            host: host.clone(),
        }));
        // SAFETY: the parent and child share this UI thread. The subclass owns
        // the allocation until parent WM_NCDESTROY, independently of child state.
        if !unsafe {
            SetWindowSubclass(parent, Some(close_wnd_proc), CLOSE_SUBCLASS, close as usize)
        }
        .as_bool()
        {
            // SAFETY: failed installation did not transfer allocation ownership.
            drop(unsafe { Box::from_raw(close) });
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err("install parent close handler".into());
        }

        let redraw_hwnd = hwnd.0 as isize;
        live_ink.set_redraw_notifier(Arc::new(move || {
            // SAFETY: posting to a stale HWND simply returns an error, which
            // is intentionally ignored during shutdown.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(redraw_hwnd as *mut c_void)),
                    WM_NAYATI_REDRAW,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }));
        *exit_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host.clone());
        unsafe { SetTimer(Some(hwnd), CLOSE_TIMER, 50, None) };
        state.resize(hwnd);

        println!(
            "desktop-shell event=native-canvas-created elapsed_ms={} input=child-hwnd render=wgpu",
            elapsed_since_launch(),
        );
        Ok(Self {
            hwnd,
            live_ink: live_ink.clone(),
            activation: state.activation.clone(),
            lifetime: Arc::new(RenderLifetime(host)),
        })
    }

    pub(crate) fn start_render_worker(&self) {
        self.lifetime.0.start();
    }

    pub(crate) fn set_geometry(
        &self,
        x_css: f64,
        y_css: f64,
        width_css: f64,
        height_css: f64,
        scale: f64,
    ) {
        if self.live_ink.is_closing() {
            self.hide();
            return;
        }
        let values = [x_css, y_css, width_css, height_css, scale];
        if values.iter().any(|value| !value.is_finite()) || scale <= 0.0 {
            self.hide();
            return;
        }
        let x = saturating_i32(x_css * scale);
        let y = saturating_i32(y_css * scale);
        let width = saturating_i32(width_css * scale).max(1);
        let height = saturating_i32(height_css * scale).max(1);
        if width_css <= 0.0 || height_css <= 0.0 {
            self.hide();
            return;
        }

        // SAFETY: geometry is finite and clamped to Win32's signed range.
        if let Err(error) = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOP),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE,
            )
        } {
            eprintln!("desktop-shell event=canvas-position-failed error={error}");
            return;
        }
        // WebView2 is created after the canvas child. Raising and showing the
        // canvas here establishes the required sibling z-order.
        // SAFETY: the child HWND is owned by this desktop window.
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNA) };
        if let Some(state) = unsafe { canvas_state(self.hwnd) } {
            state.resize(self.hwnd);
        }
    }

    pub(crate) fn hide(&self) {
        self.live_ink.invalidate_canvas_viewport();
        // SAFETY: hiding a live or already-destroyed child is harmless.
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    pub(crate) fn open_path(&self, path: std::path::PathBuf) -> Result<(), String> {
        if self.live_ink.is_closing() {
            return Err("저장을 마친 뒤 파일을 열어주세요".into());
        }
        self.activation.open_from_dialog(path)
    }

    pub(crate) fn save_as(&self, path: std::path::PathBuf) -> Result<(), String> {
        crate::save_as::validate_target(&path)?;
        self.live_ink.queue_save_as(path)?;
        // SAFETY: posts a payload-free wakeup to the owned child window.
        if let Err(error) =
            unsafe { PostMessageW(Some(self.hwnd), WM_NAYATI_SAVE_AS, WPARAM(0), LPARAM(0)) }
        {
            self.live_ink.take_save_as();
            self.live_ink.finish_save_as();
            return Err(error.to_string());
        }
        Ok(())
    }

    pub(crate) fn request_close(&self) {
        // SAFETY: request the same writer-draining close as the titlebar X.
        if let Ok(parent) = unsafe { GetParent(self.hwnd) } {
            if self
                .live_ink
                .push_ui_editor_command(nyatidraw_api::EditorCommand::Project(
                    nyatidraw_api::ProjectCommand::Save,
                ))
                .is_err()
            {
                self.live_ink.publish_activation_notice(
                    "저장 요청을 처리 중입니다. 업데이트를 잠시 뒤 다시 시도해주세요".into(),
                );
                return;
            }
            if !crate::updates::request_restart() {
                return;
            }
            let _ = unsafe { PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
    }

    pub(crate) fn reopen_after_close_failure(&self) {
        // SAFETY: asynchronous, pointer-free message to the owned child.
        let _ = unsafe { PostMessageW(Some(self.hwnd), WM_NAYATI_REOPEN, WPARAM(0), LPARAM(0)) };
    }

    pub(crate) fn confirm_failed_close(&self) {
        if matches!(self.live_ink.close_status(), CloseStatus::Failed { .. }) {
            self.live_ink.publish_close_status(CloseStatus::Ready);
            // SAFETY: the child remains alive until the parent closes.
            if let Ok(parent) = unsafe { GetParent(self.hwnd) } {
                let _ = unsafe { PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0)) };
            }
        }
    }
}

struct CloseWindowState {
    child: HWND,
    live_ink: LiveInkBridge,
    host: RenderHost,
}

unsafe extern "system" fn close_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        // SAFETY: remove our chain entry before reclaiming its unique allocation.
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(close_wnd_proc), id) };
        drop(unsafe { Box::from_raw(data as *mut CloseWindowState) });
        return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    }
    // SAFETY: only this parent subclass owns data, until WM_NCDESTROY above.
    let state = unsafe { &*(data as *const CloseWindowState) };
    if message == WM_CLOSE && state.live_ink.close_status() != CloseStatus::Ready {
        if state.live_ink.begin_close() {
            let _ =
                unsafe { PostMessageW(Some(state.child), WM_NAYATI_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        return LRESULT(0);
    }
    if message == WM_CLOSE
        && let Err(error) = crate::updates::apply_after_saved_close()
    {
        state.live_ink.publish_activation_notice(error);
        state.live_ink.publish_close_status(CloseStatus::Failed {
            project_saved: true,
            project_path: String::new(),
        });
        let _ = unsafe { PostMessageW(Some(state.child), WM_NAYATI_REOPEN, WPARAM(0), LPARAM(0)) };
        return LRESULT(0);
    }
    if message == WM_CLOSE && !state.host.finished() {
        state.host.retire();
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

#[allow(clippy::cast_possible_truncation)]
fn saturating_i32(value: f64) -> i32 {
    if value <= f64::from(i32::MIN) {
        i32::MIN
    } else if value >= f64::from(i32::MAX) {
        i32::MAX
    } else {
        value.round() as i32
    }
}

fn module_instance() -> Result<HINSTANCE, String> {
    // SAFETY: a null module name asks for the current executable module.
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("get executable module: {error}"))?;
    Ok(HINSTANCE(module.0))
}

fn register_canvas_class() -> Result<(), String> {
    static REGISTERED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            let instance = module_instance()?;
            // SAFETY: IDC_ARROW is a process-independent system cursor.
            let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
                .map_err(|error| format!("load canvas cursor: {error}"))?;
            let class = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
                lpfnWndProc: Some(canvas_wnd_proc),
                hInstance: instance,
                hCursor: cursor,
                lpszClassName: w!("NyatiDrawWgpuCanvas"),
                ..Default::default()
            };
            // SAFETY: `class` and its static class name remain valid for the
            // duration of registration.
            let atom = unsafe { RegisterClassW(&raw const class) };
            (atom != 0)
                .then_some(())
                .ok_or_else(|| "register child canvas window class".to_owned())
        })
        .clone()
}

struct CanvasWindowState {
    live_ink: LiveInkBridge,
    // The HWND owns the single-instance guard for its full lifetime. Dioxus
    // drops its one-shot window configuration immediately after construction.
    _instance: PrimaryInstance,
    activation: ActivationInbox,
    viewport: WindowsViewportInput,
    mouse: WindowsMouseInput,
    pen: WindowsPenInput,
    host: Option<RenderHost>,
    reopening: bool,
}

impl CanvasWindowState {
    fn new(live_ink: LiveInkBridge, instance: PrimaryInstance) -> Self {
        let activation = instance.inbox();
        Self {
            viewport: WindowsViewportInput::new(live_ink.clone()),
            mouse: WindowsMouseInput::new(live_ink.clone()),
            pen: WindowsPenInput::new(live_ink.clone()),
            live_ink,
            _instance: instance,
            activation,
            host: None,
            reopening: false,
        }
    }

    fn resize(&mut self, hwnd: HWND) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(hwnd, &raw mut rect) }.is_err() {
            self.live_ink.invalidate_canvas_viewport();
            return;
        }
        let width = u32::try_from(rect.right.saturating_sub(rect.left)).unwrap_or(0);
        let height = u32::try_from(rect.bottom.saturating_sub(rect.top)).unwrap_or(0);
        let scale = f64::from(unsafe { GetDpiForWindow(hwnd) }.max(96)) / 96.0;
        if self.live_ink.publish_canvas_surface(width, height, scale) {
            println!(
                "native-input event=canvas-surface-updated width={width} height={height} scale={scale} owner=ui"
            );
        }
        if let Some(host) = &self.host {
            host.geometry(Geometry {
                width,
                height,
                scale,
                epoch: self.live_ink.canvas_viewport_snapshot().geometry_epoch,
            });
        }
    }

    fn render(&mut self, hwnd: HWND) {
        self.live_ink.begin_redraw();
        if let Some(host) = &self.host {
            host.wake();
        }
        self.poll_close(hwnd);
    }

    fn process_activations(&mut self) {
        while let Some(path) = self.activation.take_activation() {
            if self.live_ink.is_closing() || self.live_ink.is_saving_as() {
                self.live_ink.publish_activation_notice(
                    "저장 후 종료 중입니다. 종료가 끝난 뒤 파일을 다시 열어주세요".into(),
                );
                continue;
            }
            let Some(host) = self.host.as_ref() else {
                self.live_ink.publish_activation_notice(
                    "캔버스가 아직 준비되지 않아 파일을 열 수 없습니다".into(),
                );
                return;
            };
            if let Err(error) = host.activate(path) {
                self.live_ink.publish_activation_notice(error);
            }
        }
    }

    fn begin_close(&mut self, hwnd: HWND) {
        if let Some(host) = self.host.as_ref() {
            if host.has_started() && host.finished() {
                self.live_ink.publish_close_status(CloseStatus::Failed {
                    project_saved: false,
                    project_path: String::new(),
                });
            } else {
                host.close();
            }
        } else {
            self.live_ink.publish_close_status(CloseStatus::Ready);
        }
        // A UI timer observes thread completion without joining a live worker.
        unsafe { SetTimer(Some(hwnd), CLOSE_TIMER, 50, None) };
        self.poll_close(hwnd);
    }

    fn process_save_as(&mut self) {
        if let Some(host) = &self.host {
            host.save_as();
        } else {
            self.live_ink.take_save_as();
            self.live_ink.finish_save_as();
            self.live_ink
                .publish_activation_notice("캔버스가 아직 준비되지 않았습니다".into());
        }
    }

    fn poll_close(&mut self, hwnd: HWND) {
        if self.reopening && !self.live_ink.is_closing() {
            self.reopening = false;
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
            return;
        }
        match self.live_ink.close_status() {
            CloseStatus::Ready => {
                if let Ok(parent) = unsafe { GetParent(hwnd) } {
                    let _ = unsafe { PostMessageW(Some(parent), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                }
            }
            CloseStatus::Failed { .. } => {
                crate::updates::cancel_restart();
            }
            _ => {}
        }
    }

    fn observe_input(&mut self, message: &MSG) -> bool {
        match message.message {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP | WM_POINTERCAPTURECHANGED => {
                self.pen.observe(message)
            }
            WM_LBUTTONDOWN | WM_MOUSEMOVE | WM_LBUTTONUP => self.mouse.observe(message),
            _ => false,
        }
    }

    fn observe_viewport(&mut self, hwnd: HWND, message: &MSG) -> bool {
        self.viewport.observe(hwnd, message)
    }

    fn reopen_after_close_failure(&mut self) {
        if let Some(host) = &self.host {
            if host.finished() {
                self.live_ink.publish_activation_notice(
                    "렌더러가 종료되었습니다. 앱을 종료한 뒤 프로젝트를 다시 열어주세요".into(),
                );
                return;
            }
            self.reopening = true;
            host.reopen();
        }
    }
}

unsafe extern "system" fn canvas_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: WM_NCCREATE lParam is a readable CREATESTRUCTW supplied by
        // CreateWindowExW for this call.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        // SAFETY: the creator's stack Option lives for CreateWindowExW. Only
        // NCCREATE takes it; NCDESTROY exclusively reclaims the resulting box.
        let transfer = unsafe {
            &mut *create
                .lpCreateParams
                .cast::<Option<Box<CanvasWindowState>>>()
        };
        let Some(state) = transfer.take() else {
            return LRESULT(0);
        };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize) };
        return LRESULT(1);
    }

    if message == WM_NCDESTROY {
        // SAFETY: read and clear the unique Box pointer installed during
        // WM_NCCREATE before any default teardown can trigger more dispatch.
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CanvasWindowState;
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        if !pointer.is_null() {
            // SAFETY: WM_NCDESTROY reclaims the HWND-owned allocation once.
            let state = unsafe { Box::from_raw(pointer) };
            state.live_ink.invalidate_canvas_viewport();
            // The actor's strong Tao parent anchor prevents parent/child
            // destruction until every surface is gone. No join is needed here.
            debug_assert!(state.host.as_ref().is_none_or(RenderHost::surface_retired));
            println!("desktop-render event=canvas-hwnd-destroyed surface-retired=true");
        }
        // SAFETY: default final non-client destruction processing.
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    // SAFETY: the pointer was installed at WM_NCCREATE and remains owned by
    // this HWND until WM_NCDESTROY.
    if message == WM_NAYATI_CLOSE {
        // These calls may synchronously reenter the window procedure. Hide
        // the native sibling and release capture before borrowing its state.
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        let _ = unsafe { ReleaseCapture() };
    }
    let state = unsafe { canvas_state(hwnd) };
    if let Some(state) = state {
        match message {
            WM_NAYATI_CLOSE => {
                state.begin_close(hwnd);
                return LRESULT(0);
            }
            WM_TIMER if wparam.0 == CLOSE_TIMER => {
                state.poll_close(hwnd);
                return LRESULT(0);
            }
            WM_NAYATI_REOPEN => {
                state.reopen_after_close_failure();
                return LRESULT(0);
            }
            WM_SIZE => {
                state.resize(hwnd);
                state.render(hwnd);
                return LRESULT(0);
            }
            WM_NAYATI_REDRAW => {
                // Keep the bridge's pending bit set until WM_PAINT begins.
                // Posted wakeups must not run a surface wait ahead of queued
                // pointer messages. Windows coalesces the invalid region and
                // paints after higher-priority input/posted messages drain.
                // SAFETY: invalidate this live child without synchronous paint
                // or background erasure; WM_PAINT owns BeginPaint/EndPaint.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                return LRESULT(0);
            }
            message if message == crate::single_instance::activation_message() => {
                state.process_activations();
                state.render(hwnd);
                return LRESULT(0);
            }
            WM_NAYATI_SAVE_AS => {
                state.process_save_as();
                state.render(hwnd);
                return LRESULT(0);
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: `paint` is valid for the BeginPaint/EndPaint pair.
                unsafe { BeginPaint(hwnd, &raw mut paint) };
                state.render(hwnd);
                // SAFETY: balances the BeginPaint call above.
                let _ = unsafe { EndPaint(hwnd, &raw const paint) };
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_POINTERDOWN
            | WM_POINTERUPDATE
            | WM_POINTERUP
            | WM_POINTERCAPTURECHANGED
            | WM_LBUTTONDOWN
            | WM_MOUSEMOVE
            | WM_LBUTTONUP
            | WM_MBUTTONDOWN
            | WM_MBUTTONUP
            | WM_MOUSEWHEEL
            | WM_MOUSEHWHEEL
            | WM_KEYDOWN
            | WM_CAPTURECHANGED => {
                if state.live_ink.is_closing() {
                    return LRESULT(0);
                }
                let msg = current_message(hwnd, message, wparam, lparam);
                if state.observe_viewport(hwnd, &msg) {
                    state.live_ink.request_redraw();
                    return LRESULT(0);
                }
                if state.observe_input(&msg) {
                    // Admission already requests a wakeup. This is idempotent
                    // for capture/phase-only paths, without entering rendering
                    // from this input dispatch.
                    state.live_ink.request_redraw();
                }
            }
            _ => {}
        }
    }

    // SAFETY: unhandled messages use the platform default behavior.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

#[derive(Clone, Copy)]
struct ViewportDrag {
    last_client_px: [i32; 2],
}

struct WindowsViewportInput {
    live_ink: LiveInkBridge,
    drag: Option<ViewportDrag>,
    zoom_wheel_remainder: i32,
}

impl WindowsViewportInput {
    const WHEEL_DELTA: i32 = 120;
    const WHEEL_PAN_LOGICAL: f64 = 64.0;
    const MK_CONTROL_MASK: usize = 0x0008;
    const MK_SHIFT_MASK: usize = 0x0004;

    fn new(live_ink: LiveInkBridge) -> Self {
        Self {
            live_ink,
            drag: None,
            zoom_wheel_remainder: 0,
        }
    }

    /// Returns true when the message belongs to viewport navigation and must
    /// not reach the paint input path.
    #[allow(clippy::too_many_lines)]
    fn observe(&mut self, hwnd: HWND, message: &MSG) -> bool {
        match message.message {
            WM_KEYDOWN => {
                let command = match message.wParam.0 {
                    0x1b => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::CancelGesture,
                    )),
                    0x42 => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::CycleBrushFamily,
                    )),
                    0x47 => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::Select(nyatidraw_api::DrawingTool::Move),
                    )),
                    0x45 => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::Select(nyatidraw_api::DrawingTool::Eraser),
                    )),
                    0x57 => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::Select(nyatidraw_api::DrawingTool::Wand),
                    )),
                    0x4c => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::Select(nyatidraw_api::DrawingTool::Lasso),
                    )),
                    0x46 => Some(nyatidraw_api::EditorCommand::Tool(
                        nyatidraw_api::ToolCommand::Select(if shift_is_down() {
                            nyatidraw_api::DrawingTool::Gradient
                        } else {
                            nyatidraw_api::DrawingTool::Fill
                        }),
                    )),
                    0x53 => Some(nyatidraw_api::EditorCommand::Project(
                        nyatidraw_api::ProjectCommand::Save,
                    )),
                    0x5a if shift_is_down() => Some(nyatidraw_api::EditorCommand::History(
                        nyatidraw_api::HistoryCommand::Redo,
                    )),
                    0x5a => Some(nyatidraw_api::EditorCommand::History(
                        nyatidraw_api::HistoryCommand::Undo,
                    )),
                    _ => None,
                };
                if let Some(command) = command {
                    let _ = self.live_ink.push_ui_editor_command(command);
                    true
                } else {
                    false
                }
            }
            WM_MBUTTONDOWN => {
                self.drag = Some(ViewportDrag {
                    last_client_px: client_point(message.lParam),
                });
                // SAFETY: capture is scoped to this live child HWND and is
                // released on button-up or capture loss during destruction.
                let _ = unsafe { SetCapture(hwnd) };
                true
            }
            WM_LBUTTONDOWN if self.live_ink.navigation_tool() || space_is_down() => {
                self.drag = Some(ViewportDrag {
                    last_client_px: client_point(message.lParam),
                });
                // SAFETY: capture is scoped to this live child HWND and is
                // released on button-up or capture loss.
                let _ = unsafe { SetCapture(hwnd) };
                true
            }
            WM_POINTERDOWN if self.live_ink.navigation_tool() || space_is_down() => {
                self.drag = Some(ViewportDrag {
                    last_client_px: screen_message_point(hwnd, message.lParam),
                });
                // SAFETY: capture is scoped to this live child HWND.
                let _ = unsafe { SetCapture(hwnd) };
                true
            }
            WM_POINTERUPDATE if self.drag.is_some() => {
                let point = screen_message_point(hwnd, message.lParam);
                self.move_drag(hwnd, point);
                true
            }
            WM_MOUSEMOVE => {
                let Some(drag) = self.drag.as_mut() else {
                    return false;
                };
                let point = client_point(message.lParam);
                let delta = [
                    point[0].saturating_sub(drag.last_client_px[0]),
                    point[1].saturating_sub(drag.last_client_px[1]),
                ];
                drag.last_client_px = point;
                if delta == [0, 0] {
                    return true;
                }
                let scale = f64::from(
                    // SAFETY: this is the live canvas child handling the
                    // message; its DPI is valid for the duration of dispatch.
                    unsafe { GetDpiForWindow(hwnd) }.max(96),
                ) / 96.0;
                self.pan(
                    saturating_i32(f64::from(delta[0]) / scale),
                    saturating_i32(f64::from(delta[1]) / scale),
                );
                true
            }
            WM_MBUTTONUP => {
                if self.drag.take().is_some() {
                    // SAFETY: balances the capture started for the viewport
                    // drag. A prior external release is harmless.
                    let _ = unsafe { ReleaseCapture() };
                }
                true
            }
            WM_LBUTTONUP if self.drag.is_some() => {
                self.drag = None;
                // SAFETY: balances capture from a Space/Move left-button drag.
                let _ = unsafe { ReleaseCapture() };
                true
            }
            WM_POINTERUP if self.drag.is_some() => {
                self.drag = None;
                // SAFETY: balances capture from a pen Move/Space drag.
                let _ = unsafe { ReleaseCapture() };
                true
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                true
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = wheel_delta(message.wParam);
                let flags = message.wParam.0 & 0xffff;
                if flags & Self::MK_CONTROL_MASK != 0 {
                    self.zoom_wheel_remainder = self.zoom_wheel_remainder.saturating_add(delta);
                    let steps = self.zoom_wheel_remainder / Self::WHEEL_DELTA;
                    self.zoom_wheel_remainder %= Self::WHEEL_DELTA;
                    if steps != 0 {
                        let screen = client_point(message.lParam);
                        let mut focus = POINT {
                            x: screen[0],
                            y: screen[1],
                        };
                        // SAFETY: `hwnd` is the live child handling this wheel
                        // message and `focus` is a valid mutable screen point.
                        let _ = unsafe { ScreenToClient(hwnd, &raw mut focus) };
                        let scale = f64::from(
                            // SAFETY: the live child owns this dispatch.
                            unsafe { GetDpiForWindow(hwnd) }.max(96),
                        ) / 96.0;
                        self.command(nyatidraw_api::ViewportCommand::ZoomAt {
                            steps: i8::try_from(
                                steps.clamp(i32::from(i8::MIN), i32::from(i8::MAX)),
                            )
                            .expect("clamped wheel steps fit i8"),
                            logical_x: saturating_i32(f64::from(focus.x) / scale),
                            logical_y: saturating_i32(f64::from(focus.y) / scale),
                        });
                    }
                } else {
                    let distance = saturating_i32(
                        f64::from(delta) / f64::from(Self::WHEEL_DELTA) * Self::WHEEL_PAN_LOGICAL,
                    );
                    if message.message == WM_MOUSEHWHEEL || flags & Self::MK_SHIFT_MASK != 0 {
                        self.pan(-distance, 0);
                    } else {
                        self.pan(0, distance);
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn pan(&self, logical_x: i32, logical_y: i32) {
        if logical_x != 0 || logical_y != 0 {
            self.command(nyatidraw_api::ViewportCommand::PanBy {
                logical_x,
                logical_y,
            });
        }
    }

    fn move_drag(&mut self, hwnd: HWND, point: [i32; 2]) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let delta = [
            point[0].saturating_sub(drag.last_client_px[0]),
            point[1].saturating_sub(drag.last_client_px[1]),
        ];
        drag.last_client_px = point;
        if delta == [0, 0] {
            return;
        }
        let scale = f64::from(
            // SAFETY: this is the live child handling the message.
            unsafe { GetDpiForWindow(hwnd) }.max(96),
        ) / 96.0;
        self.pan(
            saturating_i32(f64::from(delta[0]) / scale),
            saturating_i32(f64::from(delta[1]) / scale),
        );
    }

    fn command(&self, command: nyatidraw_api::ViewportCommand) {
        if self
            .live_ink
            .push_ui_editor_command(nyatidraw_api::EditorCommand::Viewport(command))
            .is_err()
        {
            eprintln!("desktop-input event=viewport-command-dropped reason=queue-full");
        }
    }
}

fn space_is_down() -> bool {
    // SAFETY: GetKeyState reads thread input state and has no pointer contract.
    unsafe { GetKeyState(i32::from(VK_SPACE.0)) < 0 }
}

fn shift_is_down() -> bool {
    // SAFETY: GetKeyState reads thread input state and has no pointer contract.
    unsafe { GetKeyState(0x10) < 0 }
}

fn screen_message_point(hwnd: HWND, lparam: LPARAM) -> [i32; 2] {
    let packed = client_point(lparam);
    let mut point = POINT {
        x: packed[0],
        y: packed[1],
    };
    // SAFETY: point is a valid screen-coordinate buffer for the live child.
    let _ = unsafe { ScreenToClient(hwnd, &raw mut point) };
    [point.x, point.y]
}

fn client_point(lparam: LPARAM) -> [i32; 2] {
    let packed = lparam.0.cast_unsigned();
    [
        i32::from(u16::try_from(packed & 0xffff).unwrap().cast_signed()),
        i32::from(
            u16::try_from((packed >> 16) & 0xffff)
                .unwrap()
                .cast_signed(),
        ),
    ]
}

fn wheel_delta(wparam: WPARAM) -> i32 {
    let word = u16::try_from((wparam.0 >> 16) & 0xffff).unwrap();
    i32::from(word.cast_signed())
}

unsafe fn canvas_state(hwnd: HWND) -> Option<&'static mut CanvasWindowState> {
    // SAFETY: reading this HWND's user-data slot is valid during dispatch.
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CanvasWindowState;
    // SAFETY: the pointer, when non-null, refers to the Box owned by this HWND.
    unsafe { pointer.as_mut() }
}

fn current_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> MSG {
    // SAFETY: both functions read metadata for the message currently being
    // dispatched on this thread.
    let packed = unsafe { GetMessagePos() };
    let time = u32::from_ne_bytes(unsafe { GetMessageTime() }.to_ne_bytes());
    MSG {
        hwnd,
        message,
        wParam: wparam,
        lParam: lparam,
        time,
        pt: POINT {
            x: i32::from(
                u16::try_from(packed & 0xffff)
                    .expect("masked coordinate fits u16")
                    .cast_signed(),
            ),
            y: i32::from(
                u16::try_from(packed >> 16)
                    .expect("shifted coordinate fits u16")
                    .cast_signed(),
            ),
        },
    }
}

pub(crate) struct CanvasSurfaceRenderer {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) config: wgpu::SurfaceConfiguration,
    presenter: TexturePresenter,
    pub(crate) canvas: SharedGpuCanvas,
    pub(crate) scale: f64,
    geometry_epoch: u64,
    live_ink: LiveInkBridge,
    configured: bool,
    first_presented: bool,
}

impl Drop for CanvasSurfaceRenderer {
    fn drop(&mut self) {
        crate::performance::flush("canvas");
    }
}

impl CanvasSurfaceRenderer {
    pub(crate) fn new(hwnd: HWND, live_ink: LiveInkBridge) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let raw_window =
            Win32WindowHandle::new(NonZeroIsize::new(hwnd.0 as isize).ok_or("child HWND is null")?);
        // SAFETY: only render_host constructs this renderer. Its worker owns
        // a strong Tao parent anchor until this surface and all GPU state drop.
        // Only creation failure (before actor start) explicitly destroys child.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: RawDisplayHandle::Windows(WindowsDisplayHandle::new()),
                raw_window_handle: RawWindowHandle::Win32(raw_window),
            })
        }
        .map_err(|error| format!("create WGPU child surface: {error}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|error| format!("request WGPU adapter: {error}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nayati-desktop-canvas-device"),
            ..Default::default()
        }))
        .map_err(|error| format!("request WGPU device: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("WGPU surface exposes no texture format")?;
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Opaque);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 1,
            height: 1,
            present_mode,
            alpha_mode,
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
        };
        let presenter = TexturePresenter::new(&device, format);
        let mut canvas = SharedGpuCanvas::new(live_ink.clone());
        canvas.resume(&adapter, &device, &queue);
        let info = adapter.get_info();
        println!(
            "desktop-canvas event=gpu-ready backend={:?} adapter={:?} format={format:?} present={present_mode:?}",
            info.backend, info.name,
        );
        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            config,
            presenter,
            canvas,
            scale: 1.0,
            geometry_epoch: 0,
            live_ink,
            configured: false,
            first_presented: false,
        })
    }

    pub(crate) fn resize_geometry(&mut self, geometry: Geometry) {
        let Geometry {
            width,
            height,
            scale,
            epoch,
        } = geometry;
        if self.configured && self.geometry_epoch == epoch {
            return;
        }
        self.geometry_epoch = epoch;
        self.canvas.set_geometry_epoch(epoch);
        if width == 0 || height == 0 {
            self.configured = false;
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.scale = scale;
        self.surface.configure(&self.device, &self.config);
        self.configured = true;
    }

    pub(crate) fn activate_project(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        self.canvas.activate_project(
            &self.device,
            &self.queue,
            path,
            [self.config.width, self.config.height],
            self.scale,
        )
    }

    pub(crate) fn save_as(&mut self, path: std::path::PathBuf) -> Result<(), String> {
        self.canvas.save_as(
            &self.device,
            &self.queue,
            path,
            [self.config.width, self.config.height],
            self.scale,
        )
    }

    pub(crate) fn render(&mut self) -> Result<(), String> {
        use crate::performance::{Span, Stage};
        self.canvas.poll_save_as(&self.device, &self.queue);
        if !self.configured {
            return Ok(());
        }
        // CreateWindowExW briefly exposes the child at 1x1 before the DOM
        // observer publishes its real rectangle. Do not let that bootstrap
        // size initialize a permanently invalid (< 1%) fitted viewport.
        if self.config.width < 16 || self.config.height < 16 {
            return Ok(());
        }
        let _frame_timing = Span::new(Stage::Frame);
        let Some(display) = self
            .canvas
            .render(self.config.width, self.config.height, self.scale)
        else {
            return Ok(());
        };
        if self.live_ink.canvas_viewport_snapshot().geometry_epoch != self.geometry_epoch {
            return Ok(());
        }

        let acquire_timing = Span::new(Stage::SurfaceAcquire);
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => return Ok(()),
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err("WGPU surface is out of memory".to_owned());
            }
        };
        drop(acquire_timing);
        let present_timing = Span::new(Stage::SurfacePresent);
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.presenter
            .present(&self.device, &self.queue, &display.texture, &target);
        frame.present();
        drop(present_timing);
        // Admission changes only after this frame was submitted for present.
        // This is not first-visible-pixel proof. A concurrent geometry change
        // still invalidates this publication under the viewport mailbox lock.
        if let Some(viewport) = display.viewport {
            let _ = self.live_ink.publish_renderer_view(
                viewport.pan,
                viewport.zoom,
                viewport.rotation_radians,
                display.geometry_epoch,
            );
        }
        crate::performance::presented();
        if let Some(timing) = display.history_timing {
            timing.presented();
        }
        if !self.first_presented {
            self.first_presented = true;
            println!(
                "desktop-shell event=first-native-wgpu-present elapsed_ms={} width={} height={}",
                elapsed_since_launch(),
                self.config.width,
                self.config.height,
            );
        }
        Ok(())
    }
}

struct TexturePresenter {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
}

impl TexturePresenter {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nayati-desktop-present-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nayati-desktop-present-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nayati-desktop-present-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("desktop_present.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nayati-desktop-present-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nayati-desktop-present-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        Self {
            layout,
            sampler,
            pipeline,
        }
    }

    fn present(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        target: &wgpu::TextureView,
    ) {
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nayati-desktop-present-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("nayati-desktop-present-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nayati-desktop-present-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

#[derive(Clone, Copy)]
struct MouseStroke {
    mapping: nyatidraw_input_platform::ViewportSnapshot,
    last_document_point: nyatidraw_input::Point,
}

struct WindowsMouseInput {
    batch: nyatidraw_input_platform::WindowsMouseMessageBuffer,
    policy: nyatidraw_input::InStrokeViewportPolicy,
    live_ink: LiveInkBridge,
    active: Option<MouseStroke>,
    next_sequence: u64,
    timestamp_epoch_ms: u64,
    last_timestamp_ms: Option<u32>,
}

impl WindowsMouseInput {
    fn new(live_ink: LiveInkBridge) -> Self {
        Self {
            batch: nyatidraw_input_platform::WindowsMouseMessageBuffer::default(),
            policy: nyatidraw_input::InStrokeViewportPolicy::default(),
            live_ink,
            active: None,
            next_sequence: 1_u64 << 63,
            timestamp_epoch_ms: 0,
            last_timestamp_ms: None,
        }
    }

    fn observe(&mut self, message: &MSG) -> bool {
        // SAFETY: `message` is a live stack value for this call only.
        if let Err(error) = unsafe {
            nyatidraw_input_platform::record_win32_mouse_message_into(
                &mut self.batch,
                std::ptr::from_ref(message).cast(),
            )
        } {
            eprintln!("desktop-input event=mouse-error error={error:?}");
            return false;
        }
        let events = self.batch.events().to_vec();
        let mut admitted = false;
        for event in events {
            admitted |= self.admit(event);
        }
        admitted
    }

    fn admit(&mut self, event: nyatidraw_input_platform::WindowsMouseEvent) -> bool {
        match event.message {
            nyatidraw_input_platform::WindowsMouseMessage::Down => self.begin(event),
            nyatidraw_input_platform::WindowsMouseMessage::Move => self.move_to(event),
            nyatidraw_input_platform::WindowsMouseMessage::Up => self.end(event),
        }
    }

    fn begin(&mut self, event: nyatidraw_input_platform::WindowsMouseEvent) -> bool {
        if self.active.is_some() {
            let _ = self.cancel(event.timestamp_ms);
        }
        let viewport = self.live_ink.canvas_viewport_snapshot();
        let Some(mapping) = viewport.recorder_mapping() else {
            return false;
        };
        let Some(document) = viewport.document_point_for_client(event.position_client_px) else {
            return false;
        };
        if self.push(
            nyatidraw_input::PointerPhase::Begin,
            document,
            mapping.revision,
            event.timestamp_ms,
        ) {
            self.active = Some(MouseStroke {
                mapping,
                last_document_point: document,
            });
            true
        } else {
            self.active = None;
            false
        }
    }

    fn move_to(&mut self, event: nyatidraw_input_platform::WindowsMouseEvent) -> bool {
        let Some(active) = self.active else {
            return false;
        };
        let viewport = self.live_ink.canvas_viewport_snapshot();
        if viewport.revision != active.mapping.revision {
            return self.cancel(event.timestamp_ms);
        }
        let document = active.mapping.document_point(event.position_client_px);
        if let Some(active) = self.active.as_mut() {
            active.last_document_point = document;
        }
        if self.push(
            nyatidraw_input::PointerPhase::Move,
            document,
            active.mapping.revision,
            event.timestamp_ms,
        ) {
            true
        } else {
            self.active = None;
            false
        }
    }

    fn end(&mut self, event: nyatidraw_input_platform::WindowsMouseEvent) -> bool {
        if self.active.is_some_and(|active| {
            self.live_ink.canvas_viewport_snapshot().revision != active.mapping.revision
        }) {
            return self.cancel(event.timestamp_ms);
        }
        let Some(active) = self.active.take() else {
            return false;
        };
        self.push(
            nyatidraw_input::PointerPhase::End,
            active.mapping.document_point(event.position_client_px),
            active.mapping.revision,
            event.timestamp_ms,
        )
    }

    fn cancel(&mut self, timestamp_ms: u32) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        self.push(
            nyatidraw_input::PointerPhase::Cancel,
            active.last_document_point,
            active.mapping.revision,
            timestamp_ms,
        )
    }

    fn push(
        &mut self,
        phase: nyatidraw_input::PointerPhase,
        position_document: nyatidraw_input::Point,
        viewport_revision: u64,
        timestamp_ms: u32,
    ) -> bool {
        if let Some(last) = self.last_timestamp_ms
            && timestamp_ms < last
            && last.wrapping_sub(timestamp_ms) > u32::MAX / 2
        {
            self.timestamp_epoch_ms = self
                .timestamp_epoch_ms
                .saturating_add(u64::from(u32::MAX) + 1);
        }
        self.last_timestamp_ms = Some(timestamp_ms);
        if !self.policy.admit(phase, viewport_revision) {
            return false;
        }
        let sample = nyatidraw_input::StylusSample {
            sequence: self.next_sequence,
            timestamp_ns: self
                .timestamp_epoch_ms
                .saturating_add(u64::from(timestamp_ms))
                .saturating_mul(1_000_000),
            device_id: u64::MAX,
            phase,
            position_document,
            pressure: 1.0,
            tilt: None,
            twist_radians: None,
            tangential_pressure: None,
            buttons: if matches!(
                phase,
                nyatidraw_input::PointerPhase::Begin | nyatidraw_input::PointerPhase::Move
            ) {
                nyatidraw_input_platform::PEN_BUTTON_PRIMARY
            } else {
                nyatidraw_input::PenButtons::default()
            },
            eraser: false,
            viewport_revision,
        };
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.live_ink.push(sample).is_ok()
    }
}

struct WindowsPenInput {
    recorder: nyatidraw_input_platform::WindowsPenRecorder,
    batch: nyatidraw_input_platform::WindowsPenMessageBuffer,
    policy: nyatidraw_input::InStrokeViewportPolicy,
    live_ink: LiveInkBridge,
}

impl WindowsPenInput {
    fn new(live_ink: LiveInkBridge) -> Self {
        Self {
            recorder: nyatidraw_input_platform::WindowsPenRecorder::new(
                nyatidraw_input_platform::ViewportSnapshot::identity(0),
            ),
            batch: nyatidraw_input_platform::WindowsPenMessageBuffer::default(),
            policy: nyatidraw_input::InStrokeViewportPolicy::default(),
            live_ink,
        }
    }

    fn observe(&mut self, message: &MSG) -> bool {
        let viewport = self.live_ink.canvas_viewport_snapshot();
        let mapping = viewport.recorder_mapping();
        if let Some(mapping) = mapping
            && self.recorder.viewport() != mapping
        {
            self.recorder.set_viewport(mapping);
        }
        // SAFETY: `message` is a live stack value for this call only.
        if let Err(error) = unsafe {
            nyatidraw_input_platform::record_win32_message_into(
                &mut self.recorder,
                &mut self.batch,
                std::ptr::from_ref(message).cast(),
            )
        } {
            eprintln!("desktop-input event=pen-error error={error:?}");
            return false;
        }
        let records = self.batch.events().to_vec();
        records.into_iter().fold(false, |wake, record| {
            wake | self.admit(record, viewport, mapping.is_some())
        })
    }

    fn admit(
        &mut self,
        recorded: nyatidraw_input_platform::RecordedStylusEvent,
        viewport: CanvasViewportSnapshot,
        has_mapping: bool,
    ) -> bool {
        let closing = matches!(
            recorded.sample.phase,
            nyatidraw_input::PointerPhase::End | nyatidraw_input::PointerPhase::Cancel
        );
        if !closing
            && (!has_mapping
                || !viewport.contains_document_point(recorded.sample.position_document))
        {
            return false;
        }
        if !self
            .policy
            .admit(recorded.sample.phase, recorded.sample.viewport_revision)
        {
            return false;
        }
        self.live_ink.push(recorded.sample).is_ok()
    }
}

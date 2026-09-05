//! Win32-only `WM_POINTER` extraction. No Win32 type escapes this module.

use core::ffi::c_void;

use nyatidraw_input::{PenButtons, Point};
use windows::Win32::{
    Foundation::{HWND, POINT, RECT, WPARAM},
    Graphics::Gdi::ClientToScreen,
    UI::{
        Input::KeyboardAndMouse::{GMMP_USE_DISPLAY_POINTS, GetMouseMovePointsEx, MOUSEMOVEPOINT},
        Input::Pointer::{
            GetPointerDeviceRects, GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType,
            POINTER_FLAG_CANCELED, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_FOURTHBUTTON,
            POINTER_FLAG_INCONTACT, POINTER_FLAG_SECONDBUTTON, POINTER_FLAG_THIRDBUTTON,
            POINTER_FLAGS, POINTER_PEN_INFO,
        },
        WindowsAndMessaging::{
            GetMessageExtraInfo, GetMessageTime, MSG, PEN_FLAG_BARREL, PEN_FLAG_ERASER,
            PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_ROTATION, PEN_MASK_TILT_X,
            PEN_MASK_TILT_Y, POINTER_INPUT_TYPE, PT_PEN, WM_LBUTTONDOWN, WM_LBUTTONUP,
            WM_MOUSEMOVE, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE,
        },
    },
};

use crate::{
    PEN_BUTTON_BARREL, PEN_BUTTON_FOURTH, PEN_BUTTON_PRIMARY, PEN_BUTTON_SECONDARY,
    PEN_BUTTON_TERTIARY, RecordedStylusEvent, WindowsPenEvent, WindowsPenRecorder,
    WindowsPointerMessage,
};

/// A pen message above this size is abnormal. Refusing it avoids allowing a
/// malformed/coalesced message to turn the native input hot path into an
/// unbounded allocator while making the loss visible to the desktop owner.
const MAX_COALESCED_PEN_SAMPLES: usize = 4_096;
const MAX_COALESCED_MOUSE_SAMPLES: usize = 64;
const MI_WP_SIGNATURE_MASK: usize = 0xFFFF_FF00;
const MI_WP_SIGNATURE: usize = 0xFF51_5700;

/// Failure while extracting a `WM_POINTER` pen event.
#[derive(Debug)]
pub enum WindowsMessageError {
    NullMessagePointer,
    InvalidPointerDeviceRect,
    HistoryTooLarge { available: usize, limit: usize },
    MouseHistoryCurrentMissing,
    Win32(windows::core::Error),
}

impl From<windows::core::Error> for WindowsMessageError {
    fn from(error: windows::core::Error) -> Self {
        Self::Win32(error)
    }
}

/// Reusable, event-loop-owned storage for one decoded `WM_POINTER` message.
///
/// Windows reports `GetPointerPenInfoHistory` newest first. The adapter stores
/// it here, reverses it while recording, and returns [`events`](Self::events)
/// oldest first. Keeping this object beside the native message hook prevents a
/// fresh allocation for ordinary coalesced pen messages.
#[derive(Default)]
pub struct WindowsPenMessageBuffer {
    pen_history: Vec<POINTER_PEN_INFO>,
    events: Vec<RecordedStylusEvent>,
}

impl WindowsPenMessageBuffer {
    #[must_use]
    pub fn events(&self) -> &[RecordedStylusEvent] {
        &self.events
    }

    fn clear_events(&mut self) {
        self.events.clear();
    }
}

/// Mouse input phase decoded before winit dispatch. The desktop host turns
/// this into a pressure-1 `StylusSample` only after canvas admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsMouseMessage {
    Down,
    Move,
    Up,
}

/// A Win32 mouse point converted to fractional client coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowsMouseEvent {
    pub message: WindowsMouseMessage,
    pub timestamp_ms: u32,
    pub position_client_px: Point,
}

/// Whether mouse history was usable for the most recent decoded message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsMouseHistoryStatus {
    NotUsed,
    Complete,
    /// The returned history did not contain the last accepted point. Because
    /// Windows retains points from other threads, only the current point was
    /// accepted and the desktop can log an explicit discontinuity.
    CurrentOnly,
}

/// Reusable event-loop storage for native coalesced mouse history.
///
/// Windows retains at most 64 global mouse points. This buffer allocates its
/// fixed capacity at construction and never grows on the message hot path.
pub struct WindowsMouseMessageBuffer {
    history: Vec<MOUSEMOVEPOINT>,
    events: Vec<WindowsMouseEvent>,
    anchor: Option<MOUSEMOVEPOINT>,
    left_button_down: bool,
    history_status: WindowsMouseHistoryStatus,
}

impl Default for WindowsMouseMessageBuffer {
    fn default() -> Self {
        Self {
            history: vec![MOUSEMOVEPOINT::default(); MAX_COALESCED_MOUSE_SAMPLES],
            events: Vec::with_capacity(MAX_COALESCED_MOUSE_SAMPLES),
            anchor: None,
            left_button_down: false,
            history_status: WindowsMouseHistoryStatus::NotUsed,
        }
    }
}

impl WindowsMouseMessageBuffer {
    #[must_use]
    pub fn events(&self) -> &[WindowsMouseEvent] {
        &self.events
    }

    #[must_use]
    pub const fn history_status(&self) -> WindowsMouseHistoryStatus {
        self.history_status
    }

    fn clear_events(&mut self) {
        self.events.clear();
        self.history_status = WindowsMouseHistoryStatus::NotUsed;
    }
}

/// Decodes a raw Win32 mouse message without consuming it from winit.
///
/// Promoted pen/touch compatibility mouse messages are ignored so a physical
/// pen is never recorded a second time through the mouse path. `WM_MOUSEMOVE`
/// is expanded through `GetMouseMovePointsEx`; if its global history lacks the
/// prior accepted anchor, only the current point is returned.
///
/// # Safety
///
/// `message` must be a readable Win32 `MSG` for the call's duration.
///
/// # Errors
///
/// Returns [`WindowsMessageError`] if the message pointer is null or a Win32
/// coordinate/history query fails.
pub unsafe fn record_win32_mouse_message_into(
    buffer: &mut WindowsMouseMessageBuffer,
    message: *const c_void,
) -> Result<&[WindowsMouseEvent], WindowsMessageError> {
    // SAFETY: upheld by caller; only a read occurs during this callback.
    let message =
        unsafe { message.cast::<MSG>().as_ref() }.ok_or(WindowsMessageError::NullMessagePointer)?;
    record_wm_mouse_into(buffer, message)
}

fn record_wm_mouse_into<'a>(
    buffer: &'a mut WindowsMouseMessageBuffer,
    message: &MSG,
) -> Result<&'a [WindowsMouseEvent], WindowsMessageError> {
    buffer.clear_events();
    let Some(kind) = mouse_message(message.message) else {
        return Ok(buffer.events());
    };
    if is_promoted_pen_or_touch_mouse() {
        return Ok(buffer.events());
    }

    match kind {
        WindowsMouseMessage::Down => {
            // The mouse history is global and is documented for WM_MOUSEMOVE.
            // Do not use a possibly stale global record as a stroke begin.
            let point = client_point_from_screen(message.pt, message.hwnd)?;
            buffer.anchor = None;
            buffer.left_button_down = true;
            buffer.events.push(WindowsMouseEvent {
                message: kind,
                timestamp_ms: message.time,
                position_client_px: point,
            });
        }
        WindowsMouseMessage::Move => {
            if !buffer.left_button_down {
                return Ok(buffer.events());
            }
            let count = match read_mouse_history(message, &mut buffer.history) {
                Ok(count) => count,
                // A dispatched move can already have fallen out of the global
                // history (also common with SendInput). Retain the current MSG
                // point via the CurrentOnly path; missing history is not a
                // reason to discard a valid native move or its gesture.
                Err(WindowsMessageError::Win32(_)) => 0,
                Err(error) => return Err(error),
            };
            let history = &buffer.history[..count];
            let client_origin = client_origin(message.hwnd)?;
            let current = history
                .iter()
                .copied()
                .find(|point| point.time == message.time);
            let anchor_index = current.and_then(|_| {
                buffer
                    .anchor
                    .and_then(|anchor| history.iter().position(|point| *point == anchor))
            });
            if let (Some(_current), Some(anchor_index)) = (current, anchor_index) {
                for point in history[..anchor_index].iter().rev() {
                    push_mouse_event_if_distinct(
                        &mut buffer.events,
                        WindowsMouseMessage::Move,
                        *point,
                        client_origin,
                    );
                }
                buffer.history_status = WindowsMouseHistoryStatus::Complete;
            } else {
                if let Some(current) = current {
                    push_mouse_event_if_distinct(
                        &mut buffer.events,
                        WindowsMouseMessage::Move,
                        current,
                        client_origin,
                    );
                    buffer.anchor = Some(current);
                } else {
                    // A global-history lookup can fail to find the exact
                    // dispatched timestamp. Never substitute a possibly
                    // foreign record: retain only this WM_MOUSEMOVE point and
                    // require the next successful history result to re-anchor.
                    buffer.events.push(WindowsMouseEvent {
                        message: WindowsMouseMessage::Move,
                        timestamp_ms: message.time,
                        position_client_px: client_point_from_screen(message.pt, message.hwnd)?,
                    });
                    buffer.anchor = None;
                }
                buffer.history_status = WindowsMouseHistoryStatus::CurrentOnly;
            }
            if let Some(current) = current {
                buffer.anchor = Some(current);
            }
        }
        WindowsMouseMessage::Up => {
            if !buffer.left_button_down {
                return Ok(buffer.events());
            }
            let point = client_point_from_screen(message.pt, message.hwnd)?;
            buffer.events.push(WindowsMouseEvent {
                message: kind,
                timestamp_ms: message.time,
                position_client_px: point,
            });
            buffer.anchor = None;
            buffer.left_button_down = false;
        }
    }
    Ok(buffer.events())
}

fn mouse_message(message: u32) -> Option<WindowsMouseMessage> {
    match message {
        WM_LBUTTONDOWN => Some(WindowsMouseMessage::Down),
        WM_MOUSEMOVE => Some(WindowsMouseMessage::Move),
        WM_LBUTTONUP => Some(WindowsMouseMessage::Up),
        _ => None,
    }
}

fn is_promoted_pen_or_touch_mouse() -> bool {
    // SAFETY: this API reads the extra information for the current message on
    // the event-loop thread; it takes no pointers.
    let extra = unsafe { GetMessageExtraInfo().0.cast_unsigned() };
    has_promoted_pointer_signature(extra)
}

fn has_promoted_pointer_signature(extra: usize) -> bool {
    extra & MI_WP_SIGNATURE_MASK == MI_WP_SIGNATURE
}

fn read_mouse_history(
    message: &MSG,
    output: &mut [MOUSEMOVEPOINT],
) -> Result<usize, WindowsMessageError> {
    let input = MOUSEMOVEPOINT {
        // The API's negative-monitor workaround requires the coordinate to
        // pass through as an unsigned 16-bit display value before it searches
        // the global history.
        x: message.pt.x & 0xffff,
        y: message.pt.y & 0xffff,
        time: message.time,
        dwExtraInfo: 0,
    };
    // SAFETY: `input` and `output` are valid for the duration of the call.
    let count = unsafe {
        GetMouseMovePointsEx(
            u32::try_from(core::mem::size_of::<MOUSEMOVEPOINT>()).expect("size fits u32"),
            &raw const input,
            output,
            GMMP_USE_DISPLAY_POINTS,
        )
    };
    if count < 0 {
        return Err(WindowsMessageError::Win32(
            windows::core::Error::from_thread(),
        ));
    }
    let count = usize::try_from(count).expect("non-negative count fits usize");
    if count > output.len() {
        return Err(WindowsMessageError::HistoryTooLarge {
            available: count,
            limit: output.len(),
        });
    }
    Ok(count)
}

fn client_origin(hwnd: HWND) -> Result<POINT, WindowsMessageError> {
    let mut origin = POINT::default();
    // SAFETY: `origin` is writable and hwnd is from the current MSG.
    unsafe { ClientToScreen(hwnd, &raw mut origin).ok()? };
    Ok(origin)
}

fn client_point_from_screen(screen: POINT, hwnd: HWND) -> Result<Point, WindowsMessageError> {
    let origin = client_origin(hwnd)?;
    Ok(Point {
        x: f64::from(screen.x - origin.x),
        y: f64::from(screen.y - origin.y),
    })
}

fn display_point_to_client_point(point: MOUSEMOVEPOINT, client_origin: POINT) -> Point {
    // GetMouseMovePointsEx display points encode negative virtual-desktop
    // coordinates in the low 16 bits. Recover that signed value before
    // subtracting the client origin, so a negative monitor remains correct.
    let screen_x = sign_extend_low_16(point.x);
    let screen_y = sign_extend_low_16(point.y);
    Point {
        x: f64::from(screen_x - client_origin.x),
        y: f64::from(screen_y - client_origin.y),
    }
}

const fn sign_extend_low_16(value: i32) -> i32 {
    let low = value & 0xffff;
    if low >= 0x8000 { low - 0x1_0000 } else { low }
}

fn push_mouse_event_if_distinct(
    output: &mut Vec<WindowsMouseEvent>,
    message: WindowsMouseMessage,
    point: MOUSEMOVEPOINT,
    client_origin: POINT,
) {
    let event = WindowsMouseEvent {
        message,
        timestamp_ms: point.time,
        position_client_px: display_point_to_client_point(point, client_origin),
    };
    if output.last().is_none_or(|last| {
        last.timestamp_ms != event.timestamp_ms
            || last.position_client_px != event.position_client_px
    }) {
        output.push(event);
    }
}

/// Converts the Win32 `MSG` supplied by a native event-loop message hook.
///
/// This opaque-pointer entry point keeps Win32 types inside this platform
/// adapter. It is intended for winit's Windows `with_msg_hook` callback, which
/// invokes the hook before dispatching the message to its window procedure.
///
/// # Safety
///
/// `message` must be a non-null pointer to a readable Win32 `MSG` that remains
/// valid for the duration of this call.
///
/// # Errors
///
/// Returns [`WindowsMessageError::NullMessagePointer`] for a null pointer, or
/// forwards the errors documented by [`record_wm_pointer`].
pub unsafe fn record_win32_message(
    recorder: &mut WindowsPenRecorder,
    message: *const c_void,
) -> Result<Option<crate::RecordedStylusEvent>, WindowsMessageError> {
    // SAFETY: The caller guarantees that `message` points to a readable Win32
    // `MSG` for the duration of this call. `as_ref` performs the null check.
    let message =
        unsafe { message.cast::<MSG>().as_ref() }.ok_or(WindowsMessageError::NullMessagePointer)?;

    record_wm_pointer(recorder, message.hwnd, message.message, message.wParam)
}

/// Decodes one Win32 message into a reusable, ordered batch of stylus events.
///
/// For `WM_POINTERUPDATE`, every coalesced pen input is returned oldest to
/// newest. `Down`, `Up`, and `CaptureLost` remain a single transition each so
/// callers cannot accidentally duplicate or reorder stroke boundaries.
///
/// The returned slice is borrowed from `buffer` and remains valid until the
/// next call using that buffer.
///
/// # Safety
///
/// `message` must point to a readable Win32 `MSG` for this call's duration.
///
/// # Errors
///
/// Returns [`WindowsMessageError`] when the message pointer is null, Win32
/// cannot expose the current pointer metadata, or history exceeds the bounded
/// native-input scratch limit.
pub unsafe fn record_win32_message_into<'a>(
    recorder: &mut WindowsPenRecorder,
    buffer: &'a mut WindowsPenMessageBuffer,
    message: *const c_void,
) -> Result<&'a [RecordedStylusEvent], WindowsMessageError> {
    // SAFETY: The caller guarantees that `message` points to a readable Win32
    // `MSG` for the duration of this call. `as_ref` performs the null check.
    let message =
        unsafe { message.cast::<MSG>().as_ref() }.ok_or(WindowsMessageError::NullMessagePointer)?;

    record_wm_pointer_into(
        recorder,
        buffer,
        message.hwnd,
        message.message,
        message.wParam,
    )
}

/// Converts one `WM_POINTER` message into a platform-neutral recorded event.
///
/// Call this from the native window procedure before the event is handed to a
/// UI framework. Unknown messages and non-pen pointers return `Ok(None)`.
/// `WM_POINTERCAPTURECHANGED` emits a synthetic `Cancel` from the last active
/// pen when Windows no longer exposes pointer metadata.
///
/// # Errors
///
/// Returns [`WindowsMessageError::Win32`] when Windows fails to provide pen
/// metadata or convert its screen coordinate to the target window's client
/// coordinate.
pub fn record_wm_pointer(
    recorder: &mut WindowsPenRecorder,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
) -> Result<Option<crate::RecordedStylusEvent>, WindowsMessageError> {
    let pointer_id = pointer_id_from_wparam(wparam);
    let timestamp_ms = message_time_ms();

    let Some(kind) = pointer_message(message) else {
        return Ok(None);
    };

    match read_pen_event(hwnd, kind, pointer_id) {
        Ok(Some(event)) => Ok(recorder.record_admitted(event)),
        Ok(None) => Ok(None),
        Err(_) if kind == WindowsPointerMessage::CaptureLost => {
            Ok(recorder.cancel_active(pointer_id, timestamp_ms))
        }
        Err(error) => Err(error),
    }
}

/// Decodes one `WM_POINTER` message into `buffer` without per-message storage.
///
/// `WM_POINTERUPDATE` is expanded through `GetPointerPenInfoHistory`; Windows
/// supplies that history newest first, while this function records and exposes
/// it oldest first. The message history includes the current sample exactly
/// once. All transition messages intentionally bypass history expansion.
///
/// # Errors
///
/// Returns [`WindowsMessageError`] when Win32 cannot expose the pointer type,
/// pen metadata, device rectangles, or when the history exceeds the bounded
/// scratch limit.
pub fn record_wm_pointer_into<'a>(
    recorder: &mut WindowsPenRecorder,
    buffer: &'a mut WindowsPenMessageBuffer,
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
) -> Result<&'a [RecordedStylusEvent], WindowsMessageError> {
    buffer.clear_events();
    let pointer_id = pointer_id_from_wparam(wparam);
    let timestamp_ms = message_time_ms();

    let Some(kind) = pointer_message(message) else {
        return Ok(buffer.events());
    };

    if uses_pen_history(kind) {
        read_pen_history(pointer_id, &mut buffer.pen_history)?;
        for pen in history_oldest_first(&buffer.pen_history) {
            if pen.pointerInfo.pointerType != PT_PEN || !is_contact_update(pen) {
                continue;
            }
            if let Some(recorded) = recorder.record_admitted(convert_pen_event(hwnd, kind, pen)?) {
                buffer.events.push(recorded);
            }
        }
        return Ok(buffer.events());
    }

    match read_pen_event(hwnd, kind, pointer_id) {
        Ok(Some(event)) => {
            if let Some(recorded) = recorder.record_admitted(event) {
                buffer.events.push(recorded);
            }
        }
        Ok(None) => {}
        Err(_) if kind == WindowsPointerMessage::CaptureLost => {
            if let Some(cancel) = recorder.cancel_active(pointer_id, timestamp_ms) {
                buffer.events.push(cancel);
            }
        }
        Err(error) => return Err(error),
    }
    Ok(buffer.events())
}

fn uses_pen_history(message: WindowsPointerMessage) -> bool {
    message == WindowsPointerMessage::Update
}

fn history_oldest_first(
    history: &[POINTER_PEN_INFO],
) -> impl DoubleEndedIterator<Item = &POINTER_PEN_INFO> {
    history.iter().rev()
}

fn pointer_message(message: u32) -> Option<WindowsPointerMessage> {
    match message {
        WM_POINTERDOWN => Some(WindowsPointerMessage::Down),
        WM_POINTERUPDATE => Some(WindowsPointerMessage::Update),
        WM_POINTERUP => Some(WindowsPointerMessage::Up),
        WM_POINTERCAPTURECHANGED => Some(WindowsPointerMessage::CaptureLost),
        _ => None,
    }
}

fn pointer_id_from_wparam(wparam: WPARAM) -> u32 {
    (wparam.0 as u64 & 0xffff) as u32
}

fn message_time_ms() -> u32 {
    // SAFETY: GetMessageTime has no pointer arguments and may be called from the
    // thread dispatching the current Windows message.
    let signed = unsafe { GetMessageTime() };
    u32::from_ne_bytes(signed.to_ne_bytes())
}

fn read_pen_event(
    hwnd: HWND,
    message: WindowsPointerMessage,
    pointer_id: u32,
) -> Result<Option<WindowsPenEvent>, WindowsMessageError> {
    if !is_pen_pointer(pointer_id)? {
        return Ok(None);
    }
    let mut pen = POINTER_PEN_INFO::default();
    // SAFETY: `pen` is a valid, writable structure for the duration of the call.
    unsafe { GetPointerPenInfo(pointer_id, &raw mut pen)? };

    (pen.pointerInfo.pointerType == PT_PEN)
        .then(|| convert_pen_event(hwnd, message, &pen))
        .transpose()
}

fn read_pen_history(
    pointer_id: u32,
    output: &mut Vec<POINTER_PEN_INFO>,
) -> Result<(), WindowsMessageError> {
    if !is_pen_pointer(pointer_id)? {
        output.clear();
        return Ok(());
    }
    let mut current = POINTER_PEN_INFO::default();
    // SAFETY: `current` is valid writable storage for one pen record.
    unsafe { GetPointerPenInfo(pointer_id, &raw mut current)? };
    if current.pointerInfo.pointerType != PT_PEN {
        output.clear();
        return Ok(());
    }

    let mut capacity = bounded_history_capacity(current.pointerInfo.historyCount)?;
    loop {
        output.resize(capacity, POINTER_PEN_INFO::default());
        let mut entries = u32::try_from(capacity).expect("history capacity fits u32");
        // SAFETY: `output` has exactly `entries` initialized slots and remains
        // stable throughout the Win32 call.
        unsafe {
            GetPointerPenInfoHistory(pointer_id, &raw mut entries, Some(output.as_mut_ptr()))?;
        };
        let available = bounded_history_capacity(entries)?;
        if available <= capacity {
            output.truncate(available);
            return Ok(());
        }
        capacity = available;
    }
}

fn is_pen_pointer(pointer_id: u32) -> Result<bool, WindowsMessageError> {
    let mut pointer_type = POINTER_INPUT_TYPE::default();
    // SAFETY: `pointer_type` is valid writable storage for the pointer's type.
    unsafe { GetPointerType(pointer_id, &raw mut pointer_type)? };
    Ok(pointer_type == PT_PEN)
}

fn bounded_history_capacity(count: u32) -> Result<usize, WindowsMessageError> {
    let available = usize::try_from(count.max(1)).expect("u32 fits usize on supported Windows");
    if available > MAX_COALESCED_PEN_SAMPLES {
        return Err(WindowsMessageError::HistoryTooLarge {
            available,
            limit: MAX_COALESCED_PEN_SAMPLES,
        });
    }
    Ok(available)
}

fn is_contact_update(pen: &POINTER_PEN_INFO) -> bool {
    pen.pointerInfo
        .pointerFlags
        .contains(POINTER_FLAG_INCONTACT)
}

fn convert_pen_event(
    hwnd: HWND,
    message: WindowsPointerMessage,
    pen: &POINTER_PEN_INFO,
) -> Result<WindowsPenEvent, WindowsMessageError> {
    Ok(WindowsPenEvent {
        message: effective_message(message, pen.pointerInfo.pointerFlags),
        pointer_id: pen.pointerInfo.pointerId,
        timestamp_ms: pen.pointerInfo.dwTime,
        device_id: pen.pointerInfo.sourceDevice.0 as usize as u64,
        position_client_px: himetric_client_point(hwnd, pen)?,
        pressure: pen_mask_value(pen.penMask, PEN_MASK_PRESSURE, pen.pressure),
        tilt: pen_tilt(pen),
        rotation_degrees: pen_mask_value(pen.penMask, PEN_MASK_ROTATION, pen.rotation),
        buttons: pen_buttons(pen.pointerInfo.pointerFlags, pen.penFlags),
        eraser: pen.penFlags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED) != 0,
    })
}

fn effective_message(
    message: WindowsPointerMessage,
    flags: POINTER_FLAGS,
) -> WindowsPointerMessage {
    if flags.contains(POINTER_FLAG_CANCELED) {
        WindowsPointerMessage::Cancel
    } else {
        message
    }
}

fn himetric_client_point(hwnd: HWND, pen: &POINTER_PEN_INFO) -> Result<Point, WindowsMessageError> {
    let mut device = RECT::default();
    let mut display = RECT::default();
    // SAFETY: both rectangles are writable storage and `sourceDevice` belongs
    // to the pointer record that Windows returned for this message.
    unsafe {
        GetPointerDeviceRects(
            pen.pointerInfo.sourceDevice,
            &raw mut device,
            &raw mut display,
        )?;
    };
    let mut client_origin = POINT::default();
    // SAFETY: `hwnd` is the target window of the caller's event-loop message
    // and `client_origin` is valid mutable storage.
    unsafe { ClientToScreen(hwnd, &raw mut client_origin).ok()? };

    himetric_to_client_point(
        pen.pointerInfo.ptHimetricLocation,
        device,
        display,
        client_origin,
    )
}

fn himetric_to_client_point(
    himetric: POINT,
    device: RECT,
    display: RECT,
    client_origin: POINT,
) -> Result<Point, WindowsMessageError> {
    let device_width = f64::from(device.right) - f64::from(device.left);
    let device_height = f64::from(device.bottom) - f64::from(device.top);
    if device_width == 0.0 || device_height == 0.0 {
        return Err(WindowsMessageError::InvalidPointerDeviceRect);
    }
    let display_width = f64::from(display.right) - f64::from(display.left);
    let display_height = f64::from(display.bottom) - f64::from(display.top);
    // `ptHimetricLocation` is rooted at (0, 0), including on multi-monitor
    // desktops. `device` supplies only the physical range for the scale; the
    // display rectangle supplies the required desktop translation.
    let screen_x =
        f64::from(himetric.x).mul_add(display_width / device_width, f64::from(display.left));
    let screen_y =
        f64::from(himetric.y).mul_add(display_height / device_height, f64::from(display.top));
    Ok(Point {
        x: screen_x - f64::from(client_origin.x),
        y: screen_y - f64::from(client_origin.y),
    })
}

fn pen_mask_value(mask: u32, bit: u32, value: u32) -> Option<u16> {
    (mask & bit != 0)
        .then(|| u16::try_from(value).ok())
        .flatten()
}

fn pen_tilt(pen: &POINTER_PEN_INFO) -> Option<[i16; 2]> {
    let has_x = pen.penMask & PEN_MASK_TILT_X != 0;
    let has_y = pen.penMask & PEN_MASK_TILT_Y != 0;
    (has_x && has_y)
        .then(|| {
            Some([
                i16::try_from(pen.tiltX).ok()?,
                i16::try_from(pen.tiltY).ok()?,
            ])
        })
        .flatten()
}

fn pen_buttons(pointer_flags: POINTER_FLAGS, pen_flags: u32) -> PenButtons {
    let mut buttons = PenButtons(0);
    if pen_flags & PEN_FLAG_BARREL != 0 {
        buttons.0 |= PEN_BUTTON_BARREL.0;
    }
    if pointer_flags.contains(POINTER_FLAG_FIRSTBUTTON) {
        buttons.0 |= PEN_BUTTON_PRIMARY.0;
    }
    if pointer_flags.contains(POINTER_FLAG_SECONDBUTTON) {
        buttons.0 |= PEN_BUTTON_SECONDARY.0;
    }
    if pointer_flags.contains(POINTER_FLAG_THIRDBUTTON) {
        buttons.0 |= PEN_BUTTON_TERTIARY.0;
    }
    if pointer_flags.contains(POINTER_FLAG_FOURTHBUTTON) {
        buttons.0 |= PEN_BUTTON_FOURTH.0;
    }
    buttons
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_limit_is_bounded_and_reports_the_actual_count() {
        assert!(matches!(bounded_history_capacity(0), Ok(1)));
        assert!(matches!(
            bounded_history_capacity(u32::try_from(MAX_COALESCED_PEN_SAMPLES).expect("limit fits")),
            Ok(MAX_COALESCED_PEN_SAMPLES)
        ));
        assert!(matches!(
            bounded_history_capacity(
                u32::try_from(MAX_COALESCED_PEN_SAMPLES + 1).expect("limit fits")
            ),
            Err(WindowsMessageError::HistoryTooLarge {
                available,
                limit,
            }) if available == MAX_COALESCED_PEN_SAMPLES + 1 && limit == MAX_COALESCED_PEN_SAMPLES
        ));
    }

    #[test]
    fn himetric_mapping_preserves_fractional_client_coordinates() {
        let point = himetric_to_client_point(
            POINT { x: 250, y: 375 },
            RECT {
                left: 500,
                top: 100,
                right: 1_500,
                bottom: 1_100,
            },
            RECT {
                left: -1_600,
                top: 200,
                right: 0,
                bottom: 1_200,
            },
            POINT { x: -1_500, y: 180 },
        )
        .expect("non-degenerate device rect");
        assert_eq!(point, Point { x: 300.0, y: 395.0 });
    }

    #[test]
    fn hover_updates_are_excluded_without_affecting_contact_samples() {
        let mut pen = POINTER_PEN_INFO::default();
        assert!(!is_contact_update(&pen));
        pen.pointerInfo.pointerFlags = POINTER_FLAG_INCONTACT;
        assert!(is_contact_update(&pen));
    }

    #[test]
    fn cancelled_pointer_flag_wins_over_update_or_up_phase() {
        assert_eq!(
            effective_message(WindowsPointerMessage::Update, POINTER_FLAG_CANCELED),
            WindowsPointerMessage::Cancel
        );
        assert_eq!(
            effective_message(WindowsPointerMessage::Up, POINTER_FLAG_CANCELED),
            WindowsPointerMessage::Cancel
        );
    }

    #[test]
    fn history_is_oldest_first_and_never_expands_transitions() {
        let mut history = vec![POINTER_PEN_INFO::default(); 3];
        history[0].pointerInfo.dwTime = 30;
        history[1].pointerInfo.dwTime = 20;
        history[2].pointerInfo.dwTime = 10;
        assert_eq!(
            history_oldest_first(&history)
                .map(|pen| pen.pointerInfo.dwTime)
                .collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        assert!(uses_pen_history(WindowsPointerMessage::Update));
        assert!(!uses_pen_history(WindowsPointerMessage::Down));
        assert!(!uses_pen_history(WindowsPointerMessage::Up));
        assert!(!uses_pen_history(WindowsPointerMessage::CaptureLost));
    }

    #[test]
    fn display_mouse_conversion_sign_extends_negative_virtual_coordinates() {
        let point = display_point_to_client_point(
            MOUSEMOVEPOINT {
                x: 0xF9C0,
                y: 0xFC7C,
                time: 1,
                dwExtraInfo: 0,
            },
            // A window whose client origin is on the negative monitor.
            POINT { x: -1_400, y: -800 },
        );
        assert_eq!(
            point,
            Point {
                x: -200.0,
                y: -100.0
            }
        );
    }

    #[test]
    fn mouse_history_emits_only_points_after_anchor_in_oldest_first_order() {
        let anchor = MOUSEMOVEPOINT {
            x: 40,
            y: 40,
            time: 40,
            dwExtraInfo: 0,
        };
        // GetMouseMovePointsEx returns the supplied/latest point first.
        let history = [
            MOUSEMOVEPOINT {
                x: 60,
                y: 60,
                time: 60,
                dwExtraInfo: 0,
            },
            MOUSEMOVEPOINT {
                x: 50,
                y: 50,
                time: 50,
                dwExtraInfo: 0,
            },
            anchor,
        ];
        let index = history
            .iter()
            .position(|point| *point == anchor)
            .expect("anchor exists");
        let times: Vec<_> = history[..index]
            .iter()
            .rev()
            .map(|point| point.time)
            .collect();
        assert_eq!(times, [50, 60]);
    }

    #[test]
    fn promoted_pen_or_touch_signature_is_not_treated_as_mouse() {
        assert!(has_promoted_pointer_signature(MI_WP_SIGNATURE));
        assert!(has_promoted_pointer_signature(MI_WP_SIGNATURE | 0x80));
        assert!(!has_promoted_pointer_signature(0));
    }
}

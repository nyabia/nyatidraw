use std::{cell::Cell, mem::size_of, rc::Rc};

use nyatidraw_input::Point;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Input::{
            GetRawInputData, GetRegisteredRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE,
            RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RAWMOUSE, RID_HEADER, RID_INPUT,
            RIM_TYPEMOUSE,
        },
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetMessagePos, GetMessageTime, GetWindowThreadProcessId, InSendMessageEx,
            RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, RIM_INPUT, SPI_GETMOUSE,
            SPI_GETMOUSESPEED, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
            WM_INPUT, WM_NCDESTROY,
        },
    },
};

use crate::MouseMotionPolicy;

const RAW_MOUSE_SUBCLASS: usize = 0x4e59_524d;

thread_local! {
    static ACTIVE_RAW_MOUSE_RECEIVER: Cell<Option<isize>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct WindowsRawMousePacket {
    pub device_id: usize,
    pub timestamp_ms: u32,
    pub position_screen_px: Point,
    pub delta: Point,
    pub relative: bool,
    pub promoted: bool,
    pub left_down: bool,
    pub left_up: bool,
}

impl WindowsRawMousePacket {
    #[must_use]
    pub fn matches_legacy_down(self, timestamp_ms: u32, position_screen_px: Point) -> bool {
        self.device_id != 0
            && self.relative
            && !self.promoted
            && self.left_down
            && !self.left_up
            && timestamp_ms.wrapping_sub(self.timestamp_ms) <= 32
            && (self.position_screen_px.x - position_screen_px.x).abs() <= 2.0
            && (self.position_screen_px.y - position_screen_px.y).abs() <= 2.0
    }
}

struct HookState {
    installed: Cell<bool>,
    callback: fn(usize, Option<WindowsRawMousePacket>),
    context: usize,
}

/// UI-thread observation of the framework's existing raw-input receiver.
/// Registration and default `WM_INPUT` cleanup remain owned by the framework.
pub struct WindowsRawMouseHook {
    target: HWND,
    state: Rc<HookState>,
}

impl WindowsRawMouseHook {
    /// Installs without changing the process-wide raw-input registration.
    /// `None` packets report a decode failure; no packet payload is retained.
    ///
    /// # Errors
    /// Returns an error if no explicit mouse receiver exists on this UI thread
    /// or its subclass cannot be installed. The caller retains legacy input.
    pub fn install(
        context: usize,
        callback: fn(usize, Option<WindowsRawMousePacket>),
    ) -> Result<Self, &'static str> {
        let mut devices = [RAWINPUTDEVICE::default(); 64];
        let mut count = 64;
        let record_size = u32::try_from(size_of::<RAWINPUTDEVICE>())
            .map_err(|_| "raw registration record size invalid")?;
        // SAFETY: the fixed array and count are writable for this query.
        let count = unsafe {
            GetRegisteredRawInputDevices(Some(devices.as_mut_ptr()), &raw mut count, record_size)
        };
        let count = usize::try_from(count).map_err(|_| "raw registration count invalid")?;
        let devices = devices
            .get(..count)
            .ok_or("raw registration query failed")?;
        let target = devices
            .iter()
            .find(|device| device.usUsagePage == 1 && device.usUsage == 2)
            .map(|device| device.hwndTarget)
            .filter(|hwnd| !hwnd.is_invalid())
            .ok_or("no explicit registered raw mouse receiver")?;
        // SAFETY: these are read-only thread-identity queries. Subclassing a
        // receiver on another thread would violate the callback's ownership.
        if unsafe { GetWindowThreadProcessId(target, None) != GetCurrentThreadId() } {
            return Err("raw mouse receiver belongs to another thread");
        }
        if ACTIVE_RAW_MOUSE_RECEIVER.get().is_some() {
            return Err("raw mouse observer already installed");
        }
        let state = Rc::new(HookState {
            installed: Cell::new(true),
            callback,
            context,
        });
        let reference = Rc::into_raw(state.clone());
        // SAFETY: one Rc reference belongs to the subclass until removal or
        // WM_NCDESTROY; both occur on this same UI thread.
        if !unsafe {
            SetWindowSubclass(
                target,
                Some(raw_mouse_proc),
                RAW_MOUSE_SUBCLASS,
                reference as usize,
            )
        }
        .as_bool()
        {
            // SAFETY: failed installation did not retain the supplied reference.
            drop(unsafe { Rc::from_raw(reference) });
            return Err("raw mouse observer installation failed");
        }
        ACTIVE_RAW_MOUSE_RECEIVER.set(Some(target.0 as isize));
        Ok(Self { target, state })
    }
}

impl Drop for WindowsRawMouseHook {
    fn drop(&mut self) {
        if !self.state.installed.replace(false) {
            return;
        }
        // SAFETY: this !Send owner is dropped on its installation thread. If
        // removal fails, the disabled subclass retains its Rc until destruction.
        if unsafe { RemoveWindowSubclass(self.target, Some(raw_mouse_proc), RAW_MOUSE_SUBCLASS) }
            .as_bool()
        {
            release_receiver_guard(self.target);
            // SAFETY: successful removal releases precisely the subclass's Rc.
            drop(unsafe { Rc::from_raw(Rc::as_ptr(&self.state)) });
        }
    }
}

fn release_receiver_guard(hwnd: HWND) {
    if ACTIVE_RAW_MOUSE_RECEIVER.get() == Some(hwnd.0 as isize) {
        ACTIVE_RAW_MOUSE_RECEIVER.set(None);
    }
}

unsafe extern "system" fn raw_mouse_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let pointer = data as *const HookState;
    // SAFETY: the subclass owns a live Rc. This temporary strong reference
    // survives synchronous callback teardown, including removal of this hook.
    unsafe { Rc::increment_strong_count(pointer) };
    let state = unsafe { Rc::from_raw(pointer) };
    if message == WM_NCDESTROY {
        state.installed.set(false);
        // SAFETY: removal and reclamation run once on the owning UI thread.
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(raw_mouse_proc), id) };
        release_receiver_guard(hwnd);
        drop(unsafe { Rc::from_raw(data as *const HookState) });
    } else if message == WM_INPUT && state.installed.get() && wparam.0 == RIM_INPUT as usize {
        // Sent messages do not own the queued GetMessagePos/GetMessageTime
        // snapshot. Only foreground, queued WM_INPUT can supply our anchor.
        if unsafe { InSendMessageEx(None) } == 0 {
            match read_packet(lparam) {
                Ok(Some(packet)) => (state.callback)(state.context, Some(packet)),
                Err(()) => (state.callback)(state.context, None),
                Ok(None) => {}
            }
        }
    }
    // SAFETY: the original receiver must still see every message and perform
    // the documented foreground WM_INPUT cleanup exactly as before this hook.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn read_packet(lparam: LPARAM) -> Result<Option<WindowsRawMousePacket>, ()> {
    let input = HRAWINPUT(lparam.0 as *mut core::ffi::c_void);
    let header_size = u32::try_from(size_of::<RAWINPUTHEADER>()).expect("Win32 size fits u32");
    let mut header = RAWINPUTHEADER::default();
    let mut size = header_size;
    // SAFETY: input is the current WM_INPUT handle and header is aligned,
    // writable storage of the exact supplied size.
    let copied = unsafe {
        GetRawInputData(
            input,
            RID_HEADER,
            Some((&raw mut header).cast()),
            &raw mut size,
            header_size,
        )
    };
    if copied != header_size {
        return Err(());
    }
    if header.dwType != RIM_TYPEMOUSE.0 || header.wParam.0 != RIM_INPUT as usize {
        return Ok(None);
    }
    let mut raw = RAWINPUT::default();
    let mut size = u32::try_from(size_of::<RAWINPUT>()).expect("Win32 size fits u32");
    if header.dwSize > size {
        return Err(());
    }
    // SAFETY: the header established the mouse union discriminator. The stack
    // buffer holds a complete RAWINPUT and the API never grows that buffer.
    let copied = unsafe {
        GetRawInputData(
            input,
            RID_INPUT,
            Some((&raw mut raw).cast()),
            &raw mut size,
            header_size,
        )
    };
    let mouse_size = u32::try_from(size_of::<RAWMOUSE>()).expect("Win32 size fits u32");
    if copied != header.dwSize
        || copied < header_size + mouse_size
        || raw.header.dwType != RIM_TYPEMOUSE.0
    {
        return Err(());
    }
    // SAFETY: successful complete read and checked RIM_TYPEMOUSE discriminator.
    let mouse = unsafe { raw.data.mouse };
    let buttons = unsafe { mouse.Anonymous.Anonymous.usButtonFlags };
    // SAFETY: this is a queued WM_INPUT dispatch before forwarding to the
    // framework; no nested message pump occurs before these snapshot reads.
    let packed = unsafe { GetMessagePos() };
    let timestamp_ms = unsafe { GetMessageTime() }.cast_unsigned();
    let signed = |word: u32| {
        f64::from(
            u16::try_from(word & 0xffff)
                .expect("masked to 16 bits")
                .cast_signed(),
        )
    };
    Ok(Some(WindowsRawMousePacket {
        device_id: raw.header.hDevice.0 as usize,
        timestamp_ms,
        position_screen_px: Point {
            x: signed(packed),
            y: signed(packed >> 16),
        },
        delta: Point {
            x: f64::from(mouse.lLastX),
            y: f64::from(mouse.lLastY),
        },
        relative: mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0,
        promoted: mouse.ulExtraInformation & 0xffff_ff00 == 0xff51_5700,
        left_down: u32::from(buttons) & RI_MOUSE_LEFT_BUTTON_DOWN != 0,
        left_up: u32::from(buttons) & RI_MOUSE_LEFT_BUTTON_UP != 0,
    }))
}

/// Reads the current policy at stroke begin; a failed query keeps legacy input.
#[must_use]
pub fn windows_mouse_motion_policy() -> Option<MouseMotionPolicy> {
    let mut speed = 0_u32;
    let mut acceleration = [0_i32; 3];
    // SAFETY: each SPI_GET operation receives owned writable storage with the
    // documented element type and never changes Windows settings.
    unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSESPEED,
            0,
            Some((&raw mut speed).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
        SystemParametersInfoW(
            SPI_GETMOUSE,
            0,
            Some(acceleration.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    if !(1..=20).contains(&speed) || !(0..=2).contains(&acceleration[2]) {
        return None;
    }
    Some(MouseMotionPolicy {
        speed,
        acceleration: u32::try_from(acceleration[2]).ok()?,
        thresholds: [
            u32::try_from(acceleration[0]).ok()?,
            u32::try_from(acceleration[1]).ok()?,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmatched_raw_evidence_cannot_disable_legacy_stroke_movement() {
        let screen = Point { x: -120.0, y: 40.0 };
        let down = WindowsRawMousePacket {
            device_id: 1,
            timestamp_ms: u32::MAX,
            position_screen_px: screen,
            delta: Point { x: 0.0, y: 0.0 },
            relative: true,
            promoted: false,
            left_down: true,
            left_up: false,
        };
        assert!(down.matches_legacy_down(0, screen));
        for packet in [
            WindowsRawMousePacket {
                device_id: 0,
                ..down
            },
            WindowsRawMousePacket {
                relative: false,
                ..down
            },
            WindowsRawMousePacket {
                promoted: true,
                ..down
            },
            WindowsRawMousePacket {
                left_down: false,
                ..down
            },
            WindowsRawMousePacket {
                left_up: true,
                ..down
            },
            WindowsRawMousePacket {
                timestamp_ms: 1,
                ..down
            },
            WindowsRawMousePacket {
                timestamp_ms: u32::MAX - 32,
                ..down
            },
            WindowsRawMousePacket {
                position_screen_px: Point { x: -123.0, y: 40.0 },
                ..down
            },
        ] {
            assert!(!packet.matches_legacy_down(0, screen));
        }
    }
}

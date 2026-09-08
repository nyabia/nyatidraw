//! `DirectComposition` draws the web visual on the parent above the WGPU child.
//! Only the separate WRY input sink is shaped; the web visual is never clipped
//! to this region, so translucent menus retain the artwork underneath.
use super::HostLayout;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        CombineRgn, CreateRectRgn, DeleteObject, HRGN, MapWindowPoints, RGN_AND, RGN_DIFF,
        ScreenToClient, SetWindowRgn,
    },
    UI::{
        Controls::WM_MOUSELEAVE,
        Input::Pointer::{GetPointerInfo, POINTER_FLAG_INCONTACT, POINTER_INFO},
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetClientRect, GetMessageExtraInfo, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SendMessageW, SetWindowPos, WM_CANCELMODE, WM_CAPTURECHANGED, WM_LBUTTONDBLCLK,
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP,
            WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCDESTROY, WM_POINTERACTIVATE,
            WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERLEAVE,
            WM_POINTERUP, WM_POINTERUPDATE, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP,
            WM_XBUTTONDBLCLK, WM_XBUTTONDOWN, WM_XBUTTONUP,
        },
    },
};

struct Region(HRGN);
pub(crate) fn restore_input(router: &CanvasInputRouter) {
    // Restore the full input sink when the canvas observer stops.
    if router.input_live.get() {
        unsafe { SetWindowRgn(router.input, None, true) };
    }
}
impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: the region remains ours unless explicitly transferred to HWND.
        let _ = unsafe { DeleteObject(self.0.into()) };
    }
}

fn rect_region(rect: [i32; 4]) -> Result<Region, String> {
    let region = unsafe { CreateRectRgn(rect[0], rect[1], rect[2], rect[3]) };
    if region.0.is_null() {
        Err("create host input region".into())
    } else {
        Ok(Region(region))
    }
}

#[allow(clippy::cast_possible_truncation)]
fn physical(rect: [f64; 4], scale: f64) -> [i32; 4] {
    // Float-to-int conversion saturates; caller has rejected NaN/inf. Round
    // outward so a UI boundary does not accidentally admit a canvas stroke.
    [
        (rect[0] * scale).floor() as i32,
        (rect[1] * scale).floor() as i32,
        ((rect[0] + rect[2]) * scale).ceil() as i32,
        ((rect[1] + rect[3]) * scale).ceil() as i32,
    ]
}

pub(crate) fn apply_input_layout(
    input_hwnd: isize,
    router: &CanvasInputRouter,
    layout: &HostLayout,
) -> Result<(), String> {
    if !layout.valid() || !router.input_live.get() || !router.canvas_live.get() {
        return Err("invalid canvas host layout".into());
    }
    let hwnd = HWND(input_hwnd as *mut core::ffi::c_void);
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &raw mut client) }.map_err(|error| error.to_string())?;
    let outer = rect_region([client.left, client.top, client.right, client.bottom])?;
    let input = rect_region([client.left, client.top, client.right, client.bottom])?;
    let [x, y, width, height, scale] = layout.canvas;
    let canvas = rect_region(physical([x, y, width, height], scale))?;
    // SAFETY: all handles are owned valid GDI regions. An empty region is valid;
    // ERROR (zero) is not confused with NULLREGION (one).
    if unsafe { CombineRgn(Some(input.0), Some(outer.0), Some(canvas.0), RGN_DIFF) }.0 == 0 {
        return Err("subtract native canvas input region".into());
    }
    // No input-only HWND overlaps the canvas, even under a full-screen modal.
    // The canvas subclass forwards only UI-owned events using these rectangles.
    router.state.borrow_mut().regions = layout
        .ui_regions
        .iter()
        .map(|rect| physical(*rect, scale))
        .collect();
    if unsafe { CombineRgn(Some(input.0), Some(input.0), Some(outer.0), RGN_AND) }.0 == 0 {
        return Err("bound UI input region".into());
    }
    if unsafe { SetWindowRgn(hwnd, Some(input.0), true) } == 0 {
        return Err("install UI input region".into());
    }
    // SetWindowRgn transfers ownership only on success.
    core::mem::forget(input);
    if !router.input_live.get() {
        return Err("input host retired during layout".into());
    }
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
    .map_err(|error| error.to_string())?;
    Ok(())
}

const ROUTER_SUBCLASS: usize = 0x4e59_5254;
#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Canvas,
    Ui,
}
#[derive(Default)]
struct RoutingState {
    regions: Vec<[i32; 4]>,
    pointers: [Option<(u32, Owner)>; 32],
    mouse: Option<Owner>,
    buttons: u32,
    ui_hover: bool,
    pointer_overflow: bool,
}
pub(crate) struct CanvasInputRouter {
    canvas: HWND,
    input: HWND,
    canvas_live: Cell<bool>,
    input_live: Cell<bool>,
    state: RefCell<RoutingState>,
}
impl CanvasInputRouter {
    fn send_ui(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if !self.input_live.get() {
            return LRESULT(0);
        }
        unsafe { SendMessageW(self.input, msg, Some(wp), Some(lp)) }
    }
    fn continue_canvas(&self, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if !self.canvas_live.get() {
            return LRESULT(0);
        }
        unsafe { DefSubclassProc(self.canvas, msg, wp, lp) }
    }
    pub(crate) fn new(canvas: isize, input: isize) -> Result<Rc<Self>, String> {
        let router = Rc::new(Self {
            canvas: HWND(canvas as _),
            input: HWND(input as _),
            canvas_live: Cell::new(true),
            input_live: Cell::new(true),
            state: RefCell::new(RoutingState::default()),
        });
        let ptr = Rc::as_ptr(&router) as usize;
        unsafe {
            if !SetWindowSubclass(router.canvas, Some(route_input), ROUTER_SUBCLASS, ptr).as_bool()
            {
                return Err("attach canvas input router".into());
            }
            if !SetWindowSubclass(router.input, Some(route_input), ROUTER_SUBCLASS, ptr).as_bool() {
                let _ = RemoveWindowSubclass(router.canvas, Some(route_input), ROUTER_SUBCLASS);
                return Err("attach web input router".into());
            }
        }
        Ok(router)
    }
}
impl Drop for CanvasInputRouter {
    fn drop(&mut self) {
        // Detach before sending messages: Drop runs after the final Rc owner
        // disappears, so no callback may attempt to retain this allocation.
        unsafe {
            if self.canvas_live.get() {
                let _ = RemoveWindowSubclass(self.canvas, Some(route_input), ROUTER_SUBCLASS);
            }
            if self.input_live.get() {
                let _ = RemoveWindowSubclass(self.input, Some(route_input), ROUTER_SUBCLASS);
            }
        }
    }
}
fn button(msg: u32, wp: WPARAM) -> (u32, bool, bool) {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => (1, true, false),
        WM_LBUTTONUP => (1, false, true),
        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => (2, true, false),
        WM_RBUTTONUP => (2, false, true),
        WM_MBUTTONDOWN | WM_MBUTTONDBLCLK => (4, true, false),
        WM_MBUTTONUP => (4, false, true),
        WM_XBUTTONDOWN | WM_XBUTTONDBLCLK => (if wp.0 >> 16 == 1 { 8 } else { 16 }, true, false),
        WM_XBUTTONUP => (if wp.0 >> 16 == 1 { 8 } else { 16 }, false, true),
        _ => (0, false, false),
    }
}
fn hits(state: &RoutingState, p: POINT) -> bool {
    state
        .regions
        .iter()
        .any(|r| p.x >= r[0] && p.y >= r[1] && p.x < r[2] && p.y < r[3])
}
// Win32 packs signed 16-bit coordinates and unsigned pointer/button IDs in
// WPARAM/LPARAM. The narrowing and bit-preserving signed conversions here are
// the message ABI, not unchecked application geometry conversions.
#[allow(
    clippy::too_many_lines,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
unsafe extern "system" fn route_input(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // The owner removes both subclasses before releasing its Rc. An in-flight
    // dispatch retains its own reference across COM/message-loop reentrancy.
    let ptr = data as *const CanvasInputRouter;
    unsafe {
        Rc::increment_strong_count(ptr);
    }
    let router = unsafe { Rc::from_raw(ptr) };
    if msg == WM_NCDESTROY {
        if hwnd == router.input {
            router.input_live.set(false);
        } else {
            router.canvas_live.set(false);
        }
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(route_input), ROUTER_SUBCLASS) };
        return unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    }
    let (mask, down, up) = button(msg, wp);
    if hwnd == router.input {
        {
            let mut state = router.state.borrow_mut();
            if down {
                state.mouse = Some(Owner::Ui);
                state.buttons |= mask;
            }
            if up {
                state.buttons &= !mask;
                if state.buttons == 0 {
                    state.mouse = None;
                }
            }
            if matches!(msg, WM_CAPTURECHANGED | WM_CANCELMODE) && state.mouse == Some(Owner::Ui) {
                state.mouse = None;
                state.buttons = 0;
            }
            if matches!(msg, WM_POINTERUP | WM_POINTERCAPTURECHANGED | WM_CANCELMODE) {
                let id = wp.0 as u32 & 0xffff;
                for pointer in &mut state.pointers {
                    if pointer
                        .is_some_and(|p| p.1 == Owner::Ui && (msg == WM_CANCELMODE || p.0 == id))
                    {
                        *pointer = None;
                    }
                }
            }
        }
        return unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    }
    if msg == WM_CAPTURECHANGED {
        let ui = {
            let mut state = router.state.borrow_mut();
            let ui = state.mouse == Some(Owner::Ui);
            if !ui || HWND(lp.0 as _) != router.input {
                state.mouse = None;
                state.buttons = 0;
            }
            ui
        };
        if ui {
            return LRESULT(0);
        }
        return router.continue_canvas(msg, wp, lp);
    }
    if msg == WM_CANCELMODE {
        let (pointers, mouse) = {
            let mut state = router.state.borrow_mut();
            let pointers = std::mem::take(&mut state.pointers);
            let mouse = state.mouse.take();
            state.buttons = 0;
            state.pointer_overflow = false;
            (pointers, mouse)
        };
        for (id, owner) in pointers.into_iter().flatten() {
            if owner == Owner::Ui {
                let _ = router.send_ui(WM_POINTERCAPTURECHANGED, WPARAM(id as usize), LPARAM(0));
            } else {
                let _ = router.continue_canvas(
                    WM_POINTERCAPTURECHANGED,
                    WPARAM(id as usize),
                    LPARAM(0),
                );
            }
        }
        if mouse == Some(Owner::Ui) {
            let _ = router.send_ui(msg, wp, lp);
        }
        return router.continue_canvas(msg, wp, lp);
    }
    if matches!(
        msg,
        WM_POINTERDOWN
            | WM_POINTERUPDATE
            | WM_POINTERUP
            | WM_POINTERENTER
            | WM_POINTERLEAVE
            | WM_POINTERACTIVATE
            | WM_POINTERCAPTURECHANGED
    ) {
        let id = wp.0 as u32 & 0xffff;
        let mut native = POINTER_INFO::default();
        let metadata = router.input_live.get()
            && msg != WM_POINTERCAPTURECHANGED
            && unsafe { GetPointerInfo(id, &raw mut native) }.is_ok();
        let mut point = native.ptPixelLocation;
        if metadata && router.input_live.get() {
            let _ = unsafe { ScreenToClient(router.input, &raw mut point) };
        }
        let (owner, cancel) = {
            let mut state = router.state.borrow_mut();
            let index = state
                .pointers
                .iter()
                .position(|p| p.is_some_and(|p| p.0 == id));
            let existing = index.and_then(|index| state.pointers[index].map(|p| p.1));
            if !metadata || msg == WM_POINTERCAPTURECHANGED {
                if let Some(index) = index {
                    state.pointers[index] = None;
                }
                (existing, true)
            } else if msg == WM_POINTERDOWN {
                if let Some(owner) = existing {
                    (Some(owner), false)
                } else if state.pointer_overflow {
                    (None, false)
                } else if let Some(slot) = state.pointers.iter().position(Option::is_none) {
                    let owner = if hits(&state, point) {
                        Owner::Ui
                    } else {
                        Owner::Canvas
                    };
                    state.pointers[slot] = Some((id, owner));
                    (Some(owner), false)
                } else {
                    state.pointer_overflow = true;
                    eprintln!("desktop-input event=pointer-owner-capacity-exhausted");
                    (None, false)
                }
            } else {
                let owner = existing.or_else(|| {
                    if msg == WM_POINTERACTIVATE
                        || native.pointerFlags.0 & POINTER_FLAG_INCONTACT.0 == 0
                    {
                        Some(if hits(&state, point) {
                            Owner::Ui
                        } else {
                            Owner::Canvas
                        })
                    } else {
                        None
                    }
                });
                if msg == WM_POINTERUP
                    && let Some(index) = index
                {
                    state.pointers[index] = None;
                }
                (owner, false)
            }
        };
        {
            let mut state = router.state.borrow_mut();
            if state.pointers.iter().all(Option::is_none) {
                state.pointer_overflow = false;
            }
        }
        let routed_msg = if cancel {
            WM_POINTERCAPTURECHANGED
        } else {
            msg
        };
        return match owner {
            Some(Owner::Ui) => router.send_ui(routed_msg, wp, lp),
            Some(Owner::Canvas) => router.continue_canvas(routed_msg, wp, lp),
            None => LRESULT(0),
        };
    }
    let mouse = mask != 0
        || matches!(
            msg,
            WM_MOUSEMOVE | WM_MOUSEWHEEL | WM_MOUSEHWHEEL | WM_MOUSELEAVE
        );
    if !mouse {
        return router.continue_canvas(msg, wp, lp);
    }
    // Pen promotion must not start a second mouse owner or a canvas stroke.
    if unsafe { GetMessageExtraInfo() }.0 as usize & 0xffff_ff00 == 0xff51_5700 {
        return LRESULT(0);
    }
    if !router.input_live.get() {
        let canvas_owned = {
            let mut state = router.state.borrow_mut();
            let owner = state.mouse.take();
            state.buttons = 0;
            owner == Some(Owner::Canvas)
        };
        if canvas_owned {
            let _ = router.continue_canvas(WM_CAPTURECHANGED, WPARAM(0), LPARAM(0));
        }
        return LRESULT(0);
    }
    let mut point = POINT {
        x: i32::from(lp.0 as i16),
        y: i32::from((lp.0 >> 16) as i16),
    };
    let wheel = matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL);
    if wheel {
        let _ = unsafe { ScreenToClient(router.input, &raw mut point) };
    } else {
        let _ = unsafe {
            MapWindowPoints(
                Some(hwnd),
                Some(router.input),
                std::slice::from_mut(&mut point),
            )
        };
    }
    let (owner, leave) = {
        let mut state = router.state.borrow_mut();
        let owner = state.mouse.unwrap_or_else(|| {
            if hits(&state, point) {
                Owner::Ui
            } else {
                Owner::Canvas
            }
        });
        if down {
            state.mouse = Some(owner);
            state.buttons |= mask;
        }
        if up {
            state.buttons &= !mask;
            if state.buttons == 0 {
                state.mouse = None;
            }
        }
        let leave = state.ui_hover && (owner != Owner::Ui || msg == WM_MOUSELEAVE);
        state.ui_hover = owner == Owner::Ui && msg != WM_MOUSELEAVE;
        (owner, leave)
    };
    if leave {
        let _ = router.send_ui(WM_MOUSELEAVE, WPARAM(0), LPARAM(0));
    }
    if owner == Owner::Canvas {
        return router.continue_canvas(msg, wp, lp);
    }
    let mapped = if wheel {
        lp
    } else {
        LPARAM((u32::from(point.x as u16) | (u32::from(point.y as u16) << 16)) as isize)
    };
    router.send_ui(msg, wp, mapped)
}

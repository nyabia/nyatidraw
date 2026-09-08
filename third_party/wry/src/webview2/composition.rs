//! Opt-in parent composition. Pixels target the parent; input targets the
//! independently region-shaped WRY HWND. No canvas samples enter this module.
use std::{
  cell::{Cell, RefCell},
  rc::Rc,
  sync::mpsc,
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::{
  core::{IUnknown, Interface},
  Win32::{
    Foundation::*,
    Graphics::{
      DirectComposition::*,
      Gdi::{MapWindowPoints, ScreenToClient},
    },
    UI::{
      Controls::WM_MOUSELEAVE,
      Input::{KeyboardAndMouse::*, Pointer::*},
      Shell::*,
      WindowsAndMessaging::*,
    },
  },
};

const SUBCLASS: usize = 0x4e594343;

// A failed setup must close the newly created browser even before the host's
// normal Drop implementation can take ownership of it.
struct PendingController(Option<ICoreWebView2Controller>);
impl Drop for PendingController {
  fn drop(&mut self) {
    if let Some(controller) = self.0.take() {
      let _ = unsafe { controller.Close() };
    }
  }
}

pub(super) struct CompositionHost {
  pub controller: ICoreWebView2CompositionController,
  device: IDCompositionDesktopDevice,
  _target: IDCompositionTarget,
  _visual: IDCompositionVisual2,
  input: Rc<Input>,
}

struct Input {
  hwnd: HWND,
  controller: ICoreWebView2CompositionController,
  environment: ICoreWebView2Environment3,
  buttons: Cell<u32>,
  tracking: Cell<bool>,
  pointers: RefCell<[Option<(u32, ICoreWebView2PointerInfo)>; 32]>,
}

impl CompositionHost {
  pub fn new(
    parent: HWND,
    hwnd: HWND,
    env: &ICoreWebView2Environment,
    incognito: bool,
  ) -> crate::Result<Self> {
    let (tx, rx) = mpsc::channel();
    let handler = CreateCoreWebView2CompositionControllerCompletedHandler::create(Box::new(
      move |error, controller| {
        let result =
          error.and_then(|_| controller.ok_or_else(|| windows::core::Error::from(E_POINTER)));
        tx.send(result)
          .map_err(|_| windows::core::Error::from(E_UNEXPECTED))
      },
    ));
    unsafe {
      let env10 = env.cast::<ICoreWebView2Environment10>()?;
      let options = env10.CreateCoreWebView2ControllerOptions()?;
      options.SetIsInPrivateModeEnabled(incognito)?;
      if let Ok(color) = options.cast::<ICoreWebView2ControllerOptions3>() {
        color.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
          A: 0,
          R: 0,
          G: 0,
          B: 0,
        })?;
      }
      env10.CreateCoreWebView2CompositionControllerWithOptions(hwnd, &options, &handler)?;
    }
    let controller = webview2_com::wait_with_pump(rx)??;
    let mut pending = PendingController(Some(controller.cast::<ICoreWebView2Controller>()?));
    unsafe {
      let base = controller.cast::<ICoreWebView2Controller2>()?;
      base.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
        A: 0,
        R: 0,
        G: 0,
        B: 0,
      })?;
      let device: IDCompositionDesktopDevice = DCompositionCreateDevice2(None::<&IUnknown>)?;
      let target = device.CreateTargetForHwnd(parent, true)?;
      let visual = device.CreateVisual()?;
      target.SetRoot(&visual)?;
      controller.SetRootVisualTarget(&visual)?;
      device.Commit()?;
      let input = Rc::new(Input {
        hwnd,
        controller: controller.clone(),
        environment: env.cast()?,
        buttons: Cell::new(0),
        tracking: Cell::new(false),
        pointers: RefCell::new(std::array::from_fn(|_| None)),
      });
      if !SetWindowSubclass(
        hwnd,
        Some(input_proc),
        SUBCLASS,
        Rc::as_ptr(&input) as usize,
      )
      .as_bool()
      {
        let _ = base.Close();
        return Err(windows::core::Error::from_win32().into());
      }
      let host = Self {
        controller,
        device,
        _target: target,
        _visual: visual,
        input,
      };
      pending.0.take();
      Ok(host)
    }
  }
}

impl Drop for CompositionHost {
  fn drop(&mut self) {
    unsafe {
      let _ = RemoveWindowSubclass(self.input.hwnd, Some(input_proc), SUBCLASS);
      let _ = self.controller.SetRootVisualTarget(None::<&IUnknown>);
      let _ = self.device.Commit();
      if let Ok(base) = self.controller.cast::<ICoreWebView2Controller>() {
        let _ = base.Close();
      }
    }
  }
}

unsafe extern "system" fn input_proc(
  hwnd: HWND,
  msg: u32,
  wp: WPARAM,
  lp: LPARAM,
  _: usize,
  data: usize,
) -> LRESULT {
  // Keep the state alive across COM calls which may pump and re-enter messages.
  let ptr = data as *const Input;
  Rc::increment_strong_count(ptr);
  let state = Rc::from_raw(ptr);
  if msg == WM_NCDESTROY {
    let _ = RemoveWindowSubclass(hwnd, Some(input_proc), SUBCLASS);
    return DefSubclassProc(hwnd, msg, wp, lp);
  }
  match state.message(msg, wp, lp) {
    Ok(Some(result)) => result,
    Ok(None) => DefSubclassProc(hwnd, msg, wp, lp),
    Err(error) => {
      // A failed UI delivery must not fall through into promoted canvas input.
      eprintln!("wry-composition event=input-error message={msg} error={error}");
      LRESULT(0)
    }
  }
}

impl Input {
  unsafe fn message(
    &self,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
  ) -> windows::core::Result<Option<LRESULT>> {
    match msg {
      WM_SETFOCUS => {
        self
          .controller
          .cast::<ICoreWebView2Controller>()?
          .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
        return Ok(Some(LRESULT(0)));
      }
      WM_SETCURSOR if (lp.0 as u32 & 0xffff) == HTCLIENT => {
        let mut cursor = HCURSOR::default();
        self.controller.Cursor(&mut cursor)?;
        SetCursor(Some(cursor));
        return Ok(Some(LRESULT(1)));
      }
      WM_POINTERDOWN
      | WM_POINTERUP
      | WM_POINTERUPDATE
      | WM_POINTERENTER
      | WM_POINTERLEAVE
      | WM_POINTERACTIVATE
      | WM_POINTERCAPTURECHANGED => {
        if msg == WM_POINTERCAPTURECHANGED {
          self.cancel_pointer(wp.0 as u32 & 0xffff)?;
          return Ok(Some(LRESULT(0)));
        }
        let id = wp.0 as u32 & 0xffff;
        if let Err(error) = self.pointer(msg, id) {
          let _ = self.cancel_pointer(id);
          return Err(error);
        }
        return Ok(Some(LRESULT(if msg == WM_POINTERACTIVATE {
          PA_ACTIVATE as isize
        } else {
          0
        })));
      }
      WM_CAPTURECHANGED | WM_CANCELMODE => {
        // Deliver releases for every mouse button owned by this sink. Pointer
        // cancellation is delivered separately through WM_POINTERCAPTURECHANGED.
        let buttons = self.buttons.replace(0);
        if msg == WM_CANCELMODE && GetCapture() == self.hwnd {
          let _ = ReleaseCapture();
        }
        if msg == WM_CANCELMODE {
          let ids: [Option<u32>; 32] = {
            let pointers = self.pointers.borrow();
            std::array::from_fn(|index| pointers[index].as_ref().map(|p| p.0))
          };
          for id in ids.into_iter().flatten() {
            if let Err(error) = self.cancel_pointer(id) {
              eprintln!("wry-composition event=pointer-cancel-error error={error}");
            }
          }
        }
        for (mask, kind) in [
          (1, WM_LBUTTONUP),
          (2, WM_RBUTTONUP),
          (4, WM_MBUTTONUP),
          (8, WM_XBUTTONUP),
          (16, WM_XBUTTONUP),
        ] {
          if buttons & mask != 0 {
            self.controller.SendMouseInput(
              COREWEBVIEW2_MOUSE_EVENT_KIND(kind as i32),
              COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS(0),
              if mask == 8 {
                1
              } else if mask == 16 {
                2
              } else {
                0
              },
              POINT {
                x: -32768,
                y: -32768,
              },
            )?;
          }
        }
        return Ok(None);
      }
      _ => {}
    }
    let mouse = matches!(
      msg,
      WM_MOUSEMOVE
        | WM_MOUSELEAVE
        | WM_LBUTTONDOWN
        | WM_LBUTTONUP
        | WM_LBUTTONDBLCLK
        | WM_RBUTTONDOWN
        | WM_RBUTTONUP
        | WM_RBUTTONDBLCLK
        | WM_MBUTTONDOWN
        | WM_MBUTTONUP
        | WM_MBUTTONDBLCLK
        | WM_XBUTTONDOWN
        | WM_XBUTTONUP
        | WM_XBUTTONDBLCLK
        | WM_MOUSEWHEEL
        | WM_MOUSEHWHEEL
    );
    if !mouse {
      return Ok(None);
    }
    // Suppress mouse promotion of touch/pen already sent as pointer events.
    if GetMessageExtraInfo().0 as usize & 0xffffff00 == 0xff515700 {
      return Ok(Some(LRESULT(0)));
    }
    let mut point = POINT {
      x: lp.0 as i16 as i32,
      y: (lp.0 >> 16) as i16 as i32,
    };
    let data = if matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
      let _ = ScreenToClient(self.hwnd, &mut point);
      (wp.0 >> 16) as i16 as i32 as u32
    } else if matches!(msg, WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK) {
      (wp.0 >> 16) as u32
    } else {
      0
    };
    let mask = match msg {
      WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => 1,
      WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_RBUTTONUP => 2,
      WM_MBUTTONDOWN | WM_MBUTTONDBLCLK | WM_MBUTTONUP => 4,
      WM_XBUTTONDOWN | WM_XBUTTONDBLCLK | WM_XBUTTONUP => {
        if data == 1 {
          8
        } else {
          16
        }
      }
      _ => 0,
    };
    if matches!(
      msg,
      WM_LBUTTONDOWN
        | WM_LBUTTONDBLCLK
        | WM_RBUTTONDOWN
        | WM_RBUTTONDBLCLK
        | WM_MBUTTONDOWN
        | WM_MBUTTONDBLCLK
        | WM_XBUTTONDOWN
        | WM_XBUTTONDBLCLK
    ) {
      self.buttons.set(self.buttons.get() | mask);
      SetCapture(self.hwnd);
      let _ = SetFocus(Some(self.hwnd));
      self
        .controller
        .cast::<ICoreWebView2Controller>()?
        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
    }
    if msg == WM_MOUSEMOVE && !self.tracking.get() {
      let mut track = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: self.hwnd,
        dwHoverTime: 0,
      };
      TrackMouseEvent(&mut track)?;
      self.tracking.set(true);
    }
    if msg == WM_MOUSELEAVE {
      self.tracking.set(false);
      point = POINT::default();
    }
    let delivered = self.controller.SendMouseInput(
      COREWEBVIEW2_MOUSE_EVENT_KIND(msg as i32),
      COREWEBVIEW2_MOUSE_EVENT_VIRTUAL_KEYS((wp.0 & 0xffff) as i32),
      data,
      point,
    );
    if matches!(
      msg,
      WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP
    ) {
      self.buttons.set(self.buttons.get() & !mask);
      if self.buttons.get() == 0 && GetCapture() == self.hwnd {
        let _ = ReleaseCapture();
      }
    }
    delivered?;
    Ok(Some(LRESULT(
      if matches!(msg, WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK) {
        1
      } else {
        0
      },
    )))
  }

  unsafe fn pointer(&self, msg: u32, id: u32) -> windows::core::Result<()> {
    let mut native = POINTER_INFO::default();
    GetPointerInfo(id, &mut native)?;
    if native.pointerType != PT_PEN && native.pointerType != PT_TOUCH {
      return Ok(());
    }
    // Snapshot every OS pointer record before any focus/COM call can re-enter
    // the message loop and replace the current message's pointer history.
    let mut pen = POINTER_PEN_INFO::default();
    let mut touch = POINTER_TOUCH_INFO::default();
    if native.pointerType == PT_PEN {
      GetPointerPenInfo(id, &mut pen)?;
    } else {
      GetPointerTouchInfo(id, &mut touch)?;
    }
    let mut device = RECT::default();
    let mut display = RECT::default();
    GetPointerDeviceRects(native.sourceDevice, &mut device, &mut display)?;
    let _ = ScreenToClient(self.hwnd, &mut native.ptPixelLocation);
    let _ = ScreenToClient(self.hwnd, &mut native.ptPixelLocationRaw);
    if msg == WM_POINTERDOWN {
      let _ = SetFocus(Some(self.hwnd));
      self
        .controller
        .cast::<ICoreWebView2Controller>()?
        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)?;
    }
    let info = self.environment.CreateCoreWebView2PointerInfo()?;
    info.SetPointerKind(native.pointerType.0 as u32)?;
    info.SetPointerId(id)?;
    info.SetFrameId(native.frameId)?;
    info.SetPointerFlags(native.pointerFlags.0)?;
    info.SetPointerDeviceRect(device)?;
    info.SetDisplayRect(display)?;
    info.SetPixelLocation(native.ptPixelLocation)?;
    info.SetPixelLocationRaw(native.ptPixelLocationRaw)?;
    info.SetHimetricLocation(native.ptHimetricLocation)?;
    info.SetHimetricLocationRaw(native.ptHimetricLocationRaw)?;
    info.SetTime(native.dwTime)?;
    info.SetHistoryCount(native.historyCount)?;
    info.SetInputData(native.InputData)?;
    info.SetKeyStates(native.dwKeyStates)?;
    info.SetPerformanceCount(native.PerformanceCount)?;
    info.SetButtonChangeKind(native.ButtonChangeType.0)?;
    if native.pointerType == PT_PEN {
      info.SetPenFlags(pen.penFlags)?;
      info.SetPenMask(pen.penMask)?;
      info.SetPenPressure(pen.pressure)?;
      info.SetPenRotation(pen.rotation)?;
      info.SetPenTiltX(pen.tiltX)?;
      info.SetPenTiltY(pen.tiltY)?;
    } else {
      let mut points = [
        POINT {
          x: touch.rcContact.left,
          y: touch.rcContact.top,
        },
        POINT {
          x: touch.rcContact.right,
          y: touch.rcContact.bottom,
        },
        POINT {
          x: touch.rcContactRaw.left,
          y: touch.rcContactRaw.top,
        },
        POINT {
          x: touch.rcContactRaw.right,
          y: touch.rcContactRaw.bottom,
        },
      ];
      let _ = MapWindowPoints(None, Some(self.hwnd), &mut points);
      info.SetTouchFlags(touch.touchFlags)?;
      info.SetTouchMask(touch.touchMask)?;
      info.SetTouchContact(RECT {
        left: points[0].x,
        top: points[0].y,
        right: points[1].x,
        bottom: points[1].y,
      })?;
      info.SetTouchContactRaw(RECT {
        left: points[2].x,
        top: points[2].y,
        right: points[3].x,
        bottom: points[3].y,
      })?;
      info.SetTouchOrientation(touch.orientation)?;
      info.SetTouchPressure(touch.pressure)?;
    }
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE)
      && native.pointerFlags.0 & POINTER_FLAG_INCONTACT.0 != 0
    {
      let mut pointers = self.pointers.borrow_mut();
      let slot = pointers
        .iter()
        .position(|p| p.as_ref().is_some_and(|p| p.0 == id))
        .or_else(|| pointers.iter().position(Option::is_none))
        .ok_or_else(|| windows::core::Error::from(E_OUTOFMEMORY))?;
      pointers[slot] = Some((id, info.clone()));
    }
    self
      .controller
      .SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND(msg as i32), &info)?;
    if msg == WM_POINTERUP
      || (msg == WM_POINTERLEAVE && native.pointerFlags.0 & POINTER_FLAG_INCONTACT.0 == 0)
    {
      let mut pointers = self.pointers.borrow_mut();
      if let Some(slot) = pointers
        .iter_mut()
        .find(|p| p.as_ref().is_some_and(|p| p.0 == id))
      {
        *slot = None;
      }
    }
    Ok(())
  }

  unsafe fn cancel_pointer(&self, id: u32) -> windows::core::Result<()> {
    let cached = {
      let mut pointers = self.pointers.borrow_mut();
      pointers
        .iter_mut()
        .find(|p| p.as_ref().is_some_and(|p| p.0 == id))
        .and_then(Option::take)
    };
    if let Some((_, info)) = cached {
      // Capture-changed is not a supported WebView event kind. End its cached
      // contact explicitly with Win32's canceled flag, even after the OS record
      // is no longer queryable.
      info.SetPointerFlags(POINTER_FLAG_CANCELED.0 | POINTER_FLAG_UP.0)?;
      self
        .controller
        .SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND_UP, &info)?;
      self
        .controller
        .SendPointerInput(COREWEBVIEW2_POINTER_EVENT_KIND_LEAVE, &info)?;
    }
    Ok(())
  }
}

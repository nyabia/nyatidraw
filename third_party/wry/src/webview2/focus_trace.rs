//! Opt-in, bounded focus diagnostics. Never record text or arbitrary key values.
use std::sync::{
  atomic::{AtomicUsize, Ordering},
  OnceLock,
};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::Win32::{
  Foundation::{HWND, WPARAM},
  UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

fn reserve() -> Option<usize> {
  static ENABLED: OnceLock<bool> = OnceLock::new();
  static COUNT: AtomicUsize = AtomicUsize::new(0);
  if !*ENABLED.get_or_init(|| {
    std::env::var_os("NYATIDRAW_TRACE_COMPOSITION_FOCUS").as_deref()
      == Some(std::ffi::OsStr::new("1"))
  }) {
    return None;
  }
  COUNT
    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
      (count < 96).then_some(count + 1)
    })
    .ok()
}

unsafe fn state(hwnd: HWND) -> String {
  let mut info = GUITHREADINFO {
    cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
    ..Default::default()
  };
  let thread = GetWindowThreadProcessId(hwnd, None);
  let status = GetGUIThreadInfo(thread, &mut info)
    .err()
    .map_or(0, |error| error.code().0);
  format!(
    "thread={thread},status={status:#x},active={:?},focus={:?},capture={:?},foreground={:?}",
    info.hwndActive,
    info.hwndFocus,
    info.hwndCapture,
    GetForegroundWindow()
  )
}

pub(super) unsafe fn move_focus(
  controller: &ICoreWebView2Controller,
  input: HWND,
  stage: &'static str,
) -> windows::core::Result<()> {
  let record = reserve();
  let before = record.map(|_| state(input));
  let result = controller.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
  if let Some(record) = record {
    let mut parent = HWND::default();
    let parent_status = controller
      .ParentWindow(&mut parent)
      .err()
      .map_or(0, |error| error.code().0);
    let status = result.as_ref().err().map_or(0, |error| error.code().0);
    eprintln!(
      "wry-focus record={record} event=move-focus stage={stage} input={input:?} parent={parent:?} parent_status={parent_status:#x} status={status:#x} before=[{}] after=[{}]",
      before.unwrap_or_default(), state(input)
    );
  }
  result
}

pub(super) unsafe fn message(hwnd: HWND, msg: u32, wp: WPARAM, source: &'static str) {
  let event = match msg {
    WM_SETFOCUS => "set-focus",
    WM_KILLFOCUS => "kill-focus",
    WM_KEYDOWN | WM_SYSKEYDOWN => "key-down",
    _ => return,
  };
  let Some(record) = reserve() else {
    return;
  };
  let shortcut = if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN) {
    match wp.0 {
      0x41 if GetKeyState(i32::from(VK_CONTROL.0)) < 0 => "ctrl-a",
      0x75 => "f6",
      _ => "other",
    }
  } else {
    "none"
  };
  eprintln!(
    "wry-focus record={record} event={event} source={source} hwnd={hwnd:?} shortcut={shortcut} state=[{}]",
    state(hwnd)
  );
}

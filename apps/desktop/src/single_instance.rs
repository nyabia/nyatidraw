//! Windows-only single-instance activation transport.
//!
//! The named mutex answers only "is a primary already alive?". A bounded,
//! versioned named-pipe message carries an optional positional activation path
//! to that primary. The pipe never opens a project or touches editor state;
//! its only consumer is the UI thread that owns the WGPU canvas.

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HWND,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
            PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::CreateMutexW,
        },
        UI::WindowsAndMessaging::{PostMessageW, SW_RESTORE, SetForegroundWindow, ShowWindow},
    },
    core::PCWSTR,
};

const MUTEX_NAME: &str = "Local\\NyatiDraw.SingleInstance.v1";
const PIPE_NAME: &str = r"\\.\pipe\NyatiDraw.Activation.v1";
const ACTIVATION_MESSAGE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x4e2;
const WIRE_MAGIC: [u8; 4] = *b"NTDA";
const WIRE_VERSION: u16 = 1;
const MAX_PATH_UTF16_UNITS: usize = 32_768;
const MAX_PENDING_ACTIVATIONS: usize = 8;
const PIPE_BUFFER_BYTES: u32 = 65_536;
const ROUTE_RETRY_COUNT: usize = 20;
const ROUTE_RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(25);

/// The first process keeps this guard alive for the desktop runtime.
pub(crate) struct PrimaryInstance {
    mutex: HANDLE,
    inbox: ActivationInbox,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub(crate) struct ActivationInbox {
    inner: Arc<ActivationInboxInner>,
}

struct ActivationInboxInner {
    pending: Mutex<VecDeque<PathBuf>>,
    // HWND's pointer wrapper is deliberately !Send. The listener never
    // dereferences these values; it keeps their integer form until making
    // best-effort Win32 requests on behalf of the UI thread.
    parent: Mutex<Option<isize>>,
    child: Mutex<Option<isize>>,
}

pub(crate) enum InstanceDisposition {
    Primary(PrimaryInstance),
    SecondaryRouted,
}

/// Acquires the primary mutex or routes this invocation to an existing primary.
///
/// The secondary never falls back to starting a competing writer: failure to
/// contact the primary is an explicit process failure.
pub(crate) fn acquire_or_route() -> Result<InstanceDisposition, String> {
    let mutex_name = wide_null(MUTEX_NAME);
    // SAFETY: the static-sized UTF-16 name is null-terminated and no security
    // descriptor is requested.
    let mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) }
        .map_err(|error| format!("create single-instance mutex: {error}"))?;
    // SAFETY: `CreateMutexW` sets the thread last-error used solely to detect
    // an already-existing named mutex for this call.
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        // SAFETY: this process owns this temporary handle and no further use
        // occurs after closing it.
        let _ = unsafe { CloseHandle(mutex) };
        // Resolve relative CLI paths in the secondary process. The primary
        // may have a different working directory and must not reinterpret
        // their meaning after IPC transport.
        let path = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .map(absolute_invocation_path);
        route_secondary_invocation(path.as_deref())?;
        return Ok(InstanceDisposition::SecondaryRouted);
    }

    let inbox = ActivationInbox {
        inner: Arc::new(ActivationInboxInner {
            pending: Mutex::new(VecDeque::with_capacity(MAX_PENDING_ACTIVATIONS)),
            parent: Mutex::new(None),
            child: Mutex::new(None),
        }),
    };
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let listener_inbox = inbox.clone();
    let listener_shutdown = Arc::clone(&shutdown);
    let listener = thread::Builder::new()
        .name("nyatidraw-activation-pipe".into())
        .spawn(move || activation_listener(listener_inbox, listener_shutdown))
        .map_err(|error| {
            // SAFETY: listener creation failed before any other owner received
            // the handle, so this process must release the mutex itself.
            let _ = unsafe { CloseHandle(mutex) };
            format!("start activation listener: {error}")
        })?;

    println!("native-shell event=single-instance-primary pipe=named-mutex-and-pipe");
    Ok(InstanceDisposition::Primary(PrimaryInstance {
        mutex,
        inbox,
        shutdown,
        listener: Some(listener),
    }))
}

impl PrimaryInstance {
    pub(crate) fn inbox(&self) -> ActivationInbox {
        self.inbox.clone()
    }
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);
        // Wake a listener blocked in ConnectNamedPipe. The empty frame is a
        // private shutdown nudge and is discarded after the shutdown check.
        // Retry closes the race where shutdown is set just before the listener
        // creates its next pipe and blocks in ConnectNamedPipe.
        let wake = encode_frame(None).expect("empty activation frame is bounded");
        while self
            .listener
            .as_ref()
            .is_some_and(|listener| !listener.is_finished())
            && route_frame(&wake).is_err()
        {
            thread::sleep(ROUTE_RETRY_SLEEP);
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        // SAFETY: this is the primary process's unique mutex handle.
        let _ = unsafe { CloseHandle(self.mutex) };
    }
}

impl ActivationInbox {
    /// Binds the transport to the UI-thread-owned windows after Dioxus has
    /// created them. Earlier messages remain queued and are posted once the
    /// child canvas exists.
    pub(crate) fn attach_windows(&self, parent: HWND, child: HWND) {
        *self
            .inner
            .parent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(parent.0 as isize);
        *self
            .inner
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(child.0 as isize);
        self.request_foreground_and_dispatch();
    }

    /// Drains at most one accepted positional path on the canvas UI thread.
    /// A `None` pipe message asks only for foregrounding and has no queue row.
    pub(crate) fn take_activation(&self) -> Option<PathBuf> {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    fn accept(&self, path: Option<PathBuf>) {
        if let Some(path) = path {
            let accepted = {
                let mut pending = self
                    .inner
                    .pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending.len() >= MAX_PENDING_ACTIVATIONS {
                    false
                } else {
                    pending.push_back(path);
                    true
                }
            };
            if !accepted {
                eprintln!(
                    "native-shell event=activation-rejected reason=bounded-pending-queue-full capacity={MAX_PENDING_ACTIVATIONS}"
                );
            }
        }
        self.request_foreground_and_dispatch();
    }

    fn request_foreground_and_dispatch(&self) {
        let parent = *self
            .inner
            .parent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let child = *self
            .inner
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(parent) = parent {
            let parent = HWND(parent as *mut std::ffi::c_void);
            // These are best-effort foreground requests. Windows foreground
            // policy may decline SetForegroundWindow; the project activation
            // still reaches the primary through its own child message.
            // SAFETY: `parent` is recorded only from the live Dioxus window.
            let _ = unsafe { ShowWindow(parent, SW_RESTORE) };
            // SAFETY: foregrounding is advisory and has no pointer contract.
            let _ = unsafe { SetForegroundWindow(parent) };
        }
        if let Some(child) = child {
            let child = HWND(child as *mut std::ffi::c_void);
            // SAFETY: posting to a closing child may fail harmlessly. The
            // queued path stays retained until a later successful post.
            let _ = unsafe {
                PostMessageW(
                    Some(child),
                    ACTIVATION_MESSAGE,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                )
            };
        }
    }
}

pub(crate) const fn activation_message() -> u32 {
    ACTIVATION_MESSAGE
}

#[allow(clippy::needless_pass_by_value)] // ownership keeps the listener state alive
fn activation_listener(inbox: ActivationInbox, shutdown: Arc<std::sync::atomic::AtomicBool>) {
    while !shutdown.load(std::sync::atomic::Ordering::Acquire) {
        let name = wide_null(PIPE_NAME);
        // SAFETY: the pipe name is valid UTF-16 and the server owns each
        // returned handle until it disconnects and closes it below.
        let pipe = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                None,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            eprintln!("native-shell event=activation-listener-failed stage=create-pipe");
            return;
        }
        // SAFETY: this thread exclusively owns `pipe` while connecting.
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok_and(|()| true)
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected && !shutdown.load(std::sync::atomic::Ordering::Acquire) {
            match read_frame(pipe) {
                Ok(path) => inbox.accept(path),
                Err(error) => eprintln!("native-shell event=activation-rejected error={error}"),
            }
        }
        // SAFETY: the connection and handle are owned by this loop iteration.
        let _ = unsafe { DisconnectNamedPipe(pipe) };
        // SAFETY: after disconnect no code retains this server handle.
        let _ = unsafe { CloseHandle(pipe) };
    }
}

fn route_secondary_invocation(path: Option<&std::path::Path>) -> Result<(), String> {
    let frame = encode_frame(path)?;
    for attempt in 1..=ROUTE_RETRY_COUNT {
        match route_frame(&frame) {
            Ok(()) => {
                println!(
                    "native-shell event=activation-routed target=existing-primary attempt={attempt}"
                );
                return Ok(());
            }
            Err(error) if attempt < ROUTE_RETRY_COUNT => {
                if attempt == 1 {
                    eprintln!("native-shell event=activation-route-pending error={error}");
                }
                thread::sleep(ROUTE_RETRY_SLEEP);
            }
            Err(error) => {
                return Err(format!(
                    "another NyatiDraw instance owns the session but did not accept activation: {error}"
                ));
            }
        }
    }
    unreachable!("bounded retry loop always returns")
}

fn route_frame(frame: &[u8]) -> Result<(), String> {
    let name = wide_null(PIPE_NAME);
    // SAFETY: valid UTF-16 path, write-only client, and no borrowed security
    // attributes or template handle.
    let pipe = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|error| format!("connect activation pipe: {error}"))?;
    let result = write_all(pipe, frame);
    // SAFETY: the secondary owns this client pipe handle.
    let _ = unsafe { CloseHandle(pipe) };
    result
}

fn encode_frame(path: Option<&std::path::Path>) -> Result<Vec<u8>, String> {
    let wide = path.map_or_else(Vec::new, |path| encode_os(path.as_os_str()));
    if wide.len() > MAX_PATH_UTF16_UNITS {
        return Err(format!(
            "activation path exceeds {MAX_PATH_UTF16_UNITS} UTF-16 units"
        ));
    }
    let count = u32::try_from(wide.len()).map_err(|_| "activation path length overflow")?;
    let mut frame = Vec::with_capacity(8 + wide.len().saturating_mul(2));
    frame.extend_from_slice(&WIRE_MAGIC);
    frame.extend_from_slice(&WIRE_VERSION.to_le_bytes());
    frame.extend_from_slice(&count.to_le_bytes());
    for unit in wide {
        frame.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(frame)
}

fn read_frame(pipe: HANDLE) -> Result<Option<PathBuf>, String> {
    let mut header = [0_u8; 10];
    read_exact(pipe, &mut header)?;
    if header[..4] != WIRE_MAGIC {
        return Err("activation wire magic mismatch".into());
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != WIRE_VERSION {
        return Err(format!("activation wire version unsupported: {version}"));
    }
    let count = u32::from_le_bytes([header[6], header[7], header[8], header[9]]);
    let count = usize::try_from(count).map_err(|_| "activation length conversion")?;
    if count > MAX_PATH_UTF16_UNITS {
        return Err(format!("activation path is above bounded limit: {count}"));
    }
    if count == 0 {
        return Ok(None);
    }
    let byte_len = count
        .checked_mul(2)
        .ok_or("activation payload byte length overflow")?;
    let mut bytes = vec![0_u8; byte_len];
    read_exact(pipe, &mut bytes)?;
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    Ok(Some(PathBuf::from(OsString::from_wide(&units))))
}

fn write_all(pipe: HANDLE, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut written = 0_u32;
        // SAFETY: the slice lives for this synchronous call and `written` is
        // writable output storage.
        unsafe { WriteFile(pipe, Some(&bytes[offset..]), Some(&raw mut written), None) }
            .map_err(|error| format!("write activation frame: {error}"))?;
        let written = usize::try_from(written).map_err(|_| "activation write length")?;
        if written == 0 {
            return Err("activation pipe closed during write".into());
        }
        offset = offset.saturating_add(written);
    }
    Ok(())
}

fn read_exact(pipe: HANDLE, bytes: &mut [u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        let mut read = 0_u32;
        // SAFETY: the mutable tail lives for this synchronous call and `read`
        // is valid output storage.
        unsafe { ReadFile(pipe, Some(&mut bytes[offset..]), Some(&raw mut read), None) }
            .map_err(|error| format!("read activation frame: {error}"))?;
        let read = usize::try_from(read).map_err(|_| "activation read length")?;
        if read == 0 {
            return Err("activation pipe closed during read".into());
        }
        offset = offset.saturating_add(read);
    }
    Ok(())
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn encode_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

fn absolute_invocation_path(path: PathBuf) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(&path) {
        return canonical;
    }
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or(path.clone(), |directory| directory.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_frame_keeps_non_ascii_utf16_path_units() {
        let path = PathBuf::from(r"E:\게임\player.png");
        let frame = encode_frame(Some(&path)).expect("short path encodes");
        assert_eq!(&frame[..4], b"NTDA");
        assert_eq!(u16::from_le_bytes([frame[4], frame[5]]), WIRE_VERSION);
        assert_eq!(
            u32::from_le_bytes([frame[6], frame[7], frame[8], frame[9]]) as usize,
            path.as_os_str().encode_wide().count()
        );
    }

    #[test]
    fn activation_frame_rejects_unbounded_path_before_pipe_io() {
        let oversized = OsString::from_wide(&vec![u16::from(b'x'); MAX_PATH_UTF16_UNITS + 1]);
        assert!(encode_frame(Some(std::path::Path::new(&oversized))).is_err());
    }
}

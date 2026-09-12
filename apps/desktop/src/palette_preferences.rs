//! Pinned palette slots are user preferences, outside artwork and history.
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc::SyncSender},
    thread::JoinHandle,
};

pub(super) const CAPACITY: usize = 10;
pub(super) type Pins = [Option<[u8; 4]>; CAPACITY];
const MAGIC: &[u8; 8] = b"NYCOLR01";
const FILE_BYTES: usize = 8 + CAPACITY * 5;

#[derive(Clone)]
pub(super) struct PalettePreferences {
    shared: Arc<Mutex<Pending>>,
    writer: Arc<Writer>,
}

struct Pending {
    pins: Option<Pins>,
    notice: Option<String>,
}

struct Writer {
    wake: Option<SyncSender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl PalettePreferences {
    pub(super) fn open(notify: Arc<dyn Fn() + Send + Sync>) -> (Self, Pins) {
        let path = settings_path();
        let loaded = path.as_deref().map_or_else(
            || Err(std::io::Error::other("palette preference path missing")),
            load,
        );
        let (pins, writable, notice) = match loaded {
            Ok(pins) => (pins, true, None),
            Err(error) => {
                eprintln!(
                    "desktop-palette event=load-failed existing-preferences-preserved error={error}"
                );
                (
                    [None; CAPACITY],
                    false,
                    Some("고정색 설정을 읽지 못했습니다. 기존 설정 파일은 보존합니다.".into()),
                )
            }
        };
        let shared = Arc::new(Mutex::new(Pending { pins: None, notice }));
        let (wake, receiver) = std::sync::mpsc::sync_channel(1);
        let task = shared.clone();
        let thread = if writable {
            std::thread::Builder::new()
                .name("nyatidraw-palette".into())
                .spawn(move || {
                    while receiver.recv().is_ok() {
                        let pins = task
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pins
                            .take();
                        let Some(pins) = pins else { continue };
                        let result = path
                            .as_deref()
                            .ok_or_else(|| std::io::Error::other("palette preference path missing"))
                            .and_then(|path| replace(path, &encode(&pins)));
                        let notice = result.err().map(|error| {
                            eprintln!("desktop-palette event=save-failed error={error}");
                            "고정색을 저장하지 못했습니다. 고정 상태를 다시 바꾸면 재시도합니다."
                                .into()
                        });
                        task.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .notice = notice;
                        notify();
                    }
                })
                .ok()
        } else {
            None
        };
        if writable && thread.is_none() {
            shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .notice = Some("고정색 저장 작업을 시작하지 못했습니다.".into());
        }
        let wake = thread.is_some().then_some(wake);
        (
            Self {
                shared,
                writer: Arc::new(Writer { wake, thread }),
            },
            pins,
        )
    }

    pub(super) fn save(&self, pins: Pins) {
        let Some(wake) = &self.writer.wake else {
            return;
        };
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pins = Some(pins);
        // One wake and one latest value are enough even during repeated pin changes.
        let _ = wake.try_send(());
    }

    pub(super) fn notice(&self) -> Option<String> {
        self.shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .notice
            .clone()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.wake.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("NAYATI_PALETTE_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("NAYATI_LAYOUT_PATH")
                .map(|path| PathBuf::from(path).with_extension("colors"))
        })
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(|base| PathBuf::from(base).join("NyatiDraw").join("palette.colors"))
        })
}

fn load(path: &Path) -> std::io::Result<Pins> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok([None; CAPACITY]),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(u64::try_from(FILE_BYTES + 1).unwrap())
        .read_to_end(&mut bytes)?;
    let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid pinned palette");
    if bytes.len() != FILE_BYTES || !bytes.starts_with(MAGIC) {
        return Err(invalid());
    }
    let mut pins = [None; CAPACITY];
    for (index, entry) in bytes[8..].chunks_exact(5).enumerate() {
        let color = [entry[1], entry[2], entry[3], entry[4]];
        match entry[0] {
            0 if color == [0; 4] => {}
            1 if color[3] != 0 && !pins.contains(&Some(color)) => pins[index] = Some(color),
            _ => return Err(invalid()),
        }
    }
    Ok(pins)
}

fn encode(pins: &Pins) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    for pin in pins {
        bytes.push(u8::from(pin.is_some()));
        bytes.extend(pin.unwrap_or([0; 4]));
    }
    bytes
}

fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let temporary = parent.join(format!(
        ".nyatidraw-colors-{}-{nonce}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| replace_file(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<_> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both owned buffers are NUL-terminated and live through the call.
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(std::io::Error::other)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

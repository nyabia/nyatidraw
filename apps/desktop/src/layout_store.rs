//! Process-wide workspace preferences; never part of a project transaction.
use nyatidraw_api::{DockAxis, DockLayoutError, DockNode, DockTree, PanelKind};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::JoinHandle,
};

const MAGIC: &[u8; 8] = b"NYDOCK03";
const MAX_BYTES: usize = 4096;
const PANELS: [PanelKind; 12] = [
    PanelKind::Canvas,
    PanelKind::Tools,
    PanelKind::Navigator,
    PanelKind::Layers,
    PanelKind::Brush,
    PanelKind::Color,
    PanelKind::History,
    PanelKind::CanvasActions,
    PanelKind::Viewport,
    PanelKind::QuickColors,
    PanelKind::ToolProperties,
    PanelKind::BrushSizes,
];

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("NAYATI_LAYOUT_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|base| {
                PathBuf::from(base)
                    .join("NyatiDraw")
                    .join("workspace.layout")
            })
        })
}

/// Small workspace-only sizing preferences. Zero means use the content default.
#[derive(Clone, PartialEq)]
pub(crate) struct PanelHeights {
    values: [u16; 12],
    writable: bool,
}

impl PanelHeights {
    pub(crate) fn load() -> Self {
        let mut result = Self {
            values: [0; 12],
            writable: true,
        };
        let Some(path) = settings_path().map(|path| path.with_extension("heights")) else {
            result.writable = false;
            return result;
        };
        let loaded = (|| -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            File::open(path)?.take(33).read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        match loaded {
            Ok(bytes)
                if (bytes.len() == 28 && bytes.starts_with(b"NYHIGH01"))
                    || (bytes.len() == 32 && bytes.starts_with(b"NYHIGH02")) =>
            {
                for (index, pair) in bytes[8..].chunks_exact(2).enumerate() {
                    let height = u16::from_le_bytes([pair[0], pair[1]]);
                    if height != 0 && !(64..=4096).contains(&height) {
                        result.writable = false;
                        result.values = [0; 12];
                        break;
                    }
                    result.values[index] = height;
                }
                if bytes.starts_with(b"NYHIGH01") {
                    // Old height covered all three sections, not subtools alone.
                    result.values[4] = 0;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => result.writable = false,
        }
        if !result.writable {
            eprintln!("native-layout event=height-load-failed existing-preferences-preserved");
        }
        result
    }

    pub(crate) fn get(&self, panel: PanelKind) -> Option<u16> {
        PANELS
            .iter()
            .position(|value| *value == panel)
            .map(|index| self.values[index])
            .filter(|height| *height != 0)
    }

    pub(crate) fn set(&mut self, panel: PanelKind, height: u16) -> std::io::Result<()> {
        if !self.writable || !(64..=4096).contains(&height) {
            return Err(std::io::Error::other(
                "panel height preferences unavailable",
            ));
        }
        let mut values = self.values;
        let index = PANELS
            .iter()
            .position(|value| *value == panel)
            .ok_or_else(|| std::io::Error::other("unknown panel"))?;
        values[index] = height;
        self.values = values;
        Ok(())
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = b"NYHIGH02".to_vec();
        for value in self.values {
            bytes.extend(value.to_le_bytes());
        }
        bytes
    }
}

pub(crate) fn encode(tree: &DockTree) -> Vec<u8> {
    fn node(value: &DockNode, bytes: &mut Vec<u8>) {
        let panel_id =
            |panel| u8::try_from(PANELS.iter().position(|p| *p == panel).unwrap()).unwrap();
        match value {
            DockNode::Panel(panel) => bytes.extend([0, panel_id(*panel)]),
            DockNode::Tabs { active, panels } => {
                bytes.extend([
                    1,
                    u8::try_from(*active).unwrap(),
                    u8::try_from(panels.len()).unwrap(),
                ]);
                bytes.extend(panels.iter().map(|p| panel_id(*p)));
            }
            DockNode::Split {
                axis,
                first_per_mille,
                first,
                second,
            } => {
                bytes.extend([2, u8::from(*axis == DockAxis::Vertical)]);
                bytes.extend(first_per_mille.to_le_bytes());
                node(first, bytes);
                node(second, bytes);
            }
        }
    }
    let mut bytes = MAGIC.to_vec();
    bytes.push(u8::try_from(tree.top().len()).unwrap());
    for panel in tree.top() {
        bytes.push(u8::try_from(PANELS.iter().position(|p| p == panel).unwrap()).unwrap());
    }
    node(tree.root(), &mut bytes);
    bytes
}

pub(crate) fn decode(bytes: &[u8]) -> Result<DockTree, DockLayoutError> {
    fn byte(bytes: &mut &[u8]) -> Result<u8, DockLayoutError> {
        let (value, rest) = bytes.split_first().ok_or(DockLayoutError::CorruptPayload)?;
        *bytes = rest;
        Ok(*value)
    }
    fn panel(bytes: &mut &[u8]) -> Result<PanelKind, DockLayoutError> {
        PANELS
            .get(usize::from(byte(bytes)?))
            .copied()
            .ok_or(DockLayoutError::CorruptPayload)
    }
    fn node(bytes: &mut &[u8], depth: usize) -> Result<DockNode, DockLayoutError> {
        if depth > 16 {
            return Err(DockLayoutError::TooDeep);
        }
        Ok(match byte(bytes)? {
            0 => DockNode::Panel(panel(bytes)?),
            1 => {
                let active = usize::from(byte(bytes)?);
                let len = usize::from(byte(bytes)?);
                if len == 0 || len > PANELS.len() {
                    return Err(DockLayoutError::CorruptPayload);
                }
                let panels = (0..len)
                    .map(|_| panel(bytes))
                    .collect::<Result<Vec<_>, _>>()?;
                DockNode::Tabs { active, panels }
            }
            2 => {
                let axis = match byte(bytes)? {
                    0 => DockAxis::Horizontal,
                    1 => DockAxis::Vertical,
                    _ => return Err(DockLayoutError::CorruptPayload),
                };
                let first_per_mille = u16::from_le_bytes([byte(bytes)?, byte(bytes)?]);
                DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(node(bytes, depth + 1)?),
                    second: Box::new(node(bytes, depth + 1)?),
                }
            }
            _ => return Err(DockLayoutError::CorruptPayload),
        })
    }
    let legacy = bytes.starts_with(b"NYDOCK01");
    let combined_brush = legacy || bytes.starts_with(b"NYDOCK02");
    if bytes.len() > MAX_BYTES || !(bytes.starts_with(MAGIC) || combined_brush) {
        return Err(DockLayoutError::CorruptPayload);
    }
    let mut remaining = &bytes[MAGIC.len()..];
    let top = if legacy {
        PanelKind::TOOLBARS.to_vec()
    } else {
        let len = usize::from(byte(&mut remaining)?);
        if len > PanelKind::TOOLBARS.len() {
            return Err(DockLayoutError::CorruptPayload);
        }
        (0..len)
            .map(|_| panel(&mut remaining))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut root = node(&mut remaining, 0)?;
    if !remaining.is_empty() {
        return Err(DockLayoutError::CorruptPayload);
    }
    if combined_brush {
        fn expand(node: DockNode) -> DockNode {
            match node {
                DockNode::Panel(PanelKind::Brush) => DockTree::brush_stack(node),
                DockNode::Tabs { ref panels, .. } if panels.contains(&PanelKind::Brush) => {
                    DockTree::brush_stack(node)
                }
                DockNode::Split {
                    axis,
                    first_per_mille,
                    first,
                    second,
                } => DockNode::Split {
                    axis,
                    first_per_mille,
                    first: Box::new(expand(*first)),
                    second: Box::new(expand(*second)),
                },
                _ => node,
            }
        }
        root = expand(root);
    }
    DockTree::with_top(root, top)
}

pub(crate) struct LayoutStore {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
struct State {
    latest: Vec<u8>,
    pending_heights: Option<Vec<u8>>,
    pending: bool,
    writing: bool,
    stopping: bool,
    notice: Option<String>,
}

impl LayoutStore {
    pub(crate) fn open(notify: Arc<dyn Fn() + Send + Sync>) -> (Self, DockTree) {
        // The explicit override keeps scratch acceptance away from user preferences.
        let path = settings_path();
        let (tree, notice) = match path.as_deref().map(load) {
            Some(Ok(Some(tree))) => {
                println!("native-layout event=loaded");
                (tree, None)
            }
            Some(Ok(None)) => (DockTree::safe_default(), None),
            Some(Err(error)) => {
                eprintln!("native-layout event=recovered error={error}");
                (
                    DockTree::safe_default(),
                    Some(
                        "화면 배치를 읽지 못해 기본 배치로 열었습니다. 작품에는 영향이 없습니다."
                            .into(),
                    ),
                )
            }
            None => (
                DockTree::safe_default(),
                Some("화면 배치를 저장할 사용자 설정 경로가 없습니다.".into()),
            ),
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                latest: encode(&tree),
                pending_heights: None,
                pending: false,
                writing: false,
                stopping: false,
                notice,
            }),
            changed: Condvar::new(),
        });
        let task = shared.clone();
        let worker = std::thread::Builder::new().name("nyatidraw-layout".into()).spawn(move || {
            loop {
                let mut state = task.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                while !state.pending && state.pending_heights.is_none() && !state.stopping {
                    state = task.changed.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                if !state.pending && state.pending_heights.is_none() { break; }
                let bytes = state.pending.then(|| state.latest.clone());
                let heights = state.pending_heights.take();
                state.pending = false;
                state.writing = true;
                drop(state);
                let result = path.as_deref().ok_or_else(|| std::io::Error::other("user settings path missing"))
                    .and_then(|path| {
                        if let Some(bytes) = &bytes { replace(path, bytes)?; }
                        if let Some(heights) = &heights { replace(&path.with_extension("heights"), heights)?; }
                        Ok(())
                    });
                let mut state = task.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                state.writing = false;
                state.notice = result.err().map(|error| {
                    eprintln!("native-layout event=save-failed error={error}");
                    "화면 배치를 저장하지 못했습니다. 배치를 다시 변경하거나 기본 배치로 복원해 재시도하세요. 작품 저장은 별개입니다.".into()
                });
                if state.notice.is_none() {
                    println!("native-layout event=saved bytes={} heights={}", bytes.as_ref().map_or(0, Vec::len), heights.is_some());
                }
                task.changed.notify_all();
                drop(state);
                notify();
            }
        }).ok();
        if worker.is_none() {
            shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .notice = Some(
                "화면 배치 저장 작업을 시작하지 못했습니다. 앱을 다시 열어 재시도하세요.".into(),
            );
        }
        (Self { shared, worker }, tree)
    }

    pub(crate) fn submit(&self, tree: &DockTree) {
        if self.worker.is_none() {
            return;
        }
        let bytes = encode(tree);
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if bytes == state.latest && state.notice.is_none() {
            return;
        }
        state.latest = bytes;
        state.pending = true;
        self.shared.changed.notify_one();
    }

    pub(crate) fn notice(&self) -> Option<String> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .notice
            .clone()
    }

    pub(crate) fn submit_heights(&self, heights: &PanelHeights) {
        if self.worker.is_none() {
            return;
        }
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending_heights = Some(heights.encode());
        self.shared.changed.notify_one();
    }

    /// Only the background close owner waits; UI and renderer merely submit.
    pub(crate) fn flush(&self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.pending || state.pending_heights.is_some() || state.writing {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for LayoutStore {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stopping = true;
        self.shared.changed.notify_one();
        // The weak UI notification may temporarily own the final bridge on
        // this thread. It must not join itself when that bridge is released.
        if let Some(worker) = self.worker.take()
            && worker.thread().id() != std::thread::current().id()
        {
            let _ = worker.join();
        }
    }
}

fn load(path: &Path) -> std::io::Result<Option<DockTree>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(u64::try_from(MAX_BYTES + 1).unwrap())
        .read_to_end(&mut bytes)?;
    decode(&bytes)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{error:?}")))
}

fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    let temporary = parent.join(format!(
        ".nyatidraw-layout-{}-{nonce}.tmp",
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
    // SAFETY: both paths are owned, NUL-terminated UTF-16 buffers alive for this call.
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

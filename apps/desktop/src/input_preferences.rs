use nyatidraw_input::CanvasInputMode;

fn path() -> Option<std::path::PathBuf> {
    crate::layout_store::settings_path().map(|path| path.with_extension("input-mode"))
}

pub(crate) fn load() -> (CanvasInputMode, bool) {
    let Some(path) = path() else {
        return (CanvasInputMode::default(), false);
    };
    match std::fs::read_to_string(path) {
        Ok(value) => CanvasInputMode::from_storage(value.trim())
            .map_or((CanvasInputMode::default(), false), |mode| (mode, true)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (CanvasInputMode::default(), true)
        }
        Err(_) => (CanvasInputMode::default(), false),
    }
}

pub(crate) fn save(mode: CanvasInputMode) -> Result<(), String> {
    let path = path().ok_or("앱 설정 경로를 찾지 못했습니다")?;
    crate::layout_store::replace(&path, mode.storage_value().as_bytes())
        .map_err(|error| error.to_string())
}

use nyatidraw_input::PressurePreset;

fn path() -> Option<std::path::PathBuf> {
    crate::layout_store::settings_path().map(|path| path.with_extension("pressure"))
}

pub(crate) fn load() -> (PressurePreset, bool) {
    let Some(path) = path() else {
        return (PressurePreset::default(), false);
    };
    match std::fs::read_to_string(path) {
        Ok(value) => PressurePreset::from_storage(value.trim()).map_or_else(
            || {
                eprintln!("native-pressure event=load-failed existing-preferences-preserved");
                (PressurePreset::default(), false)
            },
            |preset| (preset, true),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (PressurePreset::default(), true)
        }
        Err(_) => {
            eprintln!("native-pressure event=load-failed existing-preferences-preserved");
            (PressurePreset::default(), false)
        }
    }
}

pub(crate) fn save(preset: PressurePreset) -> Result<(), String> {
    let path = path().ok_or("앱 설정 경로를 찾지 못했습니다")?;
    crate::layout_store::replace(&path, preset.storage_value().as_bytes())
        .map_err(|error| error.to_string())
}

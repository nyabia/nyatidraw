use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAGIC: &[u8; 8] = b"NYREC001";
const MAX_PROJECTS: usize = 20;
const MAX_PATH_UNITS: usize = 32_768;
const MAX_BYTES: u64 = 8 + 2 + (4 + MAX_PATH_UNITS as u64 * 2) * MAX_PROJECTS as u64;
static RECENT: OnceLock<Mutex<Result<Vec<PathBuf>, String>>> = OnceLock::new();

fn settings_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::layout_store::settings_path().map(|path| path.with_extension("recent"))
}

fn cache() -> &'static Mutex<Result<Vec<PathBuf>, String>> {
    RECENT.get_or_init(|| {
        Mutex::new(settings_path().map_or_else(
            || Ok(Vec::new()),
            |path| load(&path).map_err(|error| error.to_string()),
        ))
    })
}

pub(crate) fn list() -> Result<Vec<PathBuf>, String> {
    cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub(crate) fn last_existing() -> Option<PathBuf> {
    match list() {
        Ok(paths) => paths.into_iter().find(|path| path.is_file()),
        Err(error) => {
            eprintln!("recent-projects event=read-failed original-preserved=true error={error}");
            None
        }
    }
}

pub(crate) fn record_opened(path: PathBuf) -> Result<(), String> {
    let Some(settings) = settings_path() else {
        return Ok(());
    };
    let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
    let mut paths = list()?;
    if paths.first() == Some(&path) {
        return Ok(());
    }
    paths.retain(|previous| previous != &path);
    paths.insert(0, path);
    paths.truncate(MAX_PROJECTS);
    crate::layout_store::replace(
        &settings,
        &encode(&paths).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    *cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Ok(paths);
    Ok(())
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "최근 그림 목록을 읽지 못했습니다. 기존 목록을 보존합니다.",
    )
}

fn load(path: &Path) -> io::Result<Vec<PathBuf>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    decode(&bytes)
}

fn encode(paths: &[PathBuf]) -> io::Result<Vec<u8>> {
    if paths.len() > MAX_PROJECTS {
        return Err(invalid());
    }
    let mut bytes = MAGIC.to_vec();
    bytes.extend(
        u16::try_from(paths.len())
            .map_err(|_| invalid())?
            .to_le_bytes(),
    );
    for path in paths {
        let units: Vec<_> = path.as_os_str().encode_wide().collect();
        if !valid_path(path, &units) {
            return Err(invalid());
        }
        bytes.extend(
            u32::try_from(units.len())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        for unit in units {
            bytes.extend(unit.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn valid_path(path: &Path, units: &[u16]) -> bool {
    path.is_absolute()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ntdr"))
        && !units.is_empty()
        && units.len() <= MAX_PATH_UNITS
        && !units.contains(&0)
}

fn decode(mut bytes: &[u8]) -> io::Result<Vec<PathBuf>> {
    if bytes.len() as u64 > MAX_BYTES || !bytes.starts_with(MAGIC) {
        return Err(invalid());
    }
    bytes = &bytes[MAGIC.len()..];
    let count = usize::from(u16::from_le_bytes(take(&mut bytes)?));
    if count > MAX_PROJECTS {
        return Err(invalid());
    }
    let mut paths = Vec::with_capacity(count);
    for _ in 0..count {
        let length =
            usize::try_from(u32::from_le_bytes(take(&mut bytes)?)).map_err(|_| invalid())?;
        if length > MAX_PATH_UNITS || length > bytes.len() / 2 {
            return Err(invalid());
        }
        let mut units = Vec::with_capacity(length);
        for _ in 0..length {
            units.push(u16::from_le_bytes(take(&mut bytes)?));
        }
        let path = PathBuf::from(OsString::from_wide(&units));
        if !valid_path(&path, &units) || paths.contains(&path) {
            return Err(invalid());
        }
        paths.push(path);
    }
    if !bytes.is_empty() {
        return Err(invalid());
    }
    Ok(paths)
}

fn take<const N: usize>(bytes: &mut &[u8]) -> io::Result<[u8; N]> {
    let (value, rest) = bytes.split_at_checked(N).ok_or_else(invalid)?;
    *bytes = rest;
    value.try_into().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_paths_survive_restart_without_rewriting_artwork_or_damaged_preferences() {
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-recent-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let artwork = directory.join("새 그림.ntdr");
        let settings = directory.join("workspace.recent");
        std::fs::write(
            &artwork,
            b"artwork must never be touched by recent settings",
        )
        .unwrap();
        let paths = vec![artwork.clone()];
        crate::layout_store::replace(&settings, &encode(&paths).unwrap()).unwrap();
        assert_eq!(load(&settings).unwrap(), paths);
        for damage in [
            b"invalid".to_vec(),
            vec![0; usize::try_from(MAX_BYTES + 1).unwrap()],
            {
                let mut bytes = encode(&paths).unwrap();
                bytes.pop();
                bytes
            },
        ] {
            std::fs::write(&settings, &damage).unwrap();
            assert!(load(&settings).is_err());
            assert_eq!(std::fs::read(&settings).unwrap(), damage);
        }
        assert_eq!(
            std::fs::read(&artwork).unwrap(),
            b"artwork must never be touched by recent settings"
        );
        std::fs::remove_file(artwork).unwrap();
        std::fs::remove_file(settings).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}

use std::{
    fs::OpenOptions,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(crate) fn reserve_new_drawing() -> Result<PathBuf, String> {
    let directory = directory().ok_or("자동 복구 폴더를 찾지 못했습니다.")?;
    reserve_in(&directory)
}

pub(crate) fn directory() -> Option<PathBuf> {
    crate::layout_store::settings_path()?
        .parent()
        .map(|parent| parent.join("Sketchbook"))
}

pub(crate) fn is_managed_recovery(path: &Path) -> bool {
    let Some(directory) = directory() else {
        return false;
    };
    is_managed_recovery_in(path, &directory)
}

fn is_managed_recovery_in(path: &Path, directory: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let same_directory = match (
        std::fs::canonicalize(parent),
        std::fs::canonicalize(directory),
    ) {
        (Ok(parent), Ok(directory)) => parent == directory,
        _ => parent == directory,
    };
    same_directory
        && !path.with_extension("png").try_exists().unwrap_or(true)
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("ntdr"))
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|name| {
                name == "작업 중"
                    || name
                        .strip_prefix("새 그림 ")
                        .and_then(|index| index.parse::<u32>().ok())
                        .is_some_and(|index| (1..=100_000).contains(&index))
            })
}

fn reserve_in(directory: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|error| format!("자동 복구 폴더: {error}"))?;
    for index in 1..=100_000 {
        let path = directory.join(format!("새 그림 {index}.ntdr"));
        if path
            .with_extension("png")
            .try_exists()
            .map_err(|error| error.to_string())?
        {
            continue;
        }
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
            Err(error) => {
                return Err(format!(
                    "새 그림의 복구 파일을 준비하지 못했습니다: {error}"
                ));
            }
        }
    }
    Err("새 그림 이름을 확보하지 못했습니다. 다른 이름으로 저장을 사용해주세요.".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn new_drawing_never_overwrites_existing_project_or_paired_export() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-new-drawing-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let project = directory.join("새 그림 1.ntdr");
        let export = directory.join("새 그림 2.png");
        std::fs::write(&project, b"preserve even invalid existing artwork").unwrap();
        std::fs::write(&export, b"preserve existing export").unwrap();
        let first = super::reserve_in(&directory).unwrap();
        let second = super::reserve_in(&directory).unwrap();
        assert_eq!(first, directory.join("새 그림 3.ntdr"));
        assert_eq!(second, directory.join("새 그림 4.ntdr"));
        assert!(super::is_managed_recovery_in(&first, &directory));
        assert!(!super::is_managed_recovery_in(
            &export.with_extension("ntdr"),
            &directory
        ));
        assert!(super::is_managed_recovery_in(
            &directory.join("작업 중.ntdr"),
            &directory
        ));
        assert!(!super::is_managed_recovery_in(
            &directory.join("내 작품.ntdr"),
            &directory
        ));
        assert!(!super::is_managed_recovery_in(
            &directory.join("새 그림 2.png"),
            &directory
        ));
        assert!(!super::is_managed_recovery_in(
            &directory.join("other/새 그림 1.ntdr"),
            &directory
        ));
        assert_eq!(std::fs::metadata(&first).unwrap().len(), 0);
        assert_eq!(
            std::fs::read(&project).unwrap(),
            b"preserve even invalid existing artwork"
        );
        assert_eq!(std::fs::read(&export).unwrap(), b"preserve existing export");
        for path in [first, second, project, export] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }
}

//! Closed-database Save As. This code never replaces an existing destination.
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

use nyatidraw_project_redb::ProjectDb;

pub(crate) fn validate_target(path: &Path) -> Result<(), String> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ntdr"))
    {
        return Err("다른 이름으로 저장은 .ntdr 확장자를 사용해주세요".into());
    }
    for target in [path.to_path_buf(), path.with_extension("png")] {
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", target.display())),
            Ok(_) => {
                return Err(format!(
                    "이미 존재하는 파일은 덮어쓰지 않습니다: {}",
                    target.display()
                ));
            }
        }
    }
    Ok(())
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        // Only these three paths, exclusively created by this operation, are owned.
        let _ = fs::remove_file(self.0.join("copy.ntdr"));
        let _ = fs::remove_file(self.0.join("copy.png"));
        let _ = fs::remove_dir(&self.0);
    }
}

/// Caller must first drain and destroy the source's writer and export worker.
/// An OS read handle then excludes new writers during the byte-for-byte copy.
/// Complete immutable history branches are retained, not just the visible head.
pub(crate) fn copy_closed_project(source: &Path, target: &Path) -> Result<(), String> {
    validate_target(target)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.share_mode(1); // FILE_SHARE_READ: excludes writers and deletion.
    }
    let mut source_file = options
        .open(source)
        .map_err(|error| format!("원본 잠금: {error}"))?;
    let sequence = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let staging = target.with_file_name(format!(
        ".nyatidraw-save-as-{}-{sequence}",
        std::process::id()
    ));
    fs::create_dir(&staging).map_err(|error| format!("임시 저장 위치: {error}"))?;
    let staging = Staging(staging);
    let project = staging.0.join("copy.ntdr");
    let png = staging.0.join("copy.png");
    let mut copied = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&project)
        .map_err(|error| error.to_string())?;
    std::io::copy(&mut source_file, &mut copied)
        .map_err(|error| format!("프로젝트 복사: {error}"))?;
    copied.sync_all().map_err(|error| error.to_string())?;
    drop(copied);
    let database = ProjectDb::open(&project).map_err(|error| format!("복사본 검증: {error}"))?;
    let reopened = database
        .load_reopened()
        .map_err(|error| error.to_string())?;
    let tiles = reopened
        .as_ref()
        .map(|value| value.current_tiles().clone())
        .unwrap_or_default();
    let canvas = database
        .load_canvas_spec()
        .map_err(|error| error.to_string())?;
    let tree = database
        .load_layer_tree()
        .map_err(|error| error.to_string())?
        .unwrap_or_else(|| {
            if reopened.is_some() {
                crate::native_canvas::legacy_layer_tree()
            } else {
                crate::native_canvas::default_layer_tree()
            }
        });
    let surface = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&tiles, &tree, canvas)
        .map_err(|error| format!("PNG 합성: {error:?}"))?;
    drop(database);
    nyatidraw_png_io::encode_png(&png, &surface).map_err(|error| format!("PNG 생성: {error}"))?;
    OpenOptions::new()
        .write(true)
        .open(&png)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    // No replacement operation: hard_link atomically refuses all collisions.
    // Publish PNG first: an exposed project always has its rendered sibling.
    // A crash/late collision may leave a complete PNG; never delete a final
    // destination, which another process could already have adopted.
    fs::hard_link(&png, target.with_extension("png"))
        .map_err(|error| format!("PNG 게시 실패 (원본 유지): {error}"))?;
    fs::hard_link(&project, target)
        .map_err(|error| format!("프로젝트 게시 실패 (완료된 PNG는 유지): {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_as_never_overwrites_collisions_and_accepts_untouched_documents() {
        // Product risk: even empty/corrupt files and sibling images are user
        // data; rejecting one destination must preserve both and the source.
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-save-as-collision-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("source.ntdr");
        let database = ProjectDb::open(&source).unwrap();
        database
            .persist_canvas_spec(nyatidraw_api::CanvasSpec {
                width_px: 16,
                height_px: 16,
                pixels_per_inch: 96,
            })
            .unwrap();
        drop(database); // Untouched project deliberately has no layer metadata.
        let original = fs::read(&source).unwrap();
        for (index, extension, bytes) in [
            (0, "ntdr", b"invalid".as_slice()),
            (1, "ntdr", b"".as_slice()),
            (2, "png", b"existing image".as_slice()),
        ] {
            let target = directory.join(format!("case-{index}.ntdr"));
            let collision = target.with_extension(extension);
            fs::write(&collision, bytes).unwrap();
            assert!(copy_closed_project(&source, &target).is_err());
            assert_eq!(fs::read(&collision).unwrap(), bytes);
            assert_eq!(fs::read(&source).unwrap(), original);
            fs::remove_file(collision).unwrap();
        }
        let target = directory.join("untouched.ntdr");
        copy_closed_project(&source, &target).unwrap();
        let reopened = ProjectDb::open(&target).unwrap();
        assert!(reopened.load_reopened().unwrap().is_none());
        assert!(target.with_extension("png").is_file());
        drop(reopened);
        for path in [&source, &target, &target.with_extension("png")] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
}

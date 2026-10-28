use nyatidraw_tiles::FlattenedRgba8;
use std::{fs::OpenOptions, io::Write, path::Path};

/// Publishes a new PNG and manifest bundle. The final manifest is the completion marker.
/// # Errors
/// Refuses existing directories and preserves all preexisting output on failure.
pub fn publish_sprite_sheet_new(
    directory: &Path,
    surface: &FlattenedRgba8,
    manifest_bytes: &[u8],
) -> Result<(), String> {
    std::fs::create_dir(directory)
        .map_err(|error| format!("create new output directory: {error}"))?;
    let temporary_png = directory.join(".sheet.png.tmp");
    let png = directory.join("sheet.png");
    let temporary_manifest = directory.join(".sheet.json.tmp");
    let manifest = directory.join("sheet.json");
    let mut temporary_png_created = false;
    let mut png_created = false;
    let mut temporary_manifest_created = false;
    let result = (|| {
        crate::encode_png_reduced(&temporary_png, surface, [surface.width, surface.height])
            .map_err(|error| error.to_string())?;
        temporary_png_created = true;
        OpenOptions::new()
            .write(true)
            .open(&temporary_png)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        std::fs::hard_link(&temporary_png, &png)
            .map_err(|error| format!("publish sheet PNG: {error}"))?;
        png_created = true;
        std::fs::remove_file(&temporary_png).map_err(|error| error.to_string())?;
        temporary_png_created = false;
        {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_manifest)
                .map_err(|error| error.to_string())?;
            temporary_manifest_created = true;
            output
                .write_all(manifest_bytes)
                .and_then(|()| output.sync_all())
                .map_err(|error| error.to_string())?;
        }
        std::fs::hard_link(&temporary_manifest, &manifest)
            .map_err(|error| format!("publish completed sheet manifest: {error}"))?;
        let _ = std::fs::remove_file(&temporary_manifest);
        Ok(())
    })();
    if result.is_err() {
        if temporary_png_created {
            let _ = std::fs::remove_file(&temporary_png);
        }
        if temporary_manifest_created {
            let _ = std::fs::remove_file(&temporary_manifest);
        }
        if png_created {
            let _ = std::fs::remove_file(&png);
        }
        let _ = std::fs::remove_dir(directory);
    }
    result
}

use std::{ffi::OsString, path::Path};

pub(super) fn run(args: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let project = super::required_path(args, "project path")?;
    let destination = super::required_path(args, "new PNG destination")?;
    let width = dimension(args)?;
    let height = dimension(args)?;
    super::no_extra_args(args)?;
    export(&project, &destination, [width, height])?;
    println!(
        "{{\"event\":\"export-png\",\"width\":{width},\"height\":{height},\"source_unchanged\":true}}"
    );
    Ok(())
}

fn dimension(args: &mut impl Iterator<Item = OsString>) -> Result<u32, String> {
    args.next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(|arg| arg.parse().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| "expected a positive output pixel dimension".into())
}

fn export(project: &Path, destination: &Path, size: [u32; 2]) -> Result<(), String> {
    if !project.is_file() {
        return Err("source project does not exist".into());
    }
    if !destination
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
        || destination.exists()
        || destination.with_extension("ntdr").exists()
    {
        return Err("choose a new .png path without a matching .ntdr project".into());
    }
    let source = super::animation_export::load_source(project)?;
    let tree = source.still_tree()?;
    let canvas = source.canvas;
    if size[0] > canvas.width_px || size[1] > canvas.height_px {
        return Err("output dimensions must not exceed the source canvas".into());
    }
    let surface = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(&source.tiles, &tree, canvas)
        .map_err(|error| format!("composite: {error:?}"))?;
    let temporary = super::sibling_temporary_path(destination)?;
    if temporary.exists() {
        return Err("temporary export already exists; choose another destination".into());
    }
    nyatidraw_png_io::encode_png_reduced(&temporary, &surface, size)
        .map_err(|error| error.to_string())?;
    let result = (|| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        std::fs::hard_link(&temporary, destination)
            .map_err(|error| format!("publish new PNG: {error}"))
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#![forbid(unsafe_code)]

use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use nyatidraw_api::{HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_brush::{
    BrushEvaluator, BrushPreset, BrushPresetId, BrushSnapshot, ROUND_BRUSH_ENGINE_VERSION,
    RoundBrushEvaluator, begin_round_stroke,
};
use nyatidraw_editor::HeadlessStrokeSession;
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_project::ProjectCommitBatch;
use nyatidraw_project_redb::ProjectDb;
use nyatidraw_stroke::StrokeColor;
use nyatidraw_tiles::{FlattenedRgba8, ObjectHash, TileSnapshot};

fn main() {
    if let Err(error) = run() {
        eprintln!("nyatidraw-cli: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os();
    let _program = args.next();
    let command = args
        .next()
        .and_then(|command| command.into_string().ok())
        .ok_or_else(usage)?;
    match command.as_str() {
        "migrate-copy" => {
            let source = required_path(&mut args, "legacy project path")?;
            let target = required_path(&mut args, "new .ntdr path")?;
            no_extra_args(&mut args)?;
            nyatidraw_project_redb::migrate_legacy_copy(&source, &target)
                .map_err(|error| error.to_string())?;
            println!("migration complete; source preserved; PNG export not performed");
            Ok(())
        }
        "validate" => {
            let project = required_path(&mut args, "project path")?;
            no_extra_args(&mut args)?;
            let current = validate_project(&project)?;
            match current {
                Some(batch) => println!(
                    "{{\"event\":\"validate\",\"status\":\"ok\",\"root\":\"{}\"}}",
                    hash_hex(batch.materialized.after.root().hash)
                ),
                None => println!("{{\"event\":\"validate\",\"status\":\"empty\"}}"),
            }
            Ok(())
        }
        "export" => {
            let project = required_path(&mut args, "project path")?;
            let destination = required_path(&mut args, "PPM destination")?;
            let layer = required_layer(&mut args)?;
            no_extra_args(&mut args)?;
            let flattened = export_project(&project, &destination, layer)?;
            println!(
                concat!(
                    "{{\"event\":\"export\",\"format\":\"ppm-p6\",",
                    "\"origin\":{{\"x\":{},\"y\":{}}},",
                    "\"dimensions\":{{\"width\":{},\"height\":{}}},",
                    "\"pixel_hash\":\"{}\"}}"
                ),
                flattened.origin_x,
                flattened.origin_y,
                flattened.width,
                flattened.height,
                hash_hex(flattened.hash()),
            );
            Ok(())
        }
        "diagnostic-smoke" => {
            no_extra_args(&mut args)?;
            run_nonempty_project_smoke()
        }
        _ => Err(usage()),
    }
}

fn validate_project(path: &Path) -> Result<Option<ProjectCommitBatch>, String> {
    let database = ProjectDb::open(path).map_err(|error| format!("open: {error:?}"))?;
    database
        .load_current()
        .map_err(|error| format!("validate: {error:?}"))
}

fn export_project(
    project: &Path,
    destination: &Path,
    layer: LayerId,
) -> Result<FlattenedRgba8, String> {
    let database = ProjectDb::open(project).map_err(|error| format!("open: {error:?}"))?;
    let snapshot = database
        .load_current()
        .map_err(|error| format!("load current snapshot: {error:?}"))?
        .map_or_else(TileSnapshot::empty, |batch| batch.materialized.after);
    let canvas = database
        .load_canvas_spec()
        .map_err(|error| format!("load canvas metadata: {error:?}"))?;
    let flattened = snapshot
        .crop_base_layer_rgba8_to_canvas(layer, canvas)
        .map_err(|error| format!("flatten layer {}: {error:?}", layer.0))?;
    write_ppm_new_file(destination, &flattened)?;
    Ok(flattened)
}

/// Creates a private scratch project with one real closed stroke, then runs the
/// public validate/export commands in fresh processes and compares the PPM
/// bytes with the pre-save flattened snapshot contract.
fn run_nonempty_project_smoke() -> Result<(), String> {
    let scratch = scratch_directory("nonempty-cli-smoke")?;
    let result = (|| {
        let project = scratch.join("fixture.redb");
        let output = scratch.join("fixture.ppm");
        let layer = LayerId(4);
        let batch = prepared_batch(
            SnapshotId(1),
            TileSnapshot::empty(),
            SnapshotId(2),
            HistoryNodeId(3),
            42,
            10,
        )?;
        let before_save = batch
            .materialized
            .after
            .crop_base_layer_rgba8_to_canvas(layer, nyatidraw_api::CanvasSpec::DEFAULT)
            .map_err(|error| format!("flatten pre-save fixture: {error:?}"))?;
        let expected_ppm = ppm_bytes(&before_save)?;
        let database =
            ProjectDb::open(&project).map_err(|error| format!("create fixture: {error:?}"))?;
        database
            .commit(&batch)
            .map_err(|error| format!("commit fixture: {error:?}"))?;
        drop(database);

        // A child process cannot reuse this process's in-memory snapshot/cache.
        // Both paths are newly created scratch artifacts, never user artwork.
        let executable = env::current_exe().map_err(|error| format!("locate CLI: {error}"))?;
        let validation = Command::new(&executable)
            .arg("validate")
            .arg(&project)
            .output()
            .map_err(|error| format!("start fresh validator: {error}"))?;
        if !validation.status.success() {
            return Err(format!(
                "fresh validator failed: {}",
                String::from_utf8_lossy(&validation.stderr)
            ));
        }
        let export = Command::new(&executable)
            .arg("export")
            .arg(&project)
            .arg(&output)
            .arg(layer.0.to_string())
            .output()
            .map_err(|error| format!("start fresh exporter: {error}"))?;
        if !export.status.success() {
            return Err(format!(
                "fresh exporter failed: {}",
                String::from_utf8_lossy(&export.stderr)
            ));
        }
        let loaded = validate_project(&project)?
            .ok_or_else(|| "validate unexpectedly reported an empty fixture".to_owned())?;
        if loaded.snapshot_id != batch.snapshot_id
            || loaded.materialized.after.root() != batch.materialized.after.root()
        {
            return Err("validate reopened a different durable snapshot/root".to_owned());
        }
        let actual_ppm = fs::read(&output)
            .map_err(|error| format!("read exported PPM {}: {error}", output.display()))?;
        if actual_ppm != expected_ppm {
            return Err("exported PPM bytes differ from pre-save flatten semantics".to_owned());
        }
        println!(
            concat!(
                "{{\"event\":\"diagnostic-smoke\",\"status\":\"ok\",\"fresh_process_reopen\":true,",
                "\"snapshot_id\":{},\"root\":\"{}\",",
                "\"flattened_pixel_hash\":\"{}\",\"ppm_bytes_hash\":\"{}\"}}"
            ),
            batch.snapshot_id.0,
            hash_hex(batch.materialized.after.root().hash),
            hash_hex(before_save.hash()),
            hash_hex(ObjectHash::digest_tagged(
                b"nyatidraw-ppm-p6-v1",
                &actual_ppm
            )),
        );
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(&scratch).map_err(|error| {
        format!(
            "remove owned scratch directory {}: {error}",
            scratch.display()
        )
    });
    result?;
    cleanup
}

fn scratch_directory(label: &str) -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("read system time: {error}"))?
        .as_nanos();
    let path = env::temp_dir().join(format!("nyatidraw-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir(&path)
        .map_err(|error| format!("create scratch directory {}: {error}", path.display()))?;
    Ok(path)
}

fn prepared_batch(
    parent_snapshot: SnapshotId,
    before: TileSnapshot,
    snapshot_id: SnapshotId,
    history_id: HistoryNodeId,
    seed: u64,
    sequence: u64,
) -> Result<ProjectCommitBatch, String> {
    let preset = BrushPreset {
        id: BrushPresetId(17),
        schema_version: nyatidraw_brush::ROUND_BRUSH_PRESET_SCHEMA_VERSION,
        engine_version: ROUND_BRUSH_ENGINE_VERSION,
        size_px: 18.0,
        opacity: 0.9,
        flow: 0.6,
        spacing_ratio: 0.2,
        size_pressure: true,
        opacity_pressure: false,
        size_min_ratio: 0.2,
        opacity_min_ratio: 0.1,
        hardness: 0.3,
    };
    let samples = vec![
        sample(sequence, PointerPhase::Begin, -8.0, 5.0),
        sample(sequence + 1, PointerPhase::Move, 32.0, 18.0),
        sample(sequence + 2, PointerPhase::End, 150.0, 31.0),
    ];
    let mut evaluator = RoundBrushEvaluator::new(seed);
    let mut dabs = Vec::new();
    let mut token = begin_round_stroke(&mut evaluator, &preset, samples[0], &mut dabs);
    evaluator.push(&mut token, &samples[1..], &mut dabs);
    let recorded = evaluator.end(token, &mut dabs);
    HeadlessStrokeSession::new(parent_snapshot, before)
        .prepare_round_stroke(
            snapshot_id,
            history_id,
            12_000,
            LayerId(4),
            BrushSnapshot { preset },
            recorded,
            StrokeColor::new([18, 9, 3, 24])
                .map_err(|error| format!("fixture colour: {error:?}"))?,
            samples,
        )
        .map_err(|error| format!("prepare closed stroke fixture: {error:?}"))
}

fn sample(sequence: u64, phase: PointerPhase, x: f64, y: f64) -> StylusSample {
    StylusSample {
        sequence,
        timestamp_ns: sequence * 1_000,
        device_id: 9,
        phase,
        position_document: Point { x, y },
        pressure: 0.75,
        tilt: None,
        twist_radians: None,
        tangential_pressure: None,
        buttons: PenButtons::default(),
        eraser: false,
        viewport_revision: 4,
    }
}

fn required_path(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {name}; {}", usage()))
}

fn required_layer(args: &mut impl Iterator<Item = OsString>) -> Result<LayerId, String> {
    let value = args
        .next()
        .ok_or_else(|| format!("missing layer id; {}", usage()))?
        .into_string()
        .map_err(|_| "layer id must be Unicode decimal".to_owned())?;
    value
        .parse::<u128>()
        .map(LayerId)
        .map_err(|_| format!("invalid layer id {value:?}; expected unsigned decimal"))
}

fn no_extra_args(args: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    if args.next().is_some() {
        return Err(usage());
    }
    Ok(())
}

fn usage() -> String {
    concat!(
        "usage: nyatidraw-cli validate <project.redb> | ",
        "export <project.redb> <output.ppm> <layer-id> | ",
        "migrate-copy <source.ntdr> <new.ntdr> | diagnostic-smoke"
    )
    .to_owned()
}

fn write_ppm_new_file(destination: &Path, flattened: &FlattenedRgba8) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "refusing to replace existing export {}; choose a new path",
            destination.display()
        ));
    }
    let temporary = sibling_temporary_path(destination)?;
    let result = write_ppm(&temporary, flattened);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if destination.exists() {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "export destination appeared during generation {}; refusing replacement",
            destination.display()
        ));
    }
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "publish generated export {}: {error}",
            destination.display()
        )
    })
}

fn sibling_temporary_path(destination: &Path) -> Result<PathBuf, String> {
    let name = destination.file_name().ok_or_else(|| {
        format!(
            "export destination has no file name: {}",
            destination.display()
        )
    })?;
    let mut temporary_name = name.to_os_string();
    temporary_name.push(format!(".nyatidraw-tmp-{}", std::process::id()));
    Ok(destination.with_file_name(temporary_name))
}

fn write_ppm(path: &Path, flattened: &FlattenedRgba8) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("create temporary export {}: {error}", path.display()))?;
    file.write_all(&ppm_bytes(flattened)?)
        .map_err(|error| format!("write PPM: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync temporary export: {error}"))
}

fn ppm_bytes(flattened: &FlattenedRgba8) -> Result<Vec<u8>, String> {
    let pixel_bytes = usize::try_from(u64::from(flattened.width) * u64::from(flattened.height))
        .map_err(|_| "PPM dimensions exceed platform address space".to_owned())?
        .checked_mul(3)
        .ok_or_else(|| "PPM byte count overflow".to_owned())?;
    let mut bytes = Vec::with_capacity(pixel_bytes.saturating_add(32));
    write!(bytes, "P6\n{} {}\n255\n", flattened.width, flattened.height)
        .map_err(|error| format!("format PPM header: {error}"))?;
    for rgba in flattened.pixels.chunks_exact(4) {
        bytes.extend_from_slice(&rgba[..3]);
    }
    Ok(bytes)
}

fn hash_hex(hash: ObjectHash) -> String {
    let mut output = String::with_capacity(64);
    for byte in hash.0 {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

//! Temporary alpha container upgrade and lossless storage rebuild.
//! Never open the original source for DB writes.
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use redb::ReadableDatabase;

use super::{COMPRESSED_TILE_SCHEMA_FLAG, Envelope, ProjectDb, ProjectOpenError, RecordKind};

const STORAGE_REVISION: &str = "tile_storage_revision";

pub(super) fn needs_repack(db: &ProjectDb) -> Result<bool, ProjectOpenError> {
    let tx = db.db.begin_read().map_err(|e| db.io(e))?;
    let meta = tx.open_table(super::META).map_err(|e| db.io(e))?;
    match meta
        .get(STORAGE_REVISION)
        .map_err(|e| db.io(e))?
        .map(|v| v.value())
    {
        None => Ok(true),
        Some(1) => Ok(false),
        Some(_) => Err(db.corrupt("unsupported tile storage revision")),
    }
}

struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("copy.ntdr"));
        let _ = fs::remove_file(self.0.join("rebuilt.ntdr"));
        let _ = fs::remove_dir(&self.0);
    }
}

/// Rebuilds a closed alpha project into a new sibling `.ntdr`, preserving the
/// source and refusing every destination collision. Does not create or replace PNG.
///
/// # Errors
/// Fails for locked/invalid sources, existing destinations, unsupported tables,
/// changed records, invalid project graphs or unavailable no-clobber publication.
pub fn migrate_legacy_copy(source: &Path, target: &Path) -> Result<(), ProjectOpenError> {
    migrate_inner(source, target, false, || Ok(()))
}

// Temporary alpha compatibility. Remove this function/module and the legacy
// dependency together once pre-redb4 alpha files are no longer supported.
pub(super) fn upgrade_alpha_in_place(path: &Path) -> Result<(), ProjectOpenError> {
    migrate_inner(path, path, true, || Ok(()))
}

// Keep the ordered conversion/publication sequence together for auditing.
#[allow(clippy::too_many_lines)]
fn migrate_inner(
    source: &Path,
    target: &Path,
    replace_alpha: bool,
    before_publish: impl FnOnce() -> Result<(), ProjectOpenError>,
) -> Result<(), ProjectOpenError> {
    let io = |error: String| ProjectOpenError::Io {
        path: target.to_owned(),
        message: error,
    };
    if !replace_alpha
        && !target
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ntdr"))
    {
        return Err(io("destination must have the .ntdr extension".into()));
    }
    if !replace_alpha {
        match fs::symlink_metadata(target) {
            Ok(_) => return Err(io("destination exists; no file was replaced".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io(error.to_string())),
        }
    }
    let mut options = OpenOptions::new();
    if fs::symlink_metadata(source)
        .map_err(|e| io(e.to_string()))?
        .file_type()
        .is_symlink()
    {
        return Err(io(
            "migrate the original regular file, not a symbolic link".into()
        ));
    }
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Allow our atomic replacement while denying all writers. The exclusive
        // byte-range lock below serializes cooperating DB openers/converters.
        options.share_mode(if replace_alpha { 5 } else { 1 });
    }
    let mut input = options.open(source).map_err(|error| ProjectOpenError::Io {
        path: source.to_owned(),
        message: error.to_string(),
    })?;
    if replace_alpha {
        input.try_lock()
    } else {
        input.try_lock_shared()
    }
    .map_err(|_| ProjectOpenError::Locked {
        path: source.to_owned(),
    })?;
    if !input.metadata().map_err(|e| io(e.to_string()))?.is_file() {
        return Err(io("source must be a regular file".into()));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| io(e.to_string()))?
        .as_nanos();
    let directory = target.with_file_name(format!(
        ".nyatidraw-migration-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&directory).map_err(|e| io(e.to_string()))?;
    let staging = Staging(directory);
    let copy = staging.0.join("copy.ntdr");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy)
        .map_err(|e| io(e.to_string()))?;
    std::io::copy(&mut input, &mut output).map_err(|e| io(e.to_string()))?;
    output.sync_all().map_err(|e| io(e.to_string()))?;
    drop(output);
    // Both old v2 containers and already-upgraded v3 files need the tile rewrite.
    // Legacy repair/upgrade writes operate only on our private copy.
    let before = match redb::Database::open(&copy) {
        Ok(db) => current_digest(db.begin_read().map_err(|e| io(e.to_string()))?).map_err(&io)?,
        Err(redb::DatabaseError::UpgradeRequired(_)) => {
            let mut legacy = redb_legacy::Database::open(&copy).map_err(|e| io(e.to_string()))?;
            let before = legacy_digest(&legacy).map_err(&io)?;
            legacy.upgrade().map_err(|e| io(e.to_string()))?;
            before
        }
        Err(error) => return Err(io(error.to_string())),
    };
    let upgraded = redb::Database::open(&copy).map_err(|e| io(e.to_string()))?;
    if current_digest(upgraded.begin_read().map_err(|e| io(e.to_string()))?).map_err(&io)? != before
    {
        return Err(io("container upgrade changed project records".into()));
    }
    let rebuilt = staging.0.join("rebuilt.ntdr");
    rebuild_storage(&upgraded, &rebuilt).map_err(&io)?;
    drop(upgraded);
    fs::rename(&rebuilt, &copy).map_err(|e| io(e.to_string()))?;
    // Validate all persisted artwork/metadata without trimming >128 histories
    // during conversion. The normal editor's retention policy is separate.
    let validated = ProjectDb::open_inner(&copy, false)?;
    if current_digest(validated.db.begin_read().map_err(|e| io(e.to_string()))?).map_err(&io)?
        != before
    {
        return Err(io(
            "storage rebuild or validation changed project content".into()
        ));
    }
    drop(validated);
    OpenOptions::new()
        .write(true)
        .open(&copy)
        .and_then(|file| file.sync_all())
        .map_err(|e| io(e.to_string()))?;
    before_publish()?;
    // Atomic create-if-absent on the same filesystem. On unsupported filesystems
    // fail safely instead of falling back to overwrite or partial destination IO.
    if replace_alpha {
        let mut backup = source.as_os_str().to_os_string();
        backup.push(format!(".pre-redb4-{nonce}.bak"));
        // Backup is the exact old file, not a reserialized approximation. It is
        // retained even if replacement fails or this process dies afterwards.
        fs::hard_link(source, &backup).map_err(|e| io(e.to_string()))?;
        fs::rename(&copy, source).map_err(|e| io(e.to_string()))?;
    } else {
        fs::hard_link(&copy, target).map_err(|e| io(e.to_string()))?;
    }
    Ok(())
}

// Reinsert into a fresh DB: compacting the old allocation alone cannot eliminate
// oversized raw-value pages. All non-tile records are copied byte-for-byte.
fn rebuild_storage(source: &redb::Database, target: &Path) -> Result<(), String> {
    use redb::{ReadableTable, TableDefinition, TableHandle};
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let read = source.begin_read()?;
        if read
            .open_table(super::META)?
            .get(STORAGE_REVISION)?
            .is_some_and(|value| value.value() != 1)
        {
            return Err("unsupported tile storage revision".into());
        }
        let mut output = redb::Database::create(target)?;
        let mut write = output.begin_write()?;
        write.set_durability(redb::Durability::Immediate)?;
        for handle in read.list_tables()? {
            let name = handle.name();
            macro_rules! copy_table {
                ($key:ty, $value:ty) => {{
                    let table = read.open_table(TableDefinition::<$key, $value>::new(name))?;
                    let mut dest = write.open_table(TableDefinition::<$key, $value>::new(name))?;
                    for entry in table.iter()? {
                        let (key, value) = entry?;
                        dest.insert(key.value(), value.value())?;
                    }
                }};
            }
            match name {
                "meta" => copy_table!(&str, u64),
                "state" => copy_table!(&str, &[u8]),
                "tile_root_refs" => copy_table!(&[u8], u64),
                "objects" => {
                    let table = read.open_table(super::OBJECTS)?;
                    let mut dest = write.open_table(super::OBJECTS)?;
                    for entry in table.iter()? {
                        let (key, value) = entry?;
                        let tile = Envelope::decode(value.value(), RecordKind::Tile)?;
                        let encoded = tile.encode_for_storage();
                        dest.insert(key.value(), encoded.as_slice())?;
                    }
                }
                "roots" | "history" | "snapshots" | "strokes" | "snapshot_layers"
                | "snapshot_canvas" => copy_table!(&[u8], &[u8]),
                _ => return Err(format!("unknown project table {name}").into()),
            }
        }
        {
            let mut meta = write.open_table(super::META)?;
            let version = meta
                .get("schema_version")?
                .ok_or("missing schema marker")?
                .value();
            meta.insert("schema_version", version | COMPRESSED_TILE_SCHEMA_FLAG)?;
            meta.insert(STORAGE_REVISION, 1)?;
        }
        write.commit()?;
        output.compact()?;
        Ok(())
    })();
    result.map_err(|e| e.to_string())
}

// Hash every typed table and row in deterministic order. Do not load all tile
// blobs into an extra migration-sized Vec or silently skip future table types.
macro_rules! record_digest {
    ($read:expr, $engine:ident) => {{
        use $engine::{ReadableTable, TableDefinition, TableHandle};
        let tx = $read;
        let mut names: Vec<_> = tx
            .list_tables()
            .map_err(|e| e.to_string())?
            .map(|handle| handle.name().to_owned())
            .collect();
        names.sort();
        if tx
            .list_multimap_tables()
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err("unexpected multimap table in project".into());
        }
        let mut hash = blake3::Hasher::new();
        for name in names {
            hash.update(b"table");
            hash.update(&(name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            let mut row = |key: &[u8], value: &[u8]| {
                hash.update(b"row");
                hash.update(&(key.len() as u64).to_le_bytes());
                hash.update(key);
                hash.update(&(value.len() as u64).to_le_bytes());
                hash.update(value);
            };
            match name.as_str() {
                "meta" => {
                    let table = tx
                        .open_table(TableDefinition::<&str, u64>::new(&name))
                        .map_err(|e| e.to_string())?;
                    for entry in table.iter().map_err(|e| e.to_string())? {
                        let (k, v) = entry.map_err(|e| e.to_string())?;
                        if k.value() != STORAGE_REVISION {
                            let value = if k.value() == "schema_version" {
                                v.value() & !COMPRESSED_TILE_SCHEMA_FLAG
                            } else {
                                v.value()
                            };
                            row(k.value().as_bytes(), &value.to_le_bytes());
                        }
                    }
                }
                "state" => {
                    let table = tx
                        .open_table(TableDefinition::<&str, &[u8]>::new(&name))
                        .map_err(|e| e.to_string())?;
                    for entry in table.iter().map_err(|e| e.to_string())? {
                        let (k, v) = entry.map_err(|e| e.to_string())?;
                        row(k.value().as_bytes(), v.value());
                    }
                }
                "tile_root_refs" => {
                    let table = tx
                        .open_table(TableDefinition::<&[u8], u64>::new(&name))
                        .map_err(|e| e.to_string())?;
                    for entry in table.iter().map_err(|e| e.to_string())? {
                        let (k, v) = entry.map_err(|e| e.to_string())?;
                        row(k.value(), &v.value().to_le_bytes());
                    }
                }
                "objects" | "roots" | "history" | "snapshots" | "strokes" | "snapshot_layers"
                | "snapshot_canvas" => {
                    let table = tx
                        .open_table(TableDefinition::<&[u8], &[u8]>::new(&name))
                        .map_err(|e| e.to_string())?;
                    for entry in table.iter().map_err(|e| e.to_string())? {
                        let (k, v) = entry.map_err(|e| e.to_string())?;
                        if name == "objects" {
                            // Canonical decoded pixels/checksum, independent of codec.
                            let tile = Envelope::decode(v.value(), RecordKind::Tile)
                                .map_err(|e| e.to_string())?;
                            row(k.value(), &tile.encode());
                        } else {
                            row(k.value(), v.value());
                        }
                    }
                }
                _ => return Err(format!("unknown project table {name}")),
            }
            hash.update(b"end-table");
        }
        Ok(*hash.finalize().as_bytes())
    }};
}

fn legacy_digest(db: &redb_legacy::Database) -> Result<[u8; 32], String> {
    record_digest!(db.begin_read().map_err(|e| e.to_string())?, redb_legacy)
}
fn current_digest(read: redb::ReadTransaction) -> Result<[u8; 32], String> {
    record_digest!(read, redb)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use redb::{ReadableTable, TableHandle};

    // Recreate current application wire records in a genuine redb 2.6 v2
    // container. Only called with fresh, test-owned paths.
    pub(crate) fn write_v2_copy(source: &Path, target: &Path) {
        let source = redb::ReadOnlyDatabase::open(source).unwrap();
        let read = source.begin_read().unwrap();
        let legacy = redb_legacy::Database::create(target).unwrap();
        let write = legacy.begin_write().unwrap();
        for handle in read.list_tables().unwrap() {
            let name = handle.name();
            macro_rules! copy_table {
                ($key:ty, $value:ty) => {{
                    let table = read
                        .open_table(redb::TableDefinition::<$key, $value>::new(name))
                        .unwrap();
                    let mut output = write
                        .open_table(redb_legacy::TableDefinition::<$key, $value>::new(name))
                        .unwrap();
                    for entry in table.iter().unwrap() {
                        let (key, value) = entry.unwrap();
                        output.insert(key.value(), value.value()).unwrap();
                    }
                }};
            }
            match name {
                "meta" => {
                    copy_table!(&str, u64);
                    write
                        .open_table(redb_legacy::TableDefinition::<&str, u64>::new(name))
                        .unwrap()
                        .remove(STORAGE_REVISION)
                        .unwrap();
                }
                "state" => copy_table!(&str, &[u8]),
                "tile_root_refs" => copy_table!(&[u8], u64),
                "objects" => {
                    let table = read.open_table(super::super::OBJECTS).unwrap();
                    let mut dest = write
                        .open_table(redb_legacy::TableDefinition::<&[u8], &[u8]>::new(name))
                        .unwrap();
                    for entry in table.iter().unwrap() {
                        let (key, value) = entry.unwrap();
                        let raw = Envelope::decode(value.value(), RecordKind::Tile)
                            .unwrap()
                            .encode();
                        dest.insert(key.value(), raw.as_slice()).unwrap();
                    }
                }
                _ => copy_table!(&[u8], &[u8]),
            }
        }
        write.commit().unwrap();
        drop(legacy);
        assert!(matches!(
            redb::Database::open(target),
            Err(redb::DatabaseError::UpgradeRequired(2))
        ));
    }

    pub(crate) fn remove_verified_backup(source: &Path, expected: &[u8]) {
        let prefix = format!(
            "{}.pre-redb4-",
            source.file_name().unwrap().to_str().unwrap()
        );
        let backups: Vec<_> = fs::read_dir(source.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .collect();
        assert_eq!(backups.len(), 1, "exactly one recoverable alpha backup");
        assert!(
            fs::read(&backups[0]).unwrap() == expected,
            "backup must be original bytes"
        );
        fs::remove_file(&backups[0]).unwrap();
    }

    #[test]
    fn alpha_upgrade_process_kill_keeps_original_or_complete_replacement() {
        // Product risk: terminating the app during automatic alpha conversion
        // must never expose a partially upgraded project at its original path.
        const CHILD_ROOT: &str = "NYATIDRAW_MIGRATION_KILL_ROOT";
        const CHILD_STAGE: &str = "NYATIDRAW_MIGRATION_KILL_STAGE";
        if let Some(directory) = std::env::var_os(CHILD_ROOT) {
            let directory = PathBuf::from(directory);
            let source = directory.join("legacy.ntdr");
            let pause = || -> Result<(), ProjectOpenError> {
                fs::write(directory.join("ready"), b"ready").unwrap();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            };
            if std::env::var(CHILD_STAGE).unwrap() == "before" {
                migrate_inner(&source, &source, true, pause).unwrap();
            } else {
                upgrade_alpha_in_place(&source).unwrap();
                pause().unwrap();
            }
            unreachable!();
        }
        for stage in ["before", "after"] {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "nyatidraw-migration-kill-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let native = directory.join("native.ntdr");
            drop(ProjectDb::open(&native).unwrap());
            let source = directory.join("legacy.ntdr");
            write_v2_copy(&native, &source);
            let original = fs::read(&source).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "migration::tests::alpha_upgrade_process_kill_keeps_original_or_complete_replacement", "--nocapture"])
                .env(CHILD_ROOT, &directory).env(CHILD_STAGE, stage)
                .spawn().unwrap();
            let start = std::time::Instant::now();
            while !directory.join("ready").exists() && start.elapsed().as_secs() < 10 {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let ready = directory.join("ready").exists();
            let killed = child.kill();
            let status = child.wait().unwrap();
            assert!(ready && killed.is_ok() && !status.success());
            if stage == "before" {
                assert!(fs::read(&source).unwrap() == original);
            }
            drop(ProjectDb::open(&source).unwrap());
            remove_verified_backup(&source, &original);
            // A killed conversion can leave its private staging directory.
            // Remove only our test directory's known scratch leaves, not artwork.
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    assert!(
                        path.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".nyatidraw-migration-")
                    );
                    fs::remove_file(path.join("copy.ntdr")).unwrap();
                    fs::remove_dir(path).unwrap();
                } else {
                    fs::remove_file(path).unwrap();
                }
            }
            fs::remove_dir(directory).unwrap();
        }
    }

    #[test]
    fn alpha_upgrade_preserves_source_on_failures_and_never_clobbers_copy_targets() {
        // Product risk: malformed/locked legacy files, interrupted conversion
        // and late publication collisions must not replace any existing artwork.
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "nyatidraw-alpha-upgrade-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let native = directory.join("native.ntdr");
        drop(ProjectDb::open(&native).unwrap());
        let source = directory.join("legacy.ntdr");
        write_v2_copy(&native, &source);
        let original = fs::read(&source).unwrap();
        let target = directory.join("converted.ntdr");
        let failure = || {
            Err(ProjectOpenError::Io {
                path: target.clone(),
                message: "injected before publication".into(),
            })
        };
        assert!(migrate_inner(&source, &target, false, failure).is_err());
        assert!(!target.exists());
        assert!(fs::read(&source).unwrap() == original);
        assert!(
            migrate_inner(&source, &target, false, || {
                fs::write(&target, b"other artwork").unwrap();
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read(&target).unwrap(), b"other artwork");
        assert!(fs::read(&source).unwrap() == original);
        fs::remove_file(&target).unwrap();
        let owner = redb_legacy::Database::open(&source).unwrap();
        assert!(migrate_legacy_copy(&source, &target).is_err());
        drop(owner);
        assert!(fs::read(&source).unwrap() == original);
        migrate_legacy_copy(&source, &target).unwrap();
        drop(ProjectDb::open(&target).unwrap());
        assert!(fs::read(&source).unwrap() == original);
        // Opening now performs the same validated upgrade automatically.
        drop(ProjectDb::open(&source).unwrap());
        remove_verified_backup(&source, &original);
        drop(ProjectDb::open(&source).unwrap()); // Must not create a second backup.
        for path in [&native, &source, &target] {
            fs::remove_file(path).unwrap();
        }
        // An actual v2 DB without the application marker is never published.
        drop(redb_legacy::Database::create(&source).unwrap());
        let invalid = fs::read(&source).unwrap();
        assert!(ProjectDb::open(&source).is_err());
        assert!(fs::read(&source).unwrap() == invalid);
        fs::remove_file(&source).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}

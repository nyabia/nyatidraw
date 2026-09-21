use std::{
    io,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use nyatidraw_project::{
    Envelope, ProjectOpenError, RecordKind, decode_root_manifest, preflight_stroke_decode,
};
use redb::{Database, DatabaseError, ReadableTable, ReadableTableMetadata, StorageBackend};

use crate::{ProjectDatabase, ProjectDb};

const MEMORY_PROJECT_NAME: &str = "memory.ntdr";
const MEMORY_CACHE_LIMIT: usize = 8 * 1024 * 1024;
const MAX_MEMORY_ROOT_TILES: usize = 1024;
const MAX_MEMORY_ROOT_BYTES: usize = 112 + MAX_MEMORY_ROOT_TILES * 57;
const MAX_MEMORY_ROOTS: usize = nyatidraw_history::HISTORY_LIMIT * 2 + 2;
const MAX_MEMORY_METADATA_BYTES: usize = 8 * 1024 * 1024;
const MAX_MEMORY_METADATA_RECORD_BYTES: usize = 128 * 1024;

/// The native project format in bounded memory, without filesystem access.
/// Storage commits are not durable until the host persists the returned bytes.
#[derive(Debug)]
pub struct MemoryProjectDb {
    project: ProjectDb,
    backend: MemoryBackend,
}

impl MemoryProjectDb {
    /// Copies and validates a native project, or initializes an empty input.
    /// Existing objects, history branches and metadata remain in the container.
    ///
    /// # Errors
    /// Rejects oversized, invalid, unsupported or corrupt input. No operation
    /// modifies the supplied bytes or attempts a legacy filesystem migration.
    /// Browser admission limits each root to 1024 tiles and history to 128 nodes.
    pub fn open(bytes: &[u8], limit: usize) -> Result<Self, ProjectOpenError> {
        let backend = MemoryBackend::new(bytes, limit).map_err(storage_error)?;
        let db = Database::builder()
            .set_cache_size(limit.min(MEMORY_CACHE_LIMIT))
            .create_with_backend(backend.clone())
            .map_err(|error| match error {
                DatabaseError::UpgradeRequired(_) => ProjectOpenError::LegacyContainer {
                    path: PathBuf::from(MEMORY_PROJECT_NAME),
                },
                error if bytes.is_empty() => storage_error(error),
                _ => ProjectOpenError::InvalidNonEmpty {
                    path: PathBuf::from(MEMORY_PROJECT_NAME),
                },
            })?;
        let mut project = ProjectDb {
            db: ProjectDatabase::Writable(db),
            path: PathBuf::from(MEMORY_PROJECT_NAME),
        };
        if bytes.is_empty() {
            project.write_marker()?;
            if let ProjectDatabase::Writable(db) = &mut project.db {
                db.compact().map_err(storage_error)?;
            }
        } else {
            preflight_decode_budget(&project)?;
            project.validate_existing()?;
        }
        Ok(Self { project, backend })
    }

    #[must_use]
    pub const fn db(&self) -> &ProjectDb {
        &self.project
    }

    /// Closes redb before returning its native file bytes with a clean header.
    ///
    /// # Errors
    /// Rejects a failed storage operation rather than exporting uncertain data.
    pub fn into_bytes(self) -> Result<Vec<u8>, ProjectOpenError> {
        let Self { project, backend } = self;
        preflight_decode_budget(&project)?;
        drop(project);
        let state = Arc::try_unwrap(backend.state)
            .map_err(|_| storage_error("memory project is still open"))?
            .into_inner()
            .map_err(|_| storage_error("memory project storage lock poisoned"))?;
        if let Some(error) = state.write_failure {
            return Err(storage_error(error));
        }
        Ok(state.bytes)
    }
}

fn preflight_decode_budget(project: &ProjectDb) -> Result<(), ProjectOpenError> {
    let transaction = project.db.begin_read().map_err(|error| project.io(error))?;
    let mut metadata_bytes = 0_usize;
    for (definition, limit) in [
        (crate::HISTORY, nyatidraw_history::HISTORY_LIMIT),
        (crate::SNAPSHOTS, nyatidraw_history::HISTORY_LIMIT + 1),
        (
            crate::layer_history::SNAPSHOT_LAYERS,
            nyatidraw_history::HISTORY_LIMIT + 1,
        ),
        (
            crate::canvas_history::SNAPSHOT_CANVAS,
            nyatidraw_history::HISTORY_LIMIT + 1,
        ),
    ] {
        let table = match transaction.open_table(definition) {
            Ok(table) => table,
            Err(redb::TableError::TableDoesNotExist(_)) => continue,
            Err(error) => return Err(project.corrupt(error)),
        };
        if table.len().map_err(|error| project.corrupt(error))? > limit as u64 {
            return Err(project.corrupt("project history exceeds browser decode budget"));
        }
        for entry in table.iter().map_err(|error| project.corrupt(error))? {
            let (_, value) = entry.map_err(|error| project.corrupt(error))?;
            check_metadata_budget(project, value.value(), &mut metadata_bytes)?;
        }
    }
    match transaction.open_table(crate::STATE) {
        Ok(table) => {
            if table.len().map_err(|error| project.corrupt(error))? > 64 {
                return Err(project.corrupt("project state exceeds browser decode budget"));
            }
            for entry in table.iter().map_err(|error| project.corrupt(error))? {
                let (_, value) = entry.map_err(|error| project.corrupt(error))?;
                check_metadata_budget(project, value.value(), &mut metadata_bytes)?;
            }
        }
        Err(redb::TableError::TableDoesNotExist(_)) => {}
        Err(error) => return Err(project.corrupt(error)),
    }
    preflight_strokes(project, &transaction, &mut metadata_bytes)?;
    preflight_objects(project, &transaction)?;
    let roots = match transaction.open_table(crate::ROOTS) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(project.corrupt(error)),
    };
    if roots.len().map_err(|error| project.corrupt(error))? > MAX_MEMORY_ROOTS as u64 {
        return Err(project.corrupt("project roots exceed browser decode budget"));
    }
    for entry in roots.iter().map_err(|error| project.corrupt(error))? {
        let (key, value) = entry.map_err(|error| project.corrupt(error))?;
        let envelope = crate::root_codec::decode_with_limit(value.value(), MAX_MEMORY_ROOT_BYTES)
            .map_err(|error| project.corrupt(error))?;
        let manifest =
            decode_root_manifest(&envelope.payload).map_err(|error| project.corrupt(error))?;
        if manifest.tiles.len() > MAX_MEMORY_ROOT_TILES
            || key.value() != manifest.root.hash.0.as_slice()
        {
            return Err(project.corrupt("project root exceeds browser tile budget or is corrupt"));
        }
    }
    Ok(())
}

fn preflight_objects(
    project: &ProjectDb,
    transaction: &redb::ReadTransaction,
) -> Result<(), ProjectOpenError> {
    let objects = match transaction.open_table(crate::OBJECTS) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(project.corrupt(error)),
    };
    let max_objects = MAX_MEMORY_ROOTS * MAX_MEMORY_ROOT_TILES;
    if objects.len().map_err(|error| project.corrupt(error))? > max_objects as u64 {
        return Err(project.corrupt("project objects exceed browser decode budget"));
    }
    for entry in objects.iter().map_err(|error| project.corrupt(error))? {
        let (key, value) = entry.map_err(|error| project.corrupt(error))?;
        if key.value().len() != 32 || value.value().len() > nyatidraw_tiles::TILE_BYTE_LEN + 52 {
            return Err(project.corrupt("tile record exceeds browser decode budget or is corrupt"));
        }
    }
    Ok(())
}

fn preflight_strokes(
    project: &ProjectDb,
    transaction: &redb::ReadTransaction,
    metadata_bytes: &mut usize,
) -> Result<(), ProjectOpenError> {
    let strokes = match transaction.open_table(crate::STROKES) {
        Ok(table) => table,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(()),
        Err(error) => return Err(project.corrupt(error)),
    };
    if strokes.len().map_err(|error| project.corrupt(error))?
        > nyatidraw_history::HISTORY_LIMIT as u64
    {
        return Err(project.corrupt("project strokes exceed browser decode budget"));
    }
    for entry in strokes.iter().map_err(|error| project.corrupt(error))? {
        let (_, value) = entry.map_err(|error| project.corrupt(error))?;
        check_metadata_budget(project, value.value(), metadata_bytes)?;
        let envelope = Envelope::decode(value.value(), RecordKind::StrokeCommit)
            .map_err(|error| project.corrupt(error))?;
        preflight_stroke_decode(&envelope.payload, MAX_MEMORY_ROOT_TILES)
            .map_err(|error| project.corrupt(error))?;
    }
    Ok(())
}

fn check_metadata_budget(
    project: &ProjectDb,
    bytes: &[u8],
    total: &mut usize,
) -> Result<(), ProjectOpenError> {
    *total = total
        .checked_add(bytes.len())
        .filter(|total| *total <= MAX_MEMORY_METADATA_BYTES)
        .filter(|_| bytes.len() <= MAX_MEMORY_METADATA_RECORD_BYTES)
        .ok_or_else(|| project.corrupt("project metadata exceeds browser decode budget"))?;
    Ok(())
}

#[derive(Debug)]
struct MemoryState {
    bytes: Vec<u8>,
    write_failure: Option<String>,
}

#[derive(Clone, Debug)]
struct MemoryBackend {
    state: Arc<Mutex<MemoryState>>,
    limit: usize,
}

impl MemoryBackend {
    fn new(bytes: &[u8], limit: usize) -> io::Result<Self> {
        if bytes.len() > limit || limit == 0 {
            return Err(io::Error::other("memory project exceeds its byte limit"));
        }
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(bytes.len())
            .map_err(io::Error::other)?;
        owned.extend_from_slice(bytes);
        Ok(Self {
            state: Arc::new(Mutex::new(MemoryState {
                bytes: owned,
                write_failure: None,
            })),
            limit,
        })
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, MemoryState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("memory project storage lock poisoned"))
    }

    fn range(offset: u64, len: usize, total: usize) -> io::Result<std::ops::Range<usize>> {
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::other("memory project offset exceeds address space"))?;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= total)
            .ok_or_else(|| io::Error::other("memory project range exceeds storage"))?;
        Ok(start..end)
    }
}

impl StorageBackend for MemoryBackend {
    fn len(&self) -> io::Result<u64> {
        Ok(self.lock()?.bytes.len() as u64)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        let state = self.lock()?;
        let range = Self::range(offset, out.len(), state.bytes.len())?;
        out.copy_from_slice(&state.bytes[range]);
        Ok(())
    }

    fn set_len(&self, len: u64) -> io::Result<()> {
        let mut state = self.lock()?;
        let grow = |state: &mut MemoryState| {
            let len = usize::try_from(len)
                .ok()
                .filter(|len| *len <= self.limit)
                .ok_or_else(|| io::Error::other("memory project exceeds its byte limit"))?;
            let additional = len.saturating_sub(state.bytes.len());
            state
                .bytes
                .try_reserve_exact(additional)
                .map_err(io::Error::other)?;
            state.bytes.resize(len, 0);
            Ok::<_, io::Error>(())
        };
        let result = grow(&mut state);
        if let Err(error) = &result {
            state.write_failure = Some(error.to_string());
        }
        result
    }

    fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut state = self.lock()?;
        match Self::range(offset, data.len(), state.bytes.len()) {
            Ok(range) => {
                state.bytes[range].copy_from_slice(data);
                Ok(())
            }
            Err(error) => {
                state.write_failure = Some(error.to_string());
                Err(error)
            }
        }
    }
}

fn storage_error(error: impl std::fmt::Display) -> ProjectOpenError {
    ProjectOpenError::Io {
        path: PathBuf::from(MEMORY_PROJECT_NAME),
        message: error.to_string(),
    }
}

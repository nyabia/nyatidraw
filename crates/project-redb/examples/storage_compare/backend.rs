use std::{collections::BTreeMap, path::Path};

use redb::{ReadableDatabase, ReadableTable, TableDefinition};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
pub type Records = BTreeMap<Vec<u8>, Vec<u8>>;
pub type Batch = Vec<(Vec<u8>, Option<Vec<u8>>)>;
const RECORDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("records");

pub enum Store {
    Redb(redb::Database),
    Sqlite(rusqlite::Connection),
}

impl Store {
    pub fn open(kind: &str, path: &Path, create: bool) -> Result<Self> {
        if create && path.exists() {
            return Err("scratch target already exists".into());
        }
        match kind {
            "redb" => {
                let db = if create {
                    redb::Database::create(path)?
                } else {
                    redb::Database::open(path)?
                };
                if create {
                    let tx = db.begin_write()?;
                    tx.open_table(RECORDS)?;
                    tx.commit()?;
                }
                Ok(Self::Redb(db))
            }
            "sqlite-delete" | "sqlite-wal" => {
                let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | if create {
                        rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                    } else {
                        rusqlite::OpenFlags::empty()
                    };
                let db = rusqlite::Connection::open_with_flags(path, flags)?;
                let mode = if kind == "sqlite-wal" {
                    "WAL"
                } else {
                    "DELETE"
                };
                let actual: String =
                    db.query_row(&format!("PRAGMA journal_mode={mode}"), [], |row| row.get(0))?;
                assert_eq!(actual.to_uppercase(), mode, "journal mode not accepted");
                db.execute_batch("PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000;")?;
                if create {
                    db.execute_batch(
                        "CREATE TABLE records (key BLOB PRIMARY KEY, value BLOB NOT NULL) WITHOUT ROWID;",
                    )?;
                }
                Ok(Self::Sqlite(db))
            }
            _ => Err("unknown candidate".into()),
        }
    }

    pub fn apply(&mut self, batch: &Batch, before_commit: impl FnOnce()) -> Result<()> {
        match self {
            Self::Redb(db) => {
                let mut tx = db.begin_write()?;
                tx.set_durability(redb::Durability::Immediate)?;
                {
                    let mut table = tx.open_table(RECORDS)?;
                    for (key, value) in batch {
                        if let Some(value) = value {
                            table.insert(key.as_slice(), value.as_slice())?;
                        } else {
                            table.remove(key.as_slice())?;
                        }
                    }
                }
                before_commit();
                tx.commit()?;
            }
            Self::Sqlite(db) => {
                let tx = db.transaction()?;
                {
                    let mut put = tx.prepare_cached(
                        "INSERT INTO records VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    )?;
                    let mut delete = tx.prepare_cached("DELETE FROM records WHERE key=?1")?;
                    for (key, value) in batch {
                        if let Some(value) = value {
                            put.execute(rusqlite::params![key, value])?;
                        } else {
                            delete.execute([key])?;
                        }
                    }
                }
                before_commit();
                tx.commit()?;
            }
        }
        Ok(())
    }

    pub fn all(&self) -> Result<Records> {
        let mut records = Records::new();
        match self {
            Self::Redb(db) => {
                let tx = db.begin_read()?;
                for row in tx.open_table(RECORDS)?.iter()? {
                    let (key, value) = row?;
                    records.insert(key.value().to_vec(), value.value().to_vec());
                }
            }
            Self::Sqlite(db) => {
                let mut stmt = db.prepare("SELECT key,value FROM records ORDER BY key")?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                for row in rows {
                    let (key, value) = row?;
                    records.insert(key, value);
                }
            }
        }
        Ok(records)
    }

    pub fn compact(&mut self) -> Result<()> {
        match self {
            Self::Redb(db) => {
                db.compact()?;
            }
            Self::Sqlite(db) => db.execute_batch("VACUUM;")?,
        }
        Ok(())
    }
}

pub fn delta(before: &Records, after: &Records) -> Batch {
    let mut batch: Batch = after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), Some(value.clone())))
        .collect();
    batch.extend(
        before
            .keys()
            .filter(|key| !after.contains_key(*key))
            .map(|key| (key.clone(), None)),
    );
    batch
}

// Canonical, length-framed comparison bytes, not a new project wire format.
pub fn canonical(records: &Records) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (key, value) in records {
        bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value);
    }
    bytes
}

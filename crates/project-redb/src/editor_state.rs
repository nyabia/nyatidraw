use nyatidraw_api::LayerId;
use redb::Durability;

use super::{ProjectDb, ProjectOpenError, STATE};

const TOOL_STATE_KEY: &str = "editor_tool_state";
const MAX_TOOL_STATE_BYTES: usize = 512;
const ACTIVE_LAYER_KEY: &str = "editor_active_layer";
const ACTIVE_LAYER_MAGIC: &[u8; 8] = b"NYLAYER1";

impl ProjectDb {
    /// Loads optional application-owned tool preferences, independently of artwork validation.
    ///
    /// # Errors
    /// Returns storage failure or an oversized preference record without changing the project.
    pub fn load_editor_tool_state(&self) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        self.load_editor_state(TOOL_STATE_KEY)
    }

    /// # Errors
    /// Rejects unknown or malformed optional state without altering artwork.
    pub fn load_active_layer(&self) -> Result<Option<LayerId>, ProjectOpenError> {
        self.load_editor_state(ACTIVE_LAYER_KEY)?
            .map(|bytes| {
                if bytes.len() != 24 || !bytes.starts_with(ACTIVE_LAYER_MAGIC) {
                    return Err(self.corrupt("unrecognized active layer state"));
                }
                let id = u128::from_le_bytes(
                    bytes[8..]
                        .try_into()
                        .map_err(|_| self.corrupt("invalid active layer identifier"))?,
                );
                Ok(LayerId(id))
            })
            .transpose()
    }

    /// # Errors
    /// Preserves malformed/future state and leaves artwork and history unchanged.
    pub fn persist_active_layer(&self, layer: LayerId) -> Result<(), ProjectOpenError> {
        if self.load_active_layer()? == Some(layer) {
            return Ok(());
        }
        let mut bytes = [0; 24];
        bytes[..8].copy_from_slice(ACTIVE_LAYER_MAGIC);
        bytes[8..].copy_from_slice(&layer.0.to_le_bytes());
        self.persist_editor_state(ACTIVE_LAYER_KEY, &bytes)
    }

    fn load_editor_state(&self, key: &str) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        let read = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match read.open_table(STATE) {
            Ok(state) => state,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(bytes) = state.get(key).map_err(|error| self.io(error))? else {
            return Ok(None);
        };
        if bytes.value().len() > MAX_TOOL_STATE_BYTES {
            return Err(self.corrupt("editor state exceeds its byte limit"));
        }
        Ok(Some(bytes.value().to_vec()))
    }

    /// Stores bounded application-owned preferences without changing any artwork or history head.
    ///
    /// # Errors
    /// Returns invalid length or storage failure without replacing the prior record.
    pub fn persist_editor_tool_state(&self, bytes: &[u8]) -> Result<(), ProjectOpenError> {
        self.persist_editor_state(TOOL_STATE_KEY, bytes)
    }

    fn persist_editor_state(&self, key: &str, bytes: &[u8]) -> Result<(), ProjectOpenError> {
        if bytes.is_empty() || bytes.len() > MAX_TOOL_STATE_BYTES {
            return Err(self.io("invalid editor tool state length"));
        }
        let mut write = self.db.begin_write().map_err(|error| self.io(error))?;
        write
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        {
            let mut state = write.open_table(STATE).map_err(|error| self.io(error))?;
            state.insert(key, bytes).map_err(|error| self.io(error))?;
        }
        write.commit().map_err(|error| self.io(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryProjectDb;

    #[test]
    fn malformed_active_layer_state_cannot_block_artwork_or_be_overwritten() {
        let memory = MemoryProjectDb::open(&[], 32 * 1024 * 1024).unwrap();
        let db = memory.db();
        for invalid in [b"NYLAYER2future".as_slice(), b"NYLAYER1short"] {
            db.persist_editor_state(ACTIVE_LAYER_KEY, invalid).unwrap();
            assert!(db.load_active_layer().is_err());
            assert!(db.persist_active_layer(LayerId(1)).is_err());
            assert_eq!(
                db.load_editor_state(ACTIVE_LAYER_KEY).unwrap().unwrap(),
                invalid
            );
            assert!(db.load_reopened().unwrap().is_none());
        }
    }
}

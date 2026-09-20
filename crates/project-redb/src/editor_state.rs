use redb::Durability;

use super::{ProjectDb, ProjectOpenError, STATE};

const TOOL_STATE_KEY: &str = "editor_tool_state";
const MAX_TOOL_STATE_BYTES: usize = 512;

impl ProjectDb {
    /// Loads optional application-owned tool preferences, independently of artwork validation.
    ///
    /// # Errors
    /// Returns storage failure or an oversized preference record without changing the project.
    pub fn load_editor_tool_state(&self) -> Result<Option<Vec<u8>>, ProjectOpenError> {
        let read = self.db.begin_read().map_err(|error| self.io(error))?;
        let state = match read.open_table(STATE) {
            Ok(state) => state,
            Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(error) => return Err(self.io(error)),
        };
        let Some(bytes) = state.get(TOOL_STATE_KEY).map_err(|error| self.io(error))? else {
            return Ok(None);
        };
        if bytes.value().len() > MAX_TOOL_STATE_BYTES {
            return Err(self.corrupt("editor tool state exceeds its byte limit"));
        }
        Ok(Some(bytes.value().to_vec()))
    }

    /// Stores bounded application-owned preferences without changing any artwork or history head.
    ///
    /// # Errors
    /// Returns invalid length or storage failure without replacing the prior record.
    pub fn persist_editor_tool_state(&self, bytes: &[u8]) -> Result<(), ProjectOpenError> {
        if bytes.is_empty() || bytes.len() > MAX_TOOL_STATE_BYTES {
            return Err(self.io("invalid editor tool state length"));
        }
        let mut write = self.db.begin_write().map_err(|error| self.io(error))?;
        write
            .set_durability(Durability::Immediate)
            .map_err(|error| self.io(error))?;
        {
            let mut state = write.open_table(STATE).map_err(|error| self.io(error))?;
            state
                .insert(TOOL_STATE_KEY, bytes)
                .map_err(|error| self.io(error))?;
        }
        write.commit().map_err(|error| self.io(error))
    }
}

//! Count immutable-root references, not pixel copies or history operations.
//! Legacy databases without an index are intentionally not collected on open.
use std::collections::BTreeSet;

use super::{
    Envelope, META, OBJECTS, ObjectHash, ProjectDb, ProjectOpenError, ROOTS, ReadableTable,
    RecordKind, TableDefinition, TileSnapshot, decode_root_manifest, store_root,
};

pub(super) const REFERENCE_VERSION_KEY: &str = "tile_root_refs_version";
pub(super) const TILE_ROOT_REFS: TableDefinition<&[u8], u64> =
    TableDefinition::new("tile_root_refs");

impl ProjectDb {
    fn references_enabled(&self, tx: &redb::WriteTransaction) -> Result<bool, ProjectOpenError> {
        let meta = tx.open_table(META).map_err(|e| self.io(e))?;
        match meta
            .get(REFERENCE_VERSION_KEY)
            .map_err(|e| self.io(e))?
            .map(|v| v.value())
        {
            None => Ok(false),
            Some(1) => Ok(true),
            Some(_) => Err(self.corrupt("unsupported tile reference index")),
        }
    }

    pub(super) fn store_roots(
        &self,
        tx: &redb::WriteTransaction,
        before: &TileSnapshot,
        after: &TileSnapshot,
    ) -> Result<(), ProjectOpenError> {
        let indexed = self.references_enabled(tx)?;
        let mut roots = tx.open_table(ROOTS).map_err(|e| self.io(e))?;
        for snapshot in [before, after] {
            let exists = roots
                .get(snapshot.root().hash.0.as_slice())
                .map_err(|e| self.io(e))?
                .is_some();
            if indexed && !exists {
                let mut refs = tx.open_table(TILE_ROOT_REFS).map_err(|e| self.io(e))?;
                let hashes: BTreeSet<_> = snapshot.iter().map(|(_, tile)| tile.hash()).collect();
                for hash in hashes {
                    let count = refs
                        .get(hash.0.as_slice())
                        .map_err(|e| self.io(e))?
                        .map_or(0, |v| v.value());
                    let next = count
                        .checked_add(1)
                        .ok_or_else(|| self.corrupt("tile reference overflow"))?;
                    refs.insert(hash.0.as_slice(), next)
                        .map_err(|e| self.io(e))?;
                }
            }
            store_root(&mut roots, snapshot).map_err(|e| self.io(e))?;
        }
        Ok(())
    }

    /// Only roots belonging to evicted operations are candidates. Retained
    /// branches, the rebased initial state and the current cursor stay pinned.
    pub(super) fn reclaim_roots(
        &self,
        tx: &redb::WriteTransaction,
        candidates: &BTreeSet<ObjectHash>,
        live: &BTreeSet<ObjectHash>,
    ) -> Result<(), ProjectOpenError> {
        if !self.references_enabled(tx)? {
            return Ok(());
        }
        let mut roots = tx.open_table(ROOTS).map_err(|e| self.io(e))?;
        let mut refs = tx.open_table(TILE_ROOT_REFS).map_err(|e| self.io(e))?;
        let mut objects = tx.open_table(OBJECTS).map_err(|e| self.io(e))?;
        for hash in candidates.difference(live) {
            let bytes = roots
                .get(hash.0.as_slice())
                .map_err(|e| self.io(e))?
                .ok_or_else(|| self.corrupt("evicted root missing"))?
                .value()
                .to_vec();
            let envelope =
                Envelope::decode(&bytes, RecordKind::ContentRoot).map_err(|e| self.corrupt(e))?;
            let manifest = decode_root_manifest(&envelope.payload).map_err(|e| self.corrupt(e))?;
            if manifest.root.hash != *hash {
                return Err(self.corrupt("evicted root hash mismatch"));
            }
            let hashes: BTreeSet<_> = manifest.tiles.into_iter().map(|(_, hash)| hash).collect();
            for object in hashes {
                let count = refs
                    .get(object.0.as_slice())
                    .map_err(|e| self.io(e))?
                    .ok_or_else(|| self.corrupt("tile reference missing"))?
                    .value();
                match count {
                    0 => return Err(self.corrupt("tile reference underflow")),
                    1 => {
                        refs.remove(object.0.as_slice()).map_err(|e| self.io(e))?;
                        objects
                            .remove(object.0.as_slice())
                            .map_err(|e| self.io(e))?;
                    }
                    _ => {
                        refs.insert(object.0.as_slice(), count - 1)
                            .map_err(|e| self.io(e))?;
                    }
                }
            }
            roots.remove(hash.0.as_slice()).map_err(|e| self.io(e))?;
        }
        Ok(())
    }
}

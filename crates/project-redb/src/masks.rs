use super::{META, ProjectDb, ProjectOpenError, ROOTS, decode_root_manifest, root_codec};
use nyatidraw_document::LayerTree;
use nyatidraw_tiles::{ContentRoot, TileSnapshot};

impl ProjectDb {
    pub(super) fn validate_mask_transition(
        &self,
        before: &TileSnapshot,
        after: &TileSnapshot,
        next_tree: Option<&LayerTree>,
    ) -> Result<(), ProjectOpenError> {
        let current = self.load_layer_tree()?;
        self.validate_mask_owners(
            before.coverage_iter().map(|(key, _)| key.mask),
            current.as_ref(),
        )?;
        self.validate_mask_owners(
            after.coverage_iter().map(|(key, _)| key.mask),
            next_tree.or(current.as_ref()),
        )
    }

    fn validate_mask_owners(
        &self,
        masks: impl Iterator<Item = nyatidraw_api::MaskId>,
        tree: Option<&LayerTree>,
    ) -> Result<(), ProjectOpenError> {
        let owned = tree.map(LayerTree::mask_ids).unwrap_or_default();
        if masks.into_iter().any(|mask| !owned.contains(&mask)) {
            return Err(self.corrupt("coverage plane has no unique layer owner"));
        }
        Ok(())
    }

    fn validate_mask_root(
        &self,
        root: ContentRoot,
        tree: Option<&LayerTree>,
    ) -> Result<(), ProjectOpenError> {
        let transaction = self.db.begin_read().map_err(|error| self.io(error))?;
        let roots = transaction
            .open_table(ROOTS)
            .map_err(|error| self.corrupt(error))?;
        let record = roots
            .get(root.hash.0.as_slice())
            .map_err(|error| self.corrupt(error))?
            .ok_or_else(|| self.corrupt("mask root missing"))?;
        let envelope = root_codec::decode(record.value()).map_err(|error| self.corrupt(error))?;
        let manifest =
            decode_root_manifest(&envelope.payload).map_err(|error| self.corrupt(error))?;
        if !manifest.coverage.is_empty() {
            let metadata = transaction
                .open_table(META)
                .map_err(|error| self.corrupt(error))?;
            let flags = metadata
                .get("schema_version")
                .map_err(|error| self.corrupt(error))?
                .map_or(0, |value| value.value());
            if flags & nyatidraw_project::MASK_SCHEMA_FLAG == 0 {
                return Err(self.corrupt("coverage planes lack required reader capability"));
            }
        }
        self.validate_mask_owners(manifest.coverage.into_iter().map(|(key, _)| key.mask), tree)
    }

    pub(super) fn validate_mask_history(&self) -> Result<(), ProjectOpenError> {
        let Some(reopened) = self.load_reopened()? else {
            return Ok(());
        };
        let mut cursors = reopened.into_parts().cursors;
        if let Some(initial) = self.load_initial_cursor()? {
            cursors.insert(None, initial);
        }
        for cursor in cursors.values() {
            let tree = self.load_cursor_layer_tree(*cursor)?;
            self.validate_mask_root(cursor.root, tree.as_ref())?;
        }
        Ok(())
    }
}

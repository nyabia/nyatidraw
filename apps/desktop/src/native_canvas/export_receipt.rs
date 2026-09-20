use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use nyatidraw_api::SnapshotId;

pub(super) struct ExportReceipt {
    snapshot: SnapshotId,
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

impl ExportReceipt {
    pub(super) fn capture(snapshot: SnapshotId, path: &Path) -> Option<Self> {
        let metadata = path.metadata().ok()?;
        let modified = metadata.modified().ok()?;
        metadata.is_file().then(|| Self {
            snapshot,
            path: path.to_owned(),
            bytes: metadata.len(),
            modified,
        })
    }

    pub(super) fn matches(&self, snapshot: SnapshotId, path: &Path) -> bool {
        self.snapshot == snapshot
            && self.path == path
            && Self::capture(snapshot, path).is_some_and(|current| {
                self.bytes == current.bytes && self.modified == current.modified
            })
    }
}

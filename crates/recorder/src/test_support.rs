//! Helpers shared by the unit tests.

use std::path::{Path, PathBuf};

/// A unique directory under the system temp directory, removed (best effort) when dropped.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir()
            .join("lmu-recorder-tests")
            .join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

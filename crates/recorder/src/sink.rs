//! Destination of segment bytes. Production writes `std::fs::File`s; unit tests inject I/O faults through
//! the same seam to exercise the write-failure path deterministically.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// An open segment file.
pub(crate) trait SegmentFile: Write + Send {
    /// Flushes file data (not necessarily metadata) to stable storage (`FlushFileBuffers` on Windows).
    fn sync_data(&mut self) -> io::Result<()>;
}

impl SegmentFile for File {
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(self)
    }
}

/// Creates segment files.
pub(crate) trait SegmentFileFactory: Send + Sync {
    /// Creates `path`, failing if it already exists (a segment file is never overwritten).
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn SegmentFile>>;
}

/// The operating system's file system.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct OsFiles;

impl SegmentFileFactory for OsFiles {
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn SegmentFile>> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Box::new(file))
    }
}

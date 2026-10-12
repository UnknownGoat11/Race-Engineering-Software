//! Errors of the tool (a capture that stops early is not an error: it is reported in the report itself).

use std::path::PathBuf;

use thiserror::Error;

/// Errors that stop a command.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// The output directory or a report file could not be written.
    #[error("cannot write {path}: {source}")]
    Write {
        /// File or directory.
        path: PathBuf,
        /// Root cause.
        #[source]
        source: std::io::Error,
    },
    /// The report could not be serialized to JSON.
    #[error("cannot serialize the report: {0}")]
    Json(#[from] serde_json::Error),
    /// Invalid command-line input.
    #[error("{0}")]
    Usage(String),
}

//! Error types. Every variant keeps its root cause (`#[source]`) so logs show the original OS/SQLite
//! error.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::format::{FormatError, RecordKind};
use crate::index::IndexError;
use crate::scan::ScanError;
use crate::types::{SessionId, SessionStatus};

/// Errors from store, session, recovery and reader operations.
#[derive(Debug, Error)]
pub enum RecorderError {
    #[error("invalid recorder configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid session metadata: {0}")]
    InvalidMeta(String),
    #[error("recordings directory {path} is in use by another recorder instance")]
    DirectoryInUse { path: PathBuf },
    #[error("I/O error ({op}) on {path}: {source}")]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error("format error in {path} at byte {offset}: {source}")]
    Format {
        path: PathBuf,
        offset: u64,
        #[source]
        source: FormatError,
    },
    #[error("scan of {path} failed: {source}")]
    Scan {
        path: PathBuf,
        #[source]
        source: ScanError,
    },
    #[error("session {0} not found in the index")]
    SessionNotFound(SessionId),
    #[error("session {id} is {status}; this operation requires {required}")]
    InvalidSessionState {
        id: SessionId,
        status: SessionStatus,
        required: &'static str,
    },
    #[error("session {0} is being recorded by this process")]
    SessionBusy(SessionId),
    #[error("session {0} is marked KEEP; retention policies never delete it")]
    RetentionKeep(SessionId),
    #[error(
        "{path} belongs to session {found_session} segment {found_segment}, expected session {session} segment {segment}"
    )]
    SegmentMismatch {
        path: PathBuf,
        session: SessionId,
        segment: u32,
        found_session: SessionId,
        found_segment: u32,
    },
    #[error("path {0} is not inside the recordings directory")]
    PathOutsideRoot(PathBuf),
    #[error("zstd context could not be created: {0}")]
    Codec(#[source] io::Error),
    #[error("could not spawn the recorder writer thread: {0}")]
    SpawnWriter(#[source] io::Error),
    #[error("the recorder writer thread panicked")]
    WriterPanicked,
    #[error("the recorder writer thread has stopped")]
    WriterStopped,
    #[error("timed out while {operation}")]
    Timeout { operation: &'static str },
    #[error(
        "index entry for chunk {chunk_seq} of {path} does not match the file at byte {offset}: {detail}"
    )]
    IndexMismatch {
        path: PathBuf,
        offset: u64,
        chunk_seq: u32,
        detail: String,
    },
}

impl RecorderError {
    pub(crate) fn io(op: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        RecorderError::Io {
            op,
            path: path.into(),
            source,
        }
    }
}

/// Errors returned to producers by the non-blocking append path. Overruns are already counted and
/// reported as events when this is returned; callers may ignore the result.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppendError {
    #[error("recorder input queue full: record dropped and counted as overrun")]
    Overrun,
    #[error("the recording session has finished")]
    Stopped,
    #[error("record kind {} is reserved for the recorder", .0.as_str())]
    ReservedKind(RecordKind),
    #[error("record payload is {len} bytes, maximum is {max}")]
    TooLarge { len: usize, max: usize },
    #[error("capture time {capture_ns} ns exceeds the supported maximum {max}")]
    CaptureTimeOutOfRange { capture_ns: u64, max: u64 },
    #[error("recorder control queue full")]
    ControlQueueFull,
    /// A JSON field of a lap, stint or metadata update is not valid JSON; nothing was queued.
    #[error("{field} is not valid JSON: {reason}")]
    InvalidJson { field: &'static str, reason: String },
}

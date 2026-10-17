//! Crash-tolerant local recording for LMU Race Engineering (decision D-006, `docs/STORAGE_DESIGN.md`).
//!
//! * A [`RecordingStore`] owns one recordings directory: an exclusive lock file, the SQLite session index
//!   (`index.sqlite3`) and one sub-directory per session. Opening it runs startup recovery: sessions left
//!   ACTIVE by a crash are rescanned and marked INTERRUPTED with their last valid capture time.
//! * [`RecordingStore::start_session`] returns a [`RecordingSession`] whose writer runs on its own OS thread.
//!   Producers hand it already-encoded records ([`RecordKind`], capture time, bytes) through
//!   [`RecordingSession::append`] / [`RecordSender::append`], which never block: a full queue drops the
//!   record, counts it and reports [`RecorderEvent::RecorderOverrun`].
//! * Records are packed into self-delimiting, CRC-protected, zstd-compressed chunks (≈ 1 s each) appended to
//!   `.lmurec` segment files that rotate by size and age. Data is flushed and `sync_data`ed about once per
//!   second; a crash loses at most the unsynced tail, and recovery never modifies a recording file.
//! * [`SessionReader`] streams records back by capture-time range, chunk by chunk (bounded memory), using
//!   the chunk offset index; [`ReplayClock`] is the pure playback clock for replay.
//!
//! The recorder is payload-agnostic: it never decodes telemetry, so it does not depend on the frame types.

mod chunk;
mod clock;
mod config;
mod cpu;
mod disk;
mod error;
pub mod format;
mod index;
mod reader;
mod recovery;
mod replay;
mod retention;
pub mod scan;
mod sink;
mod store;
mod types;
mod writer;

#[cfg(test)]
mod scenario_tests;
#[cfg(test)]
mod test_support;

/// `component` field of every log line emitted by this crate.
pub(crate) const COMPONENT: &str = "recorder";

pub use clock::CaptureClock;
pub use config::{
    DEFAULT_CHUNK_MAX_AGE, DEFAULT_CHUNK_MAX_CAPTURE_SPAN, DEFAULT_CHUNK_MAX_RAW_BYTES,
    DEFAULT_CONTROL_QUEUE_CAPACITY, DEFAULT_DISK_CHECK_INTERVAL, DEFAULT_DISK_CRITICAL_FREE_BYTES,
    DEFAULT_DISK_RESUME_FREE_BYTES, DEFAULT_DISK_WARN_FREE_BYTES, DEFAULT_EVENT_QUEUE_CAPACITY,
    DEFAULT_FAILURE_RETRY_INTERVAL, DEFAULT_QUEUE_CAPACITY_BYTES, DEFAULT_QUEUE_CAPACITY_RECORDS,
    DEFAULT_SEGMENT_MAX_BYTES, DEFAULT_SEGMENT_MAX_DURATION, DEFAULT_SYNC_INTERVAL,
    DEFAULT_ZSTD_LEVEL, INDEX_FILE_NAME, LOCK_FILE_NAME, MAX_ZSTD_LEVEL, MIN_SEGMENT_MAX_BYTES,
    MIN_ZSTD_LEVEL, RecorderConfig, SEGMENT_FILE_EXTENSION, SESSIONS_DIR_NAME,
    parse_segment_file_name, parse_session_dir_name, segment_file_name, session_dir_name,
};
pub use disk::{DiskSpaceProbe, SystemDiskSpace};
pub use error::{AppendError, RecorderError};
pub use format::{FormatError, GapNote, GapReason, MAX_CAPTURE_NS, MAX_RECORD_BYTES, RecordKind};
pub use index::{
    ChunkEntry, EventQuery, INDEX_APPLICATION_ID, INDEX_SCHEMA_VERSION, IndexError, SegmentInfo,
    SessionInfo,
};
pub use reader::{RecordStream, RecordedRecord, SessionReader};
pub use recovery::{RecoveredSession, RecoveryFailure, RecoveryReport};
pub use replay::{PlaybackSpeed, ReplayClock};
pub use retention::{KeepAll, MaxAge, RetentionFailure, RetentionPolicy, RetentionReport};
pub use store::RecordingStore;
pub use types::{
    EventCategory, EventRecord, IntegrityStatus, LapRecord, RetentionClass, SegmentStatus,
    SessionId, SessionMeta, SessionMetaUpdate, SessionStatus, Severity, StintRecord, TimeRange,
};
pub use writer::{
    RecordSender, RecorderEvent, RecorderStats, RecordingSession, SessionOptions, SessionSummary,
    SyncReport, WriterState,
};

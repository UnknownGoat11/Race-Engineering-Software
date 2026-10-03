//! Recorder configuration and on-disk layout of a recordings directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::clock::format_utc_compact;
use crate::error::RecorderError;
use crate::format::{FILE_HEADER_LEN, MAX_CHUNK_RAW_BYTES};
use crate::types::SessionId;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
const GIB: u64 = 1024 * 1024 * 1024;

/// Default input queue capacity in records: ≈18 s at 110 records/s (100 Hz telemetry plus scoring,
/// weather and events), enough to ride out a disk stall or a segment rotation.
pub const DEFAULT_QUEUE_CAPACITY_RECORDS: usize = 2048;
/// Default cap on bytes held in the input queue (bounds memory if large raw blocks are recorded).
pub const DEFAULT_QUEUE_CAPACITY_BYTES: usize = 64 * MIB;
/// Default capacity of the control queue (laps, stints, events, metadata, sync requests).
pub const DEFAULT_CONTROL_QUEUE_CAPACITY: usize = 256;
/// Default capacity of the event queue towards the application.
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 256;
/// Default maximum age of the chunk being built before it is sealed.
pub const DEFAULT_CHUNK_MAX_AGE: Duration = Duration::from_secs(1);
/// Default capture-time span of one chunk (a chunk covers less than this much capture time).
pub const DEFAULT_CHUNK_MAX_CAPTURE_SPAN: Duration = Duration::from_secs(1);
/// Default maximum uncompressed chunk size before it is sealed.
pub const DEFAULT_CHUNK_MAX_RAW_BYTES: usize = MIB;
/// Default flush + fsync cadence.
pub const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(1);
/// Default zstd level (low levels only: the live recorder must stay cheap).
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;
/// Lowest zstd level accepted for live recording.
pub const MIN_ZSTD_LEVEL: i32 = 1;
/// Highest zstd level accepted for live recording.
pub const MAX_ZSTD_LEVEL: i32 = 3;
/// Default segment size limit.
pub const DEFAULT_SEGMENT_MAX_BYTES: u64 = 256 * MIB as u64;
/// Default segment duration limit.
pub const DEFAULT_SEGMENT_MAX_DURATION: Duration = Duration::from_secs(30 * 60);
/// Smallest segment size limit accepted (header plus a few chunks).
pub const MIN_SEGMENT_MAX_BYTES: u64 = 4 * KIB as u64;
/// Default free-space warning threshold.
pub const DEFAULT_DISK_WARN_FREE_BYTES: u64 = 5 * GIB;
/// Default free-space critical threshold (recording suspends below it).
pub const DEFAULT_DISK_CRITICAL_FREE_BYTES: u64 = GIB;
/// Default free space at which a suspended recording resumes (hysteresis above the critical threshold so
/// recording does not flap open/closed around it).
pub const DEFAULT_DISK_RESUME_FREE_BYTES: u64 = 2 * GIB;
/// Default disk-space polling interval.
pub const DEFAULT_DISK_CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// Default retry interval after a write failure.
pub const DEFAULT_FAILURE_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Name of the SQLite index file inside the recordings directory.
pub const INDEX_FILE_NAME: &str = "index.sqlite3";
/// Name of the lock file inside the recordings directory.
pub const LOCK_FILE_NAME: &str = "recorder.lock";
/// Name of the directory holding one sub-directory per session.
pub const SESSIONS_DIR_NAME: &str = "sessions";
/// Extension of segment files.
pub const SEGMENT_FILE_EXTENSION: &str = "lmurec";
const SEGMENT_FILE_PREFIX: &str = "seg-";
/// Minimum number of digits in a segment file's index (zero-padded).
const SEGMENT_INDEX_MIN_DIGITS: usize = 6;

/// File name of segment `index`, e.g. `seg-000042.lmurec`.
pub fn segment_file_name(index: u32) -> String {
    format!(
        "{SEGMENT_FILE_PREFIX}{index:0width$}.{SEGMENT_FILE_EXTENSION}",
        width = SEGMENT_INDEX_MIN_DIGITS
    )
}

/// Parses a name produced by [`segment_file_name`].
pub fn parse_segment_file_name(name: &str) -> Option<u32> {
    let digits = name
        .strip_prefix(SEGMENT_FILE_PREFIX)?
        .strip_suffix(SEGMENT_FILE_EXTENSION)?
        .strip_suffix('.')?;
    if digits.len() < SEGMENT_INDEX_MIN_DIGITS || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Session directory name: `<YYYYMMDDTHHMMSSZ>_<uuid>` (UTC start time, sortable, then the session id).
pub fn session_dir_name(started_unix_ns: i64, id: SessionId) -> String {
    format!("{}_{id}", format_utc_compact(started_unix_ns))
}

/// Extracts the session id from a name produced by [`session_dir_name`].
pub fn parse_session_dir_name(name: &str) -> Option<SessionId> {
    let (_, uuid) = name.rsplit_once('_')?;
    uuid.parse().ok()
}

/// Recorder configuration. Construct with [`RecorderConfig::new`] and adjust fields as needed.
#[derive(Debug, Clone, PartialEq)]
pub struct RecorderConfig {
    /// Root of the recordings directory (index, lock file, `sessions/`).
    pub recordings_dir: PathBuf,
    /// Input queue capacity in records.
    pub queue_capacity_records: usize,
    /// Input queue capacity in payload bytes.
    pub queue_capacity_bytes: usize,
    /// Capacity of the control queue (laps, stints, events, metadata, sync and rotation requests).
    pub control_queue_capacity: usize,
    /// Capacity of the [`crate::RecorderEvent`] queue towards the application.
    pub event_queue_capacity: usize,
    /// A chunk is sealed when its first record is this old (wall clock)…
    pub chunk_max_age: Duration,
    /// …or when the next record would make it span this much capture time…
    pub chunk_max_capture_span: Duration,
    /// …or when its uncompressed size would exceed this.
    pub chunk_max_raw_bytes: usize,
    /// Flush + `sync_data` cadence for written chunks (the open chunk is not sealed early for it, so the
    /// data at risk on a crash is bounded by `chunk_max_age` + `sync_interval`).
    pub sync_interval: Duration,
    /// zstd level, [`MIN_ZSTD_LEVEL`]..=[`MAX_ZSTD_LEVEL`].
    pub zstd_level: i32,
    /// Rotate to a new segment file when the current one reaches this size…
    pub segment_max_bytes: u64,
    /// …or this age.
    pub segment_max_duration: Duration,
    /// Emit `DiskSpaceLow` below this many free bytes.
    pub disk_warn_free_bytes: u64,
    /// Suspend recording (closing the segment cleanly) below this many free bytes.
    pub disk_critical_free_bytes: u64,
    /// Resume a suspended recording once at least this many bytes are free (≥ the critical threshold).
    pub disk_resume_free_bytes: u64,
    /// How often free disk space is measured.
    pub disk_check_interval: Duration,
    /// Interval between attempts to open a new segment after a write failure.
    pub failure_retry_interval: Duration,
    /// Re-read and fully validate each segment when it is closed.
    pub validate_on_close: bool,
}

impl RecorderConfig {
    /// Default configuration for `recordings_dir`.
    pub fn new(recordings_dir: impl Into<PathBuf>) -> Self {
        Self {
            recordings_dir: recordings_dir.into(),
            queue_capacity_records: DEFAULT_QUEUE_CAPACITY_RECORDS,
            queue_capacity_bytes: DEFAULT_QUEUE_CAPACITY_BYTES,
            control_queue_capacity: DEFAULT_CONTROL_QUEUE_CAPACITY,
            event_queue_capacity: DEFAULT_EVENT_QUEUE_CAPACITY,
            chunk_max_age: DEFAULT_CHUNK_MAX_AGE,
            chunk_max_capture_span: DEFAULT_CHUNK_MAX_CAPTURE_SPAN,
            chunk_max_raw_bytes: DEFAULT_CHUNK_MAX_RAW_BYTES,
            sync_interval: DEFAULT_SYNC_INTERVAL,
            zstd_level: DEFAULT_ZSTD_LEVEL,
            segment_max_bytes: DEFAULT_SEGMENT_MAX_BYTES,
            segment_max_duration: DEFAULT_SEGMENT_MAX_DURATION,
            disk_warn_free_bytes: DEFAULT_DISK_WARN_FREE_BYTES,
            disk_critical_free_bytes: DEFAULT_DISK_CRITICAL_FREE_BYTES,
            disk_resume_free_bytes: DEFAULT_DISK_RESUME_FREE_BYTES,
            disk_check_interval: DEFAULT_DISK_CHECK_INTERVAL,
            failure_retry_interval: DEFAULT_FAILURE_RETRY_INTERVAL,
            validate_on_close: true,
        }
    }

    /// Checks every field; returns the first problem found.
    pub fn validate(&self) -> Result<(), RecorderError> {
        let bad = |msg: String| Err(RecorderError::InvalidConfig(msg));
        if self.recordings_dir.as_os_str().is_empty() {
            return bad("recordings_dir is empty".into());
        }
        for (name, v) in [
            ("queue_capacity_records", self.queue_capacity_records),
            ("queue_capacity_bytes", self.queue_capacity_bytes),
            ("control_queue_capacity", self.control_queue_capacity),
            ("event_queue_capacity", self.event_queue_capacity),
        ] {
            if v == 0 {
                return bad(format!("{name} must be at least 1"));
            }
        }
        if self.chunk_max_raw_bytes == 0 || self.chunk_max_raw_bytes > MAX_CHUNK_RAW_BYTES {
            return bad(format!(
                "chunk_max_raw_bytes must be in 1..={MAX_CHUNK_RAW_BYTES}"
            ));
        }
        for (name, d) in [
            ("chunk_max_age", self.chunk_max_age),
            ("chunk_max_capture_span", self.chunk_max_capture_span),
            ("sync_interval", self.sync_interval),
            ("segment_max_duration", self.segment_max_duration),
            ("disk_check_interval", self.disk_check_interval),
            ("failure_retry_interval", self.failure_retry_interval),
        ] {
            if d.is_zero() {
                return bad(format!("{name} must be non-zero"));
            }
        }
        if !(MIN_ZSTD_LEVEL..=MAX_ZSTD_LEVEL).contains(&self.zstd_level) {
            return bad(format!(
                "zstd_level must be in {MIN_ZSTD_LEVEL}..={MAX_ZSTD_LEVEL}"
            ));
        }
        if self.segment_max_bytes < MIN_SEGMENT_MAX_BYTES.max(FILE_HEADER_LEN as u64) {
            return bad(format!(
                "segment_max_bytes must be at least {MIN_SEGMENT_MAX_BYTES}"
            ));
        }
        if self.disk_critical_free_bytes > self.disk_warn_free_bytes {
            return bad("disk_critical_free_bytes must not exceed disk_warn_free_bytes".into());
        }
        if self.disk_resume_free_bytes < self.disk_critical_free_bytes {
            return bad("disk_resume_free_bytes must be at least disk_critical_free_bytes".into());
        }
        Ok(())
    }

    /// [`Self::chunk_max_capture_span`] in capture-clock nanoseconds (saturating).
    pub fn chunk_max_capture_span_ns(&self) -> u64 {
        u64::try_from(self.chunk_max_capture_span.as_nanos()).unwrap_or(u64::MAX)
    }

    /// Path of the SQLite index.
    pub fn index_path(&self) -> PathBuf {
        self.recordings_dir.join(INDEX_FILE_NAME)
    }

    /// Path of the lock file.
    pub fn lock_path(&self) -> PathBuf {
        self.recordings_dir.join(LOCK_FILE_NAME)
    }

    /// Directory holding the session sub-directories.
    pub fn sessions_dir(&self) -> PathBuf {
        self.recordings_dir.join(SESSIONS_DIR_NAME)
    }

    /// Resolves a path stored in the index (relative to the recordings directory).
    pub fn resolve(&self, relative: &Path) -> PathBuf {
        self.recordings_dir.join(relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_checks_fire() {
        let c = RecorderConfig::new("x");
        assert!(c.validate().is_ok());
        let mut bad = c.clone();
        bad.zstd_level = 9;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.disk_critical_free_bytes = bad.disk_warn_free_bytes + 1;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.sync_interval = Duration::ZERO;
        assert!(bad.validate().is_err());
        let mut bad = c.clone();
        bad.disk_resume_free_bytes = bad.disk_critical_free_bytes - 1;
        assert!(bad.validate().is_err());
        let mut bad = c;
        bad.queue_capacity_records = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn layout_names_roundtrip() {
        assert_eq!(segment_file_name(42), "seg-000042.lmurec");
        assert_eq!(parse_segment_file_name("seg-000042.lmurec"), Some(42));
        assert_eq!(
            parse_segment_file_name(&segment_file_name(u32::MAX)),
            Some(u32::MAX)
        );
        for bad in [
            "seg-42.lmurec",
            "seg-00004x.lmurec",
            "seg-000042.lmurecx",
            "x-000042.lmurec",
            "seg-000042lmurec",
        ] {
            assert_eq!(parse_segment_file_name(bad), None, "{bad}");
        }
        let id = SessionId::new_random();
        let name = session_dir_name(0, id);
        assert!(name.starts_with("19700101T000000Z_"), "{name}");
        assert_eq!(parse_session_dir_name(&name), Some(id));
        assert_eq!(parse_session_dir_name("no-underscore"), None);
        assert_eq!(parse_session_dir_name("20260101T000000Z_not-a-uuid"), None);
    }
}

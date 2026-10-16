//! Recording writer: one OS thread per recording session (decision D-006, ARCHITECTURE Â§3).
//!
//! Data path: producers call [`RecordSender::append`], which never blocks: the record is copied into a
//! bounded queue (bounded in records and in bytes) or, when the queue is full, counted as an overrun. The
//! writer thread packs records into chunks (sealed at `chunk_max_age` or `chunk_max_raw_bytes`), appends
//! sealed chunks to the current segment through a write buffer and, every `sync_interval`, flushes and
//! `sync_data`s the file before recording the new chunks in the index. Segments rotate by size and age.
//!
//! Failure policy: nothing ever blocks or crashes the producer. On a write or sync error the segment is
//! abandoned (a validation scan establishes exactly what reached the file), records are dropped and
//! counted, `RecordingFailed` is raised and a new segment is attempted every `failure_retry_interval`.
//! Below the critical free-space threshold the segment is closed cleanly and recording is suspended until
//! space returns. Every drop (overrun, write failure, disk full) is counted in [`RecorderStats`], reported
//! as an event, logged, indexed, and written into the recording as a recorder note ([`GapNote`]) as soon
//! as writing is possible again: a gap is never silent.

use std::fmt;
use std::fs;
use std::io::{self, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{
    Receiver, RecvTimeoutError, SendTimeoutError, Sender, TrySendError, bounded, select,
};
use zstd::bulk::Compressor;

use crate::COMPONENT;
use crate::chunk::{ChunkBuilder, SealedChunk};
use crate::clock::{CaptureClock, unix_now_ns};
use crate::config::{RecorderConfig, segment_file_name};
use crate::cpu::current_thread_cpu_time;
use crate::disk::{DiskSpaceProbe, SystemDiskSpace};
use crate::error::{AppendError, RecorderError};
use crate::format::{
    FILE_HEADER_LEN, FORMAT_VERSION, FileHeader, GAP_NOTE_LEN, GapNote, GapReason, MAX_CAPTURE_NS,
    MAX_RECORD_BYTES, MONO_NS_UNKNOWN, RecordKind,
};
use crate::index::{
    ChunkEntry, ChunkRows, Index, IndexError, SegmentInfo, SegmentProgress, SessionEnd,
    SessionProgress,
};
use crate::recovery::{Outcome, apply_assessment, assess_segment, session_checksum};
use crate::scan::ScanStop;
use crate::sink::{OsFiles, SegmentFile, SegmentFileFactory};
use crate::types::{
    EventCategory, EventRecord, IntegrityStatus, LapRecord, SegmentStatus, SessionId,
    SessionMetaUpdate, SessionStatus, Severity, StintRecord,
};

/// Longest the writer waits without checking its timers (bounds the latency of overrun reports).
const MAX_IDLE_WAIT: Duration = Duration::from_millis(250);
/// Minimum interval between overrun reports (event, log line, index row) during a sustained overrun.
const OVERRUN_REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// Minimum interval between log lines for repeated failed attempts to reopen a segment.
const RETRY_LOG_INTERVAL: Duration = Duration::from_secs(60);
/// Write buffer in front of each segment file.
const WRITE_BUFFER_BYTES: usize = 256 * 1024;
/// Chunk rows kept for a retry while index updates fail (about an hour at one chunk per second). Beyond
/// this the segment's chunk rows stop, and readers locate later chunks by walking chunk headers.
const MAX_PENDING_INDEX_CHUNKS: usize = 4096;
/// Attempts at finding an unused segment file name: an existing file is never overwritten.
const SEGMENT_CREATE_ATTEMPTS: u32 = 16;
/// "Free disk space unknown" in [`Shared::disk_free`].
const DISK_FREE_UNKNOWN: u64 = u64::MAX;

/// Writer state as seen by producers and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriterState {
    /// Records are being written.
    Recording,
    /// A write failed; records are dropped (and counted) until a new segment can be opened.
    Failed,
    /// Free disk space is below the critical threshold; records are dropped (and counted) until it recovers.
    Suspended,
    /// The session has finished; appends are rejected.
    Finished,
}

impl WriterState {
    const fn code(self) -> u8 {
        match self {
            WriterState::Recording => 0,
            WriterState::Failed => 1,
            WriterState::Suspended => 2,
            WriterState::Finished => 3,
        }
    }

    const fn from_code(code: u8) -> Self {
        match code {
            0 => WriterState::Recording,
            1 => WriterState::Failed,
            2 => WriterState::Suspended,
            _ => WriterState::Finished,
        }
    }
}

/// Events raised by a recording session. The queue is bounded: when the application does not drain it,
/// further events are counted in [`RecorderStats::events_dropped`] (they are still logged and indexed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderEvent {
    /// A new segment file was created.
    SegmentOpened {
        /// Index of the new segment.
        segment_index: u32,
        /// Absolute path of its file.
        path: PathBuf,
    },
    /// A segment was closed (rotation, suspension or finish) and validated.
    SegmentClosed {
        /// Index of the closed segment.
        segment_index: u32,
        /// Valid bytes in the file.
        bytes: u64,
        /// Records in the file (producer records plus recorder notes).
        records: u64,
        /// Result of the validation at close.
        integrity: IntegrityStatus,
    },
    /// Producer records were dropped because the input queue was full (reported at most once per second
    /// while it lasts).
    RecorderOverrun {
        /// Dropped since the previous report.
        dropped: u64,
        /// Dropped since the session started.
        total_dropped: u64,
        /// Capture time of the earliest record dropped since the previous report.
        first_capture_ns: Option<u64>,
        /// Capture time of the latest record dropped since the previous report.
        last_capture_ns: Option<u64>,
    },
    /// Writing failed; records are dropped (and counted) until a new segment can be opened.
    RecordingFailed {
        /// Segment being written when it failed (`None` when no segment could be opened).
        segment_index: Option<u32>,
        /// File involved, when there is one.
        path: Option<PathBuf>,
        /// What the writer was doing.
        operation: &'static str,
        /// The operating-system error message.
        error: String,
        /// The raw OS error code, when there is one.
        os_error: Option<i32>,
        /// Portable classification of the error.
        kind: ErrorKind,
    },
    /// Writing resumed in a new segment after a failure.
    RecordingResumed {
        /// The new segment.
        segment_index: u32,
        /// Producer records dropped since the failure (noted in the new segment).
        dropped_records: u64,
    },
    /// Free space fell below the warning threshold.
    DiskSpaceLow {
        /// Free bytes available to the user.
        available_bytes: u64,
        /// The warning threshold.
        threshold_bytes: u64,
    },
    /// Free space fell below the critical threshold: the segment was closed and recording is suspended.
    DiskSpaceCritical {
        /// Free bytes available to the user.
        available_bytes: u64,
        /// The critical threshold.
        threshold_bytes: u64,
    },
    /// Recording continues after a disk-space suspension: free space is back above the resume threshold
    /// (`Some`), or it can no longer be measured (`None`; write failures are then handled as usual).
    DiskSpaceRecovered {
        /// Free bytes available to the user, if measurable.
        available_bytes: Option<u64>,
    },
    /// An index update failed (the recording itself is unaffected; recovery can rebuild the index).
    /// Raised once per streak of failures; every failure is counted in [`RecorderStats::index_errors`].
    IndexWriteFailed {
        /// What the writer was recording in the index.
        operation: &'static str,
        /// The SQLite error.
        error: String,
    },
}

/// Counters of a recording session (a consistent-enough snapshot of independent atomic counters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderStats {
    /// Current writer state.
    pub state: WriterState,
    /// Producer records accepted into the queue.
    pub records_accepted: u64,
    /// Producer records written to a segment and not later found missing.
    pub records_written: u64,
    /// Producer records dropped at `append` because the queue was full.
    pub records_overrun: u64,
    /// Producer records lost to write failures (dropped while failed, or written but not persisted).
    pub records_dropped_write_failure: u64,
    /// Producer records dropped while recording was suspended for lack of disk space.
    pub records_dropped_disk_full: u64,
    /// Appends rejected without being queued (reserved kind, oversized, out-of-range time, after finish).
    pub records_rejected: u64,
    /// Recorder notes written into the recording.
    pub notes_written: u64,
    /// Chunks handed to segment files.
    pub chunks_written: u64,
    /// Bytes written to segment files (headers and compressed chunks).
    pub bytes_written: u64,
    /// Uncompressed size of the written chunk bodies.
    pub raw_bytes_written: u64,
    /// Segment files created.
    pub segments_opened: u64,
    /// Successful flush + `sync_data` calls on segment data.
    pub syncs: u64,
    /// Wall time spent in those calls (successful or not): a slow or failing disk shows up here first.
    pub sync_time_total: Duration,
    /// Longest single flush + `sync_data` call.
    pub sync_time_max: Duration,
    /// Write/sync/create failures (each one abandons a segment or fails a retry to open one).
    pub write_failures: u64,
    /// Failed index updates (the recording itself is unaffected).
    pub index_errors: u64,
    /// Events not delivered because the application did not drain the event queue.
    pub events_dropped: u64,
    /// Lap/stint/event/metadata messages refused because the control queue was full or one of their JSON
    /// fields was invalid ([`AppendError::InvalidJson`]).
    pub control_dropped: u64,
    /// Records currently queued.
    pub queue_len: usize,
    /// Payload bytes currently queued.
    pub queue_bytes: usize,
    /// Free space at the last successful probe.
    pub disk_free_bytes: Option<u64>,
    /// The current error, while the writer is failed or suspended.
    pub last_error: Option<String>,
}

impl RecorderStats {
    /// Producer records that were accepted or offered but are not in the recording.
    pub fn records_lost(&self) -> u64 {
        self.records_overrun + self.records_dropped_write_failure + self.records_dropped_disk_full
    }
}

/// Result of [`RecordingSession::sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Writer state after the sync (anything but `Recording` means the records went to the loss counters).
    pub state: WriterState,
    /// Segment being written, if one is open.
    pub segment_index: Option<u32>,
    /// Bytes of that segment known durable.
    pub durable_bytes: u64,
    /// As [`RecorderStats::records_written`].
    pub records_written: u64,
}

/// Final account of a recording session, returned by [`RecordingSession::finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// The session.
    pub session_id: SessionId,
    /// Absolute path of the session directory.
    pub dir: PathBuf,
    /// Worst integrity over all segments.
    pub integrity: IntegrityStatus,
    /// Per-segment details of anything that is not OK.
    pub integrity_detail: Option<String>,
    /// Final rows of every segment, in order.
    pub segments: Vec<SegmentInfo>,
    /// Records (producer records plus recorder notes) in the valid parts of all segments.
    pub record_count: u64,
    /// Earliest capture time recorded.
    pub first_capture_ns: Option<u64>,
    /// Latest capture time recorded.
    pub last_capture_ns: Option<u64>,
    /// See [`crate::SessionInfo::checksum_crc32c`].
    pub checksum_crc32c: u32,
    /// Final counters.
    pub stats: RecorderStats,
    /// CPU time used by the writer thread (Windows only).
    pub writer_cpu_time: Option<Duration>,
    /// Whether the final index update succeeded (if not, startup recovery completes it).
    pub index_updated: bool,
}

/// Per-session options beyond [`RecorderConfig`].
pub struct SessionOptions {
    disk_probe: Box<dyn DiskSpaceProbe>,
    capture_clock: Option<Arc<dyn CaptureClock>>,
    files: Arc<dyn SegmentFileFactory>,
    /// Test hook: makes the writer thread's creation fail.
    #[cfg(test)]
    fail_thread_spawn: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            disk_probe: Box::new(SystemDiskSpace),
            capture_clock: None,
            files: Arc::new(OsFiles),
            #[cfg(test)]
            fail_thread_spawn: false,
        }
    }
}

impl fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionOptions")
            .field("capture_clock", &self.capture_clock.is_some())
            .finish_non_exhaustive()
    }
}

impl SessionOptions {
    /// Defaults: the operating system's free-space probe and no capture clock.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the free-space probe (tests simulate a filling disk with it).
    pub fn with_disk_probe(mut self, probe: impl DiskSpaceProbe + 'static) -> Self {
        self.disk_probe = Box::new(probe);
        self
    }

    /// The producer's capture clock. With it, each segment header anchors capture times to wall time and
    /// recorder-health events carry a capture time.
    pub fn with_capture_clock(mut self, clock: Arc<dyn CaptureClock>) -> Self {
        self.capture_clock = Some(clock);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_files(mut self, files: Arc<dyn SegmentFileFactory>) -> Self {
        self.files = files;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_failing_thread_spawn(mut self) -> Self {
        self.fail_thread_spawn = true;
        self
    }
}

/// Producer-record gap bookkeeping (count and capture-time range of the dropped records).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GapAccumulator {
    dropped: u64,
    first_ns: u64,
    last_ns: u64,
}

impl GapAccumulator {
    fn add(&mut self, n: u64, first_ns: u64, last_ns: u64) {
        if n == 0 {
            return;
        }
        if self.dropped == 0 {
            self.first_ns = first_ns;
            self.last_ns = last_ns;
        } else {
            self.first_ns = self.first_ns.min(first_ns);
            self.last_ns = self.last_ns.max(last_ns);
        }
        self.dropped = self.dropped.saturating_add(n);
    }

    fn merge(&mut self, other: GapAccumulator) {
        self.add(other.dropped, other.first_ns, other.last_ns);
    }

    fn take(&mut self) -> GapAccumulator {
        std::mem::take(self)
    }

    fn is_empty(&self) -> bool {
        self.dropped == 0
    }

    fn range(&self) -> (Option<u64>, Option<u64>) {
        if self.is_empty() {
            (None, None)
        } else {
            (Some(self.first_ns), Some(self.last_ns))
        }
    }
}

/// State shared between producers ([`RecordSender`]), the session handle and the writer thread.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Set when the writer starts finishing; later appends are rejected.
    closing: AtomicBool,
    /// Appends currently between their `closing` check and their enqueue (see `RecordSender::append`).
    inflight: AtomicUsize,
    queued_bytes: AtomicUsize,
    state: AtomicU8,
    accepted: AtomicU64,
    written: AtomicU64,
    overrun: AtomicU64,
    dropped_write: AtomicU64,
    dropped_disk: AtomicU64,
    rejected: AtomicU64,
    notes: AtomicU64,
    chunks: AtomicU64,
    bytes: AtomicU64,
    raw_bytes: AtomicU64,
    segments: AtomicU64,
    syncs: AtomicU64,
    sync_ns_total: AtomicU64,
    sync_ns_max: AtomicU64,
    write_failures: AtomicU64,
    index_errors: AtomicU64,
    events_dropped: AtomicU64,
    control_dropped: AtomicU64,
    disk_free: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// Overruns not yet collected by the writer (count and capture range), updated under the lock so the
    /// count and range always agree.
    overrun_gap: Mutex<GapAccumulator>,
}

/// Locks a mutex, ignoring poisoning: the protected values are plain counters/strings that stay valid even
/// if a holder panicked.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn new() -> Self {
        let s = Self::default();
        s.disk_free.store(DISK_FREE_UNKNOWN, Ordering::Relaxed);
        s
    }

    fn set_state(&self, state: WriterState) {
        self.state.store(state.code(), Ordering::Release);
    }

    fn set_last_error(&self, error: Option<String>) {
        *lock(&self.last_error) = error;
    }

    fn record_overrun(&self, capture_ns: u64) {
        let mut gap = lock(&self.overrun_gap);
        gap.add(1, capture_ns, capture_ns);
        self.overrun.fetch_add(1, Ordering::Relaxed);
    }

    fn take_overruns(&self) -> GapAccumulator {
        lock(&self.overrun_gap).take()
    }

    fn lost(&self) -> u64 {
        self.overrun.load(Ordering::Relaxed)
            + self.dropped_write.load(Ordering::Relaxed)
            + self.dropped_disk.load(Ordering::Relaxed)
    }

    fn record_sync_time(&self, elapsed: Duration) {
        let ns = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.sync_ns_total.fetch_add(ns, Ordering::Relaxed);
        self.sync_ns_max.fetch_max(ns, Ordering::Relaxed);
    }

    fn snapshot(&self, queue_len: usize) -> RecorderStats {
        let r = |c: &AtomicU64| c.load(Ordering::Relaxed);
        let disk = r(&self.disk_free);
        RecorderStats {
            state: WriterState::from_code(self.state.load(Ordering::Acquire)),
            records_accepted: r(&self.accepted),
            records_written: r(&self.written),
            records_overrun: r(&self.overrun),
            records_dropped_write_failure: r(&self.dropped_write),
            records_dropped_disk_full: r(&self.dropped_disk),
            records_rejected: r(&self.rejected),
            notes_written: r(&self.notes),
            chunks_written: r(&self.chunks),
            bytes_written: r(&self.bytes),
            raw_bytes_written: r(&self.raw_bytes),
            segments_opened: r(&self.segments),
            syncs: r(&self.syncs),
            sync_time_total: Duration::from_nanos(r(&self.sync_ns_total)),
            sync_time_max: Duration::from_nanos(r(&self.sync_ns_max)),
            write_failures: r(&self.write_failures),
            index_errors: r(&self.index_errors),
            events_dropped: r(&self.events_dropped),
            control_dropped: r(&self.control_dropped),
            queue_len,
            queue_bytes: self.queued_bytes.load(Ordering::Relaxed),
            disk_free_bytes: (disk != DISK_FREE_UNKNOWN).then_some(disk),
            last_error: lock(&self.last_error).clone(),
        }
    }
}

/// Keeps `inflight` raised for the duration of one append.
struct InflightGuard<'a>(&'a AtomicUsize);

impl<'a> InflightGuard<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A queued producer record.
struct QueuedRecord {
    kind: RecordKind,
    capture_ns: u64,
    bytes: Vec<u8>,
}

/// Messages from the session handle to the writer.
enum Control {
    Lap(LapRecord),
    Stint(StintRecord),
    Event(EventRecord),
    Meta(SessionMetaUpdate),
    Rotate,
    Sync(Sender<SyncReport>),
    Finish,
    /// Test hook: the writer signals `entered`, then blocks until `release` receives or disconnects (makes
    /// queue overruns deterministic).
    #[cfg(test)]
    Stall {
        entered: Sender<()>,
        release: Receiver<()>,
    },
    /// Test hook: simulates a process crash. The writer stops at once without syncing, closing or updating
    /// the index; with `flush_pending` the pending chunk is first sealed and handed to the OS (unsynced),
    /// as if the crash hit right after that write.
    #[cfg(test)]
    Crash {
        flush_pending: bool,
    },
}

/// Cloneable, non-blocking append handle for producers on any thread.
#[derive(Clone)]
pub struct RecordSender {
    tx: Sender<QueuedRecord>,
    shared: Arc<Shared>,
    max_queue_bytes: usize,
}

impl fmt::Debug for RecordSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordSender")
            .field("queue_len", &self.tx.len())
            .finish_non_exhaustive()
    }
}

impl RecordSender {
    /// Queues one already-encoded record. Never blocks.
    ///
    /// Every call has exactly one outcome: queued (it will be written, or counted as lost if writing
    /// fails), [`AppendError::Overrun`] (dropped now, counted, reported as `RecorderOverrun` and noted in
    /// the recording), or another error (rejected and counted in [`RecorderStats::records_rejected`]).
    pub fn append(
        &self,
        kind: RecordKind,
        capture_mono_ns: u64,
        bytes: &[u8],
    ) -> Result<(), AppendError> {
        if !kind.is_producer_kind() {
            return self.reject(AppendError::ReservedKind(kind));
        }
        if bytes.len() > MAX_RECORD_BYTES {
            return self.reject(AppendError::TooLarge {
                len: bytes.len(),
                max: MAX_RECORD_BYTES,
            });
        }
        if capture_mono_ns > MAX_CAPTURE_NS {
            return self.reject(AppendError::CaptureTimeOutOfRange {
                capture_ns: capture_mono_ns,
                max: MAX_CAPTURE_NS,
            });
        }
        // Shutdown handshake (both sides SeqCst): either this load sees `closing` and the record is rejected,
        // or the writer's later read of `inflight` sees this append and waits for it to finish enqueueing
        // before its final drain. A record can therefore never be enqueued after the final drain.
        let _inflight = InflightGuard::enter(&self.shared.inflight);
        if self.shared.closing.load(Ordering::SeqCst) {
            return self.reject(AppendError::Stopped);
        }
        let len = bytes.len();
        let before = self.shared.queued_bytes.fetch_add(len, Ordering::AcqRel);
        // A record larger than the whole byte budget is still accepted into an empty queue.
        if before > 0 && before.saturating_add(len) > self.max_queue_bytes {
            self.shared.queued_bytes.fetch_sub(len, Ordering::AcqRel);
            self.shared.record_overrun(capture_mono_ns);
            return Err(AppendError::Overrun);
        }
        match self.tx.try_send(QueuedRecord {
            kind,
            capture_ns: capture_mono_ns,
            bytes: bytes.to_vec(),
        }) {
            Ok(()) => {
                self.shared.accepted.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.shared.queued_bytes.fetch_sub(len, Ordering::AcqRel);
                self.shared.record_overrun(capture_mono_ns);
                Err(AppendError::Overrun)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.queued_bytes.fetch_sub(len, Ordering::AcqRel);
                self.reject(AppendError::Stopped)
            }
        }
    }

    fn reject(&self, e: AppendError) -> Result<(), AppendError> {
        self.shared.rejected.fetch_add(1, Ordering::Relaxed);
        Err(e)
    }

    /// Current counters.
    pub fn stats(&self) -> RecorderStats {
        self.shared.snapshot(self.tx.len())
    }

    /// Records currently queued (cheap; for diagnostics such as the `rec.queue_depth` channel).
    pub fn queue_len(&self) -> usize {
        self.tx.len()
    }

    /// Capacity of the input queue in records.
    pub fn queue_capacity(&self) -> usize {
        self.tx.capacity().unwrap_or(usize::MAX)
    }
}

/// A recording in progress. Created by [`crate::RecordingStore::start_session`].
///
/// Dropping it without [`RecordingSession::finish`] still finalizes the session in the background (the
/// writer seals, syncs and closes the segment); if the process exits first, startup recovery marks the
/// session INTERRUPTED.
pub struct RecordingSession {
    id: SessionId,
    dir: PathBuf,
    sender: RecordSender,
    control: Sender<Control>,
    events: Receiver<RecorderEvent>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<SessionSummary>>,
}

impl fmt::Debug for RecordingSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordingSession")
            .field("id", &self.id)
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl RecordingSession {
    /// Session id (also in every segment header and the index).
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Absolute path of the session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Queues one record; see [`RecordSender::append`]. Never blocks.
    pub fn append(
        &self,
        kind: RecordKind,
        capture_mono_ns: u64,
        bytes: &[u8],
    ) -> Result<(), AppendError> {
        self.sender.append(kind, capture_mono_ns, bytes)
    }

    /// A cloneable append handle for other producer threads.
    pub fn sender(&self) -> RecordSender {
        self.sender.clone()
    }

    /// Events raised by the writer (see [`RecorderEvent`]).
    pub fn events(&self) -> &Receiver<RecorderEvent> {
        &self.events
    }

    /// Current counters.
    pub fn stats(&self) -> RecorderStats {
        self.sender.stats()
    }

    /// Indexes (or updates) a lap. Non-blocking. `metrics_json` must be valid JSON
    /// ([`AppendError::InvalidJson`] otherwise: the lap is not queued).
    pub fn record_lap(&self, lap: LapRecord) -> Result<(), AppendError> {
        check_ns(lap.start_capture_ns)?;
        check_opt_ns(lap.end_capture_ns)?;
        self.check_json("LapRecord::metrics_json", lap.metrics_json.as_deref())?;
        self.control(Control::Lap(lap))
    }

    /// Indexes (or updates) a stint. Non-blocking. `metrics_json` must be valid JSON
    /// ([`AppendError::InvalidJson`] otherwise: the stint is not queued).
    pub fn record_stint(&self, stint: StintRecord) -> Result<(), AppendError> {
        check_ns(stint.start_capture_ns)?;
        check_opt_ns(stint.end_capture_ns)?;
        self.check_json("StintRecord::metrics_json", stint.metrics_json.as_deref())?;
        self.control(Control::Stint(stint))
    }

    /// Indexes an event (alert, pit, flagâ€¦). Non-blocking. To also keep the event in the recording itself,
    /// append it as a [`RecordKind::SessionEvent`] record.
    pub fn record_event(&self, event: EventRecord) -> Result<(), AppendError> {
        check_opt_ns(event.capture_ns)?;
        self.control(Control::Event(event))
    }

    /// Updates session metadata that became known after the start (vehicle, trackâ€¦). Non-blocking.
    /// `data_availability_json` must be valid JSON ([`AppendError::InvalidJson`] otherwise: nothing of the
    /// update is applied).
    pub fn update_meta(&self, update: SessionMetaUpdate) -> Result<(), AppendError> {
        self.check_json(
            "SessionMetaUpdate::data_availability_json",
            update.data_availability_json.as_deref(),
        )?;
        self.control(Control::Meta(update))
    }

    /// Rejects (and counts in [`RecorderStats::control_dropped`]) a JSON field that is not valid JSON, on
    /// the caller's thread: queued, it would fail the index's `json_valid` check and the whole row would be
    /// lost as an index failure, and laps, stints and metadata cannot be rebuilt from the recording.
    fn check_json(&self, field: &'static str, json: Option<&str>) -> Result<(), AppendError> {
        let Some(json) = json else {
            return Ok(());
        };
        serde_json::from_str::<serde::de::IgnoredAny>(json).map_err(|e| {
            self.shared.control_dropped.fetch_add(1, Ordering::Relaxed);
            AppendError::InvalidJson {
                field,
                reason: e.to_string(),
            }
        })?;
        Ok(())
    }

    /// Closes the current segment after the records appended before this call; the next record opens a new
    /// one (e.g. when LMU's elapsed time jumps backwards). Non-blocking.
    pub fn request_rotation(&self) -> Result<(), AppendError> {
        self.control(Control::Rotate)
    }

    /// Makes every record appended before this call (from this thread, or from any thread whose appends
    /// happen-before it) durable: the writer processes them, seals the pending chunk, flushes and fsyncs, and
    /// indexes the new chunks. Waits up to `timeout` for each step. When the returned state is not
    /// [`WriterState::Recording`], the records went to the loss counters instead (see [`RecorderStats`]).
    pub fn sync(&self, timeout: Duration) -> Result<SyncReport, RecorderError> {
        let (tx, rx) = bounded(1);
        self.control
            .send_timeout(Control::Sync(tx), timeout)
            .map_err(|e| match e {
                SendTimeoutError::Timeout(_) => RecorderError::Timeout {
                    operation: "queueing a sync request",
                },
                SendTimeoutError::Disconnected(_) => RecorderError::WriterStopped,
            })?;
        rx.recv_timeout(timeout).map_err(|e| match e {
            RecvTimeoutError::Timeout => RecorderError::Timeout {
                operation: "waiting for the writer to sync",
            },
            RecvTimeoutError::Disconnected => RecorderError::WriterStopped,
        })
    }

    /// Finishes the session: drains the queue, seals, syncs, closes and validates the last segment, marks
    /// the session COMPLETE in the index and returns the summary. Blocks until the writer is done.
    pub fn finish(mut self) -> Result<SessionSummary, RecorderError> {
        let Some(thread) = self.thread.take() else {
            return Err(RecorderError::WriterStopped);
        };
        // Blocking send: the writer drains the control queue continuously, so this only waits while the
        // queue is momentarily full. If the writer is gone, `join` reports why.
        if self.control.send(Control::Finish).is_err() {
            tracing::warn!(component = COMPONENT, session = %self.id, "writer thread had already stopped");
        }
        thread.join().map_err(|_| RecorderError::WriterPanicked)
    }

    /// Test hook: blocks the writer until the returned sender is used or dropped. Returns once the writer is
    /// blocked.
    #[cfg(test)]
    pub(crate) fn stall_writer(&self) -> Sender<()> {
        let (entered_tx, entered_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        self.control
            .send(Control::Stall {
                entered: entered_tx,
                release: release_rx,
            })
            .expect("writer alive");
        entered_rx.recv().expect("writer entered the stall");
        release_tx
    }

    /// Test hook: simulates a process crash (see `Control::Crash`) and waits for the writer thread to stop.
    #[cfg(test)]
    pub(crate) fn simulate_crash(mut self, flush_pending: bool) {
        self.control
            .send(Control::Crash { flush_pending })
            .expect("writer alive");
        if let Some(thread) = self.thread.take() {
            thread.join().expect("writer thread");
        }
    }

    fn control(&self, c: Control) -> Result<(), AppendError> {
        self.control.try_send(c).map_err(|e| match e {
            TrySendError::Full(_) => {
                self.shared.control_dropped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(component = COMPONENT, session = %self.id, "recorder control queue full; message dropped");
                AppendError::ControlQueueFull
            }
            TrySendError::Disconnected(_) => AppendError::Stopped,
        })
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        if self.thread.is_some() {
            tracing::warn!(
                component = COMPONENT,
                session = %self.id,
                "recording session dropped without finish(); the writer finalizes it in the background"
            );
            // If the queue is full the writer still stops: dropping the control sender disconnects it.
            let _ = self.control.try_send(Control::Finish);
        }
    }
}

fn check_ns(ns: u64) -> Result<(), AppendError> {
    if ns > MAX_CAPTURE_NS {
        return Err(AppendError::CaptureTimeOutOfRange {
            capture_ns: ns,
            max: MAX_CAPTURE_NS,
        });
    }
    Ok(())
}

fn check_opt_ns(ns: Option<u64>) -> Result<(), AppendError> {
    ns.map_or(Ok(()), check_ns)
}

/// Runs a closure when dropped (the store uses it to release the session when the writer exits, even by
/// panic).
pub(crate) struct ExitGuard(Option<Box<dyn FnOnce() + Send>>);

impl ExitGuard {
    pub(crate) fn new(f: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(f)))
    }
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// Everything the store hands to a new session.
pub(crate) struct SessionSpawn {
    pub(crate) config: RecorderConfig,
    pub(crate) session_id: SessionId,
    /// Session directory relative to the recordings directory, `/`-separated.
    pub(crate) rel_dir: String,
    pub(crate) dir: PathBuf,
    pub(crate) app_version: String,
    pub(crate) schema_version: u16,
    /// The writer's own index connection.
    pub(crate) index: Index,
    pub(crate) options: SessionOptions,
    pub(crate) on_exit: ExitGuard,
}

/// Creates the first segment synchronously (so configuration problems surface as an error here) and starts
/// the writer thread.
pub(crate) fn spawn(s: SessionSpawn) -> Result<RecordingSession, RecorderError> {
    let (data_tx, data_rx) = bounded(s.config.queue_capacity_records);
    let (control_tx, control_rx) = bounded(s.config.control_queue_capacity);
    let (events_tx, events_rx) = bounded(s.config.event_queue_capacity);
    let shared = Arc::new(Shared::new());
    let id = s.session_id;
    let dir = s.dir.clone();
    let max_queue_bytes = s.config.queue_capacity_bytes;
    #[cfg(test)]
    let fail_thread_spawn = s.options.fail_thread_spawn;
    let mut writer = Writer::new(s, data_rx, control_rx, events_tx, Arc::clone(&shared))?;
    let now = Instant::now();
    writer
        .open_segment(now)
        .map_err(|e| RecorderError::io(e.op, e.path, e.source))?;
    let first_segment = writer.seg.as_ref().map(|seg| seg.path.clone());
    let builder = std::thread::Builder::new().name(format!("recorder-{}", &id.to_string()[..8]));
    #[cfg(test)]
    let started = if fail_thread_spawn {
        drop(writer);
        Err(io::Error::other("injected writer thread spawn failure"))
    } else {
        builder.spawn(move || writer.run())
    };
    #[cfg(not(test))]
    let started = builder.spawn(move || writer.run());
    // On failure the writer (and its segment file handle) has already been dropped.
    let thread = match started {
        Ok(thread) => thread,
        Err(e) => {
            if let Some(path) = first_segment {
                remove_unwritten_segment(&path);
            }
            return Err(RecorderError::SpawnWriter(e));
        }
    };
    tracing::info!(component = COMPONENT, session = %id, dir = %dir.display(), "recording session started");
    Ok(RecordingSession {
        id,
        dir,
        sender: RecordSender {
            tx: data_tx,
            shared: Arc::clone(&shared),
            max_queue_bytes,
        },
        control: control_tx,
        events: events_rx,
        shared,
        thread: Some(thread),
    })
}

/// Removes the first segment of a session whose writer thread never started. Only a file holding exactly
/// the file header (no record was ever written to it) is removed; anything else is left in place.
fn remove_unwritten_segment(path: &Path) {
    let header_only = fs::metadata(path).is_ok_and(|m| m.len() == FILE_HEADER_LEN as u64);
    if !header_only {
        tracing::warn!(component = COMPONENT, path = %path.display(), "segment of a failed session start is not header-only; left in place");
        return;
    }
    if let Err(e) = fs::remove_file(path) {
        tracing::warn!(component = COMPONENT, path = %path.display(), error = %e, "could not remove the segment of a failed session start");
    }
}

/// Flushes the write buffer and `sync_data`s the segment, accounting the wall time in the stats.
fn flush_and_sync(seg: &mut OpenSegment, shared: &Shared) -> io::Result<()> {
    let start = Instant::now();
    let result = seg.out.flush().and_then(|()| seg.out.get_mut().sync_data());
    shared.record_sync_time(start.elapsed());
    result
}

/// Why opening a segment failed.
struct SegmentOpenError {
    op: &'static str,
    path: PathBuf,
    source: io::Error,
}

/// A chunk whose write failed: its records are lost, its gap notes go back to the accumulators.
struct LostChunk {
    producer_records: u32,
    min_ns: u64,
    max_ns: u64,
    notes: Vec<(GapReason, GapAccumulator)>,
}

/// A chunk handed to the file but not yet covered by a successful sync.
struct UnsyncedChunk {
    entry: ChunkEntry,
    producer_records: u32,
    notes: Vec<(GapReason, GapAccumulator)>,
}

/// The segment being written.
struct OpenSegment {
    index: u32,
    rel_path: String,
    path: PathBuf,
    out: BufWriter<Box<dyn SegmentFile>>,
    opened_at: Instant,
    created_unix_ns: i64,
    created_mono_ns: Option<u64>,
    /// Bytes handed to `out` (file header included).
    written: u64,
    /// CRC-32C of bytes `[0, written)`.
    crc: u32,
    /// Bytes covered by the last successful sync.
    synced: u64,
    synced_crc: u32,
    synced_records: u64,
    synced_chunks: u32,
    next_seq: u32,
    /// Records (producer records and notes) in written chunks.
    records: u64,
    notes: u64,
    first_ns: Option<u64>,
    last_ns: Option<u64>,
    unsynced: Vec<UnsyncedChunk>,
    /// Written chunks not yet in the index (contiguous after the indexed prefix).
    unindexed: Vec<ChunkEntry>,
    /// False once index rows had to be abandoned for this segment (readers then walk chunk headers).
    index_rows_complete: bool,
}

impl OpenSegment {
    fn row(&self, session_id: SessionId, status: SegmentStatus) -> SegmentInfo {
        SegmentInfo {
            session_id,
            segment_index: self.index,
            path: self.rel_path.clone(),
            status,
            created_unix_ns: self.created_unix_ns,
            created_mono_ns: self.created_mono_ns,
            closed_unix_ns: None,
            byte_size: self.written,
            file_len: None,
            first_capture_ns: self.first_ns,
            last_capture_ns: self.last_ns,
            record_count: self.records,
            chunk_count: u64::from(self.next_seq),
            file_crc32c: Some(self.crc),
            integrity: IntegrityStatus::Unverified,
            integrity_detail: None,
        }
    }

    fn mark_synced(&mut self) {
        self.synced = self.written;
        self.synced_crc = self.crc;
        self.synced_records = self.records;
        self.synced_chunks = self.next_seq;
        self.unsynced.clear();
    }
}

/// Producer records dropped per reason, not yet written into the recording as notes.
#[derive(Debug, Default)]
struct Gaps {
    overrun: GapAccumulator,
    write_failure: GapAccumulator,
    disk_full: GapAccumulator,
}

impl Gaps {
    fn get_mut(&mut self, reason: GapReason) -> &mut GapAccumulator {
        match reason {
            GapReason::Overrun => &mut self.overrun,
            GapReason::WriteFailure => &mut self.write_failure,
            GapReason::DiskSpaceCritical => &mut self.disk_full,
        }
    }

    fn total(&self) -> u64 {
        self.overrun.dropped + self.write_failure.dropped + self.disk_full.dropped
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Recording,
    Failed { next_retry: Instant },
    Suspended,
}

/// Sets the shared state to Failed if the writer thread unwinds.
struct PanicGuard(Arc<Shared>);

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.closing.store(true, Ordering::SeqCst);
            self.0.set_state(WriterState::Failed);
            self.0
                .set_last_error(Some("recorder writer thread panicked".to_owned()));
        }
    }
}

struct Writer {
    cfg: RecorderConfig,
    session_id: SessionId,
    rel_dir: String,
    dir: PathBuf,
    app_version: String,
    schema_version: u16,
    index: Index,
    probe: Box<dyn DiskSpaceProbe>,
    clock: Option<Arc<dyn CaptureClock>>,
    files: Arc<dyn SegmentFileFactory>,
    data_rx: Receiver<QueuedRecord>,
    control_rx: Receiver<Control>,
    events_tx: Sender<RecorderEvent>,
    shared: Arc<Shared>,
    compressor: Compressor<'static>,
    chunk: ChunkBuilder,
    /// Gap notes inside `chunk` (re-queued if the chunk is lost).
    chunk_notes: Vec<(GapReason, GapAccumulator)>,
    seg: Option<OpenSegment>,
    next_segment_index: u32,
    /// Final rows of closed, failed or abandoned segments.
    closed: Vec<SegmentInfo>,
    mode: Mode,
    gaps: Gaps,
    overrun_taken: u64,
    overrun_unreported: GapAccumulator,
    last_overrun_report: Option<Instant>,
    last_retry_log: Option<Instant>,
    next_sync: Instant,
    next_disk_check: Instant,
    disk_low: bool,
    probe_failing: bool,
    index_failing: bool,
    _on_exit: ExitGuard,
}

impl Writer {
    fn new(
        s: SessionSpawn,
        data_rx: Receiver<QueuedRecord>,
        control_rx: Receiver<Control>,
        events_tx: Sender<RecorderEvent>,
        shared: Arc<Shared>,
    ) -> Result<Self, RecorderError> {
        let compressor = Compressor::new(s.config.zstd_level).map_err(RecorderError::Codec)?;
        let now = Instant::now();
        Ok(Self {
            next_sync: now + s.config.sync_interval,
            // The first free-space check runs immediately.
            next_disk_check: now,
            cfg: s.config,
            session_id: s.session_id,
            rel_dir: s.rel_dir,
            dir: s.dir,
            app_version: s.app_version,
            schema_version: s.schema_version,
            index: s.index,
            probe: s.options.disk_probe,
            clock: s.options.capture_clock,
            files: s.options.files,
            data_rx,
            control_rx,
            events_tx,
            shared,
            compressor,
            chunk: ChunkBuilder::new(),
            chunk_notes: Vec::new(),
            seg: None,
            next_segment_index: 0,
            closed: Vec::new(),
            mode: Mode::Recording,
            gaps: Gaps::default(),
            overrun_taken: 0,
            overrun_unreported: GapAccumulator::default(),
            last_overrun_report: None,
            last_retry_log: None,
            disk_low: false,
            probe_failing: false,
            index_failing: false,
            _on_exit: s.on_exit,
        })
    }

    fn run(mut self) -> SessionSummary {
        let _panic_guard = PanicGuard(Arc::clone(&self.shared));
        // Local handles so `select!` does not borrow `self` while the arms mutate it.
        let data_rx = self.data_rx.clone();
        let control_rx = self.control_rx.clone();
        loop {
            let now = Instant::now();
            let wait = self.next_deadline(now).saturating_duration_since(now);
            let mut finish = false;
            #[cfg(test)]
            let mut crash = None;
            select! {
                recv(data_rx) -> msg => match msg {
                    Ok(rec) => self.on_record(rec, Instant::now()),
                    // Every sender is gone, which implies the session handle is gone too.
                    Err(_) => finish = true,
                },
                recv(control_rx) -> msg => match msg {
                    Ok(Control::Finish) | Err(_) => finish = true,
                    #[cfg(test)]
                    Ok(Control::Crash { flush_pending }) => crash = Some(flush_pending),
                    Ok(other) => self.on_control(other, Instant::now()),
                },
                default(wait) => {},
            }
            #[cfg(test)]
            if let Some(flush_pending) = crash {
                return self.crash(flush_pending);
            }
            if finish {
                break;
            }
            self.on_tick(Instant::now());
        }
        self.finish()
    }

    fn next_deadline(&self, now: Instant) -> Instant {
        let mut deadline = (now + MAX_IDLE_WAIT).min(self.next_disk_check);
        if let Some(opened) = self.chunk.opened_at() {
            deadline = deadline.min(opened + self.cfg.chunk_max_age);
        }
        if self.seg.as_ref().is_some_and(|s| s.synced != s.written) {
            deadline = deadline.min(self.next_sync);
        }
        if let Mode::Failed { next_retry } = self.mode {
            deadline = deadline.min(next_retry);
        }
        deadline
    }

    fn on_tick(&mut self, now: Instant) {
        self.collect_overruns(now, false);
        match self.mode {
            Mode::Recording => {
                self.flush_gap_notes(now);
                if self
                    .chunk
                    .opened_at()
                    .is_some_and(|t| now.duration_since(t) >= self.cfg.chunk_max_age)
                {
                    self.seal_pending(now);
                }
                // The periodic sync does not seal the pending chunk, so chunks keep their ~1 s size; the
                // data at risk on a crash is bounded by chunk_max_age + sync_interval.
                if now >= self.next_sync && self.seg.as_ref().is_some_and(|s| s.synced != s.written)
                {
                    self.sync(now, false);
                    self.next_sync = now + self.cfg.sync_interval;
                }
            }
            Mode::Failed { next_retry } => {
                if now >= next_retry {
                    self.try_resume(now);
                }
            }
            Mode::Suspended => {}
        }
        if now >= self.next_disk_check {
            self.check_disk(now);
            self.next_disk_check = now + self.cfg.disk_check_interval;
        }
    }

    // ------------------------------------------------------------------------------------------------
    // Records and chunks
    // ------------------------------------------------------------------------------------------------

    fn on_record(&mut self, rec: QueuedRecord, now: Instant) {
        self.shared
            .queued_bytes
            .fetch_sub(rec.bytes.len(), Ordering::AcqRel);
        if self.mode == Mode::Recording && self.chunk_full_for(rec.bytes.len(), rec.capture_ns) {
            self.seal_pending(now);
        }
        match self.mode {
            Mode::Recording => {
                if let Err(e) = self.chunk.push(rec.kind, rec.capture_ns, &rec.bytes, now) {
                    // Unreachable in practice (`append` validated kind and size); counted, never silent.
                    tracing::error!(component = COMPONENT, session = %self.session_id, error = %e, "record refused by the chunk builder");
                    self.drop_records(GapReason::WriteFailure, 1, rec.capture_ns, rec.capture_ns);
                }
            }
            Mode::Failed { .. } => {
                self.drop_records(GapReason::WriteFailure, 1, rec.capture_ns, rec.capture_ns)
            }
            Mode::Suspended => self.drop_records(
                GapReason::DiskSpaceCritical,
                1,
                rec.capture_ns,
                rec.capture_ns,
            ),
        }
    }

    /// Whether the pending chunk must be sealed before a record of `payload_len` bytes at `capture_ns` is
    /// added: by raw size, or by capture-time span (a chunk covers less than `chunk_max_capture_span`, so
    /// chunk boundaries follow the data and do not depend on how fast the writer runs).
    fn chunk_full_for(&self, payload_len: usize, capture_ns: u64) -> bool {
        self.chunk
            .would_exceed(payload_len, self.cfg.chunk_max_raw_bytes)
            || self
                .chunk
                .would_exceed_span(capture_ns, self.cfg.chunk_max_capture_span_ns())
    }

    /// Counts producer records that will not be written (overruns are counted by `append` itself).
    fn drop_records(&mut self, reason: GapReason, n: u64, first_ns: u64, last_ns: u64) {
        self.gaps.get_mut(reason).add(n, first_ns, last_ns);
        match reason {
            GapReason::WriteFailure => self.shared.dropped_write.fetch_add(n, Ordering::Relaxed),
            GapReason::DiskSpaceCritical => {
                self.shared.dropped_disk.fetch_add(n, Ordering::Relaxed)
            }
            GapReason::Overrun => 0,
        };
    }

    /// Seals the pending chunk (if any) and writes it, opening a segment first when none is open.
    fn seal_pending(&mut self, now: Instant) {
        if self.chunk.is_empty() {
            return;
        }
        if self.mode != Mode::Recording {
            // Defensive: records are only buffered while recording.
            let lost = self.discard_pending();
            let reason = if self.mode == Mode::Suspended {
                GapReason::DiskSpaceCritical
            } else {
                GapReason::WriteFailure
            };
            self.drop_records(reason, lost.dropped, lost.first_ns, lost.last_ns);
            return;
        }
        if self.seg.is_none()
            && let Err(e) = self.open_segment(now)
        {
            self.fail(e.op, e.source, Some(e.path), None, now);
            return;
        }
        let Some(seg) = self.seg.as_mut() else { return };
        let Some(sealed) = self.chunk.seal(seg.next_seq, &mut self.compressor) else {
            return;
        };
        let notes = std::mem::take(&mut self.chunk_notes);
        self.write_sealed(sealed, notes, now);
    }

    /// Empties the chunk builder; returns its producer records as a gap and re-queues its gap notes.
    fn discard_pending(&mut self) -> GapAccumulator {
        let d = self.chunk.discard();
        for (reason, acc) in std::mem::take(&mut self.chunk_notes) {
            self.gaps.get_mut(reason).merge(acc);
        }
        let mut lost = GapAccumulator::default();
        if let Some((min, max)) = d.range {
            lost.add(u64::from(d.producer_records), min, max);
        }
        lost
    }

    fn write_sealed(
        &mut self,
        sealed: SealedChunk,
        notes: Vec<(GapReason, GapAccumulator)>,
        now: Instant,
    ) {
        let Some(seg) = self.seg.as_mut() else {
            // Unreachable: `seal_pending` opens a segment first. Counted, never silent.
            for (reason, acc) in notes {
                self.gaps.get_mut(reason).merge(acc);
            }
            let h = sealed.header;
            self.drop_records(
                GapReason::WriteFailure,
                u64::from(sealed.producer_records()),
                h.min_capture_ns,
                h.max_capture_ns,
            );
            return;
        };
        let h = sealed.header;
        let entry = ChunkEntry {
            chunk_seq: h.chunk_seq,
            offset: seg.written,
            body_len: h.body_len,
            record_count: h.record_count,
            min_capture_ns: h.min_capture_ns,
            max_capture_ns: h.max_capture_ns,
        };
        if let Err(e) = seg.out.write_all(&sealed.bytes) {
            let path = seg.path.clone();
            let lost = LostChunk {
                producer_records: sealed.producer_records(),
                min_ns: h.min_capture_ns,
                max_ns: h.max_capture_ns,
                notes,
            };
            self.fail("writing a chunk", e, Some(path), Some(lost), now);
            return;
        }
        let len = sealed.bytes.len() as u64;
        seg.crc = crc32c::crc32c_append(seg.crc, &sealed.bytes);
        seg.written += len;
        seg.next_seq += 1;
        seg.records += u64::from(h.record_count);
        seg.notes += u64::from(sealed.note_count);
        seg.first_ns = Some(
            seg.first_ns
                .map_or(h.min_capture_ns, |v| v.min(h.min_capture_ns)),
        );
        seg.last_ns = Some(
            seg.last_ns
                .map_or(h.max_capture_ns, |v| v.max(h.max_capture_ns)),
        );
        seg.unsynced.push(UnsyncedChunk {
            entry,
            producer_records: sealed.producer_records(),
            notes,
        });
        if seg.index_rows_complete {
            seg.unindexed.push(entry);
        }
        let rotate = seg.written >= self.cfg.segment_max_bytes
            || now.duration_since(seg.opened_at) >= self.cfg.segment_max_duration;
        self.shared.chunks.fetch_add(1, Ordering::Relaxed);
        self.shared.bytes.fetch_add(len, Ordering::Relaxed);
        self.shared
            .raw_bytes
            .fetch_add(u64::from(h.raw_len), Ordering::Relaxed);
        self.shared
            .written
            .fetch_add(u64::from(sealed.producer_records()), Ordering::Relaxed);
        self.shared
            .notes
            .fetch_add(u64::from(sealed.note_count), Ordering::Relaxed);
        if rotate {
            self.close_segment(now);
        }
    }

    /// Writes the pending gap accounts into the chunk being built as recorder notes.
    fn flush_gap_notes(&mut self, now: Instant) {
        for reason in [
            GapReason::Overrun,
            GapReason::WriteFailure,
            GapReason::DiskSpaceCritical,
        ] {
            if self.mode != Mode::Recording {
                return;
            }
            let acc = self.gaps.get_mut(reason).take();
            if acc.is_empty() {
                continue;
            }
            if self.chunk_full_for(GAP_NOTE_LEN, acc.first_ns) {
                self.seal_pending(now);
                if self.mode != Mode::Recording {
                    self.gaps.get_mut(reason).merge(acc);
                    return;
                }
            }
            let note = GapNote {
                reason,
                dropped_records: acc.dropped,
                first_capture_ns: Some(acc.first_ns),
                last_capture_ns: Some(acc.last_ns),
            };
            // The note sits at the start of the gap on the capture timeline; the chunk header is widened to
            // the end of the gap so that range reads starting inside the gap still find the note.
            match self
                .chunk
                .push(RecordKind::RecorderNote, acc.first_ns, &note.encode(), now)
            {
                Ok(()) => {
                    self.chunk.cover_capture_ns(acc.last_ns);
                    self.chunk_notes.push((reason, acc));
                }
                Err(e) => {
                    // Unreachable (fixed-size note, reserved kind); the counters still hold the gap.
                    tracing::error!(component = COMPONENT, session = %self.session_id, error = %e, "gap note refused by the chunk builder");
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------------------
    // Segments
    // ------------------------------------------------------------------------------------------------

    fn open_segment(&mut self, now: Instant) -> Result<(), SegmentOpenError> {
        fs::create_dir_all(&self.dir).map_err(|source| SegmentOpenError {
            op: "creating the session directory",
            path: self.dir.clone(),
            source,
        })?;
        let mut last_error = None;
        for _ in 0..SEGMENT_CREATE_ATTEMPTS {
            let index = self.next_segment_index;
            let name = segment_file_name(index);
            let path = self.dir.join(&name);
            match self.files.create_new(&path) {
                Ok(file) => {
                    self.next_segment_index = index.saturating_add(1);
                    return self.init_segment(index, &name, path, file, now);
                }
                Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                    // A stale file (e.g. from an attempt whose index entry was lost): never overwrite it.
                    tracing::warn!(component = COMPONENT, path = %path.display(), "segment file already exists; using the next index");
                    self.next_segment_index = index.saturating_add(1);
                    last_error = Some(SegmentOpenError {
                        op: "creating a segment file",
                        path,
                        source,
                    });
                }
                // Nothing was created: the same index is retried next time.
                Err(source) => {
                    return Err(SegmentOpenError {
                        op: "creating a segment file",
                        path,
                        source,
                    });
                }
            }
        }
        Err(last_error.unwrap_or_else(|| SegmentOpenError {
            op: "creating a segment file",
            path: self.dir.clone(),
            source: io::Error::new(ErrorKind::AlreadyExists, "no free segment file name"),
        }))
    }

    fn init_segment(
        &mut self,
        index: u32,
        name: &str,
        path: PathBuf,
        file: Box<dyn SegmentFile>,
        now: Instant,
    ) -> Result<(), SegmentOpenError> {
        let created_unix_ns = unix_now_ns();
        let created_mono_ns = self.clock.as_ref().map(|c| c.now_ns());
        let header = FileHeader {
            format_version: FORMAT_VERSION,
            telemetry_schema_version: self.schema_version,
            session_id: self.session_id.0,
            segment_index: index,
            created_unix_ns,
            created_mono_ns: created_mono_ns.unwrap_or(MONO_NS_UNKNOWN),
            app_version: self.app_version.clone(),
        };
        let bytes = header.encode().map_err(|e| SegmentOpenError {
            op: "encoding a segment header",
            path: path.clone(),
            source: io::Error::new(ErrorKind::InvalidInput, e),
        })?;
        let mut out = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
        // The header is made durable at once, so an existing segment file always starts with a valid header
        // unless the crash hit this very write.
        let written = out
            .write_all(&bytes)
            .and_then(|()| out.flush())
            .and_then(|()| out.get_mut().sync_data());
        if let Err(source) = written {
            let (file, _unflushed) = out.into_parts();
            drop(file);
            // The file holds no records; remove it so it does not linger as a damaged segment.
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(component = COMPONENT, path = %path.display(), error = %e, "could not remove a segment file whose header failed to write");
            }
            return Err(SegmentOpenError {
                op: "writing a segment header",
                path,
                source,
            });
        }
        let crc = crc32c::crc32c(&bytes);
        let seg = OpenSegment {
            index,
            rel_path: format!("{}/{name}", self.rel_dir),
            path,
            out,
            opened_at: now,
            created_unix_ns,
            created_mono_ns,
            written: FILE_HEADER_LEN as u64,
            crc,
            synced: FILE_HEADER_LEN as u64,
            synced_crc: crc,
            synced_records: 0,
            synced_chunks: 0,
            next_seq: 0,
            records: 0,
            notes: 0,
            first_ns: None,
            last_ns: None,
            unsynced: Vec::new(),
            unindexed: Vec::new(),
            index_rows_complete: true,
        };
        let row = seg.row(self.session_id, SegmentStatus::Open);
        let r = self.index.put_segment(&row, ChunkRows::Replace(Vec::new()));
        self.index_result("registering a segment", r);
        self.shared.segments.fetch_add(1, Ordering::Relaxed);
        tracing::info!(component = COMPONENT, session = %self.session_id, segment = index, path = %seg.path.display(), "segment opened");
        self.emit(RecorderEvent::SegmentOpened {
            segment_index: index,
            path: seg.path.clone(),
        });
        self.seg = Some(seg);
        Ok(())
    }

    /// Seals nothing (callers seal first); flushes, syncs, closes and validates the current segment.
    fn close_segment(&mut self, now: Instant) {
        let Some(mut seg) = self.seg.take() else {
            return;
        };
        if seg.synced != seg.written {
            if let Err(e) = flush_and_sync(&mut seg, &self.shared) {
                let path = seg.path.clone();
                self.seg = Some(seg);
                self.fail(
                    "syncing a segment before closing it",
                    e,
                    Some(path),
                    None,
                    now,
                );
                return;
            }
            self.shared.syncs.fetch_add(1, Ordering::Relaxed);
            seg.mark_synced();
        }
        let mut row = seg.row(self.session_id, SegmentStatus::Closed);
        row.closed_unix_ns = Some(unix_now_ns());
        row.file_len = Some(seg.written);
        let OpenSegment {
            out,
            path,
            index,
            written,
            crc,
            records,
            unindexed,
            index_rows_complete,
            ..
        } = seg;
        // Everything was flushed above, so the buffer is empty.
        let (file, _empty_buffer) = out.into_parts();
        drop(file);
        let unindexed = if index_rows_complete {
            ChunkRows::Append(unindexed)
        } else {
            ChunkRows::Keep
        };
        let rows = if self.cfg.validate_on_close {
            // Re-reads what was just written (through the OS cache): verifies the whole encode path
            // end to end. It cannot detect media errors, which surface on later reads.
            let a = assess_segment(&path, self.session_id, index);
            match a.scan() {
                Some(scan)
                    if scan.stop == ScanStop::CleanEof
                        && scan.valid_len == written
                        && scan.valid_crc32c == crc
                        && scan.record_count == records =>
                {
                    row.integrity = IntegrityStatus::Ok;
                    row.file_len = Some(scan.file_len);
                    ChunkRows::Replace(scan.chunks.iter().map(ChunkEntry::from).collect())
                }
                Some(scan) => {
                    let mismatch = format!(
                        "validation after close disagrees with the writer: wrote {written} bytes, {records} records, \
                         CRC-32C {crc:#010x}; the file has {} valid bytes, {} records, CRC-32C {:#010x}",
                        scan.valid_len, scan.record_count, scan.valid_crc32c
                    );
                    tracing::error!(component = COMPONENT, session = %self.session_id, segment = index, "{mismatch}");
                    let (mut r, rows) = apply_assessment(row, &a);
                    r.integrity = IntegrityStatus::Damaged;
                    r.integrity_detail = Some(match r.integrity_detail.take() {
                        Some(d) => format!("{mismatch}; {d}"),
                        None => mismatch,
                    });
                    row = r;
                    rows
                }
                None => {
                    let unreadable = matches!(a.outcome, Outcome::Unreadable);
                    let (r, rows) = apply_assessment(row, &a);
                    row = r;
                    if unreadable { unindexed } else { rows }
                }
            }
        } else {
            unindexed
        };
        let integrity = row.integrity;
        let r = self.index.put_segment(&row, rows);
        self.index_result("closing a segment", r);
        tracing::info!(
            component = COMPONENT,
            session = %self.session_id,
            segment = row.segment_index,
            bytes = row.byte_size,
            records = row.record_count,
            integrity = %integrity,
            "segment closed"
        );
        self.emit(RecorderEvent::SegmentClosed {
            segment_index: row.segment_index,
            bytes: row.byte_size,
            records: row.record_count,
            integrity,
        });
        self.closed.push(row);
    }

    /// Flush + `sync_data`, then record the newly durable chunks in the index. With `seal_first` the pending
    /// chunk is sealed and written first (explicit sync requests), so everything appended so far is durable.
    fn sync(&mut self, now: Instant, seal_first: bool) {
        if seal_first {
            self.seal_pending(now);
        }
        let Some(seg) = self.seg.as_mut() else { return };
        if seg.synced == seg.written {
            return;
        }
        if let Err(e) = flush_and_sync(seg, &self.shared) {
            let path = seg.path.clone();
            self.fail("syncing a segment", e, Some(path), None, now);
            return;
        }
        seg.mark_synced();
        self.shared.syncs.fetch_add(1, Ordering::Relaxed);
        self.write_progress();
    }

    fn write_progress(&mut self) {
        let session = self.session_progress();
        let Some(seg) = self.seg.as_mut() else { return };
        let progress = SegmentProgress {
            byte_size: seg.synced,
            crc32c: seg.synced_crc,
            first_capture_ns: seg.first_ns,
            last_capture_ns: seg.last_ns,
            record_count: seg.synced_records,
            chunk_count: u64::from(seg.synced_chunks),
        };
        let result = self.index.record_progress(
            self.session_id,
            seg.index,
            &progress,
            &session,
            &seg.unindexed,
        );
        if result.is_ok() {
            seg.unindexed.clear();
        } else if seg.unindexed.len() > MAX_PENDING_INDEX_CHUNKS {
            // From now on only the counters are retried; the chunk rows are rebuilt when the segment closes
            // (validation) or by recovery, and readers walk chunk headers meanwhile.
            tracing::warn!(
                component = COMPONENT,
                session = %self.session_id,
                segment = seg.index,
                "index unavailable for too long; chunk rows for this segment are abandoned (readers walk chunk headers)"
            );
            seg.unindexed.clear();
            seg.index_rows_complete = false;
        }
        // On failure the pending rows stay queued and are retried at the next sync.
        self.index_result("recording sync progress", result);
    }

    fn session_progress(&self) -> SessionProgress {
        let mut first: Option<u64> = None;
        let mut last: Option<u64> = None;
        let mut records = 0u64;
        let current = self
            .seg
            .as_ref()
            .map(|s| (s.first_ns, s.last_ns, s.synced_records));
        for (f, l, n) in self
            .closed
            .iter()
            .map(|s| (s.first_capture_ns, s.last_capture_ns, s.record_count))
            .chain(current)
        {
            first = match (first, f) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            last = last.max(l);
            records += n;
        }
        SessionProgress {
            first_capture_ns: first,
            last_capture_ns: last,
            record_count: records,
            dropped_records: self.shared.lost(),
        }
    }

    // ------------------------------------------------------------------------------------------------
    // Failure, retry and disk space
    // ------------------------------------------------------------------------------------------------

    fn fail(
        &mut self,
        op: &'static str,
        err: io::Error,
        path: Option<PathBuf>,
        lost_chunk: Option<LostChunk>,
        now: Instant,
    ) {
        let mut lost = GapAccumulator::default();
        if let Some(c) = lost_chunk {
            lost.add(u64::from(c.producer_records), c.min_ns, c.max_ns);
            for (reason, acc) in c.notes {
                self.gaps.get_mut(reason).merge(acc);
            }
        }
        lost.merge(self.discard_pending());
        let segment_index = self.seg.as_ref().map(|s| s.index);
        if let Some(seg) = self.seg.take() {
            lost.merge(self.abandon_segment(seg, &err));
        }
        self.gaps.write_failure.merge(lost);
        self.shared
            .dropped_write
            .fetch_add(lost.dropped, Ordering::Relaxed);
        self.shared.write_failures.fetch_add(1, Ordering::Relaxed);
        self.mode = Mode::Failed {
            next_retry: now + self.cfg.failure_retry_interval,
        };
        self.shared.set_state(WriterState::Failed);
        let message = err.to_string();
        self.shared.set_last_error(Some(format!("{op}: {message}")));
        tracing::error!(
            component = COMPONENT,
            session = %self.session_id,
            segment = ?segment_index,
            path = ?path,
            op,
            error = %err,
            os_error = ?err.raw_os_error(),
            lost_records = lost.dropped,
            "recording write failed; records are dropped and counted until a new segment can be opened"
        );
        let where_ = path
            .as_ref()
            .map(|p| format!(" ({})", p.display()))
            .unwrap_or_default();
        self.emit(RecorderEvent::RecordingFailed {
            segment_index,
            path,
            operation: op,
            error: message.clone(),
            os_error: err.raw_os_error(),
            kind: err.kind(),
        });
        self.health_event(
            Severity::Critical,
            None,
            format!(
                "recording failed while {op}{where_}: {message}; {} records lost so far; records are dropped and \
                 counted until a new segment can be opened",
                lost.dropped
            ),
        );
    }

    /// Abandons a segment after a failure without writing anything more to it, establishes by validation
    /// what actually reached the file, records the segment as FAILED, and returns the producer records that
    /// were handed to the file but are not in it.
    fn abandon_segment(&mut self, seg: OpenSegment, err: &io::Error) -> GapAccumulator {
        let mut row = seg.row(self.session_id, SegmentStatus::Failed);
        row.closed_unix_ns = Some(unix_now_ns());
        let OpenSegment {
            out,
            path,
            index,
            records,
            notes,
            first_ns,
            last_ns,
            synced,
            synced_crc,
            synced_records,
            synced_chunks,
            unsynced,
            unindexed,
            index_rows_complete,
            ..
        } = seg;
        // `into_parts` never flushes: whatever is still buffered is discarded with the handle.
        let (file, _unflushed) = out.into_parts();
        drop(file);
        let a = assess_segment(&path, self.session_id, index);
        let valid_end = a.scan().map(|s| s.valid_len);
        let mut lost = GapAccumulator::default();
        let mut lost_notes = 0u64;
        for u in unsynced {
            // Unverifiable (no scan): every unsynced chunk is counted as lost (conservative).
            if valid_end.is_some_and(|end| u.entry.end() <= end) {
                continue;
            }
            lost.add(
                u64::from(u.producer_records),
                u.entry.min_capture_ns,
                u.entry.max_capture_ns,
            );
            lost_notes += u64::from(u.entry.record_count - u.producer_records);
            for (reason, acc) in u.notes {
                self.gaps.get_mut(reason).merge(acc);
            }
        }
        if let Some(scan) = a.scan() {
            // Synced data that did not survive (storage acknowledged a sync it did not perform, or damage).
            let written_producer = records - notes;
            let missing = written_producer
                .saturating_sub(scan.producer_record_count())
                .saturating_sub(lost.dropped);
            if missing > 0
                && let (Some(first), Some(last)) = (first_ns, last_ns)
            {
                lost.add(missing, first, last);
            }
        }
        // Every lost record had been counted as written; move it to the loss counters.
        self.shared.written.fetch_sub(
            lost.dropped
                .min(self.shared.written.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        self.shared.notes.fetch_sub(
            lost_notes.min(self.shared.notes.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );

        let (mut row, rows) = match a.outcome {
            Outcome::Unreadable => {
                // Nothing could be verified: the index keeps what was known durable before the failure.
                row.byte_size = synced;
                row.file_crc32c = Some(synced_crc);
                row.record_count = synced_records;
                row.chunk_count = u64::from(synced_chunks);
                row.integrity = a.integrity;
                row.integrity_detail = a.detail.clone();
                let rows = if index_rows_complete {
                    ChunkRows::Append(
                        unindexed
                            .into_iter()
                            .filter(|c| c.end() <= synced)
                            .collect(),
                    )
                } else {
                    ChunkRows::Keep
                };
                (row, rows)
            }
            _ => apply_assessment(row, &a),
        };
        let cause = format!("abandoned after a write failure ({err})");
        row.integrity_detail = Some(match row.integrity_detail.take() {
            Some(d) => format!("{cause}; {d}"),
            None => cause,
        });
        let r = self.index.put_segment(&row, rows);
        self.index_result("recording a failed segment", r);
        self.closed.push(row);
        lost
    }

    fn try_resume(&mut self, now: Instant) {
        if let Some(available) = self.measure_disk()
            && available < self.cfg.disk_critical_free_bytes
        {
            // Space ran out meanwhile: switch to the disk-space policy, which resumes when space returns.
            self.enter_suspended(available);
            return;
        }
        match self.open_segment(now) {
            Ok(()) => {
                let dropped = self.gaps.write_failure.dropped;
                let segment_index = self.seg.as_ref().map_or(0, |s| s.index);
                self.mode = Mode::Recording;
                self.shared.set_state(WriterState::Recording);
                self.shared.set_last_error(None);
                tracing::info!(component = COMPONENT, session = %self.session_id, segment = segment_index, dropped, "recording resumed after a write failure");
                self.emit(RecorderEvent::RecordingResumed {
                    segment_index,
                    dropped_records: dropped,
                });
                self.health_event(
                    Severity::Info,
                    None,
                    format!("recording resumed in segment {segment_index}; {dropped} records were dropped while writing was failing"),
                );
                self.flush_gap_notes(now);
            }
            Err(e) => {
                if self
                    .last_retry_log
                    .is_none_or(|t| now.duration_since(t) >= RETRY_LOG_INTERVAL)
                {
                    self.last_retry_log = Some(now);
                    tracing::warn!(component = COMPONENT, session = %self.session_id, op = e.op, path = %e.path.display(), error = %e.source, "still unable to open a new segment; retrying");
                }
                self.shared
                    .set_last_error(Some(format!("{}: {}", e.op, e.source)));
                self.mode = Mode::Failed {
                    next_retry: now + self.cfg.failure_retry_interval,
                };
            }
        }
    }

    fn measure_disk(&mut self) -> Option<u64> {
        match self.probe.available_bytes(&self.cfg.recordings_dir) {
            Ok(available) => {
                self.shared.disk_free.store(available, Ordering::Relaxed);
                self.probe_failing = false;
                Some(available)
            }
            Err(e) => {
                self.shared
                    .disk_free
                    .store(DISK_FREE_UNKNOWN, Ordering::Relaxed);
                if !self.probe_failing {
                    self.probe_failing = true;
                    tracing::warn!(component = COMPONENT, session = %self.session_id, error = %e, "cannot measure free disk space; recording continues");
                }
                None
            }
        }
    }

    fn check_disk(&mut self, now: Instant) {
        let Some(available) = self.measure_disk() else {
            if self.mode == Mode::Suspended {
                // Free space cannot be measured any more: resume and rely on the write-failure path (which
                // reports and retries) rather than stay suspended indefinitely.
                self.resume_from_suspension(None);
            }
            return;
        };
        if available < self.cfg.disk_warn_free_bytes {
            // Raised once per low-space episode, before any critical-level action.
            if !self.disk_low {
                self.disk_low = true;
                let threshold = self.cfg.disk_warn_free_bytes;
                tracing::warn!(component = COMPONENT, session = %self.session_id, available, threshold, "free disk space is low");
                self.emit(RecorderEvent::DiskSpaceLow {
                    available_bytes: available,
                    threshold_bytes: threshold,
                });
                self.health_event(
                    Severity::Warning,
                    None,
                    format!(
                        "free disk space is low: {available} bytes (warning threshold {threshold})"
                    ),
                );
            }
        } else {
            self.disk_low = false;
        }
        if available < self.cfg.disk_critical_free_bytes {
            if self.mode == Mode::Recording {
                // Close the segment cleanly (seal, sync, validate) before suspending.
                self.seal_pending(now);
                if self.mode == Mode::Recording {
                    self.close_segment(now);
                }
                if self.mode == Mode::Recording {
                    self.enter_suspended(available);
                }
            }
        } else if self.mode == Mode::Suspended && available >= self.cfg.disk_resume_free_bytes {
            self.resume_from_suspension(Some(available));
        }
    }

    /// Leaves the disk-space suspension. `available` is `None` when free space could not be measured. The
    /// next chunk opens a new segment; the gap note is written on the next tick.
    fn resume_from_suspension(&mut self, available: Option<u64>) {
        self.mode = Mode::Recording;
        self.shared.set_state(WriterState::Recording);
        self.shared.set_last_error(None);
        let dropped = self.gaps.disk_full.dropped;
        let space = available.map_or_else(
            || "unknown (free space cannot be measured)".to_owned(),
            |a| format!("{a} bytes"),
        );
        tracing::info!(component = COMPONENT, session = %self.session_id, available = ?available, dropped, "recording resumes after a disk-space suspension");
        self.emit(RecorderEvent::DiskSpaceRecovered {
            available_bytes: available,
        });
        self.health_event(
            Severity::Info,
            None,
            format!("recording resumes; free disk space {space}; {dropped} records were dropped while suspended"),
        );
    }

    fn enter_suspended(&mut self, available: u64) {
        let threshold = self.cfg.disk_critical_free_bytes;
        self.mode = Mode::Suspended;
        self.shared.set_state(WriterState::Suspended);
        self.shared.set_last_error(Some(format!(
            "recording suspended: {available} bytes free, below the critical threshold of {threshold}"
        )));
        tracing::error!(component = COMPONENT, session = %self.session_id, available, threshold, "free disk space critical; recording suspended");
        self.emit(RecorderEvent::DiskSpaceCritical {
            available_bytes: available,
            threshold_bytes: threshold,
        });
        self.health_event(
            Severity::Critical,
            None,
            format!(
                "free disk space critical ({available} bytes, threshold {threshold}): recording suspended; records are \
                 dropped and counted until space recovers"
            ),
        );
    }

    // ------------------------------------------------------------------------------------------------
    // Overruns, control messages, events
    // ------------------------------------------------------------------------------------------------

    fn collect_overruns(&mut self, now: Instant, force_report: bool) {
        if self.shared.overrun.load(Ordering::Relaxed) != self.overrun_taken {
            let acc = self.shared.take_overruns();
            self.overrun_taken += acc.dropped;
            self.gaps.overrun.merge(acc);
            self.overrun_unreported.merge(acc);
        }
        let due = force_report
            || self
                .last_overrun_report
                .is_none_or(|t| now.duration_since(t) >= OVERRUN_REPORT_INTERVAL);
        if self.overrun_unreported.is_empty() || !due {
            return;
        }
        let acc = self.overrun_unreported.take();
        self.last_overrun_report = Some(now);
        let total = self.shared.overrun.load(Ordering::Relaxed);
        tracing::warn!(
            component = COMPONENT,
            session = %self.session_id,
            dropped = acc.dropped,
            total,
            first_capture_ns = acc.first_ns,
            last_capture_ns = acc.last_ns,
            "recorder overrun: input queue full, records dropped"
        );
        let (first_capture_ns, last_capture_ns) = acc.range();
        self.emit(RecorderEvent::RecorderOverrun {
            dropped: acc.dropped,
            total_dropped: total,
            first_capture_ns,
            last_capture_ns,
        });
        self.health_event(
            Severity::Warning,
            first_capture_ns,
            format!(
                "recorder overrun: {} records dropped (input queue full) between capture ns {} and {}; {total} in total",
                acc.dropped, acc.first_ns, acc.last_ns
            ),
        );
    }

    fn on_control(&mut self, c: Control, now: Instant) {
        match c {
            Control::Lap(lap) => {
                let r = self.index.put_lap(self.session_id, &lap);
                self.index_result("writing a lap", r);
            }
            Control::Stint(stint) => {
                let r = self.index.put_stint(self.session_id, &stint);
                self.index_result("writing a stint", r);
            }
            Control::Event(event) => {
                let r = self.index.insert_event(self.session_id, &event);
                self.index_result("writing an event", r);
            }
            Control::Meta(update) => {
                let r = self.index.update_session_meta(self.session_id, &update);
                self.index_result("updating session metadata", r);
            }
            Control::Rotate => {
                self.process_queued_records(now);
                if self.mode == Mode::Recording {
                    self.seal_pending(now);
                    if self.mode == Mode::Recording
                        && self.seg.as_ref().is_some_and(|s| s.next_seq > 0)
                    {
                        self.close_segment(now);
                    }
                }
            }
            Control::Sync(ack) => {
                self.process_queued_records(now);
                if self.mode == Mode::Recording {
                    self.collect_overruns(now, false);
                    self.flush_gap_notes(now);
                    self.sync(now, true);
                }
                let report = SyncReport {
                    state: WriterState::from_code(self.shared.state.load(Ordering::Acquire)),
                    segment_index: self.seg.as_ref().map(|s| s.index),
                    durable_bytes: self.seg.as_ref().map_or(0, |s| s.synced),
                    records_written: self.shared.written.load(Ordering::Relaxed),
                };
                // The requester may have timed out and gone; nothing to do then.
                let _ = ack.send(report);
            }
            // Handled by the run loop.
            Control::Finish => {}
            #[cfg(test)]
            Control::Crash { .. } => {}
            #[cfg(test)]
            Control::Stall { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }
    }

    /// Processes the records queued when a control message arrives, so `sync`/`rotate` cover everything the
    /// caller appended before sending it (the two queues are otherwise unordered with respect to each
    /// other). Bounded by the queue length at entry, so it terminates even while producers keep appending.
    fn process_queued_records(&mut self, now: Instant) {
        for _ in 0..self.data_rx.len() {
            match self.data_rx.try_recv() {
                Ok(rec) => self.on_record(rec, now),
                Err(_) => break,
            }
        }
    }

    fn emit(&self, event: RecorderEvent) {
        if self.events_tx.try_send(event).is_err() {
            self.shared.events_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Indexes a recorder-health event. The capture time is the given one, else the capture clock's, else
    /// unknown (`None`); the wall-clock time is always recorded.
    fn health_event(&mut self, severity: Severity, capture_ns: Option<u64>, description: String) {
        let capture_ns = capture_ns
            .or_else(|| self.clock.as_ref().map(|c| c.now_ns()))
            .filter(|&ns| ns <= MAX_CAPTURE_NS);
        let event = EventRecord {
            capture_ns,
            unix_ns: Some(unix_now_ns()),
            lap: None,
            category: EventCategory::Recorder,
            severity,
            description,
            channel_id: None,
            value: None,
            threshold: None,
        };
        let r = self.index.insert_event(self.session_id, &event);
        self.index_result("recording a recorder event", r);
    }

    fn index_result<T>(&mut self, op: &'static str, r: Result<T, IndexError>) -> Option<T> {
        match r {
            Ok(v) => {
                self.index_failing = false;
                Some(v)
            }
            Err(e) => {
                self.shared.index_errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(component = COMPONENT, session = %self.session_id, op, error = %e, "index update failed; the recording itself is unaffected");
                if !self.index_failing {
                    self.index_failing = true;
                    self.emit(RecorderEvent::IndexWriteFailed {
                        operation: op,
                        error: e.to_string(),
                    });
                }
                None
            }
        }
    }

    // ------------------------------------------------------------------------------------------------
    // Finish
    // ------------------------------------------------------------------------------------------------

    fn finish(mut self) -> SessionSummary {
        // Shutdown handshake, writer side (see `RecordSender::append`).
        self.shared.closing.store(true, Ordering::SeqCst);
        while self.shared.inflight.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
        let now = Instant::now();
        while let Ok(rec) = self.data_rx.try_recv() {
            self.on_record(rec, now);
        }
        // Control messages sent before the session ended (the loop can also end on a disconnected data
        // queue while laps or events are still queued).
        while let Ok(c) = self.control_rx.try_recv() {
            self.on_control(c, now);
        }
        self.collect_overruns(now, true);
        if matches!(self.mode, Mode::Failed { .. }) {
            // One last attempt, so pending gap notes can still reach the recording.
            self.try_resume(now);
        }
        if self.mode == Mode::Recording {
            self.flush_gap_notes(now);
            self.seal_pending(now);
            self.close_segment(now);
        }
        if let Some(seg) = self.seg.take() {
            // Only reachable if closing failed and left nothing open; kept defensive.
            let e = io::Error::other("session finished with a segment still open");
            let lost = self.abandon_segment(seg, &e);
            self.drop_records(
                GapReason::WriteFailure,
                lost.dropped,
                lost.first_ns,
                lost.last_ns,
            );
        }
        let unnoted = self.gaps.total();
        if unnoted > 0 {
            // Writing was unavailable at the end: the gap is in the counters, the index and the log.
            tracing::warn!(component = COMPONENT, session = %self.session_id, dropped = unnoted, "dropped records could not be noted in the recording");
            self.health_event(
                Severity::Warning,
                None,
                format!("{unnoted} dropped records could not be noted in the recording because writing was unavailable at the end of the session"),
            );
        }

        if self.shared.index_errors.load(Ordering::Relaxed) > 0 {
            self.rebuild_index_rows();
        }

        let segments = std::mem::take(&mut self.closed);
        let mut integrity = IntegrityStatus::Ok;
        let mut details = Vec::new();
        for s in &segments {
            integrity = integrity.worst(s.integrity);
            if let Some(d) = &s.integrity_detail {
                details.push(format!("segment {}: {d}", s.segment_index));
            }
        }
        let integrity_detail = (!details.is_empty()).then(|| details.join("; "));
        let record_count = segments.iter().map(|s| s.record_count).sum();
        let first_capture_ns = segments.iter().filter_map(|s| s.first_capture_ns).min();
        let last_capture_ns = segments.iter().filter_map(|s| s.last_capture_ns).max();
        let checksum_crc32c = session_checksum(&segments);
        let end = SessionEnd {
            status: SessionStatus::Complete,
            ended_unix_ns: Some(unix_now_ns()),
            recovered_unix_ns: None,
            first_capture_ns,
            last_capture_ns,
            record_count,
            dropped_records: self.shared.lost(),
            integrity,
            integrity_detail: integrity_detail.clone(),
            checksum_crc32c: Some(checksum_crc32c),
        };
        let index_updated = match self.index.end_session(self.session_id, &end) {
            Ok(()) => true,
            Err(e) => {
                self.shared.index_errors.fetch_add(1, Ordering::Relaxed);
                tracing::error!(component = COMPONENT, session = %self.session_id, error = %e, "could not mark the session COMPLETE; startup recovery will finish it");
                false
            }
        };
        self.shared.set_state(WriterState::Finished);
        let stats = self.shared.snapshot(self.data_rx.len());
        tracing::info!(
            component = COMPONENT,
            session = %self.session_id,
            records = record_count,
            written = stats.records_written,
            lost = stats.records_lost(),
            bytes = stats.bytes_written,
            integrity = %integrity,
            "recording session finished"
        );
        SessionSummary {
            session_id: self.session_id,
            dir: self.dir.clone(),
            integrity,
            integrity_detail,
            segments,
            record_count,
            first_capture_ns,
            last_capture_ns,
            checksum_crc32c,
            stats,
            writer_cpu_time: current_thread_cpu_time(),
            index_updated,
        }
    }

    /// After index write failures during the session, rebuilds every segment row and its chunk rows from the
    /// files (the files are the source of truth), keeping the worst integrity verdict seen.
    fn rebuild_index_rows(&mut self) {
        let mut rows = std::mem::take(&mut self.closed);
        for row in &mut rows {
            let path = self.cfg.resolve(Path::new(&row.path));
            let a = assess_segment(&path, self.session_id, row.segment_index);
            let (mut rebuilt, chunk_rows) = apply_assessment(row.clone(), &a);
            rebuilt.status = row.status;
            rebuilt.integrity = row.integrity.worst(a.integrity);
            rebuilt.integrity_detail = match (row.integrity_detail.take(), a.detail.clone()) {
                (Some(old), Some(new)) if old != new => Some(format!("{old}; {new}")),
                (old, new) => old.or(new),
            };
            let r = self.index.put_segment(&rebuilt, chunk_rows);
            self.index_result("rebuilding segment rows after index failures", r);
            *row = rebuilt;
        }
        self.closed = rows;
    }

    /// Test hook (see `Control::Crash`): stops without syncing, closing or touching the index. Whatever was
    /// handed to the OS stays in the file; the write buffer is discarded.
    #[cfg(test)]
    fn crash(mut self, flush_pending: bool) -> SessionSummary {
        self.shared.closing.store(true, Ordering::SeqCst);
        if flush_pending {
            if let Some(seg) = self.seg.as_mut()
                && let Some(sealed) = self.chunk.seal(seg.next_seq, &mut self.compressor)
            {
                seg.out.write_all(&sealed.bytes).expect("test crash: write");
                seg.next_seq += 1;
            }
            if let Some(seg) = self.seg.as_mut() {
                seg.out.flush().expect("test crash: flush to the OS");
            }
        }
        if let Some(seg) = self.seg.take() {
            // `into_parts` never flushes: anything still buffered is lost, as in a real crash.
            let (file, _lost) = seg.out.into_parts();
            drop(file);
        }
        self.shared.set_state(WriterState::Finished);
        let stats = self.shared.snapshot(self.data_rx.len());
        SessionSummary {
            session_id: self.session_id,
            dir: self.dir.clone(),
            integrity: IntegrityStatus::Unverified,
            integrity_detail: Some("simulated crash".to_owned()),
            segments: Vec::new(),
            record_count: 0,
            first_capture_ns: None,
            last_capture_ns: None,
            checksum_crc32c: 0,
            stats,
            writer_cpu_time: None,
            index_updated: false,
        }
    }
}

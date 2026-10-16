//! End-to-end scenarios that need crate internals: deterministic overruns (stalled writer), injected I/O
//! faults, a simulated filling disk, simulated crashes, and direct index manipulation.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::clock::unix_now_ns;
use crate::config::{DEFAULT_DISK_CRITICAL_FREE_BYTES, INDEX_FILE_NAME, RecorderConfig};
use crate::disk::DiskSpaceProbe;
use crate::error::{AppendError, RecorderError};
use crate::format::{GapNote, GapReason, MAX_CAPTURE_NS, MAX_RECORD_BYTES, RecordKind};
use crate::index::{
    ChunkEntry, ChunkRows, EventQuery, Index, SegmentInfo, SessionEnd, SessionInfo,
};
use crate::reader::{RecordedRecord, SegmentPlan, SessionReader};
use crate::recovery::recover;
use crate::sink::{SegmentFile, SegmentFileFactory};
use crate::store::RecordingStore;
use crate::test_support::TempDir;
use crate::types::{
    EventCategory, IntegrityStatus, LapRecord, SegmentStatus, SessionId, SessionMeta,
    SessionStatus, Severity, TimeRange,
};
use crate::writer::{RecorderEvent, RecordingSession, SessionOptions, WriterState};

/// One record per capture millisecond.
const MS: u64 = 1_000_000;
const GIB: u64 = 1 << 30;
/// Upper bound for anything the tests wait for (generous: CI machines can be slow).
const WAIT: Duration = Duration::from_secs(20);

#[cfg(windows)]
const DISK_FULL_OS_ERROR: i32 = 112; // ERROR_DISK_FULL
#[cfg(not(windows))]
const DISK_FULL_OS_ERROR: i32 = 28; // ENOSPC

/// Free space reported to the writer, adjustable by the test.
#[derive(Clone)]
struct SharedSpace(Arc<AtomicU64>);

impl DiskSpaceProbe for SharedSpace {
    fn available_bytes(&self, _path: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn plenty_of_space() -> SharedSpace {
    SharedSpace(Arc::new(AtomicU64::new(100 * GIB)))
}

/// Chunks follow capture time only: 10 records (10 ms) per chunk.
fn config(dir: &Path) -> RecorderConfig {
    let mut cfg = RecorderConfig::new(dir);
    cfg.chunk_max_capture_span = Duration::from_millis(10);
    cfg.chunk_max_age = Duration::from_secs(600);
    cfg.sync_interval = Duration::from_secs(600);
    cfg.failure_retry_interval = Duration::from_millis(20);
    cfg.disk_check_interval = Duration::from_millis(10);
    cfg
}

fn meta() -> SessionMeta {
    SessionMeta {
        app_version: "0.1.0-test".into(),
        telemetry_schema_version: 1,
        ..Default::default()
    }
}

fn options(space: &SharedSpace) -> SessionOptions {
    SessionOptions::new().with_disk_probe(space.clone())
}

fn payload(i: u64) -> Vec<u8> {
    format!("record-{i:06}-payload").into_bytes()
}

fn append_range(session: &RecordingSession, range: std::ops::Range<u64>) {
    for i in range {
        session
            .append(RecordKind::TelemetryTlv, i * MS, &payload(i))
            .expect("append");
    }
}

/// Returns once the writer has fully processed every record queued so far: the queue is empty, and the
/// writer (which handles one message at a time) has reached a control message sent afterwards.
fn settle(session: &RecordingSession) {
    let deadline = Instant::now() + WAIT;
    while session.sender().queue_len() > 0 {
        assert!(
            Instant::now() < deadline,
            "the writer did not drain its queue"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(session.stall_writer());
}

fn wait_for(
    events: &Receiver<RecorderEvent>,
    seen: &mut Vec<RecorderEvent>,
    pred: impl Fn(&RecorderEvent) -> bool,
) -> RecorderEvent {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = events
            .recv_timeout(left)
            .expect("the expected recorder event did not arrive");
        seen.push(event.clone());
        if pred(&event) {
            return event;
        }
    }
}

fn read_all(store: &RecordingStore, id: SessionId) -> Vec<RecordedRecord> {
    store
        .open_reader(id)
        .expect("reader")
        .records(TimeRange::ALL)
        .expect("stream")
        .collect::<Result<Vec<_>, _>>()
        .expect("every chunk verifies")
}

/// Capture milliseconds of the producer (telemetry) records.
fn produced_ms(records: &[RecordedRecord]) -> Vec<u64> {
    records
        .iter()
        .filter(|r| r.kind == RecordKind::TelemetryTlv)
        .map(|r| r.capture_ns / MS)
        .collect()
}

fn gap_notes(records: &[RecordedRecord]) -> Vec<GapNote> {
    records
        .iter()
        .filter_map(RecordedRecord::gap_note)
        .map(|n| n.expect("well-formed gap note"))
        .collect()
}

fn check_payloads(records: &[RecordedRecord]) {
    for r in records
        .iter()
        .filter(|r| r.kind == RecordKind::TelemetryTlv)
    {
        assert_eq!(
            r.bytes,
            payload(r.capture_ns / MS),
            "payload of record at {} ms",
            r.capture_ns / MS
        );
    }
}

fn segment_path(store: &RecordingStore, id: SessionId, index: usize) -> PathBuf {
    let seg = &store.segments(id).expect("segments")[index];
    store.config().resolve(Path::new(&seg.path))
}

fn ms_range(range: std::ops::Range<u64>) -> Vec<u64> {
    range.collect()
}

// ------------------------------------------------------------------------------------------------------
// Injected I/O faults
// ------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct FaultPlan {
    fail_writes: AtomicBool,
    fail_creates: AtomicBool,
}

fn disk_full() -> io::Error {
    io::Error::from_raw_os_error(DISK_FULL_OS_ERROR)
}

struct FaultyFile {
    inner: File,
    plan: Arc<FaultPlan>,
}

impl Write for FaultyFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.plan.fail_writes.load(Ordering::SeqCst) {
            return Err(disk_full());
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl SegmentFile for FaultyFile {
    fn sync_data(&mut self) -> io::Result<()> {
        self.inner.sync_data()
    }
}

struct FaultyFiles(Arc<FaultPlan>);

impl SegmentFileFactory for FaultyFiles {
    fn create_new(&self, path: &Path) -> io::Result<Box<dyn SegmentFile>> {
        if self.0.fail_creates.load(Ordering::SeqCst) {
            return Err(disk_full());
        }
        let inner = OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(Box::new(FaultyFile {
            inner,
            plan: Arc::clone(&self.0),
        }))
    }
}

// ------------------------------------------------------------------------------------------------------
// Scenarios
// ------------------------------------------------------------------------------------------------------

#[test]
fn public_handles_have_the_documented_thread_safety() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<RecordingStore>();
    send_sync::<RecordingSession>();
    send_sync::<crate::writer::RecordSender>();
    send_sync::<SessionReader>();
    send::<crate::reader::RecordStream>();
    send_sync::<crate::replay::ReplayClock>();
    send_sync::<crate::error::RecorderError>();
}

#[test]
fn overrun_is_counted_reported_indexed_and_noted_in_the_recording() {
    let dir = TempDir::new("overrun");
    let mut cfg = config(dir.path());
    cfg.queue_capacity_records = 8;
    let store = RecordingStore::open(cfg).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    let events = session.events().clone();

    let release = session.stall_writer();
    for i in 1..=8 {
        session
            .append(RecordKind::TelemetryTlv, i * MS, &payload(i))
            .expect("queued");
    }
    for i in 9..=13 {
        assert_eq!(
            session.append(RecordKind::TelemetryTlv, i * MS, &payload(i)),
            Err(AppendError::Overrun)
        );
    }
    let stats = session.stats();
    assert_eq!(
        (
            stats.records_accepted,
            stats.records_overrun,
            stats.queue_len
        ),
        (8, 5, 8)
    );
    drop(release);
    let summary = session.finish().expect("finish");

    let s = &summary.stats;
    assert_eq!(
        (
            s.records_written,
            s.records_overrun,
            s.records_lost(),
            s.notes_written
        ),
        (8, 5, 5, 1)
    );
    let overruns: Vec<_> = events
        .try_iter()
        .filter_map(|e| match e {
            RecorderEvent::RecorderOverrun {
                dropped,
                total_dropped,
                first_capture_ns,
                last_capture_ns,
            } => Some((dropped, total_dropped, first_capture_ns, last_capture_ns)),
            _ => None,
        })
        .collect();
    assert_eq!(
        overruns,
        vec![(5, 5, Some(9 * MS), Some(13 * MS))],
        "reported once, with the exact range"
    );

    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.dropped_records),
        (SessionStatus::Complete, 5)
    );
    let q = EventQuery {
        category: Some(EventCategory::Recorder),
        ..Default::default()
    };
    let indexed = store.events(id, &q).expect("events");
    assert!(
        indexed.iter().any(|e| e.severity == Severity::Warning
            && e.description.contains("overrun")
            && e.capture_ns == Some(9 * MS)),
        "{indexed:?}"
    );

    let records = read_all(&store, id);
    assert_eq!(
        gap_notes(&records),
        vec![GapNote {
            reason: GapReason::Overrun,
            dropped_records: 5,
            first_capture_ns: Some(9 * MS),
            last_capture_ns: Some(13 * MS)
        }]
    );
    assert_eq!(produced_ms(&records), ms_range(1..9));
    check_payloads(&records);
}

#[test]
fn write_failure_is_reported_drops_are_counted_and_recording_resumes_in_a_new_segment() {
    let dir = TempDir::new("write-failure");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let plan = Arc::new(FaultPlan::default());
    let opts = options(&plenty_of_space()).with_files(Arc::new(FaultyFiles(Arc::clone(&plan))));
    let session = store.start_session(meta(), opts).expect("session");
    let id = session.id();
    let events = session.events().clone();
    let mut seen = Vec::new();

    append_range(&session, 0..25);
    session.sync(WAIT).expect("sync"); // chunks 0..2 (25 records) durable
    append_range(&session, 25..35); // one pending chunk
    settle(&session);
    plan.fail_writes.store(true, Ordering::SeqCst);
    plan.fail_creates.store(true, Ordering::SeqCst);
    let report = session.sync(WAIT).expect("the sync request is answered");
    assert_eq!(
        (report.state, report.segment_index),
        (WriterState::Failed, None)
    );

    append_range(&session, 35..41); // dropped while failed
    settle(&session);
    let stats = session.stats();
    assert_eq!(stats.state, WriterState::Failed);
    assert_eq!(
        (stats.records_written, stats.records_dropped_write_failure),
        (25, 16)
    );
    // The latest failure: the sync, or a retry to open a new segment that has failed since.
    let os_error = format!("os error {DISK_FULL_OS_ERROR}");
    assert!(
        stats
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains(&os_error)),
        "{stats:?}"
    );

    plan.fail_writes.store(false, Ordering::SeqCst);
    plan.fail_creates.store(false, Ordering::SeqCst);
    let resumed = wait_for(&events, &mut seen, |e| {
        matches!(e, RecorderEvent::RecordingResumed { .. })
    });
    assert_eq!(
        resumed,
        RecorderEvent::RecordingResumed {
            segment_index: 1,
            dropped_records: 16
        }
    );
    append_range(&session, 41..46);
    let summary = session.finish().expect("finish");
    seen.extend(events.try_iter());

    let failed = seen
        .iter()
        .find_map(|e| match e {
            RecorderEvent::RecordingFailed {
                segment_index,
                operation,
                os_error,
                ..
            } => Some((*segment_index, *operation, *os_error)),
            _ => None,
        })
        .expect("RecordingFailed is reported");
    assert_eq!(
        failed,
        (Some(0), "syncing a segment", Some(DISK_FULL_OS_ERROR))
    );

    let s = &summary.stats;
    assert_eq!(
        (
            s.records_accepted,
            s.records_written,
            s.records_dropped_write_failure
        ),
        (46, 30, 16)
    );
    assert_eq!(
        s.records_accepted,
        s.records_written + s.records_lost(),
        "every accepted record is accounted for"
    );
    assert_eq!(s.write_failures, 1);

    let segs = store.segments(id).expect("segments");
    assert_eq!(segs.len(), 2);
    assert_eq!(
        (segs[0].status, segs[0].integrity, segs[0].record_count),
        (SegmentStatus::Failed, IntegrityStatus::Ok, 25)
    );
    assert!(
        segs[0]
            .integrity_detail
            .as_deref()
            .is_some_and(|d| d.contains("abandoned after a write failure"))
    );
    assert_eq!(
        (segs[1].status, segs[1].integrity),
        (SegmentStatus::Closed, IntegrityStatus::Ok)
    );
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.dropped_records, info.integrity),
        (SessionStatus::Complete, 16, IntegrityStatus::Ok)
    );

    let records = read_all(&store, id);
    assert_eq!(
        produced_ms(&records),
        ms_range(0..25)
            .into_iter()
            .chain(41..46)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        gap_notes(&records),
        vec![GapNote {
            reason: GapReason::WriteFailure,
            dropped_records: 16,
            first_capture_ns: Some(25 * MS),
            last_capture_ns: Some(40 * MS)
        }]
    );
    check_payloads(&records);
    let indexed = store.events(id, &EventQuery::default()).expect("events");
    assert!(indexed.iter().any(|e| {
        e.severity == Severity::Critical
            && e.description
                .contains("recording failed while syncing a segment")
    }));
    assert!(
        indexed
            .iter()
            .any(|e| e.severity == Severity::Info && e.description.contains("recording resumed"))
    );
}

#[test]
fn critical_disk_space_suspends_recording_until_space_returns() {
    let dir = TempDir::new("disk-space");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let space = plenty_of_space();
    let session = store
        .start_session(meta(), options(&space))
        .expect("session");
    let id = session.id();
    let events = session.events().clone();
    let mut seen = Vec::new();

    append_range(&session, 0..15);
    session.sync(WAIT).expect("sync");
    space.0.store(GIB / 2, Ordering::SeqCst);
    let critical = wait_for(&events, &mut seen, |e| {
        matches!(e, RecorderEvent::DiskSpaceCritical { .. })
    });
    assert_eq!(
        critical,
        RecorderEvent::DiskSpaceCritical {
            available_bytes: GIB / 2,
            threshold_bytes: DEFAULT_DISK_CRITICAL_FREE_BYTES
        }
    );
    assert_eq!(session.stats().state, WriterState::Suspended);
    append_range(&session, 15..22); // dropped while suspended
    settle(&session);
    assert_eq!(session.stats().records_dropped_disk_full, 7);
    space.0.store(3 * GIB, Ordering::SeqCst);
    let recovered = wait_for(&events, &mut seen, |e| {
        matches!(e, RecorderEvent::DiskSpaceRecovered { .. })
    });
    assert_eq!(
        recovered,
        RecorderEvent::DiskSpaceRecovered {
            available_bytes: Some(3 * GIB)
        }
    );
    append_range(&session, 22..27);
    let summary = session.finish().expect("finish");
    seen.extend(events.try_iter());

    let low: Vec<_> = seen
        .iter()
        .filter(|e| matches!(e, RecorderEvent::DiskSpaceLow { .. }))
        .collect();
    assert_eq!(
        low.len(),
        1,
        "the low-space warning is raised once while space stays low: {seen:?}"
    );
    let s = &summary.stats;
    assert_eq!(
        (
            s.records_accepted,
            s.records_written,
            s.records_dropped_disk_full
        ),
        (27, 20, 7)
    );
    let segs = store.segments(id).expect("segments");
    assert_eq!(
        segs.len(),
        2,
        "the segment was closed cleanly at suspension and a new one opened after"
    );
    assert!(
        segs.iter()
            .all(|s| s.status == SegmentStatus::Closed && s.integrity == IntegrityStatus::Ok)
    );
    assert_eq!(segs[0].record_count, 15);
    let records = read_all(&store, id);
    assert_eq!(
        produced_ms(&records),
        ms_range(0..15)
            .into_iter()
            .chain(22..27)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        gap_notes(&records),
        vec![GapNote {
            reason: GapReason::DiskSpaceCritical,
            dropped_records: 7,
            first_capture_ns: Some(15 * MS),
            last_capture_ns: Some(21 * MS)
        }]
    );
    assert_eq!(
        store
            .session(id)
            .expect("read")
            .expect("present")
            .dropped_records,
        7
    );
}

/// Puts the index back to the state the crash left (session ACTIVE, segment OPEN with the synced prefix).
fn restore_crash_state(
    index: &mut Index,
    s: &SessionInfo,
    seg: &SegmentInfo,
    chunks: &[ChunkEntry],
) {
    let end = SessionEnd {
        status: s.status,
        ended_unix_ns: s.ended_unix_ns,
        recovered_unix_ns: s.recovered_unix_ns,
        first_capture_ns: s.first_capture_ns,
        last_capture_ns: s.last_capture_ns,
        record_count: s.record_count,
        dropped_records: s.dropped_records,
        integrity: s.integrity,
        integrity_detail: s.integrity_detail.clone(),
        checksum_crc32c: s.checksum_crc32c,
    };
    index.end_session(s.id, &end).expect("restore session row");
    index
        .put_segment(seg, ChunkRows::Replace(chunks.to_vec()))
        .expect("restore segment rows");
}

fn reader_from_index(cfg: &RecorderConfig, index: &Index, id: SessionId) -> SessionReader {
    let snapshot = index
        .session_snapshot(id)
        .expect("snapshot")
        .expect("session present");
    let plans = snapshot
        .segments
        .into_iter()
        .map(|(info, chunks)| SegmentPlan { info, chunks })
        .collect();
    SessionReader::new(cfg.recordings_dir.clone(), snapshot.session, plans)
}

#[test]
fn crash_during_the_last_write_recovers_every_complete_chunk_at_every_truncation_offset() {
    let dir = TempDir::new("kill-mid-write");
    let cfg = config(dir.path());
    let (id, seg_path) = {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        let session = store
            .start_session(meta(), options(&plenty_of_space()))
            .expect("session");
        let id = session.id();
        append_range(&session, 0..30);
        session.sync(WAIT).expect("sync"); // chunks 0..2 durable and indexed
        append_range(&session, 30..37); // pending chunk 3
        settle(&session);
        // Chunk 3 reaches the OS but is never synced or indexed; then the process "dies".
        session.simulate_crash(true);
        (id, segment_path(&store, id, 0))
    };

    let mut index = Index::open(&cfg.index_path()).expect("index");
    let crash_session = index.session(id).expect("read").expect("present");
    assert_eq!(crash_session.status, SessionStatus::Active);
    let crash_seg = index.segments(id).expect("segments").remove(0);
    assert_eq!(crash_seg.status, SegmentStatus::Open);
    let crash_chunks = index.chunks(id, 0).expect("chunks");
    assert_eq!(crash_chunks.len(), 3);
    let last_start = crash_chunks[2].end();
    assert_eq!(
        crash_seg.byte_size, last_start,
        "the index holds the synced prefix"
    );
    let full = fs::read(&seg_path).expect("segment");
    let full_len = full.len() as u64;
    assert!(full_len > last_start, "the crashed write reached the file");

    for cut in last_start..=full_len {
        // A crash at any point of the last write leaves a prefix of it in the file.
        fs::write(&seg_path, &full[..cut as usize]).expect("cut");
        restore_crash_state(&mut index, &crash_session, &crash_seg, &crash_chunks);
        let report = recover(&cfg, &mut index, unix_now_ns());
        assert!(
            report.failures.is_empty(),
            "cut {cut}: {:?}",
            report.failures
        );
        assert_eq!(report.sessions.len(), 1, "cut {cut}");
        let r = &report.sessions[0];
        let complete = cut == full_len;
        let expected: u64 = if complete { 37 } else { 30 };
        assert_eq!(r.record_count, expected, "cut {cut}");
        assert_eq!(
            (r.first_capture_ns, r.last_capture_ns),
            (Some(0), Some((expected - 1) * MS)),
            "cut {cut}"
        );
        let at_boundary = complete || cut == last_start;
        let integrity = if at_boundary {
            IntegrityStatus::Ok
        } else {
            IntegrityStatus::Truncated
        };
        assert_eq!(
            r.integrity, integrity,
            "cut {cut}: {:?}",
            r.integrity_detail
        );
        assert_eq!(
            r.segments[0].byte_size,
            if complete { full_len } else { last_start },
            "cut {cut}"
        );
        assert_eq!(r.segments[0].file_len, Some(cut));
        assert_eq!(
            fs::metadata(&seg_path).expect("meta").len(),
            cut,
            "recovery never modifies the file"
        );
        let info = index.session(id).expect("read").expect("present");
        assert_eq!(
            (info.status, info.last_capture_ns),
            (SessionStatus::Interrupted, Some((expected - 1) * MS))
        );
        assert!(info.recovered_unix_ns.is_some() && info.ended_unix_ns.is_none());

        let records: Vec<RecordedRecord> = reader_from_index(&cfg, &index, id)
            .records(TimeRange::ALL)
            .expect("stream")
            .collect::<Result<_, _>>()
            .expect("only verified chunks are read");
        assert_eq!(produced_ms(&records), ms_range(0..expected), "cut {cut}");
        check_payloads(&records);
    }
}

#[test]
fn startup_recovery_marks_a_crashed_session_interrupted_with_its_last_valid_time() {
    let dir = TempDir::new("startup-recovery");
    let cfg = config(dir.path());
    let id = {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        let session = store
            .start_session(meta(), options(&plenty_of_space()))
            .expect("session");
        let id = session.id();
        session
            .record_lap(LapRecord {
                lap_number: 1,
                start_capture_ns: 0,
                ..Default::default()
            })
            .expect("lap");
        append_range(&session, 0..20);
        session.sync(WAIT).expect("sync");
        append_range(&session, 20..25); // never written: lost in the crash
        settle(&session);
        assert_eq!(
            store.session(id).expect("read").expect("present").status,
            SessionStatus::Active
        );
        session.simulate_crash(false);
        id
    };

    let store = RecordingStore::open(cfg.clone()).expect("reopen");
    let report = store.recovery_report();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.sessions.len(), 1);
    let r = &report.sessions[0];
    assert_eq!((r.session_id, r.adopted, r.record_count), (id, false, 20));
    assert_eq!(
        (r.last_capture_ns, r.integrity),
        (Some(19 * MS), IntegrityStatus::Ok)
    );
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(info.status, SessionStatus::Interrupted);
    assert_eq!(
        (info.last_capture_ns, info.record_count),
        (Some(19 * MS), 20)
    );
    assert!(info.ended_unix_ns.is_none() && info.recovered_unix_ns.is_some());
    assert_eq!(info.checksum_crc32c, Some(r.checksum_crc32c));
    assert_eq!(
        store.segments(id).expect("segments")[0].status,
        SegmentStatus::Recovered
    );
    assert_eq!(
        store.laps(id).expect("laps").len(),
        1,
        "index rows written before the crash survive"
    );
    let q = EventQuery {
        category: Some(EventCategory::Recorder),
        ..Default::default()
    };
    assert!(
        store
            .events(id, &q)
            .expect("events")
            .iter()
            .any(|e| e.description.contains("was not finished"))
    );
    let records = read_all(&store, id);
    assert_eq!(produced_ms(&records), ms_range(0..20));
    check_payloads(&records);
    drop(store);

    let store = RecordingStore::open(cfg).expect("third open");
    assert!(
        store.recovery_report().sessions.is_empty(),
        "recovery is not repeated for INTERRUPTED sessions"
    );
    assert_eq!(
        store.session(id).expect("read").expect("present").status,
        SessionStatus::Interrupted
    );
}

#[test]
fn corruption_inside_the_synced_prefix_is_reported_and_intact_chunks_after_it_stay_readable() {
    let dir = TempDir::new("corrupt-durable");
    let cfg = config(dir.path());
    let (id, seg_path, chunks) = {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        let session = store
            .start_session(meta(), options(&plenty_of_space()))
            .expect("session");
        let id = session.id();
        append_range(&session, 0..50);
        session.sync(WAIT).expect("sync"); // 5 chunks durable and indexed
        session.simulate_crash(false);
        (
            id,
            segment_path(&store, id, 0),
            store.chunks(id, 0).expect("chunks"),
        )
    };
    assert_eq!(chunks.len(), 5);
    let mut bytes = fs::read(&seg_path).expect("read");
    let victim = usize::try_from(chunks[2].offset).expect("offset") + 48 + 3;
    bytes[victim] ^= 0x55;
    fs::write(&seg_path, &bytes).expect("corrupt");

    let store = RecordingStore::open(cfg).expect("reopen");
    let r = &store.recovery_report().sessions[0];
    assert_eq!(r.integrity, IntegrityStatus::Damaged);
    assert_eq!(
        (r.record_count, r.last_capture_ns),
        (40, Some(49 * MS)),
        "only the damaged chunk (records 20..29) is lost; the intact chunks after it are kept"
    );
    let detail = r.integrity_detail.clone().unwrap_or_default();
    assert!(
        detail.contains(&format!("corrupt chunk at byte {}", chunks[2].offset)),
        "{detail}"
    );
    assert!(
        detail.contains("2 intact chunk(s) after the damage are still readable"),
        "{detail}"
    );
    assert!(
        detail.contains("durable"),
        "the lost durable prefix is reported: {detail}"
    );
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.integrity),
        (SessionStatus::Interrupted, IntegrityStatus::Damaged)
    );
    assert_eq!(
        store.chunks(id, 0).expect("chunks"),
        chunks,
        "every chunk stays indexed"
    );
    let results: Vec<Result<RecordedRecord, RecorderError>> = store
        .open_reader(id)
        .expect("reader")
        .records(TimeRange::ALL)
        .expect("stream")
        .collect();
    let errors: Vec<&RecorderError> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(
        errors.len(),
        1,
        "the damaged chunk is reported once: {errors:?}"
    );
    assert!(
        matches!(errors[0], RecorderError::Format { offset, .. } if *offset == chunks[2].offset),
        "{:?}",
        errors[0]
    );
    let good: Vec<RecordedRecord> = results.into_iter().filter_map(Result::ok).collect();
    assert_eq!(
        produced_ms(&good),
        ms_range(0..20)
            .into_iter()
            .chain(30..50)
            .collect::<Vec<_>>()
    );
    check_payloads(&good);
    // Re-validation agrees and keeps the rows.
    let v = store.validate_session(id).expect("validate");
    assert_eq!(
        (v.integrity, v.record_count),
        (IntegrityStatus::Damaged, 40)
    );
    assert_eq!(store.chunks(id, 0).expect("chunks"), chunks);
    assert_eq!(
        fs::read(&seg_path).expect("read"),
        bytes,
        "the damaged file is left exactly as it was"
    );
}

#[test]
fn orphan_session_directories_are_adopted_when_the_index_is_lost() {
    let dir = TempDir::new("orphan");
    let cfg = config(dir.path());
    let id = {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        let session = store
            .start_session(meta(), options(&plenty_of_space()))
            .expect("session");
        let id = session.id();
        append_range(&session, 0..30);
        session.finish().expect("finish");
        id
    };
    for name in [
        INDEX_FILE_NAME.to_owned(),
        format!("{INDEX_FILE_NAME}-wal"),
        format!("{INDEX_FILE_NAME}-shm"),
    ] {
        match fs::remove_file(dir.path().join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => panic!("removing the index: {e}"),
        }
    }
    let store = RecordingStore::open(cfg).expect("reopen");
    let report = store.recovery_report();
    assert_eq!(report.sessions.len(), 1);
    let r = &report.sessions[0];
    assert_eq!(
        (r.session_id, r.adopted, r.record_count, r.integrity),
        (id, true, 30, IntegrityStatus::Ok)
    );
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(info.status, SessionStatus::Interrupted);
    assert_eq!(info.meta.app_version, "0.1.0-test");
    assert_eq!(produced_ms(&read_all(&store, id)), ms_range(0..30));
}

#[test]
fn a_corrupt_index_is_set_aside_and_rebuilt_from_the_segment_files() {
    let dir = TempDir::new("corrupt-index");
    let cfg = config(dir.path());
    let id = {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        let session = store
            .start_session(meta(), options(&plenty_of_space()))
            .expect("session");
        let id = session.id();
        append_range(&session, 0..12);
        session.finish().expect("finish");
        id
    };
    fs::write(dir.path().join(INDEX_FILE_NAME), vec![0xa5u8; 16 * 1024])
        .expect("corrupt the index");
    let store = RecordingStore::open(cfg).expect("reopen");
    let report = store.recovery_report();
    let aside = report
        .index_set_aside
        .clone()
        .expect("the corrupt index was set aside");
    assert_eq!(
        fs::read(&aside).expect("kept"),
        vec![0xa5u8; 16 * 1024],
        "the corrupt file is kept as it was"
    );
    assert_eq!(report.sessions.len(), 1);
    assert!(report.sessions[0].adopted);
    assert_eq!(produced_ms(&read_all(&store, id)), ms_range(0..12));
}

#[test]
fn invalid_appends_are_rejected_and_counted() {
    let dir = TempDir::new("rejections");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    assert_eq!(
        session.append(RecordKind::RecorderNote, 1, b"x"),
        Err(AppendError::ReservedKind(RecordKind::RecorderNote))
    );
    assert_eq!(
        session.append(RecordKind::Unknown(200), 1, b"x"),
        Err(AppendError::ReservedKind(RecordKind::Unknown(200)))
    );
    let too_big = vec![0u8; MAX_RECORD_BYTES + 1];
    assert_eq!(
        session.append(RecordKind::RawLmuBlock, 1, &too_big),
        Err(AppendError::TooLarge {
            len: MAX_RECORD_BYTES + 1,
            max: MAX_RECORD_BYTES
        })
    );
    assert_eq!(
        session.append(RecordKind::TelemetryTlv, MAX_CAPTURE_NS + 1, b"x"),
        Err(AppendError::CaptureTimeOutOfRange {
            capture_ns: MAX_CAPTURE_NS + 1,
            max: MAX_CAPTURE_NS
        })
    );
    assert!(matches!(
        session.record_lap(LapRecord {
            lap_number: 1,
            start_capture_ns: u64::MAX,
            ..Default::default()
        }),
        Err(AppendError::CaptureTimeOutOfRange { .. })
    ));
    // The largest record at the largest capture time is accepted and read back intact.
    let largest: Vec<u8> = (0..MAX_RECORD_BYTES).map(|i| (i % 251) as u8).collect();
    session
        .append(RecordKind::RawLmuBlock, MAX_CAPTURE_NS, &largest)
        .expect("limits are inclusive");
    let sender = session.sender();
    let summary = session.finish().expect("finish");
    assert_eq!(summary.stats.records_rejected, 4);
    assert_eq!(
        sender.append(RecordKind::TelemetryTlv, 1, b"late"),
        Err(AppendError::Stopped)
    );
    assert_eq!(sender.stats().records_rejected, 5);
    let records = read_all(&store, id);
    assert_eq!(records.len(), 1);
    assert_eq!(
        (records[0].kind, records[0].capture_ns),
        (RecordKind::RawLmuBlock, MAX_CAPTURE_NS)
    );
    assert!(records[0].bytes == largest);
}

#[test]
fn dropping_a_session_without_finish_still_completes_it() {
    let dir = TempDir::new("drop");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    append_range(&session, 0..12);
    assert!(store.is_recording(id));
    drop(session);
    let deadline = Instant::now() + WAIT;
    while store.is_recording(id) {
        assert!(
            Instant::now() < deadline,
            "the writer did not finish in the background"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.record_count, info.integrity),
        (SessionStatus::Complete, 12, IntegrityStatus::Ok)
    );
    assert_eq!(produced_ms(&read_all(&store, id)), ms_range(0..12));
}

#[test]
fn sync_makes_everything_appended_durable_and_reports_it() {
    let dir = TempDir::new("sync");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    append_range(&session, 0..25);
    let report = session.sync(WAIT).expect("sync");
    assert_eq!(
        (report.state, report.segment_index, report.records_written),
        (WriterState::Recording, Some(0), 25)
    );
    let path = segment_path(&store, id, 0);
    assert_eq!(
        report.durable_bytes,
        fs::metadata(&path).expect("meta").len()
    );
    let seg = &store.segments(id).expect("segments")[0];
    assert_eq!(
        (seg.byte_size, seg.record_count, seg.chunk_count),
        (report.durable_bytes, 25, 3)
    );
    assert_eq!(
        store.chunks(id, 0).expect("chunks").len(),
        3,
        "durable chunks are indexed at sync"
    );
    session.finish().expect("finish");
}

#[test]
fn readers_see_the_synced_part_of_a_session_being_recorded() {
    let dir = TempDir::new("live-read");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    append_range(&session, 0..25);
    session.sync(WAIT).expect("sync");
    assert_eq!(produced_ms(&read_all(&store, id)), ms_range(0..25));
    append_range(&session, 25..40);
    session.sync(WAIT).expect("sync");
    assert_eq!(produced_ms(&read_all(&store, id)), ms_range(0..40));
    session.finish().expect("finish");
}

#[test]
fn readers_walk_chunk_headers_where_the_offset_index_is_incomplete() {
    let dir = TempDir::new("walk");
    let cfg = config(dir.path());
    let store = RecordingStore::open(cfg.clone()).expect("store");
    let session = store
        .start_session(meta(), options(&plenty_of_space()))
        .expect("session");
    let id = session.id();
    append_range(&session, 0..50);
    session.finish().expect("finish");
    {
        // Keep only the first two chunk rows, as if later index updates had been lost.
        let mut index = Index::open(&cfg.index_path()).expect("index");
        let seg = index.segments(id).expect("segments").remove(0);
        let rows = index.chunks(id, 0).expect("chunks");
        assert_eq!(rows.len(), 5);
        index
            .put_segment(&seg, ChunkRows::Replace(rows[..2].to_vec()))
            .expect("truncate rows");
    }
    assert_eq!(store.chunks(id, 0).expect("chunks").len(), 2);
    let records = read_all(&store, id);
    assert_eq!(produced_ms(&records), ms_range(0..50));
    check_payloads(&records);
    let reader = store.open_reader(id).expect("reader");
    let late: Vec<u64> = reader
        .records(TimeRange::new(35 * MS, u64::MAX).expect("range"))
        .expect("stream")
        .map(|r| r.expect("record").capture_ns / MS)
        .collect();
    assert_eq!(late, ms_range(35..50));
}

#[test]
fn a_failed_writer_thread_start_leaves_nothing_behind() {
    let tmp = TempDir::new("spawn-fail");
    let store = RecordingStore::open(config(tmp.path())).expect("open");
    let err = store
        .start_session(
            meta(),
            options(&plenty_of_space()).with_failing_thread_spawn(),
        )
        .expect_err("the writer thread cannot start");
    assert!(matches!(err, RecorderError::SpawnWriter(_)), "{err}");
    assert!(store.sessions().expect("sessions").is_empty());
    assert!(store.recording_sessions().is_empty());
    let sessions_dir = store.config().sessions_dir();
    assert_eq!(
        fs::read_dir(&sessions_dir).expect("sessions dir").count(),
        0,
        "no session directory is left behind"
    );
    drop(store);
    let reopened = RecordingStore::open(config(tmp.path())).expect("reopen");
    assert!(
        reopened.sessions().expect("sessions").is_empty(),
        "no phantom session is adopted at the next startup"
    );
}

#[test]
fn a_range_read_starting_inside_a_gap_still_returns_the_gap_note() {
    let dir = TempDir::new("gap-range");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let space = plenty_of_space();
    let session = store
        .start_session(meta(), options(&space))
        .expect("session");
    let id = session.id();
    let events = session.events().clone();
    let mut seen = Vec::new();

    append_range(&session, 0..15);
    session.sync(WAIT).expect("sync");
    space.0.store(GIB / 2, Ordering::SeqCst);
    wait_for(&events, &mut seen, |e| {
        matches!(e, RecorderEvent::DiskSpaceCritical { .. })
    });
    // A gap longer than a chunk's capture span (10 ms): the note's chunk is sealed on its own.
    append_range(&session, 15..40);
    settle(&session);
    space.0.store(3 * GIB, Ordering::SeqCst);
    wait_for(&events, &mut seen, |e| {
        matches!(e, RecorderEvent::DiskSpaceRecovered { .. })
    });
    append_range(&session, 40..45);
    session.finish().expect("finish");

    let expected_note = GapNote {
        reason: GapReason::DiskSpaceCritical,
        dropped_records: 25,
        first_capture_ns: Some(15 * MS),
        last_capture_ns: Some(39 * MS),
    };
    assert_eq!(gap_notes(&read_all(&store, id)), vec![expected_note]);
    let read = |from_ms: u64| -> Vec<RecordedRecord> {
        store
            .open_reader(id)
            .expect("reader")
            .records(TimeRange::new(from_ms * MS, MAX_CAPTURE_NS).expect("range"))
            .expect("stream")
            .collect::<Result<Vec<_>, _>>()
            .expect("every chunk verifies")
    };
    let inside = read(30);
    assert_eq!(
        gap_notes(&inside),
        vec![expected_note],
        "a read starting inside the gap reports it"
    );
    assert_eq!(produced_ms(&inside), ms_range(40..45));
    let after = read(41);
    assert!(
        gap_notes(&after).is_empty(),
        "a read after the gap does not"
    );
    assert_eq!(produced_ms(&after), ms_range(41..45));
}

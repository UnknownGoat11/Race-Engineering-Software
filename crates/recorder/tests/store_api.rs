//! Integration tests through the public API: roundtrip, index lifecycle, rotation, corruption, overruns,
//! retention and directory ownership.

mod common;

use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::{CHANNELS, PERIOD_NS, Rng, TempDir, meta, read_all, telemetry_block};
use recorder::format::{CHUNK_HEADER_LEN, FormatError};
use recorder::scan::{ScanStop, validate_segment_file};
use recorder::{
    AppendError, EventCategory, EventQuery, EventRecord, GapReason, IntegrityStatus, KeepAll,
    LapRecord, MIN_SEGMENT_MAX_BYTES, MaxAge, RecordKind, RecordedRecord, RecorderConfig,
    RecorderError, RecorderEvent, RecordingStore, ReplayClock, RetentionClass, RetentionPolicy,
    SegmentStatus, SessionId, SessionInfo, SessionMetaUpdate, SessionOptions, SessionStatus,
    Severity, StintRecord, TimeRange,
};
use telemetry_model::codec;

const WAIT: Duration = Duration::from_secs(20);

/// Production defaults, except that chunks are sealed by capture span (1 s) and size only, never by wall
/// age, so chunk boundaries do not depend on how fast the test machine runs.
fn config(dir: &Path) -> RecorderConfig {
    let mut cfg = RecorderConfig::new(dir);
    cfg.chunk_max_age = Duration::from_secs(3_600);
    cfg
}

/// What a test appended, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Appended {
    kind: RecordKind,
    capture_ns: u64,
    bytes: Vec<u8>,
}

fn same_records(read: &[RecordedRecord], appended: &[Appended]) -> bool {
    read.len() == appended.len()
        && read
            .iter()
            .zip(appended)
            .all(|(r, a)| r.kind == a.kind && r.capture_ns == a.capture_ns && r.bytes == a.bytes)
}

fn segment_path(store: &RecordingStore, id: SessionId, index: usize) -> std::path::PathBuf {
    let seg = &store.segments(id).expect("segments")[index];
    store.config().resolve(Path::new(&seg.path))
}

/// Ten seconds of 100 Hz telemetry plus every other record kind.
fn append_mixed_session(session: &recorder::RecordingSession) -> Vec<Appended> {
    let mut rng = Rng::new(7);
    let mut out = Vec::new();
    let mut buf = Vec::new();
    for i in 0..1_000u64 {
        let ns = i * PERIOD_NS;
        buf.clear();
        telemetry_block(&mut buf, ns, CHANNELS, &mut rng);
        out.push(Appended {
            kind: RecordKind::TelemetryTlv,
            capture_ns: ns,
            bytes: buf.clone(),
        });
        if i % 50 == 0 {
            let extras = [
                (RecordKind::ScoringPostcard, rng.bytes(600)),
                (RecordKind::WeatherTlv, rng.bytes(40)),
                (RecordKind::SessionEvent, b"lap completed".to_vec()),
                (RecordKind::Marker, b"bookmark".to_vec()),
                (RecordKind::CapabilityPostcard, rng.bytes(120)),
                (RecordKind::RawLmuBlock, rng.bytes(4_096)),
                (RecordKind::Marker, Vec::new()),
            ];
            for (kind, bytes) in extras {
                out.push(Appended {
                    kind,
                    capture_ns: ns,
                    bytes,
                });
            }
        }
    }
    for a in &out {
        session
            .append(a.kind, a.capture_ns, &a.bytes)
            .expect("append");
    }
    out
}

#[test]
fn roundtrip_with_index_lifecycle_time_ranges_and_validation() {
    let dir = TempDir::new("roundtrip");
    let cfg = config(dir.path());
    let id;
    let appended;
    let checksum;
    {
        let store = RecordingStore::open(cfg.clone()).expect("store");
        assert!(store.recovery_report().sessions.is_empty());
        let mut m = meta();
        m.lmu_version = Some("1.2.3".into());
        m.data_availability_json =
            Some(r#"{"tyre_temps":"AVAILABLE","hybrid":"NOT_SUPPORTED"}"#.into());
        let session = store
            .start_session(m, SessionOptions::new())
            .expect("session");
        id = session.id();
        let info = store.session(id).expect("read").expect("present");
        assert_eq!(
            (info.status, info.integrity),
            (SessionStatus::Active, IntegrityStatus::Unverified)
        );
        assert!(store.is_recording(id));

        appended = append_mixed_session(&session);
        session
            .update_meta(SessionMetaUpdate {
                vehicle: Some("GT3 #7".into()),
                track: Some("Spa".into()),
                ..Default::default()
            })
            .expect("meta");
        for lap in 1..=3u32 {
            let start = u64::from(lap - 1) * 3 * 1_000_000_000;
            session
                .record_lap(LapRecord {
                    lap_number: lap,
                    stint_number: Some(1),
                    start_capture_ns: start,
                    end_capture_ns: Some(start + 3_000_000_000),
                    lap_time_s: Some(3.0),
                    sector_times_s: [Some(1.0), Some(1.0), Some(1.0)],
                    valid: Some(true),
                    pit_lap: Some(false),
                    metrics_json: Some(r#"{"fuel_used_l":2.5}"#.into()),
                })
                .expect("lap");
        }
        session
            .record_stint(StintRecord {
                stint_number: 1,
                start_capture_ns: 0,
                end_capture_ns: Some(9_990_000_000),
                start_lap: Some(1),
                end_lap: Some(3),
                driver: Some("Driver A".into()),
                metrics_json: None,
            })
            .expect("stint");
        let mut alert = EventRecord::new(
            4_200_000_000,
            EventCategory::Alert,
            Severity::Warning,
            "tyre temp high",
        );
        alert.lap = Some(2);
        alert.channel_id = Some(0x0108);
        alert.value = Some(112.5);
        alert.threshold = Some(110.0);
        session.record_event(alert).expect("event");

        let events = session.events().clone();
        let summary = session.finish().expect("finish");
        assert_eq!(
            summary.integrity,
            IntegrityStatus::Ok,
            "{:?}",
            summary.integrity_detail
        );
        assert!(summary.index_updated);
        assert_eq!(summary.record_count, appended.len() as u64);
        assert_eq!(summary.stats.records_written, appended.len() as u64);
        assert_eq!(
            (summary.stats.records_lost(), summary.stats.records_rejected),
            (0, 0)
        );
        assert_eq!(
            (summary.first_capture_ns, summary.last_capture_ns),
            (Some(0), Some(999 * PERIOD_NS))
        );
        assert_eq!(summary.segments.len(), 1);
        assert_eq!(
            summary.segments[0].chunk_count, 10,
            "one chunk per second of capture time"
        );
        assert!(
            summary.stats.bytes_written < summary.stats.raw_bytes_written,
            "chunks are compressed"
        );
        let seen: Vec<RecorderEvent> = events.try_iter().collect();
        assert!(
            matches!(
                seen.first(),
                Some(RecorderEvent::SegmentOpened {
                    segment_index: 0,
                    ..
                })
            ),
            "{seen:?}"
        );
        assert!(seen.iter().any(|e| matches!(
            e,
            RecorderEvent::SegmentClosed {
                integrity: IntegrityStatus::Ok,
                ..
            }
        )));
        checksum = summary.checksum_crc32c;
        assert!(!store.is_recording(id));
    }

    // Reopen: nothing to recover, everything indexed.
    let store = RecordingStore::open(cfg).expect("reopen");
    assert!(store.recovery_report().sessions.is_empty());
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.integrity, info.checksum_crc32c),
        (SessionStatus::Complete, IntegrityStatus::Ok, Some(checksum))
    );
    assert!(
        info.ended_unix_ns
            .is_some_and(|e| e >= info.started_unix_ns)
    );
    assert_eq!(
        (info.record_count, info.dropped_records),
        (appended.len() as u64, 0)
    );
    assert_eq!(info.meta.vehicle.as_deref(), Some("GT3 #7"));
    assert_eq!(info.meta.track.as_deref(), Some("Spa"));
    assert_eq!(info.meta.lmu_version.as_deref(), Some("1.2.3"));
    assert!(
        info.meta
            .data_availability_json
            .as_deref()
            .is_some_and(|j| j.contains("NOT_SUPPORTED"))
    );
    let laps = store.laps(id).expect("laps");
    assert_eq!(
        laps.iter().map(|l| l.lap_number).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        laps[1].metrics_json.as_deref(),
        Some(r#"{"fuel_used_l":2.5}"#)
    );
    assert_eq!(
        store.stints(id).expect("stints")[0].driver.as_deref(),
        Some("Driver A")
    );
    let alerts = store
        .events(
            id,
            &EventQuery {
                category: Some(EventCategory::Alert),
                lap: Some(2),
                ..Default::default()
            },
        )
        .expect("events");
    assert_eq!(alerts.len(), 1);
    assert_eq!(
        (alerts[0].channel_id, alerts[0].value, alerts[0].threshold),
        (Some(0x0108), Some(112.5), Some(110.0))
    );

    // Readback is byte-exact and in order; telemetry payloads decode with the shared codec.
    let records = read_all(&store, id);
    assert!(
        same_records(&records, &appended),
        "readback differs from what was appended"
    );
    for r in records
        .iter()
        .filter(|r| r.kind == RecordKind::TelemetryTlv)
        .take(50)
    {
        let (header, recs) = codec::decode(&r.bytes).expect("TLV decodes");
        assert_eq!(header.capture_mono_ns, r.capture_ns);
        assert_eq!(recs.len(), usize::from(CHANNELS));
    }

    // A time range is exact per record and reads only the chunks it overlaps.
    let reader = store.open_reader(id).expect("reader");
    let range = TimeRange::new(3_000_000_000, 4_500_000_000).expect("range");
    let got: Vec<RecordedRecord> = reader
        .records(range)
        .expect("stream")
        .collect::<Result<_, _>>()
        .expect("records");
    let expected: Vec<Appended> = appended
        .iter()
        .filter(|a| range.contains(a.capture_ns))
        .cloned()
        .collect();
    assert!(
        same_records(&got, &expected),
        "range query returns exactly the records inside it"
    );
    assert!(
        got.iter().all(|r| r.chunk_seq == 3 || r.chunk_seq == 4),
        "only chunks 3 and 4 overlap [3 s, 4.5 s]"
    );
    let empty = TimeRange::new(20_000_000_000, 30_000_000_000).expect("range");
    assert_eq!(reader.records(empty).expect("stream").count(), 0);

    // The capture bounds drive a replay clock.
    let bounds = reader.capture_bounds().expect("bounds");
    let clock = ReplayClock::new(bounds.start_ns, bounds.end_ns).expect("clock");
    assert_eq!((clock.start_ns(), clock.end_ns()), (0, 999 * PERIOD_NS));

    // On-demand validation agrees with the writer.
    let v = store.validate_session(id).expect("validate");
    assert_eq!(
        (v.integrity, v.checksum_crc32c, v.record_count),
        (IntegrityStatus::Ok, checksum, appended.len() as u64)
    );
    assert_eq!(
        store.session(id).expect("read").expect("present").status,
        SessionStatus::Complete
    );
}

#[test]
fn invalid_session_metadata_is_rejected_and_leaves_nothing_behind() {
    let dir = TempDir::new("bad-meta");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let mut bad_json = meta();
    bad_json.data_availability_json = Some("{not json".into());
    assert!(matches!(
        store.start_session(bad_json, SessionOptions::new()),
        Err(RecorderError::InvalidMeta(_))
    ));
    let mut no_version = meta();
    no_version.app_version.clear();
    assert!(matches!(
        store.start_session(no_version, SessionOptions::new()),
        Err(RecorderError::InvalidMeta(_))
    ));
    let mut no_schema = meta();
    no_schema.telemetry_schema_version = 0;
    assert!(matches!(
        store.start_session(no_schema, SessionOptions::new()),
        Err(RecorderError::InvalidMeta(_))
    ));
    assert!(store.sessions().expect("sessions").is_empty());
    assert_eq!(
        fs::read_dir(store.config().sessions_dir())
            .expect("dir")
            .count(),
        0
    );
}

#[test]
fn segments_rotate_by_size_by_age_and_on_request() {
    let dir = TempDir::new("rotation");
    let mut cfg = config(dir.path());
    cfg.segment_max_bytes = MIN_SEGMENT_MAX_BYTES;
    cfg.chunk_max_capture_span = Duration::from_millis(100); // 10 records per chunk
    let store = RecordingStore::open(cfg).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let id = session.id();
    let mut rng = Rng::new(11);
    let mut appended = Vec::new();
    // Incompressible 300-byte payloads: ~3 KB chunks, so a 4 KiB segment holds about one chunk.
    for i in 0..200u64 {
        let a = Appended {
            kind: RecordKind::RawLmuBlock,
            capture_ns: i * PERIOD_NS,
            bytes: rng.bytes(300),
        };
        session
            .append(a.kind, a.capture_ns, &a.bytes)
            .expect("append");
        appended.push(a);
    }
    session.sync(WAIT).expect("sync");
    let before_request = store.segments(id).expect("segments").len();
    session.request_rotation().expect("rotate");
    let a = Appended {
        kind: RecordKind::Marker,
        capture_ns: 200 * PERIOD_NS,
        bytes: b"after".to_vec(),
    };
    session
        .append(a.kind, a.capture_ns, &a.bytes)
        .expect("append");
    appended.push(a);
    let summary = session.finish().expect("finish");

    let segs = store.segments(id).expect("segments");
    assert!(
        segs.len() >= 10,
        "size rotation produced {} segments",
        segs.len()
    );
    assert_eq!(
        segs.len(),
        before_request + 1,
        "the requested rotation put the last record in a new segment"
    );
    assert_eq!(summary.segments.len(), segs.len());
    for (i, s) in segs.iter().enumerate() {
        assert_eq!(
            (s.segment_index, s.status, s.integrity),
            (i as u32, SegmentStatus::Closed, IntegrityStatus::Ok)
        );
        let scan = validate_segment_file(&segment_path(&store, id, i)).expect("scan");
        assert_eq!(
            (scan.header.segment_index, scan.stop.clone()),
            (i as u32, ScanStop::CleanEof)
        );
        assert_eq!(scan.header.session_id, id.0);
        assert_eq!(scan.valid_len, s.byte_size);
    }
    assert_eq!(
        segs.iter().map(|s| s.record_count).sum::<u64>(),
        appended.len() as u64
    );
    assert!(
        same_records(&read_all(&store, id), &appended),
        "records read back across segments in order"
    );

    // Rotation by age: checked when a chunk is written; a segment is opened lazily by the first chunk after
    // a rotation, so with writes 60 ms apart and a 40 ms limit the layout is [b0] [b1 b2] [b3 b4].
    let dir = TempDir::new("rotation-age");
    let mut cfg = config(dir.path());
    cfg.segment_max_duration = Duration::from_millis(40);
    let store = RecordingStore::open(cfg).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let id = session.id();
    for batch in 0..5u64 {
        session
            .append(RecordKind::Marker, batch * PERIOD_NS, b"m")
            .expect("append");
        std::thread::sleep(Duration::from_millis(60));
        session.sync(WAIT).expect("sync");
    }
    session.finish().expect("finish");
    let segs = store.segments(id).expect("segments");
    assert_eq!(
        segs.iter().map(|s| s.record_count).collect::<Vec<_>>(),
        vec![1, 2, 2]
    );
    assert_eq!(read_all(&store, id).len(), 5);
}

#[test]
fn a_corrupted_middle_chunk_is_detected_reported_and_skipped_by_readers() {
    let dir = TempDir::new("corrupt-middle");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let id = session.id();
    let mut rng = Rng::new(3);
    let mut buf = Vec::new();
    let mut appended = Vec::new();
    for i in 0..1_000u64 {
        buf.clear();
        telemetry_block(&mut buf, i * PERIOD_NS, CHANNELS, &mut rng);
        session
            .append(RecordKind::TelemetryTlv, i * PERIOD_NS, &buf)
            .expect("append");
        appended.push(Appended {
            kind: RecordKind::TelemetryTlv,
            capture_ns: i * PERIOD_NS,
            bytes: buf.clone(),
        });
    }
    session.finish().expect("finish");
    let chunks = store.chunks(id, 0).expect("chunks");
    assert_eq!(chunks.len(), 10);
    let path = segment_path(&store, id, 0);
    let mut bytes = fs::read(&path).expect("read");
    let victim = chunks[4];
    let at = usize::try_from(victim.offset).expect("offset") + CHUNK_HEADER_LEN + 17;
    bytes[at] ^= 0x08;
    fs::write(&path, &bytes).expect("corrupt one bit");

    // The scanner stops at the damaged chunk and says where and why.
    let scan = validate_segment_file(&path).expect("scan");
    assert_eq!((scan.chunks.len(), scan.valid_len), (4, victim.offset));
    assert!(
        matches!(&scan.stop, ScanStop::Corrupt { offset, error: FormatError::ChunkBodyCrc { .. } } if *offset == victim.offset),
        "{:?}",
        scan.stop
    );

    // Validation records it in the index.
    let v = store.validate_session(id).expect("validate");
    assert_eq!(v.integrity, IntegrityStatus::Damaged);
    let detail = v.integrity_detail.clone().unwrap_or_default();
    assert!(
        detail.contains(&format!("corrupt chunk at byte {}", victim.offset)),
        "{detail}"
    );
    let info = store.session(id).expect("read").expect("present");
    assert_eq!(
        (info.status, info.integrity),
        (SessionStatus::Complete, IntegrityStatus::Damaged)
    );
    assert_eq!(
        fs::read(&path).expect("read"),
        bytes,
        "validation never modifies the file"
    );

    // Readers report the damaged chunk and still return every intact record after it.
    let results: Vec<Result<RecordedRecord, RecorderError>> = store
        .open_reader(id)
        .expect("reader")
        .records(TimeRange::ALL)
        .expect("stream")
        .collect();
    let errors: Vec<&RecorderError> = results.iter().filter_map(|r| r.as_ref().err()).collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(errors[0], RecorderError::Format { offset, source: FormatError::ChunkBodyCrc { .. }, .. } if *offset == victim.offset),
        "{:?}",
        errors[0]
    );
    let good: Vec<RecordedRecord> = results.into_iter().filter_map(Result::ok).collect();
    let lost = TimeRange::new(victim.min_capture_ns, victim.max_capture_ns).expect("range");
    let expected: Vec<Appended> = appended
        .into_iter()
        .filter(|a| !lost.contains(a.capture_ns))
        .collect();
    assert!(
        same_records(&good, &expected),
        "everything outside the damaged chunk is returned intact"
    );
}

#[test]
fn overrun_accounting_is_exact_under_load_from_several_producers() {
    const PRODUCERS: u64 = 4;
    const PER_PRODUCER: u64 = 10_000;
    let dir = TempDir::new("overrun-load");
    let mut cfg = config(dir.path());
    cfg.queue_capacity_records = 4;
    let store = RecordingStore::open(cfg).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let id = session.id();
    let events = session.events().clone();
    let accepted = Arc::new(AtomicU64::new(0));
    let overrun = Arc::new(AtomicU64::new(0));
    // One shared capture clock, so records arrive in (nearly) capture order as they would from LMU.
    let clock = Arc::new(AtomicU64::new(0));
    std::thread::scope(|s| {
        for p in 0..PRODUCERS {
            let sender = session.sender();
            let (accepted, overrun, clock) = (
                Arc::clone(&accepted),
                Arc::clone(&overrun),
                Arc::clone(&clock),
            );
            s.spawn(move || {
                let payload = vec![p as u8; 512];
                for _ in 0..PER_PRODUCER {
                    let capture_ns = clock.fetch_add(1, Ordering::Relaxed) * PERIOD_NS;
                    match sender.append(RecordKind::TelemetryTlv, capture_ns, &payload) {
                        Ok(()) => accepted.fetch_add(1, Ordering::Relaxed),
                        Err(AppendError::Overrun) => overrun.fetch_add(1, Ordering::Relaxed),
                        Err(e) => panic!("unexpected append error {e}"),
                    };
                }
            });
        }
    });
    let summary = session.finish().expect("finish");
    let (accepted, overrun) = (
        accepted.load(Ordering::Relaxed),
        overrun.load(Ordering::Relaxed),
    );
    assert_eq!(accepted + overrun, PRODUCERS * PER_PRODUCER);
    assert!(
        overrun > 0,
        "a 4-record queue under four unthrottled producers must overrun"
    );
    let s = &summary.stats;
    assert_eq!(
        (s.records_accepted, s.records_overrun, s.records_written),
        (accepted, overrun, accepted)
    );
    assert_eq!(s.records_lost(), overrun);

    // Visible everywhere: events, index, and the recording itself.
    let reported = events
        .try_iter()
        .filter_map(|e| match e {
            RecorderEvent::RecorderOverrun { total_dropped, .. } => Some(total_dropped),
            _ => None,
        })
        .max();
    assert_eq!(reported, Some(overrun));
    assert_eq!(
        store
            .session(id)
            .expect("read")
            .expect("present")
            .dropped_records,
        overrun
    );
    let records = read_all(&store, id);
    let noted: u64 = records
        .iter()
        .filter_map(RecordedRecord::gap_note)
        .map(|n| n.expect("note"))
        .filter(|n| n.reason == GapReason::Overrun)
        .map(|n| n.dropped_records)
        .sum();
    assert_eq!(
        noted, overrun,
        "every dropped record is accounted for by gap notes in the file"
    );
    assert_eq!(
        records
            .iter()
            .filter(|r| r.kind == RecordKind::TelemetryTlv)
            .count() as u64,
        accepted
    );
}

/// Selects exactly the given ids, eligible or not.
struct Select(Vec<SessionId>);

impl RetentionPolicy for Select {
    fn select(&self, _candidates: &[SessionInfo], _now_unix_ns: i64) -> Vec<SessionId> {
        self.0.clone()
    }
}

#[test]
fn retention_never_deletes_by_default_and_respects_keep_and_active_sessions() {
    let dir = TempDir::new("retention");
    let store = RecordingStore::open(config(dir.path())).expect("store");
    let mut finished = Vec::new();
    for _ in 0..3 {
        let session = store
            .start_session(meta(), SessionOptions::new())
            .expect("session");
        session.append(RecordKind::Marker, 1, b"x").expect("append");
        finished.push(session.finish().expect("finish").session_id);
    }
    let active = store
        .start_session(meta(), SessionOptions::new())
        .expect("active session");
    let dir_of = |id: SessionId| {
        let info = store.session(id).expect("read").expect("present");
        store.config().resolve(Path::new(&info.dir))
    };
    let dirs: Vec<_> = finished.iter().map(|&id| dir_of(id)).collect();

    let report = store.apply_retention(&KeepAll).expect("keep all");
    assert!(report.deleted.is_empty() && report.refused.is_empty() && report.failed.is_empty());
    assert_eq!(store.sessions().expect("sessions").len(), 4);

    store
        .set_retention(finished[1], RetentionClass::Keep)
        .expect("keep");
    let report = store
        .apply_retention(&Select(vec![finished[1], active.id()]))
        .expect("select ineligible");
    assert!(report.deleted.is_empty());
    assert_eq!(
        report.refused.iter().copied().collect::<HashSet<_>>(),
        HashSet::from([finished[1], active.id()])
    );

    let report = store
        .apply_retention(&MaxAge {
            max_age: Duration::ZERO,
        })
        .expect("max age");
    assert_eq!(
        report.deleted.iter().copied().collect::<HashSet<_>>(),
        HashSet::from([finished[0], finished[2]])
    );
    assert!(!dirs[0].exists() && dirs[1].exists() && !dirs[2].exists());
    assert!(store.session(finished[0]).expect("read").is_none());
    assert!(
        store.session(finished[1]).expect("read").is_some(),
        "KEEP sessions are never offered to a policy"
    );

    assert!(matches!(
        store.delete_session(active.id()),
        Err(RecorderError::SessionBusy(_))
    ));
    store
        .delete_session(finished[1])
        .expect("an explicit delete removes a KEEP session");
    assert!(!dirs[1].exists());
    assert!(matches!(
        store.delete_session(finished[1]),
        Err(RecorderError::SessionNotFound(_))
    ));
    active.finish().expect("finish");
}

#[test]
fn a_recordings_directory_has_a_single_owner() {
    let dir = TempDir::new("lock");
    let first = RecordingStore::open(RecorderConfig::new(dir.path())).expect("first owner");
    assert!(matches!(
        RecordingStore::open(RecorderConfig::new(dir.path())),
        Err(RecorderError::DirectoryInUse { .. })
    ));
    drop(first);
    RecordingStore::open(RecorderConfig::new(dir.path()))
        .expect("free again once the owner is gone");
}

#[test]
fn a_live_session_keeps_the_directory_locked_after_its_store_is_dropped() {
    let dir = TempDir::new("lock-live");
    let store = RecordingStore::open(config(dir.path())).expect("first owner");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("start");
    let id = session.id();
    session
        .append(RecordKind::Marker, 0, b"live")
        .expect("append");
    session.sync(WAIT).expect("sync");
    drop(store);
    // The writer still records into the directory: nobody else may open (and "recover") it.
    assert!(matches!(
        RecordingStore::open(config(dir.path())),
        Err(RecorderError::DirectoryInUse { .. })
    ));
    session
        .append(RecordKind::Marker, PERIOD_NS, b"still live")
        .expect("append after the store is gone");
    session.finish().expect("finish");
    let reopened = RecordingStore::open(config(dir.path())).expect("free once the writer is done");
    assert!(
        reopened.recovery_report().sessions.is_empty(),
        "a finished session needs no recovery"
    );
    let info = reopened.session(id).expect("read").expect("row");
    assert_eq!(info.status, SessionStatus::Complete);
    assert_eq!(info.record_count, 2);
}

#[test]
fn revalidation_keeps_reporting_damage_in_the_final_chunk() {
    let dir = TempDir::new("revalidate");
    let store = RecordingStore::open(config(dir.path())).expect("open");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("start");
    let id = session.id();
    for i in 0..300u64 {
        session
            .append(RecordKind::Marker, i * PERIOD_NS, &i.to_le_bytes())
            .expect("append");
    }
    session.finish().expect("finish");
    let before = store.session(id).expect("read").expect("row");
    let bounds = store.open_reader(id).expect("reader").capture_bounds();
    let seg_before = store.segments(id).expect("segments")[0].clone();
    let path = segment_path(&store, id, 0);
    let mut bytes = fs::read(&path).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&path, &bytes).expect("damage the final chunk");

    for round in 0..2 {
        let v = store.validate_session(id).expect("validate");
        assert_eq!(
            v.integrity,
            IntegrityStatus::Damaged,
            "round {round}: {v:?}"
        );
        let info = store.session(id).expect("read").expect("row");
        assert_eq!(info.integrity, IntegrityStatus::Damaged, "round {round}");
        assert_eq!(
            (
                info.record_count,
                info.first_capture_ns,
                info.last_capture_ns
            ),
            (
                before.record_count,
                before.first_capture_ns,
                before.last_capture_ns
            ),
            "round {round}: the recorded extent is kept"
        );
        assert_eq!(
            store.open_reader(id).expect("reader").capture_bounds(),
            bounds,
            "round {round}"
        );
        let seg = &store.segments(id).expect("segments")[0];
        assert_eq!(
            (seg.byte_size, seg.file_crc32c, seg.record_count),
            (
                seg_before.byte_size,
                seg_before.file_crc32c,
                seg_before.record_count
            ),
            "round {round}: the durable reference is kept"
        );
    }
}

#[test]
fn invalid_json_is_refused_up_front_and_valid_rows_still_land() {
    let dir = TempDir::new("json");
    let store = RecordingStore::open(config(dir.path())).expect("open");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("start");
    let id = session.id();
    let lap = |metrics_json: Option<&str>| LapRecord {
        lap_number: 3,
        stint_number: Some(1),
        start_capture_ns: 0,
        end_capture_ns: Some(1_000),
        lap_time_s: Some(123.4),
        sector_times_s: [None, None, None],
        valid: Some(true),
        pit_lap: Some(false),
        metrics_json: metrics_json.map(str::to_owned),
    };
    let stint = |metrics_json: Option<&str>| StintRecord {
        stint_number: 1,
        start_capture_ns: 0,
        end_capture_ns: None,
        start_lap: Some(1),
        end_lap: None,
        driver: None,
        metrics_json: metrics_json.map(str::to_owned),
    };
    let bad_meta = SessionMetaUpdate {
        track: Some("Spa".into()),
        data_availability_json: Some("{not json".into()),
        ..Default::default()
    };
    assert!(matches!(
        session.update_meta(bad_meta),
        Err(AppendError::InvalidJson { .. })
    ));
    assert!(matches!(
        session.record_lap(lap(Some("[1,"))),
        Err(AppendError::InvalidJson { .. })
    ));
    assert!(matches!(
        session.record_stint(stint(Some("nope"))),
        Err(AppendError::InvalidJson { .. })
    ));
    assert_eq!(session.stats().control_dropped, 3);
    session
        .update_meta(SessionMetaUpdate {
            track: Some("Spa".into()),
            data_availability_json: Some(r#"{"tyres":true}"#.into()),
            ..Default::default()
        })
        .expect("valid meta");
    session.record_lap(lap(Some("[1,2]"))).expect("valid lap");
    session.record_stint(stint(None)).expect("valid stint");
    let summary = session.finish().expect("finish");
    assert_eq!(summary.stats.index_errors, 0, "{summary:?}");
    let info = store.session(id).expect("read").expect("row");
    assert_eq!(info.meta.track.as_deref(), Some("Spa"));
    let laps = store.laps(id).expect("laps");
    assert_eq!(laps.len(), 1);
    assert_eq!(laps[0].lap_time_s, Some(123.4));
    assert_eq!(store.stints(id).expect("stints").len(), 1);
}

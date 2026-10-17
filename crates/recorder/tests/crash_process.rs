//! A real process death in the middle of a recording: a child process (this test binary, re-invoked) records,
//! syncs, keeps appending and then exits abruptly without running any destructor (no `finish`, no flush, no
//! index update; the writer thread dies wherever it is). The parent then opens the directory, which runs
//! startup recovery.

mod common;

use std::process::Command;
use std::time::Duration;

use common::{CHANNELS, PERIOD_NS, Rng, TempDir, meta, read_all, telemetry_block};
use recorder::{
    IntegrityStatus, RecordKind, RecorderConfig, RecordingStore, SessionOptions, SessionStatus,
};

/// Set (to the recordings directory) only in the child process.
const CHILD_ENV: &str = "LMU_RECORDER_CRASH_CHILD_DIR";
/// Exit code proving the child reached the abrupt exit (and did not fail earlier).
const CHILD_EXIT_CODE: i32 = 42;
const SYNCED_RECORDS: u64 = 300;
const UNSYNCED_RECORDS: u64 = 250;

fn payload(i: u64) -> Vec<u8> {
    let mut rng = Rng::new(i + 1);
    let mut buf = Vec::new();
    telemetry_block(&mut buf, i * PERIOD_NS, CHANNELS, &mut rng);
    buf
}

#[test]
fn a_killed_recording_process_leaves_a_recoverable_session() {
    let dir = TempDir::new("process-kill");
    let exe = std::env::current_exe().expect("test binary path");
    let status = Command::new(exe)
        .args(["--exact", "crash_child", "--ignored", "--test-threads", "1"])
        .env(CHILD_ENV, dir.path())
        .status()
        .expect("spawn the child");
    assert_eq!(
        status.code(),
        Some(CHILD_EXIT_CODE),
        "the child must die abruptly while recording"
    );

    let store =
        RecordingStore::open(RecorderConfig::new(dir.path())).expect("reopen after the crash");
    let report = store.recovery_report();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(
        report.sessions.len(),
        1,
        "the session left ACTIVE by the dead process is recovered"
    );
    let r = &report.sessions[0];
    assert!(
        matches!(
            r.integrity,
            IntegrityStatus::Ok | IntegrityStatus::Truncated
        ),
        "a crash can only tear the tail: {:?} {:?}",
        r.integrity,
        r.integrity_detail
    );
    let info = store.session(r.session_id).expect("read").expect("present");
    assert_eq!(info.status, SessionStatus::Interrupted);
    assert!(info.recovered_unix_ns.is_some() && info.ended_unix_ns.is_none());

    // What survives is a gap-free prefix that contains at least everything synced, byte-exact.
    let records = read_all(&store, r.session_id);
    let n = records.len() as u64;
    assert!(
        (SYNCED_RECORDS..=SYNCED_RECORDS + UNSYNCED_RECORDS).contains(&n),
        "{n} records survived; {SYNCED_RECORDS} were synced"
    );
    for (i, rec) in (0u64..).zip(&records) {
        assert_eq!(
            (rec.kind, rec.capture_ns),
            (RecordKind::TelemetryTlv, i * PERIOD_NS)
        );
        assert_eq!(rec.bytes, payload(i), "record {i} is byte-exact");
    }
    assert_eq!(
        (info.record_count, info.last_capture_ns),
        (n, Some((n - 1) * PERIOD_NS))
    );
}

/// The child half of [`a_killed_recording_process_leaves_a_recoverable_session`]; does nothing unless run by
/// it.
#[test]
#[ignore = "run as a child process by a_killed_recording_process_leaves_a_recoverable_session"]
fn crash_child() {
    let Ok(dir) = std::env::var(CHILD_ENV) else {
        return;
    };
    let store = RecordingStore::open(RecorderConfig::new(dir)).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    for i in 0..SYNCED_RECORDS {
        session
            .append(RecordKind::TelemetryTlv, i * PERIOD_NS, &payload(i))
            .expect("append");
    }
    session.sync(Duration::from_secs(20)).expect("sync");
    for i in SYNCED_RECORDS..SYNCED_RECORDS + UNSYNCED_RECORDS {
        session
            .append(RecordKind::TelemetryTlv, i * PERIOD_NS, &payload(i))
            .expect("append");
    }
    // Terminates the process at once: no destructor runs on any thread, the writer dies mid-flight.
    std::process::exit(CHILD_EXIT_CODE);
}

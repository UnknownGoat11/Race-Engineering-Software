//! Integration and verification tests for `RecorderSupervisor` (Phase 1 Lane E).
//!
//! Covers:
//! - Store creation in temporary directory and startup recovery.
//! - One session per LMU session identity (clean rotation on identity change).
//! - Manual enable/disable control (`set_enabled`).
//! - Simulated disk-full probe: transitions to `Suspended` and recovers to `Active`.
//! - Record append and `SessionReader` replay roundtrip byte-for-byte equality.
//! - LMU stall and session re-attachment lifecycle.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use app_core::driver::recorder_sup::{
    DiskProbeFactory, RecorderPollOutcome, RecorderSupervisor, RecorderSupervisorConfig,
    RecordingStatus, SimulatedDiskProbe,
};
use recorder::{RecordKind, RecorderConfig, SessionMeta, TimeRange};
use telemetry_model::events::SessionEvent;

fn make_temp_store_dir(test_name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    dir.push(format!(
        "appcore_rec_{test_name}_{}_{ts}",
        std::process::id()
    ));
    dir
}

fn make_test_meta(track: &str, session_type: &str) -> SessionMeta {
    SessionMeta {
        app_version: "1.0.0".to_string(),
        telemetry_schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        protocol_version: Some(1),
        lmu_version: Some("1.4.2.0".to_string()),
        interface_version: Some("v1".to_string()),
        calculation_engine_version: Some("v1".to_string()),
        vehicle: Some("Ferrari 499P".to_string()),
        track: Some(track.to_string()),
        session_type: Some(session_type.to_string()),
        data_availability_json: None,
    }
}

#[test]
fn recorder_supervisor_initialization_and_temp_dir() {
    let dir = make_temp_store_dir("init");
    let config = RecorderSupervisorConfig {
        recorder: RecorderConfig::new(&dir),
        enabled: true,
    };

    let sup = RecorderSupervisor::open(config, None, None).expect("open recorder supervisor");
    assert_eq!(sup.status(), RecordingStatus::Off);
    assert!(sup.is_enabled());
    assert_eq!(sup.active_session_id(), None);

    let dto = sup.status_dto();
    assert_eq!(dto.state, "off");
    assert_eq!(dto.recording_id, None);
    assert_eq!(dto.bytes_written, 0);
    assert_eq!(dto.frames_written, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recorder_supervisor_one_session_per_identity() {
    let dir = make_temp_store_dir("identity");
    let config = RecorderSupervisorConfig {
        recorder: RecorderConfig::new(&dir),
        enabled: true,
    };

    let mut sup = RecorderSupervisor::open(config, None, None).expect("open supervisor");

    // Session 1: SPA practice
    let meta1 = make_test_meta("SPA_7004", "Practice");
    let evs1 = sup
        .on_session_ready(meta1.clone())
        .expect("start session 1");
    assert_eq!(evs1.len(), 1);
    assert!(matches!(evs1[0], SessionEvent::RecordingStarted { .. }));

    let id1 = sup.active_session_id().expect("session 1 active");
    assert_eq!(sup.status(), RecordingStatus::Active);

    // Repeated call with the exact same identity does not restart:
    let evs_repeat = sup.on_session_ready(meta1).expect("repeat identity");
    assert!(evs_repeat.is_empty(), "repeat identity must be a no-op");
    assert_eq!(sup.active_session_id(), Some(id1));

    // Session 2: Qualifying on Monza -> different identity rotates session:
    let meta2 = make_test_meta("Monza_5793", "Qualifying");
    let evs2 = sup.on_session_ready(meta2).expect("rotate session 2");
    assert_eq!(evs2.len(), 2);
    assert!(matches!(evs2[0], SessionEvent::RecordingStopped { .. }));
    assert!(matches!(evs2[1], SessionEvent::RecordingStarted { .. }));

    let id2 = sup.active_session_id().expect("session 2 active");
    assert_ne!(id1, id2, "new identity must produce new session id");

    // Finish session cleanly:
    let evs_end = sup.on_session_end();
    assert_eq!(evs_end.len(), 1);
    assert!(matches!(evs_end[0], SessionEvent::RecordingStopped { .. }));
    assert_eq!(sup.status(), RecordingStatus::Off);
    assert_eq!(sup.active_session_id(), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recorder_supervisor_manual_disable_and_enable() {
    let dir = make_temp_store_dir("manual_toggle");
    let config = RecorderSupervisorConfig {
        recorder: RecorderConfig::new(&dir),
        enabled: true,
    };

    let mut sup = RecorderSupervisor::open(config, None, None).expect("open supervisor");
    let meta = make_test_meta("LeMans_13626", "Race");
    sup.on_session_ready(meta).expect("start session");
    assert_eq!(sup.status(), RecordingStatus::Active);

    // Disable recording:
    let evs_off = sup.set_enabled(false).expect("disable recording");
    assert_eq!(evs_off.len(), 1);
    assert!(matches!(evs_off[0], SessionEvent::RecordingStopped { .. }));
    assert_eq!(sup.status(), RecordingStatus::Off);
    assert_eq!(sup.active_session_id(), None);

    // Appending while disabled is a no-op:
    let append_res = sup.append_telemetry(1_000_000, b"telemetry-payload");
    assert!(
        append_res.is_ok(),
        "append while disabled must succeed silently"
    );

    // Re-enable recording: re-starts session with remembered identity:
    let evs_on = sup.set_enabled(true).expect("enable recording");
    assert_eq!(evs_on.len(), 1);
    assert!(matches!(evs_on[0], SessionEvent::RecordingStarted { .. }));
    assert_eq!(sup.status(), RecordingStatus::Active);

    let _ = sup.on_session_end();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recorder_supervisor_simulated_disk_full_probe_and_recovery() {
    let dir = make_temp_store_dir("disk_probe");

    // High initial free disk space: 50 GB
    let probe = Arc::new(SimulatedDiskProbe::new(50 * 1024 * 1024 * 1024));
    let p_factory: DiskProbeFactory = {
        let p = Arc::clone(&probe);
        Arc::new(move || Box::new((*p).clone()))
    };

    let mut recorder_cfg = RecorderConfig::new(&dir);
    recorder_cfg.disk_check_interval = Duration::from_millis(10);
    recorder_cfg.sync_interval = Duration::from_millis(10);

    let config = RecorderSupervisorConfig {
        recorder: recorder_cfg,
        enabled: true,
    };

    let mut sup = RecorderSupervisor::open(config, Some(p_factory), None).expect("open supervisor");
    let meta = make_test_meta("Sebring_6019", "Race");
    sup.on_session_ready(meta).expect("start session");
    assert_eq!(sup.status(), RecordingStatus::Active);

    // Append some initial records:
    sup.append_telemetry(1_000_000_000, b"frame-1")
        .expect("append 1");
    sup.sync(Duration::from_secs(1)).expect("sync 1");

    // Simulate disk dropping below critical threshold (e.g. 5 MB < 100 MB default):
    probe.set_available_bytes(5 * 1024 * 1024);

    // Append records while disk is full:
    for i in 2..=10 {
        sup.append_telemetry(1_000_000_000 + i * 10_000_000, b"frame-filling")
            .expect("append filling");
    }

    // Wait for disk check interval to trigger:
    std::thread::sleep(Duration::from_millis(80));

    // Poll events: supervisor transitions to Suspended:
    let outcome: RecorderPollOutcome = sup.poll_events();
    assert_eq!(
        outcome.status,
        RecordingStatus::Suspended,
        "supervisor status must be Suspended when disk is critically low"
    );

    let alert_emitted = outcome
        .session_events
        .iter()
        .any(|e| matches!(e, SessionEvent::AlertRaised { alert_id, .. } if alert_id == "disk_space_critical"));
    assert!(alert_emitted, "critical disk space alert must be raised");

    // Now restore free space well above resume threshold (e.g. 50 GB > 1 GB default):
    probe.set_available_bytes(50 * 1024 * 1024 * 1024);

    std::thread::sleep(Duration::from_millis(80));

    // Append a record to prompt writer wake-up:
    let _ = sup.append_telemetry(2_000_000_000, b"frame-resumed");

    std::thread::sleep(Duration::from_millis(80));

    // Poll events again: supervisor transitions back to Active:
    let outcome2 = sup.poll_events();
    assert_eq!(
        outcome2.status,
        RecordingStatus::Active,
        "supervisor status must recover to Active when disk space is restored"
    );

    let alert_cleared = outcome2
        .session_events
        .iter()
        .any(|e| matches!(e, SessionEvent::AlertCleared { alert_id } if alert_id == "disk_space_critical"));
    assert!(
        alert_cleared,
        "critical disk space alert must be cleared upon recovery"
    );

    let _ = sup.on_session_end();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recorder_supervisor_record_and_session_reader_replay_equality() {
    let dir = make_temp_store_dir("roundtrip");
    let mut recorder_cfg = RecorderConfig::new(&dir);
    recorder_cfg.sync_interval = Duration::from_millis(10);

    let config = RecorderSupervisorConfig {
        recorder: recorder_cfg,
        enabled: true,
    };

    let mut sup = RecorderSupervisor::open(config, None, None).expect("open supervisor");
    let meta = make_test_meta("Interlagos_4309", "Qualifying");
    sup.on_session_ready(meta).expect("start session");

    let session_id = sup.active_session_id().expect("session id");

    // Prepare expected record payloads:
    let test_records: Vec<(RecordKind, u64, Vec<u8>)> = vec![
        (
            RecordKind::TelemetryTlv,
            1_000_000_000,
            b"tlv-frame-001".to_vec(),
        ),
        (
            RecordKind::TelemetryTlv,
            1_010_000_000,
            b"tlv-frame-002".to_vec(),
        ),
        (
            RecordKind::ScoringPostcard,
            1_015_000_000,
            b"scoring-snapshot-1".to_vec(),
        ),
        (
            RecordKind::TelemetryTlv,
            1_020_000_000,
            b"tlv-frame-003".to_vec(),
        ),
        (
            RecordKind::Marker,
            1_025_000_000,
            b"User bookmark: lockup turn 1".to_vec(),
        ),
        (
            RecordKind::TelemetryTlv,
            1_030_000_000,
            b"tlv-frame-004".to_vec(),
        ),
        (
            RecordKind::WeatherTlv,
            1_035_000_000,
            b"weather-update-1".to_vec(),
        ),
        (
            RecordKind::SessionEvent,
            1_040_000_000,
            b"session-event-sector1".to_vec(),
        ),
        (
            RecordKind::TelemetryTlv,
            1_050_000_000,
            b"tlv-frame-005".to_vec(),
        ),
    ];

    // Append each record through supervisor:
    for (kind, capture_ns, data) in &test_records {
        match kind {
            RecordKind::TelemetryTlv => {
                sup.append_telemetry(*capture_ns, data).expect("append tel")
            }
            RecordKind::ScoringPostcard => sup
                .append_scoring(*capture_ns, data)
                .expect("append scoring"),
            RecordKind::Marker => {
                let text = std::str::from_utf8(data).expect("valid utf-8");
                sup.append_marker(*capture_ns, text).expect("append marker");
            }
            RecordKind::WeatherTlv => sup
                .append_weather(*capture_ns, data)
                .expect("append weather"),
            RecordKind::SessionEvent => sup
                .append_session_event(*capture_ns, data)
                .expect("append event"),
            _ => unreachable!(),
        }
    }

    // Durably sync and finish session:
    sup.sync(Duration::from_secs(1)).expect("sync session");
    let _ = sup.on_session_end();

    // Replay session via SessionReader:
    let store = sup.store();
    let reader = store.open_reader(session_id).expect("open session reader");

    let stream = reader.records(TimeRange::ALL).expect("open record stream");
    let mut replayed_records = Vec::new();

    for res in stream {
        let record = res.expect("next record");
        // Ignore internal recorder notes (e.g. GapNote):
        if record.kind != RecordKind::RecorderNote {
            replayed_records.push((record.kind, record.capture_ns, record.bytes));
        }
    }

    assert_eq!(
        replayed_records.len(),
        test_records.len(),
        "replayed record count must match appended count"
    );

    for (i, (expected, actual)) in test_records.iter().zip(replayed_records.iter()).enumerate() {
        assert_eq!(expected.0, actual.0, "record {i} kind mismatch");
        assert_eq!(expected.1, actual.1, "record {i} capture_ns mismatch");
        assert_eq!(expected.2, actual.2, "record {i} payload bytes mismatch");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recorder_supervisor_stall_and_lmu_return_lifecycle() {
    let dir = make_temp_store_dir("stall_lifecycle");
    let config = RecorderSupervisorConfig {
        recorder: RecorderConfig::new(&dir),
        enabled: true,
    };

    let mut sup = RecorderSupervisor::open(config, None, None).expect("open supervisor");
    let meta = make_test_meta("Bahrain_5412", "Practice");

    // Start session:
    sup.on_session_ready(meta.clone()).expect("session ready");
    assert_eq!(sup.status(), RecordingStatus::Active);
    let id1 = sup.active_session_id().expect("id1");

    // LMU stalls (in menus / paused):
    let evs_stall = sup.on_session_stalled();
    assert_eq!(evs_stall.len(), 1);
    assert!(matches!(
        evs_stall[0],
        SessionEvent::RecordingStopped { .. }
    ));
    assert_eq!(sup.status(), RecordingStatus::Off);
    assert_eq!(sup.active_session_id(), None);

    // LMU returns with telemetry:
    let evs_return = sup.on_session_ready(meta).expect("session re-ready");
    assert_eq!(evs_return.len(), 1);
    assert!(matches!(
        evs_return[0],
        SessionEvent::RecordingStarted { .. }
    ));
    assert_eq!(sup.status(), RecordingStatus::Active);
    let id2 = sup.active_session_id().expect("id2");
    assert_ne!(
        id1, id2,
        "resuming after stall starts fresh recording segment"
    );

    let _ = sup.on_session_end();
    let _ = std::fs::remove_dir_all(&dir);
}

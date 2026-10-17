//! Integration tests for DriverSessionMachine (Phase 1 Lane F: table-driven session lifecycle).
//!
//! Blueprint §5, §12, §14:
//! - LMU exit/restart
//! - Stall vs paused
//! - Incompatible layout/version
//! - Auto-stream gating
//! - Sleep/resume (clock skew)
//! - Identity change (track/car change keeps relay session per Q3)
//! - Manual controls and status DTO emission

use app_core::bridge::status::Origin;
use app_core::driver::session::{
    DriverSessionMachine, DriverSessionSettings, DriverSessionStage, SessionAction,
    SessionInputEvent, is_identity_complete,
};
use lmu_reader::ReaderState;
use protocol::messages::LmuSourceState;
use telemetry_model::enums::{SessionType, VehicleClass};
use telemetry_model::events::{SessionEvent, Severity};
use telemetry_model::scoring::SessionIdentity;

fn sample_identity() -> SessionIdentity {
    SessionIdentity {
        vehicle_name: Some("Ferrari 499P #51".to_string()),
        vehicle_model: Some("Ferrari 499P".to_string()),
        vehicle_class: Some(VehicleClass::Hypercar),
        class_name: Some("Hypercar".to_string()),
        track_name: Some("Circuit de la Sarthe".to_string()),
        session_type: Some(SessionType::Practice(1)),
        game_version: Some(10402),
    }
}

#[test]
fn identity_completeness_rules() {
    let mut id = sample_identity();
    assert!(is_identity_complete(&id, Some(10.5)));

    // Negative or non-finite ET is not sane
    assert!(!is_identity_complete(&id, Some(-1.0)));
    assert!(!is_identity_complete(&id, Some(f64::NAN)));
    assert!(!is_identity_complete(&id, None));

    // Missing track
    id.track_name = None;
    assert!(!is_identity_complete(&id, Some(10.5)));
    id.track_name = Some("   ".to_string());
    assert!(!is_identity_complete(&id, Some(10.5)));

    // Restored track, missing vehicle
    id.track_name = Some("Spa".to_string());
    id.vehicle_name = None;
    id.vehicle_model = None;
    assert!(!is_identity_complete(&id, Some(10.5)));

    // Restored vehicle, missing session type
    id.vehicle_name = Some("BMW M Hybrid V8".to_string());
    id.session_type = None;
    assert!(!is_identity_complete(&id, Some(10.5)));

    id.session_type = Some(SessionType::Race(1));
    assert!(is_identity_complete(&id, Some(10.5)));
}

#[test]
fn session_lifecycle_lmu_exit_and_restart() {
    let settings = DriverSessionSettings {
        auto_stream: false,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine = DriverSessionMachine::new(settings);

    // Initial state: Idle, LmuSource NotRunning
    assert_eq!(machine.stage(), DriverSessionStage::Idle);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::NotRunning);

    // Reader attaches to LMU
    let actions = machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    assert_eq!(actions, vec![]);
    assert_eq!(machine.stage(), DriverSessionStage::Connecting);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Connecting);

    // First frame arrives with complete identity
    let id = sample_identity();
    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 12.5,
        identity: id.clone(),
        session_id: Some(100),
        now_unix_s: Some(1_700_000_000),
    });

    assert_eq!(machine.stage(), DriverSessionStage::SessionReady);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Live);
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::LmuConnected {
            game_version: Some(10402)
        }
    )));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, SessionAction::StartRecording(_)))
    );

    // Driver manually starts streaming
    machine.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    let actions = machine.step(SessionInputEvent::ManualStartStreaming);
    assert_eq!(actions, vec![SessionAction::StartStreaming]);
    assert_eq!(machine.stage(), DriverSessionStage::Streaming);
    assert!(machine.is_streaming());

    // LMU closes / exits: reader reports WaitingForLmu
    let actions = machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::WaitingForLmu,
        detail: Some("LMU closed by user".to_string()),
        game_version: None,
    });

    assert_eq!(machine.stage(), DriverSessionStage::WaitingForLmu);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::NotRunning);
    assert!(actions.contains(&SessionAction::SendStaleBlock));
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::LmuDisconnected {
            reason: "LMU closed by user".to_string()
        }
    )));
    assert!(actions.contains(&SessionAction::FinishRecordingSegment));
    // CRITICAL: Relay streaming session STAYS UP so engineers remain attached!
    assert!(machine.is_streaming());

    // LMU restarts: reader reports Connected
    let actions = machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    assert_eq!(actions, vec![]);

    // Frame arrives on restart with ET regression (session restarted)
    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 2_000_000_000,
        in_realtime: true,
        et_s: 1.0, // Regressed from 12.5s
        identity: id,
        session_id: Some(100),
        now_unix_s: Some(1_700_000_010),
    });

    assert_eq!(machine.lmu_source_state(), LmuSourceState::Live);
    assert!(actions.contains(&SessionAction::EmitUiReset));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, SessionAction::RecordDiscontinuity(_)))
    );
    assert!(actions.contains(&SessionAction::ResumeRecordingSegment));
    assert!(actions.contains(&SessionAction::SendSchemaNegotiate));
    assert!(actions.contains(&SessionAction::SendSnapshot));
}

#[test]
fn session_lifecycle_stall_vs_paused() {
    let settings = DriverSessionSettings {
        auto_stream: false,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine = DriverSessionMachine::new(settings);

    // Initial setup to Live
    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    let id = sample_identity();
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 100.0,
        identity: id.clone(),
        session_id: Some(200),
        now_unix_s: Some(1_700_000_000),
    });
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Live);

    // Case 1: Stalled while in realtime (car on track, LMU frozen) -> Stale
    let actions = machine.step(SessionInputEvent::FrameTimeout { in_realtime: true });
    assert_eq!(machine.stage(), DriverSessionStage::Stalled);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Stale);
    assert!(actions.contains(&SessionAction::SendStaleBlock));
    assert!(actions.contains(&SessionAction::FinishRecordingSegment));
    assert!(actions.iter().any(|a| matches!(
        a,
        SessionAction::EmitSessionEvent(SessionEvent::LmuDisconnected { .. })
    )));

    // Recovers from Stale
    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 2_000_000_000,
        in_realtime: true,
        et_s: 100.1,
        identity: id.clone(),
        session_id: Some(200),
        now_unix_s: Some(1_700_000_001),
    });
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Live);
    assert!(actions.contains(&SessionAction::ResumeRecordingSegment));

    // Case 2: Stalled while NOT in realtime (in garage, menu, replay) -> Paused
    let actions = machine.step(SessionInputEvent::FrameTimeout { in_realtime: false });
    assert_eq!(machine.stage(), DriverSessionStage::Stalled);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Paused);
    assert!(actions.contains(&SessionAction::SendStaleBlock));
    assert!(actions.contains(&SessionAction::FinishRecordingSegment));
    assert!(actions.contains(&SessionAction::EmitSessionEvent(SessionEvent::LmuPaused)));

    // Recovers from Paused (driving resumed)
    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 3_000_000_000,
        in_realtime: true,
        et_s: 105.0,
        identity: id,
        session_id: Some(200),
        now_unix_s: Some(1_700_000_005),
    });
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Live);
    assert!(actions.contains(&SessionAction::EmitSessionEvent(SessionEvent::LmuResumed)));
    assert!(actions.contains(&SessionAction::ResumeRecordingSegment));
}

#[test]
fn session_lifecycle_incompatible_interface() {
    let mut machine = DriverSessionMachine::new(DriverSessionSettings::default());

    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(300),
        now_unix_s: Some(1_700_000_000),
    });

    let actions = machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Incompatible,
        detail: Some("Structure size mismatch in LMU_Data".to_string()),
        game_version: None,
    });

    assert_eq!(machine.stage(), DriverSessionStage::Incompatible);
    assert_eq!(machine.lmu_source_state(), LmuSourceState::Error);
    assert!(actions.contains(&SessionAction::SendStaleBlock));
    assert!(actions.contains(&SessionAction::FinishRecordingSession));
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::AlertRaised {
            alert_id: "lmu_incompatible".to_string(),
            severity: Severity::Critical,
            message: "Structure size mismatch in LMU_Data".to_string(),
        }
    )));
}

#[test]
fn auto_stream_gating_and_manual_controls() {
    // 1. auto_stream = false (default per Q7)
    let settings = DriverSessionSettings {
        auto_stream: false,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine = DriverSessionMachine::new(settings);
    machine.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });

    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(400),
        now_unix_s: Some(1_700_000_000),
    });
    assert_eq!(machine.stage(), DriverSessionStage::SessionReady);
    assert!(!machine.is_streaming());
    assert!(!actions.contains(&SessionAction::StartStreaming));

    // Manual start streaming
    let actions = machine.step(SessionInputEvent::ManualStartStreaming);
    assert_eq!(actions, vec![SessionAction::StartStreaming]);
    assert_eq!(machine.stage(), DriverSessionStage::Streaming);
    assert!(machine.is_streaming());

    // Manual stop streaming: stops streaming, returns to SessionReady, local recording continues
    let actions = machine.step(SessionInputEvent::ManualStopStreaming);
    assert_eq!(actions, vec![SessionAction::StopStreaming]);
    assert_eq!(machine.stage(), DriverSessionStage::SessionReady);
    assert!(!machine.is_streaming());

    // 2. auto_stream = true
    let settings_auto = DriverSessionSettings {
        auto_stream: true,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine_auto = DriverSessionMachine::new(settings_auto);
    machine_auto.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    machine_auto.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });

    let actions = machine_auto.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(401),
        now_unix_s: Some(1_700_000_000),
    });
    assert_eq!(machine_auto.stage(), DriverSessionStage::Streaming);
    assert!(machine_auto.is_streaming());
    assert!(actions.contains(&SessionAction::StartStreaming));

    // 3. Synthetic origin: NEVER auto-streams or manually streams (Q1)
    let settings_synth = DriverSessionSettings {
        auto_stream: true,
        origin: Origin::Synthetic,
        ..Default::default()
    };
    let mut machine_synth = DriverSessionMachine::new(settings_synth);
    machine_synth.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    machine_synth.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(402),
        now_unix_s: Some(1_700_000_000),
    });
    assert!(!machine_synth.is_streaming());
    let actions = machine_synth.step(SessionInputEvent::ManualStartStreaming);
    assert_eq!(actions, vec![]);
    assert!(!machine_synth.is_streaming());
}

#[test]
fn sleep_resume_clock_skew_resynchronization() {
    let mut machine = DriverSessionMachine::new(DriverSessionSettings::default());
    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 50.0,
        identity: sample_identity(),
        session_id: Some(500),
        now_unix_s: Some(1_700_000_000),
    });

    // Clock skew > 5s detected (e.g. laptop wake from sleep)
    let actions = machine.step(SessionInputEvent::ClockSkewDetected { skew_s: 8.35 });
    assert!(actions.contains(&SessionAction::EmitUiReset));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, SessionAction::RecordDiscontinuity(_)))
    );
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::AlertRaised {
            alert_id: "clock_skew_detected".to_string(),
            severity: Severity::Warning,
            message:
                "System clock drift of 8.35s detected; resynchronizing UI and streams".to_string(),
        }
    )));
}

#[test]
fn identity_change_keeps_relay_session_per_owner_decision_q3() {
    let settings = DriverSessionSettings {
        auto_stream: false,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine = DriverSessionMachine::new(settings);
    machine.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });

    let id1 = sample_identity();
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: id1,
        session_id: Some(600),
        now_unix_s: Some(1_700_000_000),
    });
    machine.step(SessionInputEvent::ManualStartStreaming);
    assert!(machine.is_streaming());

    // Driver changes track / car: new identity arrives
    let mut id2 = sample_identity();
    id2.track_name = Some("Autodromo Nazionale Monza".to_string());
    id2.vehicle_name = Some("Porsche 963 #5".to_string());

    let actions = machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 2_000_000_000,
        in_realtime: true,
        et_s: 5.0,
        identity: id2,
        session_id: Some(600),
        now_unix_s: Some(1_700_000_100),
    });

    // 1. Relay session STAYS UP: no EndStreamingSession or StopStreaming
    assert!(machine.is_streaming());
    assert!(!actions.contains(&SessionAction::EndStreamingSession));
    assert!(!actions.contains(&SessionAction::StopStreaming));

    // 2. Old recording finishes, new recording starts
    assert!(actions.contains(&SessionAction::FinishRecordingSession));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, SessionAction::StartRecording(_)))
    );

    // 3. Events emitted for track and vehicle change
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::TrackChanged {
            track_name: Some("Autodromo Nazionale Monza".to_string())
        }
    )));
    assert!(actions.contains(&SessionAction::EmitSessionEvent(
        SessionEvent::VehicleChanged {
            vehicle_name: Some("Porsche 963 #5".to_string()),
            class_name: Some("Hypercar".to_string()),
        }
    )));

    // 4. Stream sends new schema and full snapshot
    assert!(actions.contains(&SessionAction::SendSchemaNegotiate));
    assert!(actions.contains(&SessionAction::SendSnapshot));
}

#[test]
fn manual_end_session_shuts_down_cleanly() {
    let settings = DriverSessionSettings {
        auto_stream: false,
        origin: Origin::Lmu,
        ..Default::default()
    };
    let mut machine = DriverSessionMachine::new(settings);
    machine.step(SessionInputEvent::AuthStatusChanged {
        logged_in: true,
        relay_configured: true,
    });
    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(700),
        now_unix_s: Some(1_700_000_000),
    });
    machine.step(SessionInputEvent::ManualStartStreaming);
    assert!(machine.is_streaming());

    let actions = machine.step(SessionInputEvent::ManualEndSession);
    assert!(actions.contains(&SessionAction::EndStreamingSession));
    assert!(actions.contains(&SessionAction::FinishRecordingSession));
    assert_eq!(machine.stage(), DriverSessionStage::Idle);
    assert!(!machine.is_streaming());
}

#[test]
fn composite_status_dtos_reflect_state() {
    let mut machine = DriverSessionMachine::new(DriverSessionSettings::default());
    let lmu_status = machine.lmu_source_status();
    assert_eq!(lmu_status.state, "NOT_RUNNING");

    let session_status = machine.session_status_dto();
    assert_eq!(session_status.car, None);
    assert_eq!(session_status.track, None);
    assert_eq!(session_status.started_at_unix_s, None);

    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: sample_identity(),
        session_id: Some(800),
        now_unix_s: Some(1_700_000_000),
    });

    let lmu_status = machine.lmu_source_status();
    assert_eq!(lmu_status.state, "LIVE");
    assert_eq!(lmu_status.game_version, Some(10402));

    let session_status = machine.session_status_dto();
    assert_eq!(session_status.session_id, Some(800));
    assert_eq!(session_status.car, Some("Ferrari 499P #51".to_string()));
    assert_eq!(
        session_status.track,
        Some("Circuit de la Sarthe".to_string())
    );
    assert_eq!(session_status.vehicle_class, Some("Hypercar".to_string()));
    assert_eq!(session_status.session_type, Some("Practice 1".to_string()));
    assert_eq!(session_status.started_at_unix_s, Some(1_700_000_000));
}

#[test]
fn test_finding_6_session_started_at_unix_s_lifecycle() {
    let mut machine = DriverSessionMachine::new(DriverSessionSettings::default());
    assert_eq!(machine.session_status_dto().started_at_unix_s, None);

    machine.step(SessionInputEvent::ReaderStateChanged {
        state: ReaderState::Connected,
        detail: None,
        game_version: Some(10402),
    });

    // 1. Initial frame with complete identity sets started_at_unix_s
    let id1 = sample_identity();
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 1_000_000_000,
        in_realtime: true,
        et_s: 10.0,
        identity: id1.clone(),
        session_id: Some(1001),
        now_unix_s: Some(1_700_000_123),
    });
    assert_eq!(
        machine.session_status_dto().started_at_unix_s,
        Some(1_700_000_123),
        "started_at_unix_s must be populated on session start (Finding 6)"
    );

    // 2. Subsequent frame must preserve original started_at_unix_s
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 2_000_000_000,
        in_realtime: true,
        et_s: 11.0,
        identity: id1,
        session_id: Some(1001),
        now_unix_s: Some(1_700_000_200),
    });
    assert_eq!(
        machine.session_status_dto().started_at_unix_s,
        Some(1_700_000_123),
        "started_at_unix_s must remain stable across frames in same session (Finding 6)"
    );

    // 3. Identity change (new track) starts a new session and updates started_at_unix_s
    let mut id2 = sample_identity();
    id2.track_name = Some("Silverstone".to_string());
    machine.step(SessionInputEvent::FrameReceived {
        capture_mono_ns: 3_000_000_000,
        in_realtime: true,
        et_s: 1.0,
        identity: id2,
        session_id: Some(1001),
        now_unix_s: Some(1_700_000_500),
    });
    assert_eq!(
        machine.session_status_dto().started_at_unix_s,
        Some(1_700_000_500),
        "started_at_unix_s must update on session identity change (Finding 6)"
    );

    // 4. Manual end session resets started_at_unix_s to None
    machine.step(SessionInputEvent::ManualEndSession);
    assert_eq!(
        machine.session_status_dto().started_at_unix_s,
        None,
        "started_at_unix_s must reset to None on ManualEndSession (Finding 6)"
    );
}

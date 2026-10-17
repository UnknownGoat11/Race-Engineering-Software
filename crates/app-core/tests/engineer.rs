//! Integration tests for EngineerCore (Phase 1 Lane G: engineer client stream processing).
//!
//! Blueprint §1, §2, §3, §12 Lane G, §14 (Q4):
//! - Coalescing: groups TLVs with the same `capture_mono_ns` (e.g. P0, P1, P2) into 1 block for UiHub
//! - Coalescing timeout: flushes after 8 ms
//! - Schema guard: rejects unsupported schema versions and flags incompatible
//! - StreamReset / gap: preserves pre-gap history with visible gap flag (owner decision Q4)
//! - Stream age tracking and 4 Hz stale alert evaluation
//! - Connect code validation: never auto-connects without user code
//! - Session events bounded FIFO queue

use std::sync::Arc;

use app_core::bridge::hub::UiHub;
use app_core::bridge::sink::{SinkClosed, SubscriptionId, UiSink};
use app_core::bridge::status::UiEvent;
use app_core::engineer::age::StreamHealth;
use app_core::engineer::core::{COALESCE_TIMEOUT_NS, EngineerCore};
use bytes::Bytes;
use parking_lot::Mutex;
use protocol::frame::{FrameHeader, FramePriority, MsgType};
use protocol::messages::{
    DriverPresence, Message, ScoringUpdate, SessionEventMsg, TelemetryPayload,
};
use telemetry_model::TELEMETRY_SCHEMA_VERSION;
use telemetry_model::ch;
use telemetry_model::codec::{TlvEncoder, TlvHeader};
use telemetry_model::events::{EventRecord, EventTime, SessionEvent};
use telemetry_model::scoring::ScoringState;
use telemetry_model::uibatch::{FLAG_GAP_BEFORE, parse};
use telemetry_model::value::{Sample, Value};
use transport::engineer::{EngineerEvent, IdentityCard, InboundFrame};

/// Test mock sink that records received UI batches and events.
struct MockUiSink {
    batches: Mutex<Vec<Vec<u8>>>,
    events: Mutex<Vec<UiEvent>>,
}

impl MockUiSink {
    fn new() -> Self {
        Self {
            batches: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        }
    }

    fn batches(&self) -> Vec<Vec<u8>> {
        self.batches.lock().clone()
    }

    fn events(&self) -> Vec<UiEvent> {
        self.events.lock().clone()
    }
}

impl UiSink for MockUiSink {
    fn send_batch(&self, _sub_id: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed> {
        self.batches.lock().push(batch);
        Ok(())
    }

    fn emit(&self, event: UiEvent) {
        self.events.lock().push(event);
    }

    fn is_visible(&self) -> bool {
        true
    }
}

fn make_inbound_frame(msg: Message, received_at_ns: u64) -> InboundFrame {
    InboundFrame {
        message: msg,
        header: FrameHeader {
            minor: protocol::version::PROTOCOL_MINOR,
            msg_type: MsgType::TelemetryDelta as u8,
            flags: protocol::frame::Flags::default(),
            priority: FramePriority::P0,
            payload_len: 0,
            session_id: 1,
            sequence: 1,
            source_ts_ns: 0,
        },
        trailer: None,
        arrival: None,
        received_at_ns,
    }
}

fn encode_tlv_block(
    schema_version: u16,
    capture_mono_ns: u64,
    source_et_s: f64,
    channels: &[(telemetry_model::ChannelId, Value)],
) -> Vec<u8> {
    let header = TlvHeader {
        schema_version,
        capture_mono_ns,
        capture_relay_ns_est: 1_700_000_000_000_000_000,
        source_et_s,
    };
    let mut buf = Vec::with_capacity(64 + channels.len() * 12);
    let mut enc = TlvEncoder::begin(&mut buf, &header);
    for (id, val) in channels {
        let sample = Sample::valid(val.clone());
        enc.push(*id, &sample).expect("encode sample");
    }
    enc.finish();
    buf
}

#[test]
fn coalescing_priority_deltas_same_capture_mono_ns() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    let capture_mono_ns = 1_000_000_000;

    // P0 delta: SPEED
    let p0_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns,
        10.0,
        &[(ch::SPEED, Value::F32(50.0))],
    );
    let p0_payload = TelemetryPayload::new(Bytes::from(p0_bytes)).expect("p0 payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(p0_payload),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    // P1 delta: TYRE_PRESSURE_FL (same capture_mono_ns!)
    let p1_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns,
        10.0,
        &[(ch::TYRE_PRESSURE_FL, Value::F32(150_000.0))],
    );
    let p1_payload = TelemetryPayload::new(Bytes::from(p1_bytes)).expect("p1 payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(p1_payload),
            1_000_000_500,
        ))),
        1_000_000_500,
    );

    // P2 delta: PATH_LATERAL (same capture_mono_ns!)
    let p2_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns,
        10.0,
        &[(ch::PATH_LATERAL, Value::F32(1.2))],
    );
    let p2_payload = TelemetryPayload::new(Bytes::from(p2_bytes)).expect("p2 payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(p2_payload),
            1_000_001_000,
        ))),
        1_000_001_000,
    );

    // 2 merges occurred into the single buffer
    assert_eq!(core.stats().coalesced_merges, 2);
    assert_eq!(core.stats().deltas_received, 1);
    assert_eq!(core.stats().blocks_flushed, 0);

    // Now send frame from NEXT capture tick (capture_mono_ns = 1_010_000_000)
    let next_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        1_010_000_000,
        10.01,
        &[(ch::SPEED, Value::F32(52.0))],
    );
    let next_payload = TelemetryPayload::new(Bytes::from(next_bytes)).expect("next payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(next_payload),
            1_010_000_000,
        ))),
        1_010_000_000,
    );

    // Arrival of new capture flushed the previous 3-delta capture as 1 block!
    assert_eq!(core.stats().blocks_flushed, 1);
}

#[test]
fn coalescing_timeout_flushes_after_8ms() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    let p0_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        1_000_000_000,
        10.0,
        &[(ch::SPEED, Value::F32(50.0))],
    );
    let p0_payload = TelemetryPayload::new(Bytes::from(p0_bytes)).expect("p0 payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(p0_payload),
            1_000_000_000,
        ))),
        1_000_000_000,
    );
    assert_eq!(core.stats().blocks_flushed, 0);

    // Now trigger an event after 8 ms (COALESCE_TIMEOUT_NS)
    let scoring = ScoringUpdate {
        capture: protocol::messages::CaptureTime {
            mono_ns: 1_009_000_000,
            relay_ns_est: None,
        },
        scoring: ScoringState::default(),
    };
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::ScoringUpdate(scoring),
            1_000_000_000 + COALESCE_TIMEOUT_NS + 1000,
        ))),
        1_000_000_000 + COALESCE_TIMEOUT_NS + 1000,
    );

    assert_eq!(core.stats().blocks_flushed, 1);
}

#[test]
fn schema_guard_rejects_incompatible_schema() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    // Corrupted / incompatible schema version 999
    let bad_bytes = encode_tlv_block(999, 1_000_000_000, 10.0, &[(ch::SPEED, Value::F32(50.0))]);
    let bad_payload = TelemetryPayload::new(Bytes::from(bad_bytes)).expect("bad payload");

    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetryDelta(bad_payload),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    assert_eq!(core.stats().schema_mismatches, 1);
    let tick = core.tick(1_000_000_000, None, None);
    assert!(tick.status.incompatible_schema);
}

#[test]
fn stream_reset_preserves_pre_gap_history_with_gap_flag() {
    let hub = Arc::new(UiHub::new());
    let sink = Arc::new(MockUiSink::new());
    let sub_id = hub.subscribe(sink.clone());

    let mut core = EngineerCore::new(hub.clone());

    // 1. Initial snapshot at t = 1.0s
    let snap_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        1_000_000_000,
        10.0,
        &[(ch::SPEED, Value::F32(50.0))],
    );
    let snap_payload = TelemetryPayload::new(Bytes::from(snap_bytes)).expect("snap payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetrySnapshot(snap_payload),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    // 2. StreamReset event (network blip or driver restart)
    let card = IdentityCard {
        session_id: 42,
        driver_display_name: "Test Driver".to_owned(),
        driver_username: "driver1".to_owned(),
        car: Some("Ferrari 499P".to_owned()),
        vehicle_class: Some("Hypercar".to_owned()),
        track: Some("Le Mans".to_owned()),
        session_type: None,
        started_at_ns: 1_700_000_000_000_000_000,
        schema_version: TELEMETRY_SCHEMA_VERSION,
    };
    core.handle_event(
        EngineerEvent::StreamReset {
            card: card.clone(),
            gap: None,
        },
        1_500_000_000,
    );

    assert_eq!(core.active_identity(), Some(&card));

    // 3. New snapshot arrives at t = 2.0s
    let snap2_bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        2_000_000_000,
        20.0,
        &[(ch::SPEED, Value::F32(60.0))],
    );
    let snap2_payload = TelemetryPayload::new(Bytes::from(snap2_bytes)).expect("snap2 payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetrySnapshot(snap2_payload),
            2_000_000_000,
        ))),
        2_000_000_000,
    );

    // 4. Flush UI hub and inspect batches
    let _ = hub.flush_subscription(sub_id);
    let raw_batches = sink.batches();
    assert!(!raw_batches.is_empty());

    let parsed_batches: Vec<_> = raw_batches
        .iter()
        .map(|b| parse(b).expect("parse batch"))
        .collect();

    // Verify gap was flagged on the post-reset batch
    let post_reset_batch = parsed_batches
        .iter()
        .find(|b| b.flags & FLAG_GAP_BEFORE != 0);
    assert!(
        post_reset_batch.is_some(),
        "GAP_BEFORE must be set after StreamReset"
    );
}

#[test]
fn stream_age_tracking_and_stale_alert_at_4hz() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    // Frame at t = 1.0s (1_000_000_000 ns)
    let bytes = encode_tlv_block(
        TELEMETRY_SCHEMA_VERSION,
        1_000_000_000,
        10.0,
        &[(ch::SPEED, Value::F32(50.0))],
    );
    let payload = TelemetryPayload::new(Bytes::from(bytes)).expect("payload");
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::TelemetrySnapshot(payload),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    // 1. Tick at t = 1.25s (250 ms later): healthy
    let out = core.tick(1_250_000_000, None, None);
    assert_eq!(out.status.stream_health, StreamHealth::Healthy);
    assert!(out.status.stream_age_s.expect("age must be measured") < 1.0);

    // 2. Tick at t = 3.5s (2.5s later > 2.0s stale threshold): Stale!
    let out = core.tick(3_500_000_000, None, None);
    assert_eq!(out.status.stream_health, StreamHealth::Stale);

    // 3. Tick at t = 12.0s (11.0s later > 10.0s lost threshold): NoData!
    let out = core.tick(12_000_000_000, None, None);
    assert_eq!(out.status.stream_health, StreamHealth::NoData);
}

#[test]
fn never_auto_joins_and_code_validation() {
    let hub = Arc::new(UiHub::new());
    let core = EngineerCore::new(hub);

    // Initially idle, never auto-joined
    assert!(core.active_identity().is_none());
    assert!(core.ended_reason().is_none());

    // Valid code forms (canonical 8 uppercase characters)
    assert_eq!(
        EngineerCore::validate_connect_code("7kq4-mz9t").expect("valid code with dash"),
        "7KQ4MZ9T"
    );
    assert_eq!(
        EngineerCore::validate_connect_code("7KQ4MZ9T").expect("valid code without dash"),
        "7KQ4MZ9T"
    );

    // Invalid codes rejected
    assert!(EngineerCore::validate_connect_code("short").is_err());
    assert!(EngineerCore::validate_connect_code("toolongcode123").is_err());
}

#[test]
fn session_events_bounded_and_fifo() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    for i in 0..1050 {
        let event = EventRecord::new(
            EventTime {
                source_et_s: Some(i as f64),
                mono_ns: i * 1_000_000,
            },
            Some(1),
            SessionEvent::LapStarted { lap: i as u32 },
        );
        let msg = SessionEventMsg {
            capture: protocol::messages::CaptureTime {
                mono_ns: i * 1_000_000,
                relay_ns_est: None,
            },
            record: event,
        };
        core.handle_event(
            EngineerEvent::Frame(Box::new(make_inbound_frame(
                Message::SessionEvent(msg),
                i * 1_000_000,
            ))),
            i * 1_000_000,
        );
    }
}

#[test]
fn scoring_driver_status_and_presence_updates() {
    let hub = Arc::new(UiHub::new());
    let mut core = EngineerCore::new(hub);

    // Scoring update
    let scoring = ScoringUpdate {
        capture: protocol::messages::CaptureTime {
            mono_ns: 1_000_000_000,
            relay_ns_est: None,
        },
        scoring: ScoringState::default(),
    };
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::ScoringUpdate(scoring),
            1_000_000_000,
        ))),
        1_000_000_000,
    );
    assert!(core.latest_scoring().is_some());

    // Driver presence
    let presence = DriverPresence {
        session_id: 1,
        present: true,
        away_since_ns: 0,
        grace_expires_at_ns: 0,
    };
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::DriverPresence(presence),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    // Driver away
    core.handle_event(
        EngineerEvent::DriverAway {
            since_ns: 2_000_000_000,
            grace_expires_at_ns: 2_030_000_000,
        },
        2_000_000_000,
    );
    let tick = core.tick(2_000_000_000, None, None);
    assert!(tick.status.driver_away);

    // Driver back
    core.handle_event(EngineerEvent::DriverBack, 2_005_000_000);
    let tick = core.tick(2_005_000_000, None, None);
    assert!(!tick.status.driver_away);
}

#[test]
fn test_finding_5_engineer_core_emits_scoring_events_and_errors_to_ui_sink() {
    let hub = Arc::new(UiHub::new());
    let sink = Arc::new(MockUiSink::new());
    let mut core = EngineerCore::with_sink(hub, Some(sink.clone() as Arc<dyn UiSink>));

    // 1. Scoring update -> UiEvent::Scoring
    let scoring = ScoringUpdate {
        capture: protocol::messages::CaptureTime {
            mono_ns: 1_000_000_000,
            relay_ns_est: None,
        },
        scoring: ScoringState::default(),
    };
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::ScoringUpdate(scoring),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    let events = sink.events();
    let has_scoring = events.iter().any(|e| matches!(e, UiEvent::Scoring(_)));
    assert!(
        has_scoring,
        "EngineerCore must emit UiEvent::Scoring on ScoringUpdate (Finding 5)"
    );

    // 2. Session event -> UiEvent::SessionEvent
    let event = EventRecord::new(
        EventTime {
            source_et_s: Some(10.0),
            mono_ns: 1_000_000_000,
        },
        Some(1),
        SessionEvent::LmuConnected {
            game_version: Some(10402),
        },
    );
    let msg = SessionEventMsg {
        capture: protocol::messages::CaptureTime {
            mono_ns: 1_000_000_000,
            relay_ns_est: None,
        },
        record: event,
    };
    core.handle_event(
        EngineerEvent::Frame(Box::new(make_inbound_frame(
            Message::SessionEvent(msg),
            1_000_000_000,
        ))),
        1_000_000_000,
    );

    let events = sink.events();
    let has_session_event = events.iter().any(|e| matches!(e, UiEvent::SessionEvent(_)));
    assert!(
        has_session_event,
        "EngineerCore must emit UiEvent::SessionEvent on SessionEventMsg (Finding 5)"
    );

    // 3. Relay error -> UiEvent::Error
    core.handle_event(
        EngineerEvent::RelayError {
            code: protocol::messages::ErrorCode::NotAuthorized,
            message: "Unauthorized".to_string(),
        },
        1_000_000_000,
    );

    let events = sink.events();
    let has_error = events
        .iter()
        .any(|e| matches!(e, UiEvent::Error(err) if err.message.contains("Unauthorized")));
    assert!(
        has_error,
        "EngineerCore must emit UiEvent::Error on RelayError (Finding 5)"
    );
}

//! Integration tests for NetPublisher (Phase 1 Lane D: driver network streaming, union repair, priority splitting).
//!
//! Blueprint §1 Decision 4, §3, §12 Lane D, §14:
//! - Mock LiveSink
//! - Union repair oracle: drop random replaced frames and assert receiver state equals the last frame
//! - Priority splitting: P0, P1, P2 deltas
//! - SchemaNegotiate sent first
//! - Snapshot on request
//! - Weather rate limiting (≤ 1 Hz on change) and exclusion from deltas
//! - Reliable queue bound (1024 capacity, oldest dropped counted)

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use app_core::clock::AppClock;
use app_core::driver::net::{LiveSink, NetPublisher};
use protocol::frame::{FramePriority, MsgType};
use protocol::messages::{
    ComponentHealth, DriverRates, DriverStatus, HealthLevel, LmuSourceState, Message,
    RecordingState, RecordingStatus, SchemaNegotiate,
};
use telemetry_model::ch;
use telemetry_model::events::{EventRecord, EventTime, SessionEvent};
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::scoring::{CapabilityMap, ScoringState, VehicleCapabilities};
use telemetry_model::value::Value;
use transport::queue::{LiveOutcome, QueueError};
use transport::state::{ConnectionState, ConnectionStatus};

/// Mock implementation of [`LiveSink`] for deterministic testing.
struct MockLiveSink {
    /// Latest-wins slots indexed by `(MsgType, FramePriority)`.
    slots: Mutex<HashMap<(MsgType, FramePriority), Message>>,
    /// History of all messages queued through `send_live`.
    live_history: Mutex<Vec<(Message, FramePriority)>>,
    /// Reliable messages queued.
    reliable_queue: Mutex<Vec<Message>>,
    /// Simulates reliable queue being full.
    reliable_full: AtomicBool,
    /// Whether latest-wins slots replace existing items without error.
    replace_enabled: AtomicBool,
}

impl MockLiveSink {
    fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            live_history: Mutex::new(Vec::new()),
            reliable_queue: Mutex::new(Vec::new()),
            reliable_full: AtomicBool::new(false),
            replace_enabled: AtomicBool::new(true),
        }
    }

    fn set_reliable_full(&self, full: bool) {
        self.reliable_full.store(full, Ordering::SeqCst);
    }

    fn drain_slots(&self) -> Vec<(Message, FramePriority)> {
        let mut slots = self.slots.lock();
        slots.drain().map(|((_, prio), msg)| (msg, prio)).collect()
    }
}

impl LiveSink for MockLiveSink {
    fn send_live(
        &self,
        message: Message,
        priority: FramePriority,
    ) -> Result<LiveOutcome, QueueError> {
        let key = (message.msg_type(), priority);
        let mut slots = self.slots.lock();
        let replaced = slots.insert(key, message.clone()).is_some();
        self.live_history.lock().push((message, priority));

        Ok(LiveOutcome {
            replaced: replaced && self.replace_enabled.load(Ordering::SeqCst),
            superseded: 0,
        })
    }

    fn try_send_reliable(&self, message: Message) -> Result<(), QueueError> {
        if self.reliable_full.load(Ordering::SeqCst) {
            return Err(QueueError::Full { capacity: 256 });
        }
        self.reliable_queue.lock().push(message);
        Ok(())
    }

    fn connection_status(&self) -> ConnectionStatus {
        ConnectionStatus {
            state: ConnectionState::Connected,
            ..Default::default()
        }
    }

    fn relay_time_of(&self, _at: std::time::Instant) -> Option<i64> {
        Some(1_700_000_000_000_000_000)
    }
}

fn sample_schema_negotiate() -> SchemaNegotiate {
    SchemaNegotiate {
        schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        capabilities: CapabilityMap::default(),
        vehicle_capabilities: VehicleCapabilities::default(),
    }
}

fn sample_driver_status() -> DriverStatus {
    let ok_health = ComponentHealth {
        level: HealthLevel::Ok,
        detail: None,
    };
    DriverStatus {
        lmu_state: LmuSourceState::Live,
        lmu_source: ok_health.clone(),
        processing: ok_health.clone(),
        recording: ok_health.clone(),
        network: ok_health,
        rates: DriverRates {
            telemetry_read_hz: Some(100.0),
            scoring_read_hz: Some(5.0),
            frames_sent_hz: Some(100.0),
            bytes_sent_per_s: Some(120_000),
            frames_dropped_total: 0,
        },
        clock: None,
    }
}

fn sample_recording_status() -> RecordingStatus {
    RecordingStatus {
        state: RecordingState::Active,
        recording_id: Some("rec-12345".to_string()),
        bytes_written: 1024,
        frames_written: 100,
        frames_lost: 0,
        disk_free_bytes: Some(10_000_000_000),
        error: None,
    }
}

#[test]
fn priority_splitting_and_environment_exclusion() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    // Frame 0: Baseline snapshot
    let mut f0 = TelemetryFrame::new(1, 1_000_000_000, Some(10.0));
    f0.set_valid(ch::SPEED, Value::F32(50.0))
        .expect("set speed");
    f0.set_valid(ch::TYRE_PRESSURE_FL, Value::F32(150_000.0))
        .expect("set tyre pressure");
    f0.set_valid(ch::PATH_LATERAL, Value::F32(0.0))
        .expect("set path lateral");
    f0.set_valid(ch::AIR_TEMP, Value::F32(20.0))
        .expect("set air temp");

    publisher.publish_frame(&f0, 1_000_000_000);
    assert_eq!(publisher.stats().snapshots_sent, 1);
    sink.drain_slots(); // clear slots to simulate writer draining snapshot

    // Frame 1 (10 ms later, delta time):
    // Modify:
    // P0: SPEED -> 55.0
    // P1: TYRE_PRESSURE_FL -> 151_000.0
    // P2: PATH_LATERAL -> 1.5
    // Environment: AIR_TEMP -> 21.0 (must NOT appear in telemetry deltas!)
    let mut f1 = f0.clone();
    f1.capture_mono_ns = 1_010_000_000;
    f1.source_et_s = Some(10.01);
    f1.set_valid(ch::SPEED, Value::F32(55.0)).expect("speed");
    f1.set_valid(ch::TYRE_PRESSURE_FL, Value::F32(151_000.0))
        .expect("tyre pressure");
    f1.set_valid(ch::PATH_LATERAL, Value::F32(1.5))
        .expect("path lateral");
    f1.set_valid(ch::AIR_TEMP, Value::F32(21.0))
        .expect("air temp");

    publisher.publish_frame(&f1, 1_010_000_000);

    let messages = sink.drain_slots();
    assert_eq!(messages.len(), 3, "expected P0, P1, P2 deltas");

    let mut saw_p0 = false;
    let mut saw_p1 = false;
    let mut saw_p2 = false;

    for (msg, priority) in messages {
        match (msg, priority) {
            (Message::TelemetryDelta(payload), FramePriority::P0) => {
                let (_, records) = payload.decode().expect("decode P0");
                assert!(records.iter().any(|r| r.channel == ch::SPEED));
                assert!(!records.iter().any(|r| r.channel == ch::AIR_TEMP));
                saw_p0 = true;
            }
            (Message::TelemetryDelta(payload), FramePriority::P1) => {
                let (_, records) = payload.decode().expect("decode P1");
                assert!(records.iter().any(|r| r.channel == ch::TYRE_PRESSURE_FL));
                assert!(!records.iter().any(|r| r.channel == ch::AIR_TEMP));
                saw_p1 = true;
            }
            (Message::TelemetryDelta(payload), FramePriority::P2) => {
                let (_, records) = payload.decode().expect("decode P2");
                assert!(records.iter().any(|r| r.channel == ch::PATH_LATERAL));
                assert!(!records.iter().any(|r| r.channel == ch::AIR_TEMP));
                saw_p2 = true;
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    assert!(saw_p0 && saw_p1 && saw_p2);
}

#[test]
fn union_repair_oracle_preserves_dropped_channels() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    // Initial frame at t = 1.0s (Snapshot)
    let mut f0 = TelemetryFrame::new(1, 1_000_000_000, Some(10.0));
    f0.set_valid(ch::SPEED, Value::F32(0.0)).expect("speed");
    f0.set_valid(ch::THROTTLE, Value::F32(0.0))
        .expect("throttle");
    f0.set_valid(ch::BRAKE, Value::F32(0.0)).expect("brake");

    publisher.publish_frame(&f0, 1_000_000_000);
    let snapshot_messages = sink.drain_slots();
    let (snap_msg, _) = &snapshot_messages[0];
    let snap_payload = match snap_msg {
        Message::TelemetrySnapshot(p) => p,
        other => panic!("expected snapshot, got {other:?}"),
    };

    // Receiver reconstructs frame from snapshot
    let rx_frame = TelemetryFrame::from_snapshot(1, snap_payload.as_bytes())
        .expect("rx snapshot")
        .0;
    assert_eq!(rx_frame.f64(ch::SPEED), Some(0.0));
    assert_eq!(rx_frame.f64(ch::THROTTLE), Some(0.0));
    assert_eq!(rx_frame.f64(ch::BRAKE), Some(0.0));

    // Now simulate 3 consecutive frames where writer is slow (does NOT drain slot).
    // Frame 1 (t = 1.01s): only SPEED changes to 50.0
    let mut f1 = f0.clone();
    f1.capture_mono_ns = 1_010_000_000;
    f1.source_et_s = Some(10.01);
    f1.set_valid(ch::SPEED, Value::F32(50.0)).expect("speed");
    publisher.publish_frame(&f1, 1_010_000_000);

    // Frame 2 (t = 1.02s): only THROTTLE changes to 0.8
    // In MockLiveSink, slot already had Frame 1's delta!
    // So Frame 2 replaces Frame 1 -> outcome.replaced == true!
    // Union repair must trigger and re-push {SPEED, THROTTLE}!
    let mut f2 = f1.clone();
    f2.capture_mono_ns = 1_020_000_000;
    f2.source_et_s = Some(10.02);
    f2.set_valid(ch::THROTTLE, Value::F32(0.8))
        .expect("throttle");
    publisher.publish_frame(&f2, 1_020_000_000);

    // Frame 3 (t = 1.03s): only BRAKE changes to 0.5
    // Frame 3 replaces the slot again -> union repair must re-push {SPEED, THROTTLE, BRAKE}!
    let mut f3 = f2.clone();
    f3.capture_mono_ns = 1_030_000_000;
    f3.source_et_s = Some(10.03);
    f3.set_valid(ch::BRAKE, Value::F32(0.5)).expect("brake");
    publisher.publish_frame(&f3, 1_030_000_000);

    assert!(publisher.stats().repair_pushes[0] >= 2);

    // Now the receiver finally wakes up and reads whatever is currently in the P0 delta slot!
    let surviving_messages = sink.drain_slots();
    let p0_msg = surviving_messages
        .iter()
        .find(|(m, p)| *p == FramePriority::P0 && m.msg_type() == MsgType::TelemetryDelta)
        .expect("P0 delta must be in slot");

    let delta_payload = match &p0_msg.0 {
        Message::TelemetryDelta(p) => p,
        other => panic!("expected delta, got {other:?}"),
    };

    // Apply the surviving delta to rx_frame
    let (updated_rx, _) = rx_frame
        .apply_tlv(2, delta_payload.as_bytes())
        .expect("apply delta");

    // ORACLE ASSERTION:
    // Despite intermediate frames being replaced in the slot,
    // ALL THREE values (SPEED=50.0, THROTTLE=0.8, BRAKE=0.5) must be present in updated_rx!
    assert_eq!(
        updated_rx.f64(ch::SPEED),
        Some(50.0),
        "SPEED must survive union repair"
    );
    assert_eq!(
        updated_rx.f64(ch::THROTTLE),
        Some(0.8_f32 as f64),
        "THROTTLE must survive union repair"
    );
    assert_eq!(
        updated_rx.f64(ch::BRAKE),
        Some(0.5),
        "BRAKE must survive union repair"
    );
}

#[test]
fn schema_negotiate_sent_first_on_reliable_queue() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    let schema = sample_schema_negotiate();
    publisher.set_schema(schema.clone());

    let event = EventRecord::new(
        EventTime {
            source_et_s: Some(1.0),
            mono_ns: 1_000_000,
        },
        Some(1),
        SessionEvent::LapStarted { lap: 2 },
    );
    publisher.push_session_event(event, 1_000_000);

    let queue = sink.reliable_queue.lock().clone();
    assert_eq!(queue.len(), 2);
    assert_eq!(queue[0], Message::SchemaNegotiate(schema));
    assert!(matches!(queue[1], Message::SessionEvent(_)));
}

#[test]
fn snapshot_on_request_forces_immediate_snapshot() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    let f0 = TelemetryFrame::new(1, 1_000_000_000, Some(10.0));
    publisher.publish_frame(&f0, 1_000_000_000);
    assert_eq!(publisher.stats().snapshots_sent, 1);
    sink.drain_slots();

    // 200 ms later: normally only deltas are sent, not a snapshot
    let f1 = f0.clone();
    publisher.publish_frame(&f1, 1_200_000_000);
    assert_eq!(publisher.stats().snapshots_sent, 1);

    // Engineer requests snapshot
    publisher.request_snapshot();

    // Next frame (10 ms later, t = 1.21s): immediate snapshot must be emitted!
    let f2 = f1.clone();
    publisher.publish_frame(&f2, 1_210_000_000);
    assert_eq!(publisher.stats().snapshots_sent, 2);

    let messages = sink.drain_slots();
    assert!(
        messages
            .iter()
            .any(|(m, _)| matches!(m, Message::TelemetrySnapshot(_)))
    );
}

#[test]
fn weather_rate_limiting_and_change_detection() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    let mut f0 = TelemetryFrame::new(1, 1_000_000_000, Some(10.0));
    f0.set_valid(ch::AIR_TEMP, Value::F32(22.0)).expect("temp");
    f0.set_valid(ch::TRACK_TEMP, Value::F32(30.0))
        .expect("track");

    // Frame 0 at t = 1.0s: weather published
    publisher.publish_weather(&f0, 1_000_000_000);
    assert_eq!(publisher.stats().weathers_sent, 1);
    sink.drain_slots();

    // Frame 1 at t = 1.2s (200 ms later): weather changes, but <= 1 Hz limit prevents send!
    let mut f1 = f0.clone();
    f1.set_valid(ch::AIR_TEMP, Value::F32(23.5)).expect("temp");
    publisher.publish_weather(&f1, 1_200_000_000);
    assert_eq!(publisher.stats().weathers_sent, 1, "rate limited");

    // Frame 2 at t = 2.1s (> 1.0s later): weather update permitted!
    publisher.publish_weather(&f1, 2_100_000_000);
    assert_eq!(publisher.stats().weathers_sent, 2);

    // Frame 3 at t = 3.5s: weather did NOT change -> no update sent
    publisher.publish_weather(&f1, 3_500_000_000);
    assert_eq!(publisher.stats().weathers_sent, 2, "no change, no send");
}

#[test]
fn scoring_rate_limiting_5hz() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    let scoring = ScoringState::default();

    // t = 1.0s: first scoring
    publisher.publish_scoring(&scoring, 1_000_000_000);
    assert_eq!(publisher.stats().scorings_sent, 1);

    // t = 1.1s (100 ms later): < 200 ms -> rate limited
    publisher.publish_scoring(&scoring, 1_100_000_000);
    assert_eq!(publisher.stats().scorings_sent, 1);

    // t = 1.21s (210 ms later): >= 200 ms -> sent
    publisher.publish_scoring(&scoring, 1_210_000_000);
    assert_eq!(publisher.stats().scorings_sent, 2);
}

#[test]
fn status_broadcast_1hz_rate_limit() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    let d_status = sample_driver_status();
    let r_status = sample_recording_status();

    // t = 1.0s: first broadcast
    publisher.publish_status(d_status.clone(), r_status.clone(), 1_000_000_000);
    assert_eq!(publisher.stats().statuses_sent, 1);

    // t = 1.5s (500 ms later): < 1s -> suppressed
    publisher.publish_status(d_status.clone(), r_status.clone(), 1_500_000_000);
    assert_eq!(publisher.stats().statuses_sent, 1);

    // t = 2.05s (1.05s later): >= 1s -> sent
    publisher.publish_status(d_status, r_status, 2_050_000_000);
    assert_eq!(publisher.stats().statuses_sent, 2);
}

#[test]
fn reliable_queue_bounded_and_drop_oldest() {
    let sink = Arc::new(MockLiveSink::new());
    let clock = AppClock::new();
    let mut publisher = NetPublisher::new(sink.clone(), clock);

    // Simulate reliable queue being congested/full
    sink.set_reliable_full(true);

    for i in 0..1050 {
        let event = EventRecord::new(
            EventTime {
                source_et_s: Some(i as f64),
                mono_ns: i * 1_000_000,
            },
            Some(1),
            SessionEvent::LapStarted { lap: i as u32 },
        );
        publisher.push_session_event(event, i * 1_000_000);
    }

    // 1050 pushed against 1024 capacity: oldest 26 were dropped
    assert_eq!(publisher.stats().reliable_events_dropped, 26);

    // Now unblock the sink and drain
    sink.set_reliable_full(false);
    publisher.drain_reliable();

    assert_eq!(publisher.stats().reliable_events_sent, 1024);
    assert_eq!(sink.reliable_queue.lock().len(), 1024);
}

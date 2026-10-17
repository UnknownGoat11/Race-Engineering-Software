//! Sample messages and telemetry payloads shared by the integration tests.
#![allow(dead_code)]

use bytes::Bytes;
use protocol::frame::{Frame, FrameMeta, FramePriority, MsgType};
use protocol::messages::*;
use protocol::version::{Environment, ProtocolVersion};
use telemetry_model::codec::{TlvEncoder, TlvHeader};
use telemetry_model::enums::SessionType;
use telemetry_model::events::{EventRecord, EventTime, SessionEvent};
use telemetry_model::scoring::{
    CapabilityMap, CapabilityStatus, CompetitorEntry, ScoringState, SessionIdentity,
    VehicleCapabilities,
};
use telemetry_model::{ChannelDef, Sample, TELEMETRY_SCHEMA_VERSION, Value, ValueType, registry};

pub const SESSION: u64 = 0xABCD_0001;

/// A plausible valid value of the channel's declared type.
pub fn value_for(def: &ChannelDef, k: u32) -> Value {
    let x = f64::from(k % 997) * 0.25;
    match def.value_type {
        ValueType::F32 => Value::F32(x as f32),
        ValueType::F64 => Value::F64(x),
        ValueType::I32 => Value::I32(k as i32),
        ValueType::U32 => Value::U32(k),
        ValueType::Bool => Value::Bool(k.is_multiple_of(2)),
        ValueType::U8 => Value::U8((k % 200) as u8),
        ValueType::I8 => Value::I8((k % 100) as i8),
        ValueType::U64 => Value::U64(u64::from(k)),
        ValueType::Str => Value::Str(format!("s{k}")),
        ValueType::None => Value::None,
    }
}

/// Appends a TLV block with `channels` records (every 10th unavailable) to `buf`.
pub fn encode_tlv(buf: &mut Vec<u8>, channels: usize, k: u32) -> usize {
    let header = TlvHeader {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns: 1_000_000 + u64::from(k),
        capture_relay_ns_est: 1_790_000_000_000_000_000,
        source_et_s: 123.5,
    };
    let mut enc = TlvEncoder::begin(buf, &header);
    for (i, def) in registry::all()
        .iter()
        .filter(|d| d.value_type != ValueType::None)
        .take(channels)
        .enumerate()
    {
        let sample = if i % 10 == 9 {
            Sample::unavailable()
        } else {
            Sample::valid(value_for(def, k + i as u32))
        };
        enc.push(def.id, &sample).expect("push");
    }
    enc.finish()
}

pub fn tlv_payload(channels: usize) -> TelemetryPayload {
    let mut buf = Vec::new();
    encode_tlv(&mut buf, channels, 7);
    TelemetryPayload::new(Bytes::from(buf)).expect("tlv")
}

fn scoring() -> ScoringState {
    let mut s = ScoringState::default();
    s.session.track_name = Some("Circuit de la Sarthe".into());
    s.session.session_type = Some(SessionType::from_lmu(10));
    s.session.current_et_s = Some(3600.25);
    s.session.weather.ambient_temp_c = Some(21.5);
    for slot in 0..3 {
        let mut c = CompetitorEntry::new(slot);
        c.driver = Some(format!("Driver {slot}"));
        c.place = Some(slot as u8 + 1);
        c.is_player = slot == 1;
        s.vehicles.push(c);
    }
    s
}

/// Sample connection code (canonical form). Must never appear in `Debug` output.
pub const CODE: &str = "7KQ4MZ9T";

fn session_info(resume: ResumeCredential) -> SessionInfo {
    SessionInfo {
        session_id: SESSION,
        driver_display_name: "Driver".into(),
        driver_username: "drv".into(),
        car: Some("Car #51".into()),
        vehicle_class: Some("Hypercar".into()),
        track: Some("Spa".into()),
        session_type: Some(SessionType::from_lmu(5)),
        started_at_ns: 1_790_000_000_000_000_000,
        schema_version: 1,
        resume,
    }
}

fn health(level: HealthLevel) -> ComponentHealth {
    ComponentHealth {
        level,
        detail: Some("ok".into()),
    }
}

/// One sample of every message (covers every message type; HELLO in both forms; AUTH, SESSION_INFO
/// and SESSION_RESUME in every form; CONNECT_CODE with and without a code).
pub fn all_messages() -> Vec<Message> {
    let mut caps = CapabilityMap::from_registry();
    if let Some(first) = registry::all().first() {
        caps.set_status(first.id, CapabilityStatus::Available, Some("seen".into()));
        caps.set_rate(first.id, 50.0);
    }
    let identity = SessionIdentity {
        vehicle_name: Some("Car #51".into()),
        track_name: Some("Spa".into()),
        session_type: Some(SessionType::from_lmu(1)),
        game_version: Some(1234),
        ..SessionIdentity::default()
    };
    let vcaps = VehicleCapabilities {
        supports_hybrid: Some(true),
        ..VehicleCapabilities::default()
    };
    let account = AccountId([7; 16]);
    vec![
        Message::Hello(Hello {
            app_version: "0.1.0".into(),
            protocol: ProtocolVersion::CURRENT,
            role: ClientRole::Driver,
            schema_versions: vec![1, 2],
            client_id: "install-1".into(),
        }),
        Message::ServerHello(ServerHello {
            relay_version: "0.1.0".into(),
            protocol: ProtocolVersion::CURRENT,
            server_time_ns: 1_790_000_000_000_000_000,
            limits: ServerLimits {
                max_payload_bytes: 262_144,
                heartbeat_interval_ms: 1000,
                heartbeat_timeout_ms: 5000,
                max_engineers_per_session: protocol::limits::MAX_ENGINEERS_PER_SESSION,
                session_driver_grace_s: 600,
            },
            env: Environment::Production,
        }),
        Message::Auth(Auth::Password {
            username: "drv".into(),
            password: Secret::new("hunter2"),
        }),
        Message::Auth(Auth::AccessToken {
            token: Secret::new("at"),
        }),
        Message::Auth(Auth::RefreshToken {
            token: Secret::new("rt"),
        }),
        Message::AuthOk(AuthOk {
            account,
            username: "drv".into(),
            display_name: "Driver".into(),
            role: ClientRole::Driver,
            access_token: Secret::new("a"),
            refresh_token: Secret::new("r"),
            access_expires_at_ns: 1,
            refresh_expires_at_ns: 2,
        }),
        Message::AccountRegister(AccountRegister {
            username: "eng".into(),
            password: Secret::new("pw"),
            display_name: "Engineer".into(),
        }),
        Message::SessionCreate(SessionCreate {
            identity,
            lmu_version: Some("1.2.3".into()),
            app_version: "0.1.0".into(),
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capabilities: vcaps,
        }),
        Message::SessionJoin(SessionJoin {
            code: Secret::new(CODE),
        }),
        Message::SessionInfo(session_info(ResumeCredential::DriverToken(Secret::new(
            "resume",
        )))),
        Message::SessionInfo(session_info(ResumeCredential::EngineerTicket(Secret::new(
            "ticket",
        )))),
        Message::SessionResume(SessionResume::Driver {
            session_id: SESSION,
            resume_token: Secret::new("resume"),
            new_sequence_space: true,
        }),
        Message::SessionResume(SessionResume::Engineer {
            session_id: SESSION,
            ticket: Secret::new("ticket"),
        }),
        Message::SessionClose(SessionClose {
            session_id: SESSION,
            reason: SessionCloseReason::RemovedByDriver,
        }),
        Message::ConnectCodeRequest(ConnectCodeRequest),
        Message::ConnectCode(ConnectCode {
            code: Some(IssuedCode {
                code: Secret::new(CODE),
                expires_at_ns: 1_790_000_600_000_000_000,
            }),
            engineers_connected: 1,
            engineers_max: protocol::limits::MAX_ENGINEERS_PER_SESSION,
        }),
        Message::ConnectCode(ConnectCode {
            code: None,
            engineers_connected: 2,
            engineers_max: protocol::limits::MAX_ENGINEERS_PER_SESSION,
        }),
        Message::EngineerList(EngineerList {
            engineers: vec![
                EngineerEntry {
                    connection_id: 1,
                    username: "alice".into(),
                    display_name: "Alice".into(),
                    connected_since_ns: 1_790_000_100_000_000_000,
                    link: EngineerLink::Healthy,
                },
                EngineerEntry {
                    connection_id: 2,
                    username: "bob".into(),
                    display_name: "Bob".into(),
                    connected_since_ns: 1_790_000_200_000_000_000,
                    link: EngineerLink::Reconnecting,
                },
            ],
        }),
        Message::EngineerKick(EngineerKick { connection_id: 2 }),
        Message::DriverPresence(DriverPresence::away(
            SESSION,
            1_700_000_000_000_000_000,
            1_700_000_600_000_000_000,
        )),
        Message::SchemaNegotiate(SchemaNegotiate {
            schema_version: 1,
            capabilities: caps,
            vehicle_capabilities: vcaps,
        }),
        Message::TelemetryDelta(tlv_payload(150)),
        Message::TelemetrySnapshot(tlv_payload(400)),
        Message::ScoringUpdate(ScoringUpdate {
            capture: CaptureTime {
                mono_ns: 10,
                relay_ns_est: Some(-5),
            },
            scoring: scoring(),
        }),
        Message::WeatherUpdate(tlv_payload(5)),
        Message::SessionEvent(SessionEventMsg {
            capture: CaptureTime {
                mono_ns: 11,
                relay_ns_est: None,
            },
            record: EventRecord::new(
                EventTime {
                    source_et_s: Some(12.5),
                    mono_ns: 11,
                },
                Some(3),
                SessionEvent::LapCompleted {
                    lap: 3,
                    time_s: Some(98.765),
                    valid: true,
                },
            ),
        }),
        Message::SnapshotRequest(SnapshotRequest {
            reason: SnapshotReason::SequenceGap,
        }),
        Message::DriverStatus(DriverStatus {
            lmu_state: LmuSourceState::Live,
            lmu_source: health(HealthLevel::Ok),
            processing: health(HealthLevel::Ok),
            recording: health(HealthLevel::Inactive),
            network: ComponentHealth {
                level: HealthLevel::Degraded,
                detail: None,
            },
            rates: DriverRates {
                telemetry_read_hz: Some(100.0),
                scoring_read_hz: Some(5.0),
                frames_sent_hz: None,
                bytes_sent_per_s: None,
                frames_dropped_total: 3,
            },
            clock: Some(ClockSummary {
                offset_ns: -42,
                uncertainty_ns: 1_000_000,
            }),
        }),
        Message::RecordingStatus(RecordingStatus {
            state: RecordingState::Active,
            recording_id: Some("rec-1".into()),
            bytes_written: 1 << 20,
            frames_written: 1000,
            frames_lost: 0,
            disk_free_bytes: None,
            error: None,
        }),
        Message::Ping(Ping { id: 1, t0: 2 }),
        Message::Pong(Pong {
            id: 1,
            t0: 2,
            t1: 3,
            t2: 4,
        }),
        Message::Disconnect(Disconnect {
            reason: DisconnectReason::ClientShutdown,
            message: None,
        }),
        Message::Error(ErrorMsg {
            code: ErrorCode::NotAuthorized,
            message: "no".into(),
            fatal: false,
        }),
    ]
}

/// Frame metadata appropriate for the message's class.
pub fn meta_for(msg_type: MsgType, seq: u64) -> FrameMeta {
    if msg_type.is_stream() {
        FrameMeta::stream(msg_type.default_priority(), SESSION, seq, seq * 1000)
    } else {
        FrameMeta::control(SESSION, seq * 1000)
    }
}

/// Every sample message encoded as a frame.
pub fn all_frames() -> Vec<Frame> {
    all_messages()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            m.to_frame(&meta_for(m.msg_type(), i as u64 + 1))
                .expect("encode")
        })
        .collect()
}

/// A realistic 150-channel delta frame.
pub fn delta_frame(seq: u64) -> Frame {
    Message::TelemetryDelta(tlv_payload(150))
        .to_frame(&FrameMeta::stream(FramePriority::P0, SESSION, seq, seq))
        .expect("encode")
}

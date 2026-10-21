//! End-to-end tests of the relay over real TLS 1.3 sockets with the in-memory store: handshake,
//! forwarding with relay trailer, D-022 connection codes, slots, tickets, kicks, session isolation,
//! malformed input, tokens, slow consumers, driver grace/resume/restart, relay restart, heartbeat,
//! admin endpoints, and an ignored forwarding benchmark.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{
    Client, DRIVER, DRIVER_B, ENGINEER, ENGINEER2, ENGINEER3, PASSWORD, QUIET, TestRelay, WAIT,
    code_of, http_get, raw_frame, test_config, wait_for, wait_store_status,
};
use lmu_relay::jwt::AccessTokenKeys;
use lmu_relay::outbox::LIVE_SLOTS;
use lmu_relay::store::SessionStatus;
use lmu_relay::time::{WallClock, unix_now_ns};
use lmu_relay::{MemoryStore, Store};
use protocol::frame::{FrameHeader, FrameMeta, FramePriority, MAX_PAYLOAD_BYTES, MsgType};
use protocol::messages::{
    AccountRegister, Auth, ClientRole, ConnectCodeRequest, Disconnect, DisconnectReason,
    EngineerKick, EngineerLink, ErrorCode, Hello, Message, ResumeCredential, Secret, SessionClose,
    SessionCloseReason, SessionInfo, SessionResume, SnapshotReason, SnapshotRequest,
};
use protocol::version::{Environment, ProtocolVersion};

fn engineer_ticket(info: &SessionInfo) -> Secret {
    match &info.resume {
        ResumeCredential::EngineerTicket(t) => t.clone(),
        ResumeCredential::DriverToken(_) => panic!("engineer ticket expected"),
    }
}

fn driver_token(info: &SessionInfo) -> Secret {
    match &info.resume {
        ResumeCredential::DriverToken(t) => t.clone(),
        ResumeCredential::EngineerTicket(_) => panic!("driver token expected"),
    }
}

async fn session_info(client: &mut Client) -> SessionInfo {
    client
        .recv_until(|m| match m {
            Message::SessionInfo(info) => Some(info),
            _ => None,
        })
        .await
}

async fn session_close(client: &mut Client) -> SessionClose {
    client
        .recv_until(|m| match m {
            Message::SessionClose(c) => Some(c),
            _ => None,
        })
        .await
}

// -------------------------------------------------------------------------------------------------
// Handshake and forwarding
// -------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_and_engineer_handshake_and_forwarding_with_trailer() {
    let relay = TestRelay::start(|_| {}).await;
    let mut driver = relay.client().await;
    let hello = driver.hello(ClientRole::Driver).await;
    assert_eq!(hello.protocol, ProtocolVersion::CURRENT);
    assert_eq!(hello.env, Environment::Development);
    assert_eq!(hello.limits.max_engineers_per_session, 2);
    assert_eq!(hello.limits.max_payload_bytes as usize, MAX_PAYLOAD_BYTES);
    let ok = driver
        .auth(Auth::Password {
            username: DRIVER.into(),
            password: Secret::new(PASSWORD),
        })
        .await
        .expect("login");
    assert_eq!(ok.role, ClientRole::Driver);
    assert_eq!(ok.display_name, "Dave Driver");
    let (info, cc, list) = driver.create_session().await;
    assert!(matches!(info.resume, ResumeCredential::DriverToken(_)));
    assert_eq!(info.car.as_deref(), Some("Ferrari 499P #50"));
    assert_eq!(info.vehicle_class.as_deref(), Some("Hypercar"));
    assert_eq!((cc.engineers_connected, cc.engineers_max), (0, 2));
    let first_code = code_of(&cc);
    assert_eq!(first_code.expose().len(), 8);
    assert!(list.engineers.is_empty());

    // Frames cached for the join replay.
    let schema_seq = driver
        .send_stream(
            MsgType::SchemaNegotiate,
            FramePriority::Control,
            b"schema-v1",
        )
        .await;
    let snapshot_seq = driver
        .send_stream(
            MsgType::TelemetrySnapshot,
            FramePriority::P0,
            b"full-snapshot",
        )
        .await;
    let sid = info.session_id;
    wait_for(|| {
        relay
            .handle
            .session_debug(sid)
            .is_some_and(|d| d.cached_frames == 2)
    })
    .await;

    let (mut erin, einfo) = relay.engineer_joined(ENGINEER, &first_code).await;
    assert_eq!(einfo.session_id, sid);
    assert_eq!(einfo.driver_display_name, "Dave Driver");
    assert_eq!(einfo.driver_username, DRIVER);
    assert_eq!(einfo.track.as_deref(), Some("Circuit de la Sarthe"));
    assert_eq!(einfo.started_at_ns, info.started_at_ns);
    assert!(matches!(einfo.resume, ResumeCredential::EngineerTicket(_)));
    // Replay: original header and sequence, REPLAYED trailer flag, payload untouched.
    for (seq, payload) in [
        (schema_seq, &b"schema-v1"[..]),
        (snapshot_seq, &b"full-snapshot"[..]),
    ] {
        let frame = erin.frame().await.expect("replayed frame");
        let trailer = frame.relay_trailer().expect("relay trailer");
        assert!(trailer.replayed);
        assert_eq!(frame.header().sequence, seq);
        assert_eq!(frame.payload(), payload);
    }
    // The driver is asked for a fresh snapshot and sees the engineer and a fresh code.
    let request = driver.stream_of(MsgType::SnapshotRequest).await;
    match Message::from_frame(&request).expect("decodes") {
        Message::SnapshotRequest(SnapshotRequest { reason }) => {
            assert_eq!(reason, SnapshotReason::EngineerJoined);
        }
        other => panic!("SNAPSHOT_REQUEST expected, got {other:?}"),
    }
    let cc = driver.connect_code().await;
    assert_eq!(cc.engineers_connected, 1);
    assert_ne!(code_of(&cc), first_code);
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers.len(), 1);
    assert_eq!(list.engineers[0].username, ENGINEER);
    assert_eq!(list.engineers[0].display_name, "Erin Engineer");
    assert_eq!(list.engineers[0].link, EngineerLink::Healthy);

    // Live forwarding with a relay trailer.
    let before = unix_now_ns();
    let seq = driver
        .send_stream(MsgType::TelemetryDelta, FramePriority::P1, b"live-delta")
        .await;
    let frame = erin.stream_of(MsgType::TelemetryDelta).await;
    let after = unix_now_ns();
    let trailer = *frame.relay_trailer().expect("relay trailer");
    assert!(!trailer.replayed);
    assert_eq!(trailer.relay_dropped, 0);
    assert!(before <= trailer.relay_rx_ns && trailer.relay_rx_ns <= trailer.relay_tx_ns);
    assert!(trailer.relay_tx_ns <= after);
    assert_eq!(frame.header().sequence, seq);
    assert_eq!(frame.header().session_id, sid);
    assert_eq!(frame.header().priority, FramePriority::P1);
    assert_eq!(frame.payload(), b"live-delta");

    let metrics = relay.handle.metrics();
    assert!(metrics.telemetry_packets_received.get() >= 3);
    assert!(metrics.telemetry_packets_forwarded.get() >= 3);
    assert!(metrics.relay_forward_latency.count() >= 1);
    let gauges = relay.handle.gauges();
    assert_eq!((gauges.active_sessions, gauges.engineers_connected), (1, 1));
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_are_strictly_isolated() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver_a, info_a, cc_a) = relay.driver_session(DRIVER).await;
    let (mut driver_b, info_b, cc_b) = relay.driver_session(DRIVER_B).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc_a)).await;
    let (mut eddie, _) = relay.engineer_joined(ENGINEER2, &code_of(&cc_b)).await;
    driver_a.engineer_list().await;
    driver_b.engineer_list().await;

    driver_a
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"from-A")
        .await;
    driver_b
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"from-B")
        .await;
    assert_eq!(
        erin.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"from-A"
    );
    assert_eq!(
        eddie.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"from-B"
    );
    assert!(
        erin.quiet(QUIET).await,
        "engineer of A receives nothing of B"
    );
    assert!(
        eddie.quiet(QUIET).await,
        "engineer of B receives nothing of A"
    );

    // Driver A forges B's session id: a header-rule violation, never forwarded.
    let forged = protocol::Frame::build(
        MsgType::TelemetryDelta,
        &FrameMeta::stream(FramePriority::P0, info_b.session_id, 1_000, 0),
        b"forged",
    )
    .expect("frame");
    driver_a.send_frame(&forged).await;
    assert_eq!(driver_a.error().await.code, ErrorCode::NotAuthorized);
    assert!(eddie.quiet(QUIET).await);

    // Engineer of A asks B's driver for a snapshot with B's session id: refused.
    let request = Message::SnapshotRequest(SnapshotRequest {
        reason: SnapshotReason::Manual,
    })
    .to_frame(&FrameMeta::stream(
        FramePriority::Control,
        info_b.session_id,
        1,
        0,
    ))
    .expect("frame");
    erin.send_frame(&request).await;
    assert_eq!(erin.error().await.code, ErrorCode::NotAuthorized);
    assert!(
        driver_b.quiet(QUIET).await,
        "driver B receives nothing from A's engineer"
    );
    assert_ne!(info_a.session_id, info_b.session_id);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engineers_can_only_send_snapshot_requests_towards_the_driver() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, _info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    driver.engineer_list().await;

    // Telemetry and driver controls from an engineer are refused by the direction table.
    erin.send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"not-allowed")
        .await;
    assert_eq!(erin.error().await.code, ErrorCode::NotAuthorized);
    erin.send(&Message::EngineerKick(EngineerKick { connection_id: 1 }))
        .await;
    assert_eq!(erin.error().await.code, ErrorCode::NotAuthorized);
    erin.send(&Message::ConnectCodeRequest(ConnectCodeRequest))
        .await;
    assert_eq!(erin.error().await.code, ErrorCode::NotAuthorized);

    // SNAPSHOT_REQUEST is forwarded as-is, at most one per 250 ms per engineer.
    let seq = erin
        .send_snapshot_request(SnapshotReason::SequenceGap)
        .await;
    let forwarded = driver.stream_of(MsgType::SnapshotRequest).await;
    assert_eq!(forwarded.header().sequence, seq);
    assert!(forwarded.relay_trailer().is_none());
    match Message::from_frame(&forwarded).expect("decodes") {
        Message::SnapshotRequest(r) => assert_eq!(r.reason, SnapshotReason::SequenceGap),
        other => panic!("unexpected {other:?}"),
    }
    erin.send_snapshot_request(SnapshotReason::SequenceGap)
        .await;
    assert!(
        driver.quiet(QUIET).await,
        "second request inside 250 ms is coalesced"
    );
    let second = erin.send_snapshot_request(SnapshotReason::Manual).await;
    let forwarded = driver.stream_of(MsgType::SnapshotRequest).await;
    assert_eq!(forwarded.header().sequence, second);
    relay.shutdown().await;
}

/// A CRC-correct frame with an arbitrary header minor, bypassing the encoder.
fn frame_with_minor(
    minor: u8,
    msg_type: MsgType,
    session_id: u64,
    sequence: u64,
    payload: &[u8],
) -> Vec<u8> {
    let header = FrameHeader {
        minor,
        msg_type: msg_type.id(),
        flags: protocol::frame::Flags::empty(),
        priority: FramePriority::Control,
        payload_len: u32::try_from(payload.len()).expect("fits"),
        session_id,
        sequence,
        source_ts_ns: 0,
    };
    let mut bytes = header.to_bytes().to_vec();
    bytes.extend_from_slice(payload);
    let crc = crc32c::crc32c(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engineer_bytes_never_reach_the_driver() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, _info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    driver.engineer_list().await;
    let canonical = Message::SnapshotRequest(SnapshotRequest {
        reason: SnapshotReason::Manual,
    })
    .to_frame(&FrameMeta::stream(FramePriority::Control, 1, 1, 0))
    .expect("frame");
    let newer_minor = protocol::PROTOCOL_MINOR + 1;

    // A newer minor with 200 KiB appended: refused (Malformed) and never forwarded.
    let mut junk = canonical.payload().to_vec();
    junk.extend(std::iter::repeat_n(0xEE, 200 * 1024));
    let seq = erin.next_seq();
    erin.send_raw(&frame_with_minor(
        newer_minor,
        MsgType::SnapshotRequest,
        erin.session_id,
        seq,
        &junk,
    ))
    .await;
    let e = erin.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::Malformed, false));
    assert!(driver.quiet(QUIET).await, "nothing reaches the driver");

    // A newer minor with a few appended bytes is accepted, but the driver gets the relay's
    // canonical encoding: the engineer's sequence, this build's minor, no appended byte.
    let mut appended = canonical.payload().to_vec();
    appended.extend_from_slice(&[0xEE; 8]);
    let seq = erin.next_seq();
    erin.send_raw(&frame_with_minor(
        newer_minor,
        MsgType::SnapshotRequest,
        erin.session_id,
        seq,
        &appended,
    ))
    .await;
    let forwarded = driver.stream_of(MsgType::SnapshotRequest).await;
    assert_eq!(forwarded.header().sequence, seq);
    assert_eq!(forwarded.header().minor, protocol::PROTOCOL_MINOR);
    assert_eq!(forwarded.payload(), canonical.payload());
    match Message::from_frame(&forwarded).expect("decodes") {
        Message::SnapshotRequest(r) => assert_eq!(r.reason, SnapshotReason::Manual),
        other => panic!("unexpected {other:?}"),
    }
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// D-022 connection codes, slots, kicks and tickets
// -------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_codes_are_single_use_rotate_and_fill_two_slots() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, info, cc0) = relay.driver_session(DRIVER).await;
    let first = code_of(&cc0);

    // Redeem: the driver gets a fresh code for the second slot.
    let (mut erin, erin_info) = relay.engineer_joined(ENGINEER, &first).await;
    let cc1 = driver.connect_code().await;
    assert_eq!(cc1.engineers_connected, 1);
    let second = code_of(&cc1);
    assert_ne!(second, first);
    driver.engineer_list().await;

    // A redeemed code is never reusable.
    let mut eddie = relay.client().await;
    eddie.login(ClientRole::Engineer, ENGINEER2).await;
    assert_eq!(
        eddie.join(&first).await.expect_err("used").code,
        ErrorCode::CodeInvalid
    );

    // New code: the unused one dies at once.
    driver
        .send(&Message::ConnectCodeRequest(ConnectCodeRequest))
        .await;
    let cc2 = driver.connect_code().await;
    let third = code_of(&cc2);
    assert_ne!(third, second);
    assert_eq!(
        eddie.join(&second).await.expect_err("rotated").code,
        ErrorCode::CodeInvalid
    );

    // The display form (lower case, separator) is accepted.
    let display = format!("{}-{}", &third.expose()[..4], &third.expose()[4..]).to_lowercase();
    eddie
        .join(&Secret::new(display))
        .await
        .expect("second engineer joins");
    let cc3 = driver.connect_code().await;
    assert_eq!((cc3.engineers_connected, cc3.engineers_max), (2, 2));
    assert!(cc3.code.is_none(), "no code while every slot is taken");
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers.len(), 2);

    // A third engineer is refused with the owner's wording; no code is issued while full.
    let mut emma = relay.client().await;
    emma.login(ClientRole::Engineer, ENGINEER3).await;
    let full = emma.join(&third).await.expect_err("full");
    assert_eq!(full.code, ErrorCode::SessionFull);
    assert_eq!(full.message, "Driver already has 2 engineers connected");
    assert!(!full.fatal);
    driver
        .send(&Message::ConnectCodeRequest(ConnectCodeRequest))
        .await;
    let cc4 = driver.connect_code().await;
    assert!(cc4.code.is_none());
    assert_eq!(cc4.engineers_connected, 2);

    // Kick: SESSION_CLOSE{RemovedByDriver}, disconnected, slot freed with a fresh code.
    let erin_id = list
        .engineers
        .iter()
        .find(|e| e.username == ENGINEER)
        .expect("erin listed")
        .connection_id;
    driver
        .send(&Message::EngineerKick(EngineerKick {
            connection_id: erin_id,
        }))
        .await;
    assert_eq!(
        session_close(&mut erin).await.reason,
        SessionCloseReason::RemovedByDriver
    );
    assert!(erin.closed().await, "kicked engineer is disconnected");
    let cc5 = driver.connect_code().await;
    assert_eq!(cc5.engineers_connected, 1);
    let after_kick = code_of(&cc5);
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers.len(), 1);
    assert_eq!(list.engineers[0].username, ENGINEER2);

    // The kicked engineer's ticket is dead; only a new code connects.
    let mut erin2 = relay.client().await;
    erin2.login(ClientRole::Engineer, ENGINEER).await;
    erin2
        .send(&Message::SessionResume(SessionResume::Engineer {
            session_id: info.session_id,
            ticket: engineer_ticket(&erin_info),
        }))
        .await;
    assert_eq!(erin2.error().await.code, ErrorCode::NotAuthorized);
    erin2.join(&after_kick).await.expect("new code connects");
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_codes_expire() {
    let relay =
        TestRelay::start(|c| c.timeouts.connect_code_ttl = Duration::from_millis(600)).await;
    let (mut driver, _info, cc) = relay.driver_session(DRIVER).await;
    let code = code_of(&cc);
    let expiry = driver.connect_code().await;
    assert!(
        expiry.code.is_none(),
        "the driver is told when the code expired"
    );
    let mut erin = relay.client().await;
    erin.login(ClientRole::Engineer, ENGINEER).await;
    assert_eq!(
        erin.join(&code).await.expect_err("expired").code,
        ErrorCode::CodeExpired
    );
    driver
        .send(&Message::ConnectCodeRequest(ConnectCodeRequest))
        .await;
    let fresh = code_of(&driver.connect_code().await);
    erin.join(&fresh).await.expect("a fresh code works");
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn joins_need_an_engineer_account_and_guessing_is_rate_limited() {
    // The D-022 limits: 5 per minute and 20 per hour, per account and per IP.
    let relay = TestRelay::start(|c| {
        c.limits.code_redemptions_per_minute = protocol::limits::CODE_REDEMPTIONS_PER_MINUTE;
        c.limits.code_redemptions_per_hour = protocol::limits::CODE_REDEMPTIONS_PER_HOUR;
    })
    .await;
    let (_driver, info, cc) = relay.driver_session(DRIVER).await;
    let code = code_of(&cc);

    // Wrong role: a driver account cannot redeem a code.
    let mut bianca = relay.client().await;
    bianca.login(ClientRole::Driver, DRIVER_B).await;
    bianca
        .send(&Message::SessionJoin(protocol::messages::SessionJoin {
            code: code.clone(),
        }))
        .await;
    assert_eq!(bianca.error().await.code, ErrorCode::NotAuthorized);

    // An engineer without a session cannot send stream frames into one.
    let mut erin = relay.client().await;
    erin.login(ClientRole::Engineer, ENGINEER).await;
    let request = Message::SnapshotRequest(SnapshotRequest {
        reason: SnapshotReason::Manual,
    })
    .to_frame(&FrameMeta::stream(
        FramePriority::Control,
        info.session_id,
        1,
        0,
    ))
    .expect("frame");
    erin.send_frame(&request).await;
    assert_eq!(erin.error().await.code, ErrorCode::NotAuthorized);

    // Guessing: random codes are invalid, and after 5 attempts per minute even the right code
    // is refused (per account and per IP).
    for guess in ["00000000", "7KQ4MZ9T", "ZZZZZZZZ", "not-a-code", "ABCDEFGH"] {
        assert_eq!(
            erin.join(&Secret::new(guess))
                .await
                .expect_err("guess")
                .code,
            ErrorCode::CodeInvalid
        );
    }
    let limited = erin.join(&code).await.expect_err("rate limited");
    assert_eq!(limited.code, ErrorCode::RateLimited);
    let mut eddie = relay.client().await;
    eddie.login(ClientRole::Engineer, ENGINEER2).await;
    assert_eq!(
        eddie.join(&code).await.expect_err("same IP").code,
        ErrorCode::RateLimited
    );
    assert!(relay.handle.metrics().join_failures.get() >= 7);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engineer_tickets_resume_within_the_window_only() {
    let relay =
        TestRelay::start(|c| c.timeouts.engineer_ticket_ttl = Duration::from_millis(800)).await;
    let (mut driver, info, cc) = relay.driver_session(DRIVER).await;
    let sid = info.session_id;
    let (erin, erin_info) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    driver.engineer_list().await;

    // Network blip: the slot is held and shown as reconnecting.
    erin.kill();
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers[0].link, EngineerLink::Reconnecting);

    // Within the window the still-running engineer app resumes with its ticket.
    let mut erin2 = relay.client().await;
    erin2.login(ClientRole::Engineer, ENGINEER).await;
    erin2
        .send(&Message::SessionResume(SessionResume::Engineer {
            session_id: sid,
            ticket: engineer_ticket(&erin_info),
        }))
        .await;
    let resumed = session_info(&mut erin2).await;
    erin2.session_id = sid;
    let ticket2 = engineer_ticket(&resumed);
    assert_ne!(ticket2, engineer_ticket(&erin_info), "tickets rotate");
    driver
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"after-resume")
        .await;
    assert_eq!(
        erin2.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"after-resume"
    );
    let list = driver.engineer_list().await;
    assert_ne!(list.engineers[0].link, EngineerLink::Reconnecting);

    // After the window the ticket is dead and the slot is freed.
    erin2.kill();
    let freed = driver
        .recv_until(|m| match m {
            Message::ConnectCode(c) if c.engineers_connected == 0 => Some(c),
            _ => None,
        })
        .await;
    assert!(freed.code.is_some());
    let mut erin3 = relay.client().await;
    erin3.login(ClientRole::Engineer, ENGINEER).await;
    erin3
        .send(&Message::SessionResume(SessionResume::Engineer {
            session_id: sid,
            ticket: ticket2,
        }))
        .await;
    assert_eq!(erin3.error().await.code, ErrorCode::NotAuthorized);

    // An explicit disconnect (engineer app closed) kills the ticket at once.
    let (mut erin4, info4) = relay.engineer_joined(ENGINEER, &code_of(&freed)).await;
    erin4
        .send(&Message::Disconnect(Disconnect {
            reason: DisconnectReason::ClientShutdown,
            message: None,
        }))
        .await;
    assert!(erin4.closed().await);
    let mut erin5 = relay.client().await;
    erin5.login(ClientRole::Engineer, ENGINEER).await;
    erin5
        .send(&Message::SessionResume(SessionResume::Engineer {
            session_id: sid,
            ticket: engineer_ticket(&info4),
        }))
        .await;
    assert_eq!(erin5.error().await.code, ErrorCode::NotAuthorized);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_reply_lost_in_a_second_blip_is_retried_with_the_previous_ticket() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, info, cc) = relay.driver_session(DRIVER).await;
    let sid = info.session_id;
    let (erin, erin_info) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    let joined = engineer_ticket(&erin_info);
    driver.engineer_list().await;
    let resume = |ticket: &Secret| {
        Message::SessionResume(SessionResume::Engineer {
            session_id: sid,
            ticket: ticket.clone(),
        })
    };

    // Blip 1, then blip 2 before the engineer app read the resume's SESSION_INFO: the new ticket
    // never reached it.
    erin.kill();
    let mut lost = relay.client().await;
    lost.login(ClientRole::Engineer, ENGINEER).await;
    lost.send(&resume(&joined)).await;
    let never_seen = engineer_ticket(&session_info(&mut lost).await);
    lost.kill();

    // The app presents the ticket it holds again: accepted within the grace, with a new ticket.
    let mut back = relay.client().await;
    back.login(ClientRole::Engineer, ENGINEER).await;
    back.send(&resume(&joined)).await;
    let resumed = session_info(&mut back).await;
    back.session_id = sid;
    let fresh = engineer_ticket(&resumed);
    assert!(fresh != joined && fresh != never_seen);
    driver
        .send_stream(
            MsgType::TelemetryDelta,
            FramePriority::P0,
            b"after-two-blips",
        )
        .await;
    assert_eq!(
        back.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"after-two-blips"
    );
    let list = driver
        .recv_until(|m| match m {
            Message::EngineerList(l)
                if l.engineers
                    .first()
                    .is_some_and(|e| e.link == EngineerLink::Healthy) =>
            {
                Some(l)
            }
            _ => None,
        })
        .await;
    assert_eq!(list.engineers.len(), 1, "still the same single slot");

    // The ticket that was never delivered is dead.
    let mut probe = relay.client().await;
    probe.login(ClientRole::Engineer, ENGINEER).await;
    probe.send(&resume(&never_seen)).await;
    assert_eq!(probe.error().await.code, ErrorCode::NotAuthorized);
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Driver grace, resume and restart
// -------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_reconnect_and_restart_keep_engineers_connected() {
    let relay = TestRelay::start(|_| {}).await;
    let mut driver = relay.client().await;
    let login = driver.login(ClientRole::Driver, DRIVER).await;
    let (info, cc, _) = driver.create_session().await;
    let sid = info.session_id;
    let token = driver_token(&info);
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    // The code for a second engineer, which the driver may already have shared.
    let shared = driver.connect_code().await;
    assert_eq!(shared.engineers_connected, 1);
    let shared = shared.code.expect("a slot is free");
    let seq1 = driver
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"before")
        .await;
    assert_eq!(
        erin.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"before"
    );

    // Network loss: no DISCONNECT. The session enters its grace period; the engineer stays.
    driver.kill();
    wait_for(|| {
        relay
            .handle
            .session_debug(sid)
            .is_some_and(|d| !d.driver_attached)
    })
    .await;
    // The engineer is told at once that the driver is away (H1): no frozen stream looks live.
    match erin.recv().await {
        Message::DriverPresence(p) => {
            assert!(!p.present, "driver away announced");
            assert_eq!(p.session_id, sid);
            assert!(p.grace_expires_at_ns > p.away_since_ns);
        }
        other => panic!("DRIVER_PRESENCE expected, got {other:?}"),
    }
    assert!(erin.quiet(QUIET).await, "no SESSION_CLOSE for the engineer");
    wait_store_status(relay.handle.store(), sid, SessionStatus::Grace).await;

    // Reconnect in the same sequence space: silent re-authentication and SESSION_RESUME.
    let mut driver2 = relay.client().await;
    driver2.hello(ClientRole::Driver).await;
    let login2 = driver2
        .auth(Auth::RefreshToken {
            token: login.refresh_token.clone(),
        })
        .await
        .expect("refresh token login");
    driver2
        .send(&Message::SessionResume(SessionResume::Driver {
            session_id: sid,
            resume_token: token.clone(),
            new_sequence_space: false,
        }))
        .await;
    let resumed = session_info(&mut driver2).await;
    assert_eq!(resumed.session_id, sid);
    assert_eq!(driver_token(&resumed), token);
    driver2.session_id = sid;
    let cc = driver2.connect_code().await;
    assert_eq!(cc.engineers_connected, 1);
    assert_eq!(
        cc.code.as_ref(),
        Some(&shared),
        "the unused code is re-sent unchanged (same code, same expiry)"
    );
    let list = driver2.engineer_list().await;
    assert_eq!(list.engineers[0].username, ENGINEER);
    // The engineer learns that the driver is back.
    assert!(matches!(
        erin.recv().await,
        Message::DriverPresence(p) if p.present
    ));
    driver2.continue_seq(seq1);
    driver2
        .send_stream(
            MsgType::TelemetryDelta,
            FramePriority::P0,
            b"after-reconnect",
        )
        .await;
    assert_eq!(
        erin.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"after-reconnect"
    );
    assert!(relay.handle.metrics().reconnects.get() >= 1);
    wait_store_status(relay.handle.store(), sid, SessionStatus::Live).await;

    // Driver app restart: a new sequence space. Every engineer gets SESSION_INFO before any frame
    // of the new space, and stays connected.
    driver2.kill();
    let mut driver3 = relay.client().await;
    driver3.hello(ClientRole::Driver).await;
    driver3
        .auth(Auth::RefreshToken {
            token: login2.refresh_token.clone(),
        })
        .await
        .expect("refresh token login");
    driver3
        .send(&Message::SessionResume(SessionResume::Driver {
            session_id: sid,
            resume_token: token,
            new_sequence_space: true,
        }))
        .await;
    assert_eq!(session_info(&mut driver3).await.session_id, sid);
    driver3.session_id = sid;
    assert_eq!(driver3.connect_code().await.code.as_ref(), Some(&shared));
    driver3.reset_seq();
    let restarted_seq = driver3
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"new-space")
        .await;
    assert_eq!(restarted_seq, 1);
    // The engineer's next messages: driver away (the restart), SESSION_INFO (stream reset, new
    // ticket), driver back, then the new frame.
    assert!(matches!(
        erin.recv().await,
        Message::DriverPresence(p) if !p.present
    ));
    let first = erin.frame().await.expect("open");
    match Message::from_frame(&first).expect("decodes") {
        Message::SessionInfo(reset) => {
            assert_eq!(reset.session_id, sid);
            assert!(matches!(reset.resume, ResumeCredential::EngineerTicket(_)));
        }
        other => panic!("SESSION_INFO expected before the new space, got {other:?}"),
    }
    assert!(matches!(
        erin.recv().await,
        Message::DriverPresence(p) if p.present
    ));
    let frame = erin.stream_of(MsgType::TelemetryDelta).await;
    assert_eq!(frame.header().sequence, 1);
    assert_eq!(frame.payload(), b"new-space");
    // The code shared before both driver reconnects still admits the second engineer, once.
    relay.engineer_joined(ENGINEER2, &shared.code).await;
    let mut late = relay.client().await;
    late.login(ClientRole::Engineer, ENGINEER3).await;
    assert_eq!(
        late.join(&shared.code).await.expect_err("single use").code,
        ErrorCode::SessionFull
    );
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_session_ends_when_the_driver_grace_period_elapses() {
    let relay = TestRelay::start(|c| c.timeouts.driver_grace = Duration::from_millis(700)).await;
    let mut driver = relay.client().await;
    let login = driver.login(ClientRole::Driver, DRIVER).await;
    let (info, cc, _) = driver.create_session().await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    driver.kill();
    assert_eq!(
        session_close(&mut erin).await.reason,
        SessionCloseReason::Expired
    );
    assert!(erin.closed().await);
    wait_for(|| relay.handle.session_debug(info.session_id).is_none()).await;
    let mut driver2 = relay.client().await;
    driver2.hello(ClientRole::Driver).await;
    driver2
        .auth(Auth::RefreshToken {
            token: login.refresh_token,
        })
        .await
        .expect("login");
    driver2
        .send(&Message::SessionResume(SessionResume::Driver {
            session_id: info.session_id,
            resume_token: driver_token(&info),
            new_sequence_space: false,
        }))
        .await;
    assert_eq!(driver2.error().await.code, ErrorCode::SessionClosed);
    let record =
        wait_store_status(relay.handle.store(), info.session_id, SessionStatus::Closed).await;
    assert_eq!(record.close_reason.as_deref(), Some("expired"));
    assert_eq!(record.resume_hash, None, "the resume digest is cleared");
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_session_close_ends_the_session_for_engineers() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    driver.engineer_list().await;
    driver
        .send(&Message::SessionClose(SessionClose {
            session_id: info.session_id,
            reason: SessionCloseReason::DriverStopped,
        }))
        .await;
    assert_eq!(
        session_close(&mut erin).await.reason,
        SessionCloseReason::DriverStopped
    );
    assert!(erin.closed().await);
    // The driver connection stays usable for a new session.
    driver.session_id = 0;
    let (info2, _, _) = driver.create_session().await;
    assert_ne!(info2.session_id, info.session_id);
    relay.shutdown().await;
}

/// A relay process over `store` with the fixed signing key `seed` (a restart keeps both, like a
/// production relay keeps PostgreSQL and `JWT_KEY_FILE`).
async fn relay_process(store: &MemoryStore, seed: [u8; 32], create_accounts: bool) -> TestRelay {
    TestRelay::start_with(
        test_config(),
        Store::Memory(store.clone()),
        Some(AccessTokenKeys::from_seed(seed)),
        None,
        create_accounts,
    )
    .await
}

/// Re-authenticates with a refresh token and sends SESSION_RESUME::Engineer (the answer is left
/// to the caller).
async fn engineer_resume(
    relay: &TestRelay,
    refresh_token: &Secret,
    session_id: u64,
    ticket: Secret,
) -> Client {
    let mut client = relay.client().await;
    client.hello(ClientRole::Engineer).await;
    client
        .auth(Auth::RefreshToken {
            token: refresh_token.clone(),
        })
        .await
        .expect("refresh token survives the restart");
    client
        .send(&Message::SessionResume(SessionResume::Engineer {
            session_id,
            ticket,
        }))
        .await;
    client
}

/// [`engineer_resume`] that must succeed: the client (bound to the session) and its new ticket.
async fn engineer_resumed(
    relay: &TestRelay,
    refresh_token: &Secret,
    session_id: u64,
    ticket: Secret,
) -> (Client, Secret) {
    let mut client = engineer_resume(relay, refresh_token, session_id, ticket.clone()).await;
    let resumed = session_info(&mut client).await;
    assert_eq!(resumed.session_id, session_id);
    let fresh = engineer_ticket(&resumed);
    assert_ne!(fresh, ticket, "every SESSION_INFO carries a fresh ticket");
    client.session_id = session_id;
    (client, fresh)
}

/// The driver's next ENGINEER_LIST with `count` entries (older lists are skipped).
async fn engineer_list_of(driver: &mut Client, count: usize) -> protocol::messages::EngineerList {
    driver
        .recv_until(|m| match m {
            Message::EngineerList(l) if l.engineers.len() == count => Some(l),
            _ => None,
        })
        .await
}

/// Re-authenticates a driver with its access token and resumes `info`'s session.
async fn driver_resumed(relay: &TestRelay, access_token: &Secret, info: &SessionInfo) -> Client {
    let mut driver = relay.client().await;
    driver.hello(ClientRole::Driver).await;
    driver
        .auth(Auth::AccessToken {
            token: access_token.clone(),
        })
        .await
        .expect("access token still valid");
    driver
        .send(&Message::SessionResume(SessionResume::Driver {
            session_id: info.session_id,
            resume_token: driver_token(info),
            new_sequence_space: false,
        }))
        .await;
    let restored = session_info(&mut driver).await;
    assert_eq!(restored.session_id, info.session_id);
    assert_eq!(restored.driver_username, info.driver_username);
    assert_eq!(restored.started_at_ns, info.started_at_ns);
    driver.session_id = info.session_id;
    driver
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_restarts_keep_accounts_sessions_and_engineer_slots() {
    let store = MemoryStore::new();
    let seed = [7u8; 32];
    let relay = relay_process(&store, seed, true).await;
    let mut driver = relay.client().await;
    let driver_login = driver.login(ClientRole::Driver, DRIVER).await;
    let (info, cc, _) = driver.create_session().await;
    let sid = info.session_id;
    let mut erin = relay.client().await;
    let erin_login = erin.login(ClientRole::Engineer, ENGINEER).await;
    let erin_info = erin.join(&code_of(&cc)).await.expect("joined");
    // Eddie joins and is removed by the driver: his ticket must stay dead after the restart.
    let second = code_of(&driver.connect_code().await);
    let mut eddie = relay.client().await;
    let eddie_login = eddie.login(ClientRole::Engineer, ENGINEER2).await;
    let eddie_info = eddie.join(&second).await.expect("joined");
    let list = engineer_list_of(&mut driver, 2).await;
    let eddie_id = list
        .engineers
        .iter()
        .find(|e| e.username == ENGINEER2)
        .expect("eddie listed")
        .connection_id;
    driver
        .send(&Message::EngineerKick(EngineerKick {
            connection_id: eddie_id,
        }))
        .await;
    assert_eq!(
        session_close(&mut eddie).await.reason,
        SessionCloseReason::RemovedByDriver
    );

    // Restart 1. Every client is told; nobody acts.
    relay.shutdown().await;
    let shutdown_notice = driver
        .recv_until(|m| match m {
            Message::Disconnect(d) => Some(d.reason),
            _ => None,
        })
        .await;
    assert_eq!(shutdown_notice, DisconnectReason::ServerShutdown);
    assert_eq!(
        session_close(&mut erin).await.reason,
        SessionCloseReason::RelayShutdown
    );
    let relay = relay_process(&store, seed, false).await;

    // The engineer app is back first: it re-authenticates and resumes with its in-memory ticket
    // while the driver is still away.
    let (mut erin, erin_ticket) = engineer_resumed(
        &relay,
        &erin_login.refresh_token,
        sid,
        engineer_ticket(&erin_info),
    )
    .await;
    // The kicked engineer's ticket is dead, restart or not.
    let mut eddie = engineer_resume(
        &relay,
        &eddie_login.refresh_token,
        sid,
        engineer_ticket(&eddie_info),
    )
    .await;
    assert_eq!(eddie.error().await.code, ErrorCode::NotAuthorized);

    // The driver re-authenticates silently and resumes the same session; the engineer is there.
    let mut driver = driver_resumed(&relay, &driver_login.access_token, &info).await;
    let cc = driver.connect_code().await;
    assert_eq!(cc.engineers_connected, 1, "erin holds her slot");
    assert!(cc.code.is_some(), "a code for the free slot");
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers.len(), 1);
    assert_eq!(list.engineers[0].username, ENGINEER);
    assert_eq!(list.engineers[0].link, EngineerLink::Healthy);
    let request = driver.stream_of(MsgType::SnapshotRequest).await;
    assert!(matches!(
        Message::from_frame(&request),
        Ok(Message::SnapshotRequest(SnapshotRequest {
            reason: SnapshotReason::Reconnect
        }))
    ));
    driver
        .send_stream(MsgType::TelemetryDelta, FramePriority::P0, b"after-restart")
        .await;
    assert_eq!(
        erin.stream_of(MsgType::TelemetryDelta).await.payload(),
        b"after-restart"
    );

    // Restart 2, the other order: the driver is back first and sees the engineer reconnecting.
    relay.shutdown().await;
    let relay = relay_process(&store, seed, false).await;
    let mut driver = driver_resumed(&relay, &driver_login.access_token, &info).await;
    let cc = driver.connect_code().await;
    assert_eq!(cc.engineers_connected, 1);
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers[0].link, EngineerLink::Reconnecting);
    let (_erin, _) = engineer_resumed(&relay, &erin_login.refresh_token, sid, erin_ticket).await;
    let list = driver.engineer_list().await;
    assert_eq!(list.engineers[0].link, EngineerLink::Healthy);
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Backpressure
// -------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_engineer_does_not_grow_relay_memory() {
    const MAX_FRAMES: u64 = 60_000;
    let relay = TestRelay::start(|c| {
        c.limits.connection_rate.packets_per_sec = 1_000_000;
        c.limits.connection_rate.bytes_per_sec = 4 * 1024 * 1024 * 1024;
        c.limits.session_rate = c.limits.connection_rate;
    })
    .await;
    let (mut driver, info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    let sid = info.session_id;
    // The engineer stops reading. The driver streams far more than the socket buffers hold.
    let payload = vec![0xA5u8; 4096];
    let priorities = [FramePriority::P0, FramePriority::P1, FramePriority::P2];
    let mut last_seq = 0;
    let mut dropped = 0;
    for i in 0..MAX_FRAMES {
        let p = priorities[(i % 3) as usize];
        last_seq = driver
            .send_stream(MsgType::TelemetryDelta, p, &payload)
            .await;
        if i % 500 == 499 {
            let debug = relay.handle.session_debug(sid).expect("session");
            let outbox = debug.engineers[0].outbox.expect("connected");
            assert!(
                outbox.live_pending <= LIVE_SLOTS,
                "latest-wins bounds pending live frames"
            );
            assert!(
                outbox.reliable_frames <= 8,
                "no telemetry in the reliable queue"
            );
            dropped = outbox.dropped_total;
            if dropped > 1_000 {
                break;
            }
        }
    }
    assert!(
        dropped > 1_000,
        "latest-wins dropped superseded frames ({dropped})"
    );
    // Pending memory stays bounded whatever the input rate.
    let debug = relay.handle.session_debug(sid).expect("session");
    assert!(debug.engineers[0].outbox.expect("connected").live_pending <= LIVE_SLOTS);

    // Once the engineer reads again it receives the newest frames, with the drops reported.
    let mut reported = 0u64;
    let mut received = 0u64;
    let mut highest = 0u64;
    loop {
        let frame = match erin.try_frame(Duration::from_secs(3)).await {
            Ok(Some(frame)) => frame,
            other => panic!(
                "stream stalled ({:?}): received {received}, highest {highest}, last sent {last_seq}, debug {:?}, metrics rx {} fwd {}",
                other.map(|f| f.map(|f| f.header().sequence)),
                relay.handle.session_debug(sid),
                relay.handle.metrics().telemetry_packets_received.get(),
                relay.handle.metrics().telemetry_packets_forwarded.get(),
            ),
        };
        if frame.msg_type() != Some(MsgType::TelemetryDelta) {
            continue;
        }
        received += 1;
        highest = highest.max(frame.header().sequence);
        reported += u64::from(frame.relay_trailer().expect("trailer").relay_dropped);
        if frame.header().sequence == last_seq {
            break;
        }
    }
    assert!(
        reported > 0,
        "relay_dropped tells the engineer about intentional drops"
    );
    assert!(relay.handle.metrics().live_frames_dropped.get() > 0);
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Malformed input, limits, tokens, heartbeat
// -------------------------------------------------------------------------------------------------

fn hello_frame() -> protocol::Frame {
    Message::Hello(Hello {
        app_version: "test/1".into(),
        protocol: ProtocolVersion::CURRENT,
        role: ClientRole::Driver,
        schema_versions: vec![1],
        client_id: "malformed".into(),
    })
    .to_frame(&FrameMeta::control(0, 0))
    .expect("frame")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_crc_oversized_and_garbage_input_close_the_connection() {
    let relay = TestRelay::start(|_| {}).await;

    let mut bad_crc = relay.client().await;
    let mut bytes = hello_frame().as_bytes().to_vec();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    bad_crc.send_raw(&bytes).await;
    let e = bad_crc.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::CrcMismatch, true));
    assert!(bad_crc.closed().await);

    let mut oversized = relay.client().await;
    let header = FrameHeader {
        minor: 0,
        msg_type: MsgType::TelemetryDelta.id(),
        flags: protocol::frame::Flags::empty(),
        priority: FramePriority::P0,
        payload_len: u32::try_from(MAX_PAYLOAD_BYTES).expect("fits") + 1,
        session_id: 0,
        sequence: 1,
        source_ts_ns: 0,
    };
    oversized.send_raw(&header.to_bytes()).await;
    let e = oversized.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::TooLarge, true));
    assert!(oversized.closed().await);

    let mut garbage = relay.client().await;
    garbage
        .send_raw(b"GET / HTTP/1.1\r\nHost: relay\r\n\r\n")
        .await;
    assert_eq!(garbage.error().await.code, ErrorCode::BadMagic);
    assert!(garbage.closed().await);

    // An unknown message type is skipped (forward compatibility); the connection stays usable.
    let mut client = relay.client().await;
    client.login(ClientRole::Driver, DRIVER).await;
    client
        .send_raw(&raw_frame(0x7E, 0xFF, 0, 0, b"from-the-future"))
        .await;
    client
        .send(&Message::Ping(protocol::messages::Ping { id: 9, t0: 1 }))
        .await;
    let pong = client.frame().await.expect("open");
    assert!(matches!(
        Message::from_frame(&pong).expect("decodes"),
        Message::Pong(p) if p.id == 9
    ));
    assert!(relay.handle.metrics().packet_validation_failures.get() >= 4);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_configured_frame_size_limit_applies() {
    let relay = TestRelay::start(|c| c.limits.max_payload_bytes = 16 * 1024).await;
    let (mut driver, _, _) = relay.driver_session(DRIVER).await;
    driver
        .send_stream(
            MsgType::TelemetrySnapshot,
            FramePriority::P0,
            &vec![0u8; 20 * 1024],
        )
        .await;
    let e = driver.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::TooLarge, true));
    assert!(driver.closed().await);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_payload_limit_is_small_until_authenticated() {
    let relay = TestRelay::start(|_| {}).await;
    // Before AUTH_OK a header announcing more than 16 KiB is refused at once (nothing buffered).
    let mut early = relay.client().await;
    early.hello(ClientRole::Driver).await;
    let header = FrameHeader {
        minor: protocol::PROTOCOL_MINOR,
        msg_type: MsgType::Auth.id(),
        flags: protocol::frame::Flags::empty(),
        priority: FramePriority::Control,
        payload_len: 20 * 1024,
        session_id: 0,
        sequence: 0,
        source_ts_ns: 0,
    };
    early.send_raw(&header.to_bytes()).await;
    let e = early.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::TooLarge, true));
    assert!(early.closed().await);

    // An over-long username is refused like any unknown one (and never kept by the limiter).
    let mut long_name = relay.client().await;
    long_name.hello(ClientRole::Driver).await;
    let e = long_name
        .auth(Auth::Password {
            username: "a".repeat(10 * 1024),
            password: Secret::new(PASSWORD),
        })
        .await
        .expect_err("refused");
    assert_eq!(e.code, ErrorCode::AuthFailed);

    // After AUTH_OK the configured limit applies again.
    let (mut driver, _, _) = relay.driver_session(DRIVER).await;
    driver
        .send_stream(
            MsgType::TelemetrySnapshot,
            FramePriority::P0,
            &vec![0u8; 20 * 1024],
        )
        .await;
    driver
        .send(&Message::Ping(protocol::messages::Ping { id: 5, t0: 1 }))
        .await;
    // `recv` skips PONG, so frames are read directly.
    let reply = loop {
        let frame = driver.frame().await.expect("connection open");
        match Message::from_frame(&frame) {
            Ok(Message::Error(e)) => break Err(e.code),
            Ok(Message::Pong(p)) => break Ok(p.id),
            _ => {}
        }
    };
    assert_eq!(
        reply,
        Ok(5),
        "a 20 KiB frame is accepted once authenticated"
    );
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_resumes_and_session_creates_are_limited() {
    let relay = TestRelay::start(|c| {
        c.limits.malformed_budget = 4;
        c.limits.resume_lookups_per_account_per_minute = 2;
        c.limits.session_creates_per_account_per_minute = 2;
    })
    .await;
    // Probing session ids: each lookup of a session not in memory reads the store, so it is
    // rate limited, and every refusal is charged like an invalid frame.
    let mut erin = relay.client().await;
    erin.login(ClientRole::Engineer, ENGINEER).await;
    let mut codes = Vec::new();
    let mut fatal = false;
    for id in 1..=20u64 {
        erin.send(&Message::SessionResume(SessionResume::Engineer {
            session_id: 1_000 + id,
            ticket: Secret::new("not-a-ticket"),
        }))
        .await;
        let e = erin.error().await;
        if e.fatal {
            fatal = true;
            break;
        }
        codes.push(e.code);
    }
    assert!(fatal, "the violation budget ends the probing connection");
    assert!(erin.closed().await);
    assert_eq!(
        codes.first().copied(),
        Some(ErrorCode::SessionNotFound),
        "{codes:?}"
    );
    assert!(codes.contains(&ErrorCode::RateLimited), "{codes:?}");

    // SESSION_CREATE: 2 per account per minute.
    let (_first, _, _) = relay.driver_session(DRIVER).await;
    let (_second, _, _) = relay.driver_session(DRIVER).await;
    let mut third = relay.client().await;
    third.login(ClientRole::Driver, DRIVER).await;
    third
        .send(&Message::SessionCreate(common::session_create()))
        .await;
    assert_eq!(third.error().await.code, ErrorCode::RateLimited);
    // Another driver account is not affected.
    relay.driver_session(DRIVER_B).await;
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_invalid_frames_exhaust_the_budget() {
    let relay = TestRelay::start(|c| c.limits.malformed_budget = 5).await;
    let mut client = relay.client().await;
    client.login(ClientRole::Driver, DRIVER).await;
    // PING with a non-zero sequence violates the header rules (control frames carry 0).
    let invalid = raw_frame(MsgType::Ping.id(), 0xFF, 0, 7, &[0, 0]);
    let mut non_fatal = 0;
    let mut fatal = false;
    for _ in 0..10 {
        client.send_raw(&invalid).await;
    }
    while let Some(frame) = client.frame().await {
        if let Ok(Message::Error(e)) = Message::from_frame(&frame) {
            if e.fatal {
                assert_eq!(e.code, ErrorCode::Malformed);
                fatal = true;
            } else {
                assert_eq!(e.code, ErrorCode::Malformed);
                non_fatal += 1;
            }
        }
    }
    assert!(fatal, "the connection is closed once the budget is spent");
    assert_eq!(non_fatal, 5);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fatal_error_survives_a_client_that_keeps_sending() {
    // The relay ends the connection while the client is still sending: the late bytes must not
    // make the socket reset (which can discard the unread fatal ERROR on the client).
    let relay = TestRelay::start(|c| c.limits.malformed_budget = 5).await;
    let mut client = relay.client().await;
    client.login(ClientRole::Driver, DRIVER).await;
    let invalid = raw_frame(MsgType::Ping.id(), 0xFF, 0, 7, &[0, 0]);
    for _ in 0..6 {
        client.send_raw(&invalid).await;
    }
    // The relay has spent the budget and closed its side by now; the client has not read yet.
    tokio::time::sleep(Duration::from_millis(50)).await;
    for _ in 0..4 {
        client.send_raw(&invalid).await;
    }
    let mut non_fatal = 0;
    let mut fatal = false;
    while let Some(frame) = client.frame().await {
        if let Ok(Message::Error(e)) = Message::from_frame(&frame) {
            assert_eq!(e.code, ErrorCode::Malformed);
            if e.fatal {
                fatal = true;
            } else {
                non_fatal += 1;
            }
        }
    }
    assert!(fatal, "the fatal ERROR is received");
    assert_eq!(non_fatal, 5);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_must_come_first_and_versions_must_match() {
    let relay = TestRelay::start(|_| {}).await;
    // A session message before HELLO: fatal.
    let mut early = relay.client().await;
    early
        .send(&Message::SessionCreate(common::session_create()))
        .await;
    let e = early.error().await;
    assert!(e.fatal);
    assert!(early.closed().await);
    // A different major in the HELLO payload: ERROR Incompatible, closed.
    let mut old = relay.client().await;
    old.send(&Message::Hello(Hello {
        app_version: "ancient".into(),
        protocol: ProtocolVersion::new(2, 0),
        role: ClientRole::Engineer,
        schema_versions: vec![1],
        client_id: "x".into(),
    }))
    .await;
    let e = old.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::Incompatible, true));
    assert!(old.closed().await);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_and_expired_tokens_are_rejected() {
    let now = Arc::new(AtomicU64::new(unix_now_ns()));
    let clock_now = Arc::clone(&now);
    let clock = WallClock::from_fn(move || clock_now.load(Ordering::SeqCst));
    let advance = |d: Duration| {
        now.fetch_add(u64::try_from(d.as_nanos()).expect("fits"), Ordering::SeqCst);
    };
    let relay = TestRelay::start_with(
        test_config(),
        Store::Memory(MemoryStore::new()),
        None,
        Some(clock),
        true,
    )
    .await;
    let password = Auth::Password {
        username: ENGINEER.into(),
        password: Secret::new(PASSWORD),
    };

    let mut c1 = relay.client().await;
    c1.hello(ClientRole::Engineer).await;
    let first = c1.auth(password.clone()).await.expect("login");
    // Using a refresh token rotates it.
    let mut c2 = relay.client().await;
    c2.hello(ClientRole::Engineer).await;
    let second = c2
        .auth(Auth::RefreshToken {
            token: first.refresh_token.clone(),
        })
        .await
        .expect("refresh");
    assert_ne!(second.refresh_token, first.refresh_token);
    // Replaying the rotated token after the reuse grace is refused and revokes the login.
    advance(Duration::from_secs(120));
    let mut c3 = relay.client().await;
    c3.hello(ClientRole::Engineer).await;
    let replay = c3
        .auth(Auth::RefreshToken {
            token: first.refresh_token.clone(),
        })
        .await
        .expect_err("replayed");
    assert_eq!((replay.code, replay.fatal), (ErrorCode::AuthFailed, false));
    let revoked = c3
        .auth(Auth::RefreshToken {
            token: second.refresh_token.clone(),
        })
        .await
        .expect_err("family revoked");
    assert_eq!(revoked.code, ErrorCode::AuthFailed);

    // An expired access token: TokenExpired (not fatal), then the refresh token on the same
    // connection works, as the transport client does it.
    let mut c4 = relay.client().await;
    c4.hello(ClientRole::Engineer).await;
    let fresh = c4.auth(password.clone()).await.expect("login");
    advance(Duration::from_secs(16 * 60));
    let mut c5 = relay.client().await;
    c5.hello(ClientRole::Engineer).await;
    let expired = c5
        .auth(Auth::AccessToken {
            token: fresh.access_token.clone(),
        })
        .await
        .expect_err("expired");
    assert_eq!(
        (expired.code, expired.fatal),
        (ErrorCode::TokenExpired, false)
    );
    let refreshed = c5
        .auth(Auth::RefreshToken {
            token: fresh.refresh_token.clone(),
        })
        .await
        .expect("refresh after an expired access token");
    assert_eq!(refreshed.username, ENGINEER);

    // Forged tokens and too many attempts on one connection.
    let mut c6 = relay.client().await;
    c6.hello(ClientRole::Engineer).await;
    for auth in [
        Auth::AccessToken {
            token: Secret::new("eyJhbGciOiJub25lIn0.e30."),
        },
        Auth::RefreshToken {
            token: Secret::new("not-a-token"),
        },
        Auth::Password {
            username: ENGINEER.into(),
            password: Secret::new("wrong password"),
        },
    ] {
        assert_eq!(
            c6.auth(auth).await.expect_err("refused").code,
            ErrorCode::AuthFailed
        );
    }
    let e = c6.auth(password).await.expect_err("attempt limit");
    assert_eq!((e.code, e.fatal), (ErrorCode::RateLimited, true));
    assert!(c6.closed().await);
    assert!(relay.handle.metrics().auth_failures.get() >= 6);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_access_token_login_keeps_the_held_refresh_token_live() {
    let now = Arc::new(AtomicU64::new(unix_now_ns()));
    let clock_now = Arc::clone(&now);
    let clock = WallClock::from_fn(move || clock_now.load(Ordering::SeqCst));
    let advance = |d: Duration| {
        now.fetch_add(u64::try_from(d.as_nanos()).expect("fits"), Ordering::SeqCst);
    };
    let mut config = test_config();
    config.timeouts.revocation_check_interval = Duration::from_millis(100);
    let grace = config.auth.refresh_reuse_grace;
    let relay = TestRelay::start_with(
        config,
        Store::Memory(MemoryStore::new()),
        None,
        Some(clock),
        true,
    )
    .await;

    // A legit app logs in once, then connects with its access token (twice, overlapping, as on a
    // quick stop/start). Those AUTH_OKs carry no refresh token ("none issued": empty, expiry 0),
    // so whichever the app applies last, it keeps the refresh token of its login (review F1/F2).
    let mut login_conn = relay.client().await;
    login_conn.hello(ClientRole::Engineer).await;
    let login = login_conn
        .auth(Auth::Password {
            username: ENGINEER.into(),
            password: Secret::new(PASSWORD),
        })
        .await
        .expect("login");
    let mut app = relay.client().await;
    app.hello(ClientRole::Engineer).await;
    let mut overlapping = relay.client().await;
    overlapping.hello(ClientRole::Engineer).await;
    for conn in [&mut app, &mut overlapping] {
        let via_access = conn
            .auth(Auth::AccessToken {
                token: login.access_token.clone(),
            })
            .await
            .expect("access token");
        assert!(via_access.refresh_token.expose().is_empty());
        assert_eq!(via_access.refresh_expires_at_ns, 0);
        assert!(!via_access.access_token.expose().is_empty());
    }

    // Far beyond the reuse grace (the app restarts after its access token expired): the refresh
    // token of the login is still the live one and rotates normally; nothing is revoked.
    advance(grace * 10);
    let mut restarted = relay.client().await;
    restarted.hello(ClientRole::Engineer).await;
    let rotated = restarted
        .auth(Auth::RefreshToken {
            token: login.refresh_token.clone(),
        })
        .await
        .expect("the held refresh token is live");
    assert_eq!(rotated.username, ENGINEER);
    assert_ne!(rotated.refresh_token, login.refresh_token);
    assert!(
        app.quiet(Duration::from_millis(400)).await,
        "no revocation: the app's connection stays open"
    );

    // After the grace the rotated token is reuse: the login is revoked and its open connections
    // are closed.
    advance(grace + Duration::from_secs(1));
    let mut thief = relay.client().await;
    thief.hello(ClientRole::Engineer).await;
    let reuse = thief
        .auth(Auth::RefreshToken {
            token: login.refresh_token.clone(),
        })
        .await
        .expect_err("reuse after the grace");
    assert_eq!(reuse.code, ErrorCode::AuthFailed);
    for open in [&mut app, &mut overlapping, &mut restarted] {
        let e = open.error().await;
        assert_eq!((e.code, e.fatal), (ErrorCode::AuthFailed, true));
        assert!(open.closed().await);
    }
    assert!(relay.handle.metrics().revoked_disconnects.get() >= 3);
    let mut again = relay.client().await;
    again.hello(ClientRole::Engineer).await;
    let refused = again
        .auth(Auth::RefreshToken {
            token: rotated.refresh_token.clone(),
        })
        .await
        .expect_err("the whole family is revoked");
    assert_eq!(refused.code, ErrorCode::AuthFailed);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_logins_lose_their_open_connections() {
    let relay = TestRelay::start(|c| {
        c.timeouts.revocation_check_interval = Duration::from_millis(100);
    })
    .await;
    let (mut driver, _info, cc) = relay.driver_session(DRIVER).await;
    let mut erin = relay.client().await;
    let erin_login = erin.login(ClientRole::Engineer, ENGINEER).await;
    erin.join(&code_of(&cc)).await.expect("joined");

    // Logout everywhere (`lmu-relay revoke-tokens`): the open connection is closed as well.
    let revoked = relay
        .handle
        .auth()
        .revoke_tokens(ENGINEER)
        .await
        .expect("revoked");
    assert!(revoked >= 1);
    let e = erin.error().await;
    assert_eq!((e.code, e.fatal), (ErrorCode::AuthFailed, true));
    assert!(erin.closed().await);
    assert!(relay.handle.metrics().revoked_disconnects.get() >= 1);

    // The revoked login cannot come back.
    let mut again = relay.client().await;
    again.hello(ClientRole::Engineer).await;
    let refused = again
        .auth(Auth::RefreshToken {
            token: erin_login.refresh_token,
        })
        .await
        .expect_err("revoked login");
    assert_eq!(refused.code, ErrorCode::AuthFailed);

    // Other logins are untouched: the driver sees the engineer's connection drop and still gets
    // its PONGs.
    let list = driver
        .recv_until(|m| match m {
            Message::EngineerList(l)
                if l.engineers
                    .first()
                    .is_some_and(|e| e.link == EngineerLink::Reconnecting) =>
            {
                Some(l)
            }
            _ => None,
        })
        .await;
    assert_eq!(list.engineers.len(), 1);
    driver
        .send(&Message::Ping(protocol::messages::Ping { id: 42, t0: 1 }))
        .await;
    loop {
        let frame = driver.frame().await.expect("driver connection open");
        if let Ok(Message::Pong(pong)) = Message::from_frame(&frame) {
            assert_eq!(pong.id, 42);
            break;
        }
    }
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registration_is_off_unless_enabled() {
    let request = Message::AccountRegister(AccountRegister {
        username: "newbie".into(),
        password: Secret::new(PASSWORD),
        display_name: "New Engineer".into(),
    });
    let closed = TestRelay::start(|_| {}).await;
    let mut c = closed.client().await;
    c.hello(ClientRole::Engineer).await;
    c.send(&request).await;
    assert_eq!(c.error().await.code, ErrorCode::NotAuthorized);
    closed.shutdown().await;

    let open = TestRelay::start(|c| c.open_registration = true).await;
    let mut c = open.client().await;
    c.hello(ClientRole::Engineer).await;
    c.send(&request).await;
    match c.recv().await {
        Message::AuthOk(ok) => {
            assert_eq!(ok.role, ClientRole::Engineer);
            assert_eq!(ok.username, "newbie");
        }
        other => panic!("AUTH_OK expected, got {other:?}"),
    }
    let mut again = open.client().await;
    again.hello(ClientRole::Engineer).await;
    again.send(&request).await;
    let e = again.error().await;
    assert_eq!(e.code, ErrorCode::AuthFailed);
    assert_eq!(e.message, "username already taken");
    open.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_is_answered_and_silent_connections_are_closed() {
    let relay = TestRelay::start(|c| {
        c.timeouts.heartbeat_interval = Duration::from_millis(200);
        c.timeouts.heartbeat_timeout = Duration::from_millis(800);
    })
    .await;
    let mut client = relay.client().await;
    let hello = client.hello(ClientRole::Driver).await;
    assert_eq!(hello.limits.heartbeat_timeout_ms, 800);
    client
        .auth(Auth::Password {
            username: DRIVER.into(),
            password: Secret::new(PASSWORD),
        })
        .await
        .expect("login");
    let before = unix_now_ns();
    client
        .send(&Message::Ping(protocol::messages::Ping { id: 7, t0: 123 }))
        .await;
    let pong = client.frame().await.expect("open");
    let after = unix_now_ns();
    match Message::from_frame(&pong).expect("decodes") {
        Message::Pong(p) => {
            assert_eq!((p.id, p.t0), (7, 123));
            assert!(before <= p.t1 && p.t1 <= p.t2 && p.t2 <= after);
        }
        other => panic!("PONG expected, got {other:?}"),
    }
    // Silence: DISCONNECT{IdleTimeout} and close after the heartbeat timeout.
    let silent_since = Instant::now();
    let reason = client
        .recv_until(|m| match m {
            Message::Disconnect(d) => Some(d.reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, DisconnectReason::IdleTimeout);
    assert!(client.closed().await);
    assert!(silent_since.elapsed() >= Duration::from_millis(600));
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Admin listener
// -------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_endpoints_require_the_bearer_token() {
    let token = "admin-token-for-tests-0123456789abcdef";
    let relay = TestRelay::start(|c| {
        c.admin_bind = Some("127.0.0.1:0".parse().expect("addr"));
        c.admin_token = Some(Secret::new(token));
    })
    .await;
    let admin = relay.handle.admin_addr().expect("admin listener");
    assert_eq!(http_get(admin, "/health", None).await.0, 401);
    assert_eq!(
        http_get(admin, "/metrics", Some("wrong-token")).await.0,
        401
    );
    let (_driver, _, _) = relay.driver_session(DRIVER).await;
    let (status, body) = http_get(admin, "/health", Some(token)).await;
    assert_eq!(status, 200);
    let health: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(health["status"], "ok");
    assert_eq!(health["db"], "memory");
    assert_eq!(health["active_sessions"], 1);
    assert_eq!(health["active_connections"], 1);
    let (status, metrics) = http_get(admin, "/metrics", Some(token)).await;
    assert_eq!(status, 200);
    for needle in [
        "lmu_relay_active_sessions 1",
        "lmu_relay_active_connections 1",
        "lmu_relay_telemetry_packets_received_total",
        "lmu_relay_telemetry_packets_forwarded_total",
        "lmu_relay_bytes_received_total",
        "lmu_relay_bytes_forwarded_total",
        "lmu_relay_auth_failures_total",
        "lmu_relay_packet_validation_failures_total",
        "lmu_relay_reconnects_total",
        "lmu_relay_relay_forward_latency_seconds_bucket",
    ] {
        assert!(metrics.contains(needle), "missing {needle}");
    }
    relay.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Benchmark
// -------------------------------------------------------------------------------------------------

/// Percentile of sorted samples.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "benchmark: cargo test -p lmu-relay --release --test relay -- --ignored --nocapture forwarding_benchmark"]
async fn forwarding_benchmark() {
    const FLOOD_FRAMES: u64 = 100_000;
    const PACED_SECONDS: u64 = 5;
    const PACED_HZ: u64 = 100;
    let relay = TestRelay::start(|c| {
        c.limits.connection_rate.packets_per_sec = 10_000_000;
        c.limits.connection_rate.bytes_per_sec = 16 * 1024 * 1024 * 1024;
        c.limits.session_rate = c.limits.connection_rate;
    })
    .await;
    let (mut driver, _info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    let base = Instant::now();

    // Each payload carries its send time (ns since `base`, bytes 0..8), a last-frame marker (byte
    // 8) and its phase (byte 9), so end-to-end latency is measured per phase.
    const LAST_FLAG: usize = 8;
    const PHASE: usize = 9;
    const PHASE_FLOOD: u8 = 0;
    const PHASE_PACED: u8 = 1;
    // The paced phase starts once the engineer has the last flood frame (the newest frame of its
    // key, so latest-wins always delivers it): it then measures steady state, not the drain of
    // the socket buffers the flood filled.
    let (flood_seen_tx, flood_seen_rx) = tokio::sync::oneshot::channel::<()>();
    let receiver = tokio::spawn(async move {
        let mut flood_seen = Some(flood_seen_tx);
        let mut paced_latencies = Vec::new();
        let mut paced_dropped = 0u64;
        let mut received = 0u64;
        let mut dropped = 0u64;
        while let Some(frame) = erin.frame().await {
            if frame.msg_type() != Some(MsgType::TelemetryDelta) {
                continue;
            }
            let payload = frame.payload();
            let sent = u64::from_le_bytes(payload[..8].try_into().expect("8 bytes"));
            let now = u64::try_from(base.elapsed().as_nanos()).expect("fits");
            let relay_dropped = u64::from(frame.relay_trailer().expect("trailer").relay_dropped);
            received += 1;
            dropped += relay_dropped;
            if payload[PHASE] == PHASE_PACED {
                paced_latencies.push(now.saturating_sub(sent));
                paced_dropped += relay_dropped;
            } else if payload[LAST_FLAG] == 1
                && let Some(tx) = flood_seen.take()
            {
                let _ = tx.send(());
                continue;
            }
            if payload[LAST_FLAG] == 1 {
                break;
            }
        }
        (paced_latencies, paced_dropped, received, dropped)
    });
    let mut payload = vec![0u8; 256];
    // Phase 1: flood (maximum forwarding rate; latest-wins drops what the engineer cannot take).
    payload[PHASE] = PHASE_FLOOD;
    let flood_start = Instant::now();
    for i in 0..FLOOD_FRAMES {
        let sent = u64::try_from(base.elapsed().as_nanos()).expect("fits");
        payload[..8].copy_from_slice(&sent.to_le_bytes());
        payload[LAST_FLAG] = u8::from(i + 1 == FLOOD_FRAMES);
        driver
            .send_stream(MsgType::TelemetryDelta, FramePriority::P0, &payload)
            .await;
    }
    let flood_elapsed = flood_start.elapsed();
    let flood_end = Instant::now();
    tokio::time::timeout(WAIT, flood_seen_rx)
        .await
        .expect("the engineer drains the flood")
        .expect("receiver alive");
    let drain = flood_end.elapsed();
    let received_after_flood = relay.handle.metrics().telemetry_packets_received.get();
    let forwarded_after_flood = relay.handle.metrics().telemetry_packets_forwarded.get();
    let flood_forward_p50 = relay
        .handle
        .metrics()
        .relay_forward_latency
        .quantile_upper_bound_us(0.5);
    let flood_forward_p99 = relay
        .handle
        .metrics()
        .relay_forward_latency
        .quantile_upper_bound_us(0.99);

    // Phase 2: a realistic driver (100 Hz × 3 priority classes) for end-to-end latency. Paced by
    // spinning on the clock: tokio timers are as coarse as the OS timer (≈15.6 ms on Windows) and
    // would send two ticks back to back, so latest-wins would (correctly) drop the older frame of
    // each key and the phase would not measure a steady 100 Hz stream.
    payload[PHASE] = PHASE_PACED;
    let tick_period = Duration::from_millis(1_000 / PACED_HZ);
    let total = PACED_SECONDS * PACED_HZ;
    let paced_start = Instant::now();
    for tick in 0..total {
        let due = paced_start + tick_period * u32::try_from(tick).expect("fits");
        while Instant::now() < due {
            tokio::task::yield_now().await;
        }
        for p in [FramePriority::P0, FramePriority::P1, FramePriority::P2] {
            let sent = u64::try_from(base.elapsed().as_nanos()).expect("fits");
            payload[..8].copy_from_slice(&sent.to_le_bytes());
            payload[LAST_FLAG] = u8::from(tick + 1 == total && p == FramePriority::P2);
            driver
                .send_stream(MsgType::TelemetryDelta, p, &payload)
                .await;
        }
    }
    let (mut paced_sorted, paced_dropped, received, dropped) = tokio::time::timeout(WAIT, receiver)
        .await
        .expect("receiver done")
        .expect("task");
    paced_sorted.sort_unstable();
    let paced_expected = total * 3;
    let metrics = relay.handle.metrics();
    eprintln!("=== lmu-relay forwarding benchmark (loopback, TLS 1.3, 1 driver -> 1 engineer) ===");
    eprintln!(
        "flood: {FLOOD_FRAMES} frames x 256 B sent in {:.3} s = {:.0} frames/s accepted by the relay",
        flood_elapsed.as_secs_f64(),
        FLOOD_FRAMES as f64 / flood_elapsed.as_secs_f64()
    );
    eprintln!(
        "flood: relay received {received_after_flood}, wrote {forwarded_after_flood} (latest-wins drops the rest); relay rx->socket p50 <= {flood_forward_p50:?} us, p99 <= {flood_forward_p99:?} us; engineer had the last flood frame {:.1} ms after the flood ended",
        drain.as_secs_f64() * 1_000.0
    );
    eprintln!(
        "paced {PACED_HZ} Hz x 3 classes for {PACED_SECONDS} s: {} of {paced_expected} frames received (relay_dropped {paced_dropped}); end-to-end driver->engineer latency p50 {} us, p99 {} us, max {} us",
        paced_sorted.len(),
        percentile(&paced_sorted, 0.50) / 1_000,
        percentile(&paced_sorted, 0.99) / 1_000,
        paced_sorted.last().copied().unwrap_or(0) / 1_000
    );
    eprintln!(
        "engineer received {received} delta frames in total, relay_dropped reported {dropped}; relay forward latency p50 <= {:?} us, p99 <= {:?} us over {} frames",
        metrics.relay_forward_latency.quantile_upper_bound_us(0.5),
        metrics.relay_forward_latency.quantile_upper_bound_us(0.99),
        metrics.relay_forward_latency.count()
    );
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn driver_stale_sequence_receives_malformed_error() {
    let relay = TestRelay::start(|_| {}).await;
    let (mut driver, _info, _cc) = relay.driver_session(DRIVER).await;
    let frame = driver.stream_frame(MsgType::TelemetryDelta, FramePriority::P0, b"delta1");
    driver.send_frame(&frame).await;
    // Send same frame again (non-increasing sequence)
    driver.send_frame(&frame).await;
    let err = driver.error().await;
    assert_eq!(err.code, ErrorCode::Malformed);
    assert!(err.message.contains("non-increasing sequence"));
    assert!(!err.fatal);
    assert_eq!(relay.handle.metrics().stale_sequence_frames.get(), 1);
    relay.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engineer_snapshot_request_when_driver_away_gets_driver_away_error() {
    let relay = TestRelay::start(|_| {}).await;
    let (driver, _info, cc) = relay.driver_session(DRIVER).await;
    let (mut erin, _) = relay.engineer_joined(ENGINEER, &code_of(&cc)).await;
    // Drop driver connection so driver is Away
    drop(driver);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Drain DriverPresence notification
    let _ = erin.frame().await;

    // Send snapshot request while driver is away
    erin.send_snapshot_request(SnapshotReason::Manual).await;
    let err = erin.error().await;
    assert_eq!(err.code, ErrorCode::SessionNotFound);
    assert_eq!(err.message, "driver away");
    assert!(!err.fatal);
    assert_eq!(relay.handle.metrics().snapshot_forward_failures.get(), 1);
    relay.shutdown().await;
}

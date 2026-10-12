//! Driver client against the in-process fake relay (in-memory TLS, Tokio's paused clock):
//! login once and silent re-authentication, session create / resume, heartbeat and clock sync,
//! DEGRADED / STALE / RECONNECTING on a stalled relay, reconnect with backoff and resume,
//! latest-wins dropping under a slow reader, the engineer-direction whitelist, clear re-login
//! states and the environment check, the reliable queue's streaming lifecycle, the relay's
//! payload limit, fatal relay verdicts and the re-send of a reliable message cut off by a dead
//! connection.

mod common;

use std::time::Duration;

use common::*;
use protocol::frame::{FramePriority, MsgType};
use protocol::messages::{
    ClientRole, Disconnect, DisconnectReason, EngineerKick, ErrorCode, ErrorMsg, Message,
    SchemaNegotiate, Secret, SessionJoin, SessionResume, SnapshotReason, SnapshotRequest,
};
use protocol::version::Environment;
use telemetry_model::channel::{ChannelId, Source};
use telemetry_model::scoring::{
    CapabilityMap, CapabilityStatus, ChannelCapability, VehicleCapabilities,
};
use tokio::time::Instant;
use transport::{
    ConnectionState, CredentialKind, CredentialStore, DegradedReason, DriverClient, DriverEvent,
    FailureReason, LoginError, QueueError, ReloginCause, SessionReadyKind, SnapshotTrigger, login,
};

/// Logs `user` in once (the only time a password is used) for a fresh app.
async fn logged_in_app(
    relay: &FakeRelay,
    config: &transport::ClientConfig,
    role: ClientRole,
    user: &str,
) -> (
    std::sync::Arc<transport::MemoryCredentialStore>,
    std::sync::Arc<transport::AuthManager>,
) {
    let (store, auth) = new_app(config);
    login(
        &relay.transport(),
        config,
        &auth,
        role,
        user,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login");
    (store, auth)
}

/// Expects `SessionReady` followed by `SnapshotNeeded` and returns the session id and kind.
async fn session_ready(
    events: &mut tokio::sync::mpsc::Receiver<DriverEvent>,
) -> (u64, SessionReadyKind) {
    let (session_id, kind) = match next_event(events).await {
        DriverEvent::SessionReady {
            session_id, kind, ..
        } => (session_id, kind),
        other => panic!("expected SessionReady, got {other:?}"),
    };
    match next_event(events).await {
        DriverEvent::SnapshotNeeded {
            trigger: SnapshotTrigger::SessionReady(k),
        } => assert_eq!(k, kind),
        other => panic!("expected SnapshotNeeded after SessionReady, got {other:?}"),
    }
    (session_id, kind)
}

/// Stream frames the relay received from the driver: (sequence, session id, msg type, tag).
fn driver_stream(relay: &FakeRelay) -> Vec<(u64, u64, MsgType, Option<u64>)> {
    relay
        .received()
        .into_iter()
        .filter(|r| r.username.as_deref() == Some(DRIVER) && r.message.msg_type().is_stream())
        .map(|r| {
            (
                r.header.sequence,
                r.header.session_id,
                r.message.msg_type(),
                message_tag(&r.message),
            )
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn login_once_create_session_stream_heartbeat_and_clock_sync() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let client_epoch = Instant::from_std(config.mono_epoch);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    // Only the refresh token is persisted by a login (never the password or access token).
    let ns = auth.namespace().to_owned();
    assert_eq!(store.keys(), vec![format!("{ns}#refresh_token")]);

    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    assert_eq!(status.borrow().state, ConnectionState::Disconnected);
    client.start(session_create()).await.expect("start");
    let (session_id, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    let connected = wait_state(&mut status, ConnectionState::Connected).await;
    assert_eq!(connected.session_id, Some(session_id));
    // The silent connection used the refresh token stored by the login, not a password.
    assert!(
        relay
            .received()
            .iter()
            .filter(|r| r.message.msg_type() == MsgType::Auth)
            .skip(1)
            .all(|r| matches!(
                r.message,
                Message::Auth(protocol::messages::Auth::RefreshToken { .. })
                    | Message::Auth(protocol::messages::Auth::AccessToken { .. })
            ))
    );
    // The driver's session resume token is persisted (a restart resumes the session).
    let stored = store
        .load(&ns, CredentialKind::DriverSession)
        .expect("store")
        .expect("driver session stored");
    assert!(stored.expose().starts_with(&format!("v1:{session_id}:")));

    // Connection code and slot usage for the driver UI.
    let mut code = client.connect_code();
    let current = code
        .wait_for(Option::is_some)
        .await
        .expect("code watch")
        .clone()
        .expect("code");
    assert_eq!((current.engineers_connected, current.engineers_max), (0, 2));
    let issued = current.code.expect("a code while slots are free");
    assert_eq!(transport::display_connect_code(&issued.code).len(), 9);

    // Stream: SCHEMA_NEGOTIATE (reliable), a snapshot and a delta (latest-wins).
    client
        .send_reliable(schema_negotiate())
        .await
        .expect("schema");
    client
        .send_live(Message::TelemetrySnapshot(tlv(1, 8)), FramePriority::P0)
        .expect("snapshot");
    client
        .send_live(Message::TelemetryDelta(tlv(2, 2)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(100)).await;
    let stream = driver_stream(&relay);
    assert_eq!(
        stream,
        vec![
            (1, session_id, MsgType::SchemaNegotiate, None),
            (2, session_id, MsgType::TelemetrySnapshot, Some(1)),
            (3, session_id, MsgType::TelemetryDelta, Some(2)),
        ],
        "reliable first, snapshot before delta, contiguous sequences on the bound session"
    );

    // Heartbeat: one PING per second; RTT and clock offset are measured.
    let pings_before = relay.received_from(DRIVER, MsgType::Ping).len();
    advance(Duration::from_secs(10)).await;
    let pings = relay.received_from(DRIVER, MsgType::Ping).len() - pings_before;
    assert!((9..=11).contains(&pings), "{pings} PINGs in 10 s");
    let metrics = client.metrics().borrow().clone();
    assert!(metrics.rtt.is_some() && metrics.rtt_smoothed.is_some());
    let clock = metrics.clock.expect("clock estimate");
    // The in-memory link has no delay, so the offset is exact: relay clock − client clock.
    assert_eq!(
        i128::from(clock.offset_ns),
        relay.expected_offset_ns(client_epoch)
    );
    assert_eq!(clock.uncertainty_ns, 0);
    assert_eq!(client.clock(), Some(clock));
    // Relay time of an instant, whatever epoch the caller measures from: a capture 5 s before the
    // transport's epoch maps exactly, without sharing `mono_epoch`.
    let offset = relay.expected_offset_ns(client_epoch);
    assert_eq!(
        client
            .relay_time_of(client_epoch.into_std())
            .map(i128::from),
        Some(offset)
    );
    let earlier = client_epoch - Duration::from_secs(5);
    assert_eq!(
        client.relay_time_of(earlier.into_std()).map(i128::from),
        Some(offset - 5_000_000_000)
    );
    assert!(metrics.bytes_out_per_s.is_some() && metrics.frames_out_per_s.is_some());
    assert!(metrics.bytes_in_total > 0 && metrics.frames_in_total > 0);
    assert_eq!(metrics.reconnects, 0);
    assert_eq!(metrics.dropped_obsolete, 0);
    assert_eq!(status.borrow().state, ConnectionState::Connected);
    assert_eq!(
        relay.header_violations(),
        0,
        "every client frame passes the relay's rules"
    );

    client.shutdown().await;
    assert!(!relay.received_from(DRIVER, MsgType::Disconnect).is_empty());
    assert_eq!(
        relay.sessions(),
        vec![session_id],
        "a shutdown keeps the session"
    );
}

#[tokio::test(start_paused = true)]
async fn app_restart_resumes_the_stored_session_silently_with_a_new_sequence_space() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    client
        .send_live(Message::TelemetryDelta(tlv(1, 1)), FramePriority::P0)
        .expect("delta");
    client
        .send_live(Message::TelemetryDelta(tlv(2, 1)), FramePriority::P1)
        .expect("delta");
    advance(Duration::from_millis(100)).await;
    client.shutdown().await;
    let refresh_before = store
        .load(&ns, CredentialKind::RefreshToken)
        .expect("store")
        .expect("refresh token");

    // Second app run: no login, a new auth manager over the same credential store.
    let auth = restart_app(&store, &config);
    let auths_before = relay.auth_attempts();
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (resumed_id, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::ResumedAfterRestart);
    assert_eq!(resumed_id, session_id);
    wait_state(&mut status, ConnectionState::Connected).await;
    // Silent re-authentication with the stored refresh token, which the relay rotated.
    let auths: Vec<_> = relay
        .received()
        .into_iter()
        .filter(|r| r.message.msg_type() == MsgType::Auth)
        .skip(auths_before)
        .collect();
    assert_eq!(auths.len(), 1);
    assert!(matches!(
        &auths[0].message,
        Message::Auth(protocol::messages::Auth::RefreshToken { token }) if *token == refresh_before
    ));
    let refresh_after = store
        .load(&ns, CredentialKind::RefreshToken)
        .expect("store")
        .expect("refresh token");
    assert_ne!(
        refresh_after, refresh_before,
        "the rotated refresh token is persisted"
    );
    // SESSION_RESUME announces the new sequence space; the sequence restarts at 1.
    let resumes = relay.received_from(DRIVER, MsgType::SessionResume);
    assert!(matches!(
        &resumes.last().expect("resume").message,
        Message::SessionResume(SessionResume::Driver { session_id: s, new_sequence_space: true, .. }) if *s == session_id
    ));
    client
        .send_live(Message::TelemetryDelta(tlv(3, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(100)).await;
    let stream = driver_stream(&relay);
    assert_eq!(
        stream.iter().map(|s| (s.0, s.3)).collect::<Vec<_>>(),
        vec![(1, Some(1)), (2, Some(2)), (1, Some(3))]
    );
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rotated_resume_token_is_persisted_at_once_and_used_after_a_restart() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    let stored = |store: &transport::MemoryCredentialStore| {
        store
            .load(&ns, CredentialKind::DriverSession)
            .expect("store")
            .expect("stored session")
            .expose()
            .to_owned()
    };

    // Mid-connection rotation: persisted while the connection is still up (a crash right
    // after it must not resume with a stale token).
    let token = relay.rotate_driver_token_now(session_id);
    advance(Duration::from_millis(100)).await;
    assert_eq!(status.borrow().state, ConnectionState::Connected);
    assert_eq!(stored(&store), format!("v1:{session_id}:{token}"));

    // Rotation in the reply to a resume after a network blip.
    relay.set_rotate_driver_tokens(true);
    relay.disconnect_all();
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Resumed);
    let token = relay.driver_token(session_id).expect("session");
    assert_eq!(stored(&store), format!("v1:{session_id}:{token}"));

    // Restart: the newest token resumes the session.
    client.shutdown().await;
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (resumed_id, kind) = session_ready(&mut events).await;
    assert_eq!(
        (resumed_id, kind),
        (session_id, SessionReadyKind::ResumedAfterRestart)
    );
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn stalled_relay_goes_degraded_then_stale_then_reconnects_and_resumes() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    client
        .send_live(Message::TelemetryDelta(tlv(1, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_secs(3)).await;
    let states = record_states(client.status());

    // The relay stops answering but keeps the socket open.
    let stalled_at = Instant::now();
    relay.stall_connections();
    let degraded = wait_state(&mut status, ConnectionState::Degraded).await;
    assert_eq!(degraded.degraded, Some(DegradedReason::MissedHeartbeat));
    wait_state(&mut status, ConnectionState::Stale).await;
    let reconnecting = wait_state(&mut status, ConnectionState::Reconnecting).await;
    assert!(reconnecting.last_error.is_some());
    assert!(reconnecting.retry_in.is_some());
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Resumed);
    let connected = wait_state(&mut status, ConnectionState::Connected).await;
    assert_eq!(connected.session_id, Some(session_id));

    let log = states.lock().clone();
    let seq: Vec<ConnectionState> = log.iter().map(|(s, _)| *s).collect();
    assert!(
        is_subsequence(
            &seq,
            &[
                ConnectionState::Connected,
                ConnectionState::Degraded,
                ConnectionState::Stale,
                ConnectionState::Reconnecting,
                ConnectionState::Connecting,
                ConnectionState::Connected,
            ]
        ),
        "{seq:?}"
    );
    // Thresholds (PROTOCOL.md §7, ARCHITECTURE.md §5.1), measured from the last PONG, which
    // arrived at most one heartbeat before the stall, plus one liveness check of slack.
    let at = |state| {
        log.iter()
            .find(|(s, t)| *s == state && *t >= stalled_at)
            .map(|(_, t)| t.duration_since(stalled_at))
            .expect("state observed")
    };
    let (d, s, r) = (
        at(ConnectionState::Degraded),
        at(ConnectionState::Stale),
        at(ConnectionState::Reconnecting),
    );
    let ms = Duration::from_millis;
    assert!(d >= ms(1_500) && d <= ms(2_750), "DEGRADED after {d:?}");
    assert!(s >= ms(2_000) && s <= ms(3_250), "STALE after {s:?}");
    assert!(r >= ms(4_000) && r <= ms(5_250), "RECONNECTING after {r:?}");
    assert!(d < s && s < r);

    // The resume continued the session and its sequence space.
    let resumes = relay.received_from(DRIVER, MsgType::SessionResume);
    assert!(matches!(
        &resumes.last().expect("resume").message,
        Message::SessionResume(SessionResume::Driver { session_id: id, new_sequence_space: false, .. }) if *id == session_id
    ));
    client
        .send_live(Message::TelemetryDelta(tlv(2, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_secs(2)).await;
    let stream = driver_stream(&relay);
    assert_eq!(
        stream.iter().map(|s| (s.0, s.3)).collect::<Vec<_>>(),
        vec![(1, Some(1)), (2, Some(2))]
    );
    assert_eq!(client.metrics().borrow().reconnects, 1);
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn relay_outage_reconnects_with_exponential_backoff_and_jitter() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (min, max) = (config.timing.reconnect_min, config.timing.reconnect_max);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;

    // Collect every (attempt, backoff) the client announces while the relay is unreachable.
    let mut watcher = client.status();
    let attempts = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&attempts);
    tokio::spawn(async move {
        while watcher.changed().await.is_ok() {
            let s = watcher.borrow_and_update().clone();
            if s.state == ConnectionState::Reconnecting
                && let Some(delay) = s.retry_in
            {
                let mut v = sink.lock();
                if v.last().map(|(a, _)| *a) != Some(s.reconnect_attempt) {
                    v.push((s.reconnect_attempt, delay));
                }
            }
        }
    });
    relay.set_down(true);
    relay.disconnect_all();
    advance(Duration::from_secs(30)).await;
    assert_eq!(status.borrow().state, ConnectionState::Reconnecting);
    let seen = attempts.lock().clone();
    assert!(seen.len() >= 6, "{seen:?}");
    for (i, (attempt, delay)) in seen.iter().enumerate() {
        assert_eq!(*attempt, u32::try_from(i).expect("small") + 1, "{seen:?}");
        let ceiling = min.saturating_mul(1 << (attempt - 1).min(16)).min(max);
        assert!(
            *delay >= (ceiling / 2).max(min) && *delay <= ceiling,
            "attempt {attempt}: {delay:?} outside [{:?}, {ceiling:?}]",
            ceiling / 2
        );
    }
    assert!(
        seen.iter().all(|(_, d)| *d <= max),
        "capped at RECONNECT_MAX"
    );

    // The relay is back: resume without user action.
    relay.set_down(false);
    let (resumed_id, kind) = session_ready(&mut events).await;
    assert_eq!((resumed_id, kind), (session_id, SessionReadyKind::Resumed));
    wait_status(&mut status, "healthy again", |s| {
        s.state == ConnectionState::Connected && s.reconnect_attempt == 0
    })
    .await;
    assert_eq!(client.metrics().borrow().reconnects, 1);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn latest_wins_drops_obsolete_frames_under_a_slow_reader_and_never_loses_one_silently() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;

    // The relay reads slowly (not at all for a moment): the driver's writer blocks on the full
    // stream while the application keeps producing ~4.4 kB deltas.
    const PUSHED: u64 = 400;
    relay.pause_reading(true);
    for tag in 1..=PUSHED {
        client
            .send_live(Message::TelemetryDelta(tlv(tag, 400)), FramePriority::P0)
            .expect("delta");
        tokio::task::yield_now().await;
    }
    relay.pause_reading(false);
    advance(Duration::from_secs(2)).await;
    let stream = driver_stream(&relay);
    let tags: Vec<u64> = stream.iter().filter_map(|s| s.3).collect();
    let dropped = client.metrics().borrow().dropped_obsolete;
    assert!(dropped > 0, "the slow reader forced latest-wins drops");
    assert_eq!(
        tags.len() as u64 + dropped,
        PUSHED,
        "every frame is either delivered or counted as dropped_obsolete"
    );
    assert_eq!(
        tags.last(),
        Some(&PUSHED),
        "the newest frame always gets through"
    );
    assert!(
        tags.windows(2).all(|w| w[0] < w[1]),
        "delivered in capture order"
    );
    let seqs: Vec<u64> = stream.iter().map(|s| s.0).collect();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0] + 1),
        "dropped frames leave no sequence gap: {seqs:?}"
    );

    // A snapshot supersedes pending deltas and is never superseded by later ones.
    let delivered_before = stream.len();
    relay.pause_reading(true);
    client
        .send_live(Message::TelemetryDelta(tlv(1_000, 1)), FramePriority::P1)
        .expect("delta");
    let outcome = client
        .send_live(Message::TelemetrySnapshot(tlv(1_001, 4)), FramePriority::P0)
        .expect("snapshot");
    assert_eq!(outcome.superseded, 1);
    client
        .send_live(Message::TelemetryDelta(tlv(1_002, 1)), FramePriority::P0)
        .expect("delta");
    let outcome = client
        .send_live(Message::TelemetryDelta(tlv(1_003, 1)), FramePriority::P0)
        .expect("delta");
    assert!(outcome.replaced);
    relay.pause_reading(false);
    advance(Duration::from_secs(2)).await;
    let after: Vec<(MsgType, Option<u64>)> = driver_stream(&relay)
        .into_iter()
        .skip(delivered_before)
        .map(|s| (s.2, s.3))
        .collect();
    assert_eq!(
        after,
        vec![
            (MsgType::TelemetrySnapshot, Some(1_001)),
            (MsgType::TelemetryDelta, Some(1_003)),
        ]
    );
    assert_eq!(client.metrics().borrow().dropped_obsolete, dropped + 2);
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn driver_ignores_every_inbound_type_outside_the_engineer_direction_whitelist() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    advance(Duration::from_secs(2)).await;
    drain_events(&mut events);
    let ignored_before = client.metrics().borrow().ignored_frames;

    // Types no relay may send to a driver (§3): telemetry, joins, kicks, client handshake.
    let foreign = [
        Message::TelemetryDelta(tlv(7, 1)),
        Message::TelemetrySnapshot(tlv(8, 1)),
        Message::SessionJoin(SessionJoin {
            code: Secret::new("7KQ4MZ9T"),
        }),
        Message::EngineerKick(EngineerKick { connection_id: 1 }),
        Message::SessionCreate(session_create()),
        Message::Ping(protocol::messages::Ping { id: 1, t0: 1 }),
    ];
    for (i, message) in foreign.iter().enumerate() {
        relay.send_to_drivers(&relay_frame(message, session_id, i as u64 + 1));
    }
    advance(Duration::from_secs(2)).await;
    assert!(
        drain_events(&mut events).is_empty(),
        "nothing reaches the application"
    );
    let ignored = client.metrics().borrow().ignored_frames - ignored_before;
    assert_eq!(ignored, foreign.len() as u64);
    assert_eq!(status.borrow().state, ConnectionState::Connected);

    // The whitelisted SNAPSHOT_REQUEST does reach it (rate-limited, §5).
    relay.send_to_drivers(&relay_frame(
        &Message::SnapshotRequest(SnapshotRequest {
            reason: SnapshotReason::Manual,
        }),
        session_id,
        100,
    ));
    match next_event(&mut events).await {
        DriverEvent::SnapshotNeeded {
            trigger: SnapshotTrigger::Requested(SnapshotReason::Manual),
        } => {}
        other => panic!("expected a requested snapshot, got {other:?}"),
    }
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn revoked_refresh_token_is_a_clear_relogin_state_without_retry_loop() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    client.shutdown().await;

    // Server-side revocation, then an app restart.
    relay.revoke_refresh_tokens();
    relay.revoke_access_tokens();
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth.clone()).expect("valid config");
    let mut status = client.status();
    let auths_before = relay.auth_attempts();
    client.start(session_create()).await.expect("start");
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::ReloginRequired(
            ReloginCause::RefreshTokenRejected
        ))
    );
    assert!(
        failed
            .failure
            .expect("reason")
            .to_string()
            .contains("log in")
    );
    // Never a silent retry loop: one AUTH, then nothing.
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.auth_attempts() - auths_before, 1);
    assert_eq!(status.borrow().state, ConnectionState::Failed);
    // The refused refresh token is gone; the driver session survives for the next login.
    assert_eq!(
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store"),
        None
    );
    assert!(
        store
            .load(&ns, CredentialKind::DriverSession)
            .expect("store")
            .is_some()
    );
    assert!(!auth.is_logged_in().expect("store"));

    // Logging in again (explicit user action) and starting again resumes the session.
    login(
        &relay.transport(),
        &config,
        &auth,
        ClientRole::Driver,
        DRIVER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login");
    client.start(session_create()).await.expect("start");
    let (resumed_id, kind) = session_ready(&mut events).await;
    assert_eq!(
        (resumed_id, kind),
        (session_id, SessionReadyKind::ResumedAfterRestart)
    );
    wait_state(&mut status, ConnectionState::Connected).await;
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn expired_refresh_token_and_missing_login_are_clear_relogin_states() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    // Never logged in.
    let (_store, auth) = new_app(&config);
    let (client, _events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::ReloginRequired(ReloginCause::NotLoggedIn))
    );
    client.shutdown().await;

    // Logged in, then the refresh token expired server-side.
    let (store, _auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    relay.expire_refresh_tokens();
    let auth = restart_app(&store, &config);
    let (client, _events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::ReloginRequired(
            ReloginCause::RefreshTokenExpired
        ))
    );
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn relay_of_another_environment_is_refused_before_any_credential_is_sent() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let auths_before = relay.auth_attempts();
    relay.set_env(Environment::Staging);
    let (client, _events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::EnvironmentMismatch {
            expected: Environment::Development,
            announced: Environment::Staging,
        })
    );
    let accepted = relay.connections_accepted();
    advance(Duration::from_secs(60)).await;
    assert_eq!(
        relay.auth_attempts(),
        auths_before,
        "no AUTH reached the relay"
    );
    assert_eq!(relay.connections_accepted(), accepted, "not retried");
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn stop_keeps_the_session_and_end_session_forgets_it() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;

    client.stop().await.expect("stop");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    assert_eq!(relay.sessions(), vec![session_id]);
    // Starting again within the run resumes it, continuing the sequence space.
    client.start(session_create()).await.expect("start");
    let (id, kind) = session_ready(&mut events).await;
    assert_eq!((id, kind), (session_id, SessionReadyKind::Resumed));
    wait_state(&mut status, ConnectionState::Connected).await;

    client.end_session().await.expect("end");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    advance(Duration::from_millis(100)).await;
    assert!(
        relay.sessions().is_empty(),
        "SESSION_CLOSE ended it on the relay"
    );
    assert_eq!(
        store
            .load(&ns, CredentialKind::DriverSession)
            .expect("store"),
        None
    );
    // The next start creates a new session.
    client.start(session_create()).await.expect("start");
    let (new_id, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    assert_ne!(new_id, session_id);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_session_the_relay_forgot_is_replaced_by_a_new_one() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    client.shutdown().await;
    relay.forget_sessions();
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (new_id, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    assert_ne!(new_id, session_id);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn stopping_during_auth_never_strands_the_rotated_refresh_token() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    drop(auth);
    let stored = || {
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store")
            .expect("refresh token")
    };
    let before = stored();

    // App restart: no access token in memory, so AUTH presents (and the relay rotates) the stored
    // refresh token. The relay's AUTH_OK is delayed, and the user stops meanwhile.
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    relay.hold_auth_replies(true);
    client.start(session_create()).await.expect("start");
    wait_state(&mut status, ConnectionState::Authenticating).await;
    for _ in 0..1_000 {
        if relay.held_auth_replies() == 1 {
            break;
        }
        advance(Duration::from_millis(1)).await;
    }
    assert_eq!(relay.held_auth_replies(), 1, "AUTH reached the relay");
    client.stop().await.expect("stop");
    wait_state(&mut status, ConnectionState::Disconnected).await;

    // The late AUTH_OK is still processed: its successor token is persisted.
    relay.hold_auth_replies(false);
    advance(Duration::from_millis(100)).await;
    assert_ne!(
        stored(),
        before,
        "the rotated refresh token was persisted despite the stop"
    );

    // So the next start authenticates silently; no re-login is needed.
    client.start(session_create()).await.expect("start");
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    wait_state(&mut status, ConnectionState::Connected).await;
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_login_with_an_account_of_the_other_role_stores_nothing() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = new_app(&config);
    let transport = relay.transport();
    let wrong_role = || {
        login(
            &transport,
            &config,
            &auth,
            ClientRole::Driver,
            ENGINEER,
            Secret::new(PASSWORD),
        )
    };
    let error = wrong_role()
        .await
        .expect_err("an engineer account in driver mode");
    assert!(matches!(
        error,
        LoginError::RoleMismatch {
            requested: ClientRole::Driver,
            granted: ClientRole::Engineer
        }
    ));
    assert_eq!(error.to_string(), "this account is not a driver account");
    assert!(store.keys().is_empty(), "nothing was stored");
    assert!(!auth.is_logged_in().expect("store"));

    // A previous login stays exactly as it was.
    login(
        &transport,
        &config,
        &auth,
        ClientRole::Driver,
        DRIVER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login");
    let keys = store.keys();
    let ns = auth.namespace().to_owned();
    let before = store
        .load(&ns, CredentialKind::RefreshToken)
        .expect("store");
    wrong_role().await.expect_err("still the wrong role");
    assert_eq!(store.keys(), keys);
    assert_eq!(
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store"),
        before
    );
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    client.shutdown().await;
}

/// Waits (bounded) until the published metrics satisfy `pred`.
async fn wait_metrics(
    client: &DriverClient,
    what: &str,
    pred: impl FnMut(&transport::LinkMetrics) -> bool,
) -> transport::LinkMetrics {
    let mut metrics = client.metrics();
    tokio::time::timeout(WAIT, metrics.wait_for(pred))
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
        .expect("metrics watch")
        .clone()
}

/// A SCHEMA_NEGOTIATE of roughly `note_bytes` bytes (one capability with a long note).
fn big_schema(note_bytes: usize) -> Message {
    let mut capabilities = CapabilityMap::default();
    capabilities.channels.insert(
        ChannelId(1),
        ChannelCapability {
            status: CapabilityStatus::Unavailable,
            source: Source::LmuSharedMemoryTelemetry,
            native_rate_hz: None,
            note: Some("n".repeat(note_bytes)),
        },
    );
    Message::SchemaNegotiate(SchemaNegotiate {
        schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        capabilities,
        vehicle_capabilities: VehicleCapabilities::default(),
    })
}

#[tokio::test(start_paused = true)]
async fn reliable_messages_are_accepted_only_while_streaming_and_never_delivered_late() {
    let relay = FakeRelay::new(Environment::Development);
    let mut config = dev_config(&relay);
    config.reliable_queue_capacity = 4;
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();

    // Not started: nothing would carry them, so they are refused instead of queued for later.
    assert_eq!(
        client.send_reliable(schema_negotiate()).await,
        Err(QueueError::NotStreaming)
    );
    assert_eq!(
        client.try_send_reliable(schema_negotiate()),
        Err(QueueError::NotStreaming)
    );
    assert_eq!(client.request_connect_code(), Err(QueueError::NotStreaming));
    assert_eq!(client.kick_engineer(1), Err(QueueError::NotStreaming));

    client.start(session_create()).await.expect("start");
    session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    client
        .send_reliable(schema_negotiate())
        .await
        .expect("accepted while streaming");
    advance(Duration::from_millis(100)).await;
    assert_eq!(
        relay.received_from(DRIVER, MsgType::SchemaNegotiate).len(),
        1
    );

    // An outage: messages queue up while the client reconnects ...
    relay.set_down(true);
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    for _ in 0..4 {
        client
            .try_send_reliable(schema_negotiate())
            .expect("queued during the outage");
    }
    assert_eq!(
        client.try_send_reliable(schema_negotiate()),
        Err(QueueError::Full { capacity: 4 })
    );
    // ... a producer waits for space, and stopping releases it instead of leaving it hanging
    // until some later session drains the queue.
    let (waited, ()) = tokio::join!(client.send_reliable(schema_negotiate()), async {
        advance(Duration::from_millis(100)).await;
        client.stop().await.expect("stop");
    });
    assert_eq!(waited, Err(QueueError::NotStreaming));
    wait_state(&mut status, ConnectionState::Disconnected).await;
    // The pending ones are discarded and counted, and nothing is accepted while stopped.
    let m = wait_metrics(&client, "the discarded messages are counted", |m| {
        m.reliable_discarded == 4
    })
    .await;
    assert_eq!(m.send_queue_depth, 0);
    assert_eq!(
        client.try_send_reliable(schema_negotiate()),
        Err(QueueError::NotStreaming)
    );

    // Streaming again (the same session resumes): the discarded messages are not delivered late.
    relay.set_down(false);
    client.start(session_create()).await.expect("start");
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Resumed);
    wait_state(&mut status, ConnectionState::Connected).await;
    client
        .send_reliable(schema_negotiate())
        .await
        .expect("streaming again");
    advance(Duration::from_millis(200)).await;
    assert_eq!(
        relay.received_from(DRIVER, MsgType::SchemaNegotiate).len(),
        2,
        "only the two sent while streaming reached the relay"
    );
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn session_ready_carries_the_identity_but_never_the_resume_token() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let event = next_event(&mut events).await;
    let DriverEvent::SessionReady {
        session_id,
        kind,
        identity,
    } = &event
    else {
        panic!("expected SessionReady, got {event:?}");
    };
    assert_eq!(*kind, SessionReadyKind::Created);
    assert_eq!(identity.session_id, *session_id);
    assert_eq!(identity.driver_username, DRIVER);
    assert_eq!(identity.driver_display_name, DRIVER_DISPLAY);
    assert_eq!(identity.track.as_deref(), Some("Circuit de la Sarthe"));
    let token = relay.driver_token(*session_id).expect("resume token");
    assert!(
        !format!("{event:?}").contains(&token),
        "the resume token never leaves the transport"
    );
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_refused_access_token_falls_back_to_the_refresh_token_on_the_same_connection() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    let refresh = || {
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store")
            .expect("refresh token")
    };
    // The access-token AUTH of this connection returned a successor refresh token (the real
    // relay's rule); that is the one the client holds now.
    let refresh_before = refresh();

    // The relay restarted with new signing keys: every access token it issued is refused.
    relay.revoke_access_tokens();
    let auths_before = relay.auth_attempts();
    relay.disconnect_all();
    let (resumed, kind) = session_ready(&mut events).await;
    assert_eq!((resumed, kind), (session_id, SessionReadyKind::Resumed));
    wait_state(&mut status, ConnectionState::Connected).await;
    let auths: Vec<_> = relay
        .received()
        .into_iter()
        .filter(|r| r.message.msg_type() == MsgType::Auth)
        .skip(auths_before)
        .collect();
    assert_eq!(auths.len(), 2, "{auths:?}");
    assert!(matches!(
        auths[0].message,
        Message::Auth(protocol::messages::Auth::AccessToken { .. })
    ));
    assert!(matches!(
        &auths[1].message,
        Message::Auth(protocol::messages::Auth::RefreshToken { token }) if *token == refresh_before
    ));
    assert_eq!(
        auths[0].conn, auths[1].conn,
        "the refresh token was tried on the same connection"
    );
    assert_ne!(
        refresh(),
        refresh_before,
        "the rotated successor is persisted"
    );
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn being_replaced_by_another_driver_connection_fails_without_retry() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    let hellos = relay.hellos(ClientRole::Driver);
    relay.send_to_drivers(&relay_frame(
        &Message::Disconnect(Disconnect {
            reason: DisconnectReason::Replaced,
            message: None,
        }),
        session_id,
        0,
    ));
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(failed.failure, Some(FailureReason::Replaced));
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Driver), hellos, "no reconnect");
    assert_eq!(status.borrow().state, ConnectionState::Failed);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_fatal_incompatible_error_mid_session_fails_without_retry() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let (session_id, _) = session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    let hellos = relay.hellos(ClientRole::Driver);
    relay.send_to_drivers(&relay_frame(
        &Message::Error(ErrorMsg {
            code: ErrorCode::Incompatible,
            message: "protocol 2.0 required".to_owned(),
            fatal: true,
        }),
        session_id,
        0,
    ));
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::Incompatible {
            detail: "protocol 2.0 required".to_owned()
        })
    );
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Driver), hellos, "no reconnect");
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn frames_above_the_relay_limit_are_dropped_counted_and_leave_no_sequence_gap() {
    let relay = FakeRelay::new(Environment::Development);
    let limit = u32::try_from(transport::handshake::MIN_RELAY_PAYLOAD_LIMIT).expect("fits");
    relay.set_max_payload_bytes(limit);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    // About 22 kB: above the announced 16 kB.
    client
        .send_live(Message::TelemetryDelta(tlv(1, 2_000)), FramePriority::P0)
        .expect("queued");
    advance(Duration::from_millis(100)).await;
    client
        .send_live(Message::TelemetryDelta(tlv(2, 2)), FramePriority::P0)
        .expect("queued");
    let m = wait_metrics(&client, "the oversize drop is counted", |m| {
        m.oversize_dropped == 1
    })
    .await;
    assert_eq!(m.dropped_obsolete, 0);
    let stream = driver_stream(&relay);
    assert_eq!(
        stream.iter().map(|s| (s.0, s.3)).collect::<Vec<_>>(),
        vec![(1, Some(2))],
        "the dropped frame consumed no sequence number"
    );
    assert_eq!(status.borrow().state, ConnectionState::Connected);
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unannounced_payload_limit_means_the_maximum_and_an_unusable_one_is_incompatible() {
    let relay = FakeRelay::new(Environment::Development);
    relay.set_max_payload_bytes(0);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) = DriverClient::spawn(
        config.clone(),
        relay.transport(),
        std::sync::Arc::clone(&auth),
    )
    .expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    client
        .send_live(Message::TelemetryDelta(tlv(1, 2_000)), FramePriority::P0)
        .expect("queued");
    advance(Duration::from_millis(100)).await;
    assert_eq!(driver_stream(&relay).len(), 1, "0 means not announced");
    assert_eq!(client.metrics().borrow().oversize_dropped, 0);
    client.shutdown().await;

    // A relay announcing less than any session needs: FAILED, not CONNECTED and mute.
    relay.set_max_payload_bytes(1_024);
    let (client, _events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    match failed.failure {
        Some(FailureReason::Incompatible { detail }) => {
            assert!(detail.contains("1024"), "{detail}");
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
    let accepted = relay.connections_accepted();
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.connections_accepted(), accepted, "not retried");
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_reliable_message_cut_off_by_a_dead_connection_is_sent_again_after_the_resume() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (_store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let (client, mut events) =
        DriverClient::spawn(config, relay.transport(), auth).expect("valid config");
    let mut status = client.status();
    client.start(session_create()).await.expect("start");
    session_ready(&mut events).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    let first_conn = relay.received_from(DRIVER, MsgType::SessionCreate)[0].conn;

    // The relay stops reading: a 200 kB SCHEMA_NEGOTIATE cannot be written completely.
    relay.pause_reading(true);
    let schema = big_schema(200_000);
    client.send_reliable(schema.clone()).await.expect("queued");
    advance(Duration::from_millis(200)).await;
    assert!(
        relay
            .received_from(DRIVER, MsgType::SchemaNegotiate)
            .is_empty()
    );
    // The connection dies with the frame half-written.
    relay.disconnect_all();
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Resumed);
    wait_state(&mut status, ConnectionState::Connected).await;
    advance(Duration::from_millis(500)).await;
    let schemas = relay.received_from(DRIVER, MsgType::SchemaNegotiate);
    assert_eq!(schemas.len(), 1, "sent again, once, complete");
    assert_ne!(schemas[0].conn, first_conn, "on the resumed connection");
    assert_eq!(schemas[0].message, schema);
    assert_eq!(
        schemas[0].header.sequence, 1,
        "the cut-off write consumed no sequence number"
    );
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

/// AUTH messages the relay received, oldest first.
fn auths(relay: &FakeRelay) -> Vec<protocol::messages::Auth> {
    relay
        .received()
        .into_iter()
        .filter_map(|r| match r.message {
            Message::Auth(auth) => Some(auth),
            _ => None,
        })
        .collect()
}

/// Waits (paused clock) until the relay holds `n` AUTH replies back.
async fn wait_held_auth_replies(relay: &FakeRelay, n: usize) {
    for _ in 0..1_000 {
        if relay.held_auth_replies() == n {
            break;
        }
        advance(Duration::from_millis(1)).await;
    }
    assert_eq!(relay.held_auth_replies(), n, "AUTH reached the relay");
}

#[tokio::test(start_paused = true)]
async fn overlapping_access_token_auths_applied_out_of_order_keep_the_login() {
    // Review finding F1 (relay2): the user stops while AUTH#1{AccessToken} is in flight (the
    // detached AUTH keeps running) and starts again at once: AUTH#2{AccessToken} goes out on a new
    // connection. The AUTH_OKs reach the app in reverse order. An access-token AUTH issues no
    // refresh token (PROTOCOL.md §6), so whichever AUTH_OK the app applies last, it keeps the
    // refresh token of its login, and hours later a restarted app still logs in silently.
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let stored = || {
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store")
            .expect("refresh token")
    };
    let before = stored();
    let (client, mut events) = DriverClient::spawn(
        config.clone(),
        relay.transport(),
        std::sync::Arc::clone(&auth),
    )
    .expect("valid config");
    let mut status = client.status();
    relay.hold_auth_replies(true);
    client.start(session_create()).await.expect("start");
    wait_held_auth_replies(&relay, 1).await;
    client.stop().await.expect("stop");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    client.start(session_create()).await.expect("start again");
    wait_held_auth_replies(&relay, 2).await;
    let sent = auths(&relay);
    assert!(
        sent.len() >= 2
            && sent[sent.len() - 2..]
                .iter()
                .all(|a| matches!(a, protocol::messages::Auth::AccessToken { .. })),
        "both AUTHs use the still valid access token"
    );
    // AUTH_OK #2 first, then AUTH_OK #1.
    relay.release_newest_held_auth_reply();
    advance(Duration::from_millis(100)).await;
    relay.hold_auth_replies(false);
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    advance(Duration::from_millis(100)).await;
    assert_eq!(stored(), before, "no AUTH_OK replaced the refresh token");
    client.shutdown().await;

    // Hours later (access token expired, far beyond the reuse grace): silent login.
    advance(Duration::from_secs(3 * 3_600)).await;
    let (client, mut events) = DriverClient::spawn(
        config.clone(),
        relay.transport(),
        restart_app(&store, &config),
    )
    .expect("valid config");
    client.start(session_create()).await.expect("start");
    // Ready at all means the login survived (the stored session may be resumed or replaced).
    let (_, kind) = session_ready(&mut events).await;
    assert!(matches!(
        kind,
        SessionReadyKind::Created | SessionReadyKind::ResumedAfterRestart
    ));
    assert!(matches!(
        auths(&relay).last(),
        Some(protocol::messages::Auth::RefreshToken { .. })
    ));
    assert_ne!(stored(), before, "the refresh-token AUTH rotated it");
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_lost_access_token_auth_reply_does_not_force_a_relogin() {
    // Review finding F2 (relay2): the relay processed AUTH{AccessToken} but the link died before
    // its AUTH_OK arrived. That AUTH changed no refresh token, so the token the app holds is still
    // the live one: an app restart long after the reuse grace (and the access token's expiry)
    // logs in silently.
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let (store, auth) = logged_in_app(&relay, &config, ClientRole::Driver, DRIVER).await;
    let ns = auth.namespace().to_owned();
    let before = store
        .load(&ns, CredentialKind::RefreshToken)
        .expect("store")
        .expect("refresh token");
    let (client, _events) = DriverClient::spawn(
        config.clone(),
        relay.transport(),
        std::sync::Arc::clone(&auth),
    )
    .expect("valid config");
    let mut status = client.status();
    relay.hold_auth_replies(true);
    client.start(session_create()).await.expect("start");
    wait_held_auth_replies(&relay, 1).await;
    assert!(matches!(
        auths(&relay).last(),
        Some(protocol::messages::Auth::AccessToken { .. })
    ));
    client.stop().await.expect("stop");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    // The link dies before the held AUTH_OK leaves the relay.
    relay.disconnect_all();
    advance(Duration::from_millis(100)).await;
    relay.hold_auth_replies(false);
    advance(Duration::from_millis(100)).await;
    assert_eq!(
        store
            .load(&ns, CredentialKind::RefreshToken)
            .expect("store")
            .as_ref(),
        Some(&before),
        "the app never received that AUTH_OK"
    );
    client.shutdown().await;

    advance(Duration::from_secs(20 * 60)).await;
    let (client, mut events) = DriverClient::spawn(
        config.clone(),
        relay.transport(),
        restart_app(&store, &config),
    )
    .expect("valid config");
    client.start(session_create()).await.expect("start");
    let (_, kind) = session_ready(&mut events).await;
    assert_eq!(kind, SessionReadyKind::Created);
    assert!(matches!(
        auths(&relay).last(),
        Some(protocol::messages::Auth::RefreshToken { .. })
    ));
    client.shutdown().await;
}

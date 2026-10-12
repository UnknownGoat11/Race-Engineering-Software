//! Engineer client against the in-process fake relay (D-022): join with a one-time code, identity
//! confirmation, typed join errors, cancel / disconnect, automatic ticket-based resume after a
//! network blip, the clear "connection ended" states (ticket expired, kicked, session ended), the
//! driver's code / engineer controls, the driver-restart stream reset, and that nothing about a
//! driver or session is ever persisted or reconnected across engineer app restarts.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use protocol::frame::{FramePriority, MsgType};
use protocol::messages::{
    ClientRole, EngineerEntry, ErrorCode, Message, Secret, SessionResume, SnapshotReason,
    SnapshotRequest,
};
use protocol::sequence::Arrival;
use protocol::version::Environment;
use telemetry_model::enums::SessionType;
use tokio::sync::mpsc;
use tokio::time::Instant;
use transport::{
    ClientConfig, ConnectionState, DriverClient, DriverEvent, EngineerClient, EngineerEndReason,
    EngineerEvent, EngineerPhase, FailureReason, IdentityCard, InboundFrame, JoinError,
    MemoryCredentialStore, SnapshotTrigger, display_connect_code, login,
};

/// `(msg_type, sequence, TLV tag, replayed, arrival)` of a delivered frame.
type FrameSummary = (MsgType, u64, Option<u64>, bool, Option<Arrival>);

/// A logged-in driver that created its session and streamed SCHEMA_NEGOTIATE (seq 1), a snapshot
/// (tag 1, seq 2) and a delta (tag 2, seq 3).
struct Driver {
    client: DriverClient,
    events: mpsc::Receiver<DriverEvent>,
    session_id: u64,
    store: Arc<MemoryCredentialStore>,
}

async fn streaming_driver(relay: &FakeRelay, config: &ClientConfig) -> Driver {
    let (store, auth) = new_app(config);
    login(
        &relay.transport(),
        config,
        &auth,
        ClientRole::Driver,
        DRIVER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("driver login");
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    let session_id = match next_event(&mut events).await {
        DriverEvent::SessionReady { session_id, .. } => session_id,
        other => panic!("expected SessionReady, got {other:?}"),
    };
    let mut status = client.status();
    wait_state(&mut status, ConnectionState::Connected).await;
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
    advance(Duration::from_millis(500)).await;
    drain_events(&mut events);
    Driver {
        client,
        events,
        session_id,
        store,
    }
}

/// The driver's current connection code in display form (`XXXX-XXXX`).
async fn current_code(driver: &DriverClient) -> String {
    let mut watch = driver.connect_code();
    let code = tokio::time::timeout(
        WAIT,
        watch.wait_for(|c| c.as_ref().is_some_and(|c| c.code.is_some())),
    )
    .await
    .expect("a code in time")
    .expect("code watch")
    .clone()
    .and_then(|c| c.code)
    .expect("code");
    display_connect_code(&code.code)
}

/// Waits until the driver shows a code different from `old`.
async fn next_code(driver: &DriverClient, old: &str) -> String {
    let mut watch = driver.connect_code();
    let code = tokio::time::timeout(
        WAIT,
        watch.wait_for(|c| {
            c.as_ref()
                .and_then(|c| c.code.as_ref())
                .is_some_and(|i| display_connect_code(&i.code) != old)
        }),
    )
    .await
    .expect("a new code in time")
    .expect("code watch")
    .clone()
    .and_then(|c| c.code)
    .expect("code");
    display_connect_code(&code.code)
}

struct Engineer {
    client: EngineerClient,
    events: mpsc::Receiver<EngineerEvent>,
    store: Arc<MemoryCredentialStore>,
}

/// A fresh engineer app whose user logged in once as `user`.
async fn engineer_app(relay: &FakeRelay, config: &ClientConfig, user: &str) -> Engineer {
    let (store, auth) = new_app(config);
    login(
        &relay.transport(),
        config,
        &auth,
        ClientRole::Engineer,
        user,
        Secret::new(PASSWORD),
    )
    .await
    .expect("engineer login");
    let (client, events) =
        EngineerClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    Engineer {
        client,
        events,
        store,
    }
}

/// Joins with the driver's current code and confirms the identity.
async fn joined_engineer(
    relay: &FakeRelay,
    config: &ClientConfig,
    driver: &Driver,
    user: &str,
) -> Engineer {
    let mut e = engineer_app(relay, config, user).await;
    let code = current_code(&driver.client).await;
    e.client.join(&code).await.expect("join");
    e.client.confirm().await.expect("confirm");
    let mut status = e.client.status();
    wait_state(&mut status, ConnectionState::Connected).await;
    advance(Duration::from_millis(200)).await;
    drain_events(&mut e.events);
    e
}

fn frames(events: Vec<EngineerEvent>) -> Vec<InboundFrame> {
    events
        .into_iter()
        .filter_map(|e| match e {
            EngineerEvent::Frame(f) => Some(*f),
            _ => None,
        })
        .collect()
}

/// Next `Ended` event, skipping frames.
async fn ended(events: &mut mpsc::Receiver<EngineerEvent>) -> EngineerEndReason {
    loop {
        match next_event(events).await {
            EngineerEvent::Ended(reason) => return reason,
            EngineerEvent::Frame(_) | EngineerEvent::StreamReset(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// Next `StreamReset` event, skipping frames.
async fn stream_reset(events: &mut mpsc::Receiver<EngineerEvent>) -> IdentityCard {
    loop {
        match next_event(events).await {
            EngineerEvent::StreamReset(card) => return card,
            EngineerEvent::Frame(_) => {}
            other => panic!("expected StreamReset, got {other:?}"),
        }
    }
}

/// Tickets `user` presented in its SESSION_RESUMEs, oldest first.
fn presented_tickets(relay: &FakeRelay, user: &str) -> Vec<String> {
    relay
        .received_from(user, MsgType::SessionResume)
        .iter()
        .map(|r| match &r.message {
            Message::SessionResume(SessionResume::Engineer { ticket, .. }) => {
                ticket.expose().to_owned()
            }
            other => panic!("expected an engineer resume, got {other:?}"),
        })
        .collect()
}

/// Whether `user` sent DISCONNECT on the connection that carried its (latest) SESSION_JOIN.
fn said_goodbye_on_the_join_connection(relay: &FakeRelay, user: &str) -> bool {
    let Some(join) = relay.received_from(user, MsgType::SessionJoin).pop() else {
        return false;
    };
    relay
        .received_from(user, MsgType::Disconnect)
        .iter()
        .any(|d| d.conn == join.conn)
}

/// Waits (bounded) until the driver's engineer list satisfies `pred`.
async fn wait_engineers(
    driver: &DriverClient,
    what: &str,
    pred: impl FnMut(&Vec<EngineerEntry>) -> bool,
) {
    let mut watch = driver.engineers();
    tokio::time::timeout(WAIT, watch.wait_for(pred))
        .await
        .unwrap_or_else(|_| panic!("timed out waiting until {what}"))
        .expect("engineer watch");
}

/// Only the refresh token may ever be stored by an engineer app (D-022).
fn assert_only_refresh_token(store: &MemoryCredentialStore) {
    let keys = store.keys();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(keys[0].ends_with("#refresh_token"), "{keys:?}");
}

#[tokio::test(start_paused = true)]
async fn join_with_code_confirm_identity_then_monitor_the_stream() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let mut driver = streaming_driver(&relay, &config).await;
    let mut e = engineer_app(&relay, &config, ENGINEER).await;
    let mut status = e.client.status();
    let phase = e.client.phase();
    assert_eq!(*phase.borrow(), EngineerPhase::Idle);
    // The engineer app never connects by itself.
    let accepted = relay.connections_accepted();
    advance(Duration::from_secs(30)).await;
    assert_eq!(relay.connections_accepted(), accepted);
    assert_eq!(status.borrow().state, ConnectionState::Disconnected);

    let code = current_code(&driver.client).await;
    // Typed as received over chat: lower case, with the separator.
    let card = e.client.join(&code.to_lowercase()).await.expect("join");
    assert_eq!(card.session_id, driver.session_id);
    assert_eq!(card.driver_display_name, DRIVER_DISPLAY);
    assert_eq!(card.driver_username, DRIVER);
    assert_eq!(card.car.as_deref(), Some("Porsche 963 #6"));
    assert_eq!(card.vehicle_class.as_deref(), Some("Hypercar"));
    assert_eq!(card.track.as_deref(), Some("Circuit de la Sarthe"));
    assert_eq!(card.session_type, Some(SessionType::Race(1)));
    assert!(card.started_at_ns >= RELAY_BASE_UNIX_NS);
    assert_eq!(
        *phase.borrow(),
        EngineerPhase::AwaitingConfirmation(card.clone())
    );
    wait_state(&mut status, ConnectionState::Connected).await;
    let joins = relay.received_from(ENGINEER, MsgType::SessionJoin);
    assert!(matches!(
        &joins[0].message,
        Message::SessionJoin(j) if j.code.expose() == code.replace('-', "")
    ));

    // The driver sees the engineer and gets a fresh code for a possible second one.
    let mut engineers = driver.client.engineers();
    let list = engineers
        .wait_for(|l| !l.is_empty())
        .await
        .expect("engineer list")
        .clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].username, ENGINEER);
    let second_code = next_code(&driver.client, &code).await;
    assert_ne!(second_code, code);
    let usage = driver.client.connect_code().borrow().clone().expect("code");
    assert_eq!((usage.engineers_connected, usage.engineers_max), (1, 2));
    // The relay asked the driver for a snapshot on the engineer's behalf.
    loop {
        if let DriverEvent::SnapshotNeeded {
            trigger: SnapshotTrigger::Requested(SnapshotReason::EngineerJoined),
        } = next_event(&mut driver.events).await
        {
            break;
        }
    }

    // Nothing is shown before the identity is confirmed, but frames are kept.
    driver
        .client
        .send_live(Message::TelemetryDelta(tlv(3, 2)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(500)).await;
    assert!(frames(drain_events(&mut e.events)).is_empty());
    e.client.confirm().await.expect("confirm");
    assert_eq!(*phase.borrow(), EngineerPhase::Monitoring(card.clone()));
    let delivered = frames(drain_events(&mut e.events));
    let summary: Vec<FrameSummary> = delivered
        .iter()
        .map(|f| {
            (
                f.message.msg_type(),
                f.header.sequence,
                message_tag(&f.message),
                f.replayed(),
                f.arrival,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (MsgType::SchemaNegotiate, 1, None, true, None),
            (MsgType::TelemetrySnapshot, 2, Some(1), true, None),
            (
                MsgType::TelemetryDelta,
                4,
                Some(3),
                false,
                Some(Arrival::First)
            ),
        ],
        "join replay (REPLAYED, untracked) then the live frame, in sequence order"
    );

    // Live after confirmation: the driver answers the join's snapshot request.
    driver
        .client
        .send_live(Message::TelemetrySnapshot(tlv(10, 8)), FramePriority::P0)
        .expect("snapshot");
    advance(Duration::from_millis(100)).await;
    let live = frames(drain_events(&mut e.events));
    assert_eq!(live.len(), 1);
    assert_eq!(message_tag(&live[0].message), Some(10));
    assert_eq!(live[0].arrival, Some(Arrival::InOrder));
    assert!(
        live[0].trailer.is_some(),
        "relay trailer kept for the application"
    );

    // Heartbeat and sequence statistics on the engineer side.
    advance(Duration::from_secs(3)).await;
    let m = e.client.metrics().borrow().clone();
    let seq = m.sequence.expect("engineer tracks the stream");
    assert_eq!((seq.received, seq.lost, seq.duplicated), (2, 0, 0));
    assert!(m.rtt.is_some() && m.clock.is_some() && e.client.clock().is_some());
    assert_eq!(relay.header_violations(), 0);
    assert_only_refresh_token(&e.store);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn join_errors_are_typed_for_the_ui() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e3 = engineer_app(&relay, &config, ENGINEER3).await;
    let mut status = e3.client.status();

    // Malformed input never reaches the network.
    let accepted = relay.connections_accepted();
    assert!(matches!(
        e3.client.join("7KQ4-MZ9").await,
        Err(JoinError::CodeMalformed(_))
    ));
    assert!(matches!(
        e3.client.join("7KQ4-MZ9U").await,
        Err(JoinError::CodeMalformed(_))
    ));
    assert_eq!(relay.connections_accepted(), accepted);

    // Unknown code.
    let error = e3.client.join("ZZZZ-ZZZZ").await.expect_err("unknown code");
    assert_eq!(error, JoinError::CodeInvalid);
    assert!(error.message().contains("new code"));
    assert_eq!(*e3.client.phase().borrow(), EngineerPhase::Idle);
    let s = wait_state(&mut status, ConnectionState::Disconnected).await;
    assert!(s.last_error.is_some(), "the failure is shown, not a crash");

    // Expired code.
    let code = current_code(&driver.client).await;
    relay.expire_codes();
    assert_eq!(e3.client.join(&code).await, Err(JoinError::CodeExpired));

    // "New code" on the driver, then two engineers fill both slots.
    driver.client.request_connect_code().expect("request");
    let code = next_code(&driver.client, &code).await;
    let e1 = engineer_app(&relay, &config, ENGINEER).await;
    e1.client.join(&code).await.expect("first engineer");
    let code2 = next_code(&driver.client, &code).await;
    let e2 = engineer_app(&relay, &config, ENGINEER2).await;
    e2.client.join(&code2).await.expect("second engineer");
    let mut watch = driver.client.connect_code();
    let full = watch
        .wait_for(|c| c.as_ref().is_some_and(|c| c.engineers_connected == 2))
        .await
        .expect("code watch")
        .clone()
        .expect("code");
    assert!(full.code.is_none(), "no code while every slot is taken");

    // A third engineer with a used code: the session is full.
    let error = e3.client.join(&code2).await.expect_err("full");
    assert_eq!(error, JoinError::SessionFull { max: 2 });
    assert_eq!(error.message(), "Driver already has 2 engineers connected");

    // A driver account cannot join as an engineer.
    let (_store, driver_auth) = new_app(&config);
    login(
        &relay.transport(),
        &config,
        &driver_auth,
        ClientRole::Driver,
        DRIVER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("driver login");
    let (not_engineer, _events) =
        EngineerClient::spawn(config.clone(), relay.transport(), driver_auth)
            .expect("valid config");
    let error = not_engineer.join(&code2).await.expect_err("driver account");
    assert_eq!(error, JoinError::NotAnEngineer);
    assert_eq!(error.message(), "This account is not an engineer account.");

    // Redemption rate limit.
    relay.set_join_limit(Some(2));
    let error = e3.client.join("ZZZZ-ZZZY").await.expect_err("limited");
    assert_eq!(error, JoinError::RateLimited);
    assert!(error.message().contains("Wait"));
    // A join is attempted once: no automatic retries of a failed code.
    let joins = relay.received_from(ENGINEER3, MsgType::SessionJoin).len();
    advance(Duration::from_secs(60)).await;
    assert_eq!(
        relay.received_from(ENGINEER3, MsgType::SessionJoin).len(),
        joins
    );
    assert_eq!(relay.header_violations(), 0);
    for c in [e1.client, e2.client, e3.client, not_engineer] {
        c.shutdown().await;
    }
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn cancel_at_the_identity_card_and_disconnect_need_a_new_code_afterwards() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = engineer_app(&relay, &config, ENGINEER).await;
    let mut status = e.client.status();
    let code = current_code(&driver.client).await;
    e.client.join(&code).await.expect("join");
    e.client.cancel().await.expect("cancel");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    advance(Duration::from_millis(100)).await;
    assert!(
        !relay
            .received_from(ENGINEER, MsgType::Disconnect)
            .is_empty()
    );
    let mut engineers = driver.client.engineers();
    engineers
        .wait_for(Vec::is_empty)
        .await
        .expect("slot freed on cancel");
    // The cancelled code stays used.
    assert_eq!(e.client.join(&code).await, Err(JoinError::CodeInvalid));

    // A new code connects; an explicit disconnect drops the ticket and frees the slot.
    let code = next_code(&driver.client, &code).await;
    e.client.join(&code).await.expect("join");
    e.client.confirm().await.expect("confirm");
    wait_state(&mut status, ConnectionState::Connected).await;
    e.client.disconnect().await.expect("disconnect");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    engineers
        .wait_for(Vec::is_empty)
        .await
        .expect("slot freed on disconnect");
    assert_eq!(e.client.join(&code).await, Err(JoinError::CodeInvalid));
    assert_only_refresh_token(&e.store);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn network_blip_resumes_with_the_in_memory_ticket_and_requests_a_snapshot() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();
    let card = match e.client.phase().borrow().clone() {
        EngineerPhase::Monitoring(card) => card,
        other => panic!("expected Monitoring, got {other:?}"),
    };
    let ticket_join = relay.received_from(ENGINEER, MsgType::SessionJoin).len();
    assert_eq!(ticket_join, 1);

    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    match next_event(&mut e.events).await {
        EngineerEvent::StreamReset(c) => assert_eq!(c, card),
        other => panic!("expected StreamReset, got {other:?}"),
    }
    // Resumed with the ticket (no new code, no new join), then asked for a fresh snapshot.
    assert_eq!(relay.received_from(ENGINEER, MsgType::SessionJoin).len(), 1);
    let resumes = relay.received_from(ENGINEER, MsgType::SessionResume);
    assert!(matches!(
        &resumes.last().expect("resume").message,
        Message::SessionResume(SessionResume::Engineer { session_id, .. }) if *session_id == driver.session_id
    ));
    advance(Duration::from_millis(500)).await;
    let requests = relay.received_from(ENGINEER, MsgType::SnapshotRequest);
    assert!(matches!(
        requests.last().map(|r| &r.message),
        Some(Message::SnapshotRequest(SnapshotRequest {
            reason: SnapshotReason::Reconnect
        }))
    ));
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Monitoring(card));
    assert_eq!(e.client.metrics().borrow().reconnects, 1);

    // The stream continues.
    let mut d_status = driver.client.status();
    wait_state(&mut d_status, ConnectionState::Connected).await;
    driver
        .client
        .send_live(Message::TelemetryDelta(tlv(30, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(500)).await;
    let tags: Vec<Option<u64>> = frames(drain_events(&mut e.events))
        .iter()
        .filter(|f| !f.replayed())
        .map(|f| message_tag(&f.message))
        .collect();
    assert!(tags.contains(&Some(30)), "{tags:?}");
    assert_eq!(relay.header_violations(), 0);
    assert_only_refresh_token(&e.store);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn confirming_while_the_link_reconnects_takes_effect() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = engineer_app(&relay, &config, ENGINEER).await;
    let mut status = e.client.status();
    let code = current_code(&driver.client).await;
    let card = e.client.join(&code).await.expect("join");
    wait_state(&mut status, ConnectionState::Connected).await;

    // A network blip before the user confirmed. The reconnect is slow (AUTH replies are held)
    // and the user confirms the identity card while it is in progress.
    relay.hold_auth_replies(true);
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Authenticating).await;
    e.client
        .confirm()
        .await
        .expect("confirm while reconnecting");
    assert_eq!(
        *e.client.phase().borrow(),
        EngineerPhase::Monitoring(card.clone())
    );
    relay.hold_auth_replies(false);
    wait_state(&mut status, ConnectionState::Connected).await;

    // The resumed stream is delivered: the confirmation was not lost in the reconnect.
    let mut d_status = driver.client.status();
    wait_state(&mut d_status, ConnectionState::Connected).await;
    driver
        .client
        .send_live(Message::TelemetryDelta(tlv(40, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(500)).await;
    let tags: Vec<Option<u64>> = frames(drain_events(&mut e.events))
        .iter()
        .filter(|f| !f.replayed())
        .map(|f| message_tag(&f.message))
        .collect();
    assert!(tags.contains(&Some(40)), "{tags:?}");
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Monitoring(card));
    assert_eq!(relay.header_violations(), 0);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn rejected_ticket_ends_the_connection_with_a_clear_new_code_state() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();

    relay.kill_tickets();
    relay.disconnect_all();
    assert_eq!(ended(&mut e.events).await, EngineerEndReason::TicketExpired);
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::SessionEnded(
            EngineerEndReason::TicketExpired
        ))
    );
    assert!(
        failed
            .failure
            .expect("reason")
            .to_string()
            .contains("Enter a new code from the driver")
    );
    assert_eq!(
        *e.client.phase().borrow(),
        EngineerPhase::Ended(EngineerEndReason::TicketExpired)
    );
    // No retry loop and no reconnect without a new code.
    let attempts = relay.hellos(ClientRole::Engineer);
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Engineer), attempts);
    // Acknowledged by the user.
    e.client.disconnect().await.expect("disconnect");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    assert_only_refresh_token(&e.store);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn ticket_expires_after_its_ttl_while_the_relay_is_unreachable() {
    let relay = FakeRelay::new(Environment::Development);
    let mut config = dev_config(&relay);
    let ttl = Duration::from_secs(20);
    config.timing.engineer_ticket_ttl = ttl;
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;

    relay.set_down(true);
    let dropped_at = Instant::now();
    relay.disconnect_all();
    assert_eq!(ended(&mut e.events).await, EngineerEndReason::TicketExpired);
    let after = dropped_at.elapsed();
    assert!(
        after >= ttl && after <= ttl + config.timing.reconnect_max + Duration::from_secs(1),
        "ended {after:?} after the drop"
    );
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn kicked_engineer_ends_and_the_driver_slot_is_freed() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();
    let mut engineers = driver.client.engineers();
    let list = engineers
        .wait_for(|l| l.len() == 1)
        .await
        .expect("list")
        .clone();
    driver
        .client
        .kick_engineer(list[0].connection_id)
        .expect("kick");
    assert_eq!(ended(&mut e.events).await, EngineerEndReason::Kicked);
    let failed = wait_state(&mut status, ConnectionState::Failed).await;
    assert_eq!(
        failed.failure,
        Some(FailureReason::SessionEnded(EngineerEndReason::Kicked))
    );
    assert_eq!(
        failed.failure.expect("reason").to_string(),
        "The driver disconnected you."
    );
    engineers.wait_for(Vec::is_empty).await.expect("slot freed");
    // The kicked engineer does not come back by itself.
    let attempts = relay.hellos(ClientRole::Engineer);
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Engineer), attempts);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn driver_ending_the_session_ends_it_for_the_engineer() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    driver.client.end_session().await.expect("end session");
    assert_eq!(
        ended(&mut e.events).await,
        EngineerEndReason::DriverEndedSession
    );
    assert_eq!(
        *e.client.phase().borrow(),
        EngineerPhase::Ended(EngineerEndReason::DriverEndedSession)
    );
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn driver_restart_resets_the_engineer_stream_without_false_loss() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    for tag in 3..=6 {
        driver
            .client
            .send_live(Message::TelemetryDelta(tlv(tag, 1)), FramePriority::P0)
            .expect("delta");
        advance(Duration::from_millis(50)).await;
    }
    let before = frames(drain_events(&mut e.events));
    assert_eq!(before.len(), 4);
    let Driver {
        client,
        store,
        session_id,
        ..
    } = driver;
    client.shutdown().await;

    // Driver app restart: resumes the session in a new sequence space.
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    match next_event(&mut events).await {
        DriverEvent::SessionReady { session_id: id, .. } => assert_eq!(id, session_id),
        other => panic!("expected SessionReady, got {other:?}"),
    }
    match next_event(&mut e.events).await {
        EngineerEvent::StreamReset(card) => assert_eq!(card.session_id, session_id),
        other => panic!("expected StreamReset, got {other:?}"),
    }
    let mut status = client.status();
    wait_state(&mut status, ConnectionState::Connected).await;
    client
        .send_live(Message::TelemetrySnapshot(tlv(20, 4)), FramePriority::P0)
        .expect("snapshot");
    advance(Duration::from_millis(50)).await;
    client
        .send_live(Message::TelemetryDelta(tlv(21, 1)), FramePriority::P0)
        .expect("delta");
    advance(Duration::from_millis(500)).await;
    let after: Vec<(u64, Option<u64>, Option<Arrival>)> = frames(drain_events(&mut e.events))
        .iter()
        .map(|f| (f.header.sequence, message_tag(&f.message), f.arrival))
        .collect();
    assert_eq!(
        after,
        vec![
            (1, Some(20), Some(Arrival::First)),
            (2, Some(21), Some(Arrival::InOrder)),
        ],
        "the new sequence space is delivered, not discarded as stale"
    );
    advance(Duration::from_secs(2)).await;
    let m = e.client.metrics().borrow().clone();
    assert_eq!(
        m.sequence_totals.lost, 0,
        "no false loss across the restart"
    );
    assert_eq!(m.sequence_totals.received, 6);
    e.client.shutdown().await;
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn engineer_app_restart_never_reconnects_and_a_new_code_still_needs_no_password() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let store = Arc::clone(&e.store);
    e.client.shutdown().await;
    assert_only_refresh_token(&store);

    // Restart: idle, never connects to the previous driver by itself.
    let auth = restart_app(&store, &config);
    let (client, _events) =
        EngineerClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    let accepted = relay.connections_accepted();
    advance(Duration::from_secs(120)).await;
    assert_eq!(relay.connections_accepted(), accepted);
    assert_eq!(*client.phase().borrow(), EngineerPhase::Idle);
    assert_eq!(
        client.status().borrow().state,
        ConnectionState::Disconnected
    );

    // A fresh code connects with the stored login (silent re-authentication).
    let passwords_before = relay
        .received()
        .iter()
        .filter(|r| {
            matches!(
                &r.message,
                Message::Auth(protocol::messages::Auth::Password { .. })
            )
        })
        .count();
    let code = current_code(&driver.client).await;
    let card = client.join(&code).await.expect("join after restart");
    assert_eq!(card.driver_username, DRIVER);
    let passwords_after = relay
        .received()
        .iter()
        .filter(|r| {
            matches!(
                &r.message,
                Message::Auth(protocol::messages::Auth::Password { .. })
            )
        })
        .count();
    assert_eq!(passwords_after, passwords_before, "no password needed");
    assert_only_refresh_token(&store);
    client.shutdown().await;
    driver.client.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// Ticket rotation (the relay issues a new ticket with every SESSION_INFO and, like the real relay
// today, accepts only the newest one unless a grace is configured).
// -------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn consecutive_blips_resume_with_the_ticket_of_the_newest_session_info() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();
    let card = match e.client.phase().borrow().clone() {
        EngineerPhase::Monitoring(card) => card,
        other => panic!("expected Monitoring, got {other:?}"),
    };
    let mut issued = vec![relay.engineer_ticket(ENGINEER).expect("ticket of the join")];
    for _ in 0..3 {
        relay.disconnect_all();
        wait_state(&mut status, ConnectionState::Reconnecting).await;
        wait_state(&mut status, ConnectionState::Connected).await;
        assert_eq!(stream_reset(&mut e.events).await, card);
        issued.push(
            relay
                .engineer_ticket(ENGINEER)
                .expect("ticket of the resume"),
        );
    }
    // Every resume presented the ticket of the SESSION_INFO that answered the previous join or
    // resume: a client that kept the join's ticket would have been refused at the second blip.
    let presented = presented_tickets(&relay, ENGINEER);
    assert_eq!(presented, issued[..3].to_vec());
    assert!(
        issued.windows(2).all(|w| w[0] != w[1]),
        "every SESSION_INFO carried a new ticket: {issued:?}"
    );
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Monitoring(card));
    assert_eq!(e.client.metrics().borrow().reconnects, 3);
    assert_eq!(relay.header_violations(), 0);
    assert_only_refresh_token(&e.store);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_ticket_of_a_driver_restart_notice_is_adopted_for_the_next_resume() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let joined_ticket = relay.engineer_ticket(ENGINEER).expect("ticket");
    let Driver { client, store, .. } = driver;
    client.shutdown().await;

    // Driver app restart: the relay announces the new sequence space with a SESSION_INFO on the
    // engineer's live connection, which carries a new ticket.
    let auth = restart_app(&store, &config);
    let (client, mut events) =
        DriverClient::spawn(config.clone(), relay.transport(), auth).expect("valid config");
    client.start(session_create()).await.expect("start");
    match next_event(&mut events).await {
        DriverEvent::SessionReady { .. } => {}
        other => panic!("expected SessionReady, got {other:?}"),
    }
    stream_reset(&mut e.events).await;
    let notice_ticket = relay.engineer_ticket(ENGINEER).expect("ticket");
    assert_ne!(notice_ticket, joined_ticket);

    // A blip afterwards resumes with the notice's ticket.
    let mut status = e.client.status();
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    stream_reset(&mut e.events).await;
    assert_eq!(presented_tickets(&relay, ENGINEER), vec![notice_ticket]);
    assert!(matches!(
        *e.client.phase().borrow(),
        EngineerPhase::Monitoring(_)
    ));
    e.client.shutdown().await;
    client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_resume_reply_lost_in_a_second_blip_is_retried_with_the_previous_ticket() {
    // The relay rule this relies on (PROTOCOL.md §6, the FakeRelay default like the real relay):
    // the previous ticket stays valid for PREVIOUS_TICKET_GRACE, because the client can learn the
    // new one only from the reply it lost.
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();
    let joined_ticket = relay.engineer_ticket(ENGINEER).expect("ticket");

    // Blip 1: the resume reaches the relay (which rotates the ticket), but the link drops again
    // before its SESSION_INFO arrives.
    relay.lose_next_session_replies(1);
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    stream_reset(&mut e.events).await;
    // The client never saw the rotated ticket, so it presented the joined one twice; the second
    // time the relay honoured it within the grace and answered with a fresh one.
    assert_eq!(
        presented_tickets(&relay, ENGINEER),
        vec![joined_ticket.clone(), joined_ticket.clone()]
    );
    assert!(matches!(
        *e.client.phase().borrow(),
        EngineerPhase::Monitoring(_)
    ));
    assert!(
        drain_events(&mut e.events)
            .iter()
            .all(|ev| !matches!(ev, EngineerEvent::Ended(_)))
    );

    // And it adopted that fresh ticket: the next blip resumes with it.
    let fresh = relay.engineer_ticket(ENGINEER).expect("ticket");
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    wait_state(&mut status, ConnectionState::Connected).await;
    assert_eq!(presented_tickets(&relay, ENGINEER).last(), Some(&fresh));
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_lost_resume_reply_followed_by_an_outage_past_the_grace_ends_cleanly() {
    // The relay honours the previous ticket only for PREVIOUS_TICKET_GRACE (PROTOCOL.md §6). If it
    // stays unreachable longer than that after a lost resume reply, the retry is refused and the
    // client ends with the clear "new code" state, without a retry loop.
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let joined_ticket = relay.engineer_ticket(ENGINEER).expect("ticket");
    relay.lose_next_session_replies(1);
    relay.disconnect_all();
    // The first resume reaches the relay (new ticket issued, reply lost); then the relay is
    // unreachable for longer than the grace. Reconnects are at least RECONNECT_MIN_MS apart.
    for _ in 0..10_000 {
        if !presented_tickets(&relay, ENGINEER).is_empty() {
            break;
        }
        advance(Duration::from_millis(1)).await;
    }
    relay.set_down(true);
    assert_eq!(
        presented_tickets(&relay, ENGINEER),
        vec![joined_ticket.clone()]
    );
    advance(PREVIOUS_TICKET_GRACE + Duration::from_secs(1)).await;
    relay.set_down(false);
    assert_eq!(ended(&mut e.events).await, EngineerEndReason::TicketExpired);
    assert_eq!(
        presented_tickets(&relay, ENGINEER),
        vec![joined_ticket.clone(), joined_ticket]
    );
    let attempts = relay.hellos(ClientRole::Engineer);
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Engineer), attempts);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn without_a_ticket_grace_a_lost_resume_reply_ends_the_connection_cleanly() {
    // A relay that honours only the newest ticket (`previous_ticket_grace` = 0, or a relay older
    // than the grace) cannot be resumed after a lost reply. The client then ends with the clear
    // "new code" state, without a retry loop.
    let relay = FakeRelay::new(Environment::Development);
    relay.set_ticket_grace(None);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let mut e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    relay.lose_next_session_replies(1);
    relay.disconnect_all();
    assert_eq!(ended(&mut e.events).await, EngineerEndReason::TicketExpired);
    assert_eq!(presented_tickets(&relay, ENGINEER).len(), 2);
    let attempts = relay.hellos(ClientRole::Engineer);
    advance(Duration::from_secs(60)).await;
    assert_eq!(relay.hellos(ClientRole::Engineer), attempts);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

// -------------------------------------------------------------------------------------------------
// A join or resume in progress never leaves without DISCONNECT (the slot would be held for the
// ticket lifetime).
// -------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn cancel_while_the_join_reply_is_pending_frees_the_slot_at_once() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = engineer_app(&relay, &config, ENGINEER).await;
    let code = current_code(&driver.client).await;
    relay.hold_session_replies(true);
    let (joined, ()) = tokio::join!(e.client.join(&code), async {
        // The relay redeemed the code and assigned a slot; its SESSION_INFO is held back.
        wait_engineers(&driver.client, "the slot is taken", |l| l.len() == 1).await;
        assert_eq!(*e.client.phase().borrow(), EngineerPhase::Joining);
        e.client.cancel().await.expect("cancel");
    });
    assert_eq!(joined, Err(JoinError::Cancelled));
    // DISCONNECT freed the slot now, not after the ticket lifetime.
    wait_engineers(&driver.client, "the slot is freed", Vec::is_empty).await;
    assert_eq!(relay.engineer_slots(), 0);
    assert!(said_goodbye_on_the_join_connection(&relay, ENGINEER));
    wait_state(&mut e.client.status(), ConnectionState::Disconnected).await;
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    relay.hold_session_replies(false);
    assert_eq!(relay.header_violations(), 0);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_join_whose_reply_never_arrives_says_goodbye_and_frees_the_slot() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = engineer_app(&relay, &config, ENGINEER).await;
    let code = current_code(&driver.client).await;
    relay.hold_session_replies(true);
    let started = Instant::now();
    let error = e.client.join(&code).await.expect_err("no reply");
    assert!(matches!(error, JoinError::Unreachable { .. }), "{error:?}");
    assert!(started.elapsed() >= config.timing.handshake_timeout);
    wait_engineers(&driver.client, "the slot is freed", Vec::is_empty).await;
    assert_eq!(relay.engineer_slots(), 0);
    assert!(said_goodbye_on_the_join_connection(&relay, ENGINEER));
    relay.hold_session_replies(false);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_join_reply_without_a_ticket_is_refused_and_the_slot_released() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = engineer_app(&relay, &config, ENGINEER).await;
    let code = current_code(&driver.client).await;
    relay.set_join_reply_without_ticket(true);
    let error = e.client.join(&code).await.expect_err("unusable reply");
    assert!(
        matches!(
            error,
            JoinError::Refused {
                code: ErrorCode::Malformed,
                ..
            }
        ),
        "{error:?}"
    );
    wait_engineers(&driver.client, "the slot is freed", Vec::is_empty).await;
    assert_eq!(relay.engineer_slots(), 0);
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn disconnect_while_a_resume_reply_is_pending_frees_the_slot() {
    let relay = FakeRelay::new(Environment::Development);
    let config = dev_config(&relay);
    let driver = streaming_driver(&relay, &config).await;
    let e = joined_engineer(&relay, &config, &driver, ENGINEER).await;
    let mut status = e.client.status();
    relay.hold_session_replies(true);
    relay.disconnect_all();
    wait_state(&mut status, ConnectionState::Reconnecting).await;
    // The resume reaches the relay (the slot is re-attached), its reply is held back.
    for _ in 0..1_000 {
        if !relay
            .received_from(ENGINEER, MsgType::SessionResume)
            .is_empty()
        {
            break;
        }
        advance(Duration::from_millis(10)).await;
    }
    assert_eq!(
        relay.received_from(ENGINEER, MsgType::SessionResume).len(),
        1
    );
    e.client.disconnect().await.expect("disconnect");
    wait_state(&mut status, ConnectionState::Disconnected).await;
    wait_engineers(&driver.client, "the slot is freed", Vec::is_empty).await;
    assert_eq!(relay.engineer_slots(), 0);
    assert_eq!(*e.client.phase().borrow(), EngineerPhase::Idle);
    relay.hold_session_replies(false);
    e.client.shutdown().await;
    driver.client.shutdown().await;
}

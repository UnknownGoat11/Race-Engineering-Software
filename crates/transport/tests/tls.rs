//! Real sockets: TCP + TLS 1.3 against the fake relay listening on 127.0.0.1 (Tokio's real clock).
//!
//! * Certificate validation is always on: an untrusted issuer, a certificate for another host name
//!   and a server without ALPN `lmure/1` are refused before any protocol byte is exchanged.
//! * TLS 1.3 with ALPN `lmure/1` is negotiated over a real TCP connection and frames flow.
//! * The production transport ([`TcpTlsTelemetryTransport`]) drives a driver session, including an
//!   automatic reconnect + resume after the connection is dropped.
//! * The staged connectivity test ("Test Relay Connection") passes every stage against a healthy
//!   relay without side effects, and reports the failing stage (TCP, TLS, HELLO environment, AUTH)
//!   with every later stage skipped.

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use common::*;
use protocol::frame::{FrameMeta, MsgType, NO_SESSION};
use protocol::messages::{ClientRole, Hello, Message, Secret};
use protocol::version::{Environment, ProtocolVersion};
use rustls::CertificateError;
use tokio::net::TcpListener;
use tokio::time::Instant;
use transport::connectivity::RTT_PROBES;
use transport::{
    ConnectError, ConnectStage, ConnectionState, ConnectivityReport, CredentialKind,
    CredentialStore, DriverClient, DriverEvent, ExtraCa, FrameWriter, RelayEndpoint,
    SessionReadyKind, Stage, StageOutcome, TcpTlsTelemetryTransport, TelemetryTransport, login,
    test_relay_connection,
};

/// Upper bound for conditions polled on the real clock.
const POLL_LIMIT: Duration = Duration::from_secs(10);
/// Polling period of [`wait_until`].
const POLL_PERIOD: Duration = Duration::from_millis(10);

/// Endpoint of a relay listening on `addr` (127.0.0.1) in the development environment, trusting
/// `ca` (PEM) in addition to the public roots.
fn dev_endpoint(addr: SocketAddr, ca: Vec<u8>) -> RelayEndpoint {
    RelayEndpoint::new(
        Ipv4Addr::LOCALHOST.to_string(),
        addr.port(),
        "local",
        Environment::Development,
    )
    .expect("endpoint")
    .with_extra_ca(ExtraCa::Pem(ca))
    .expect("development may trust a test CA")
}

/// Production endpoint of a relay listening on `addr` (public roots only).
fn production_endpoint(addr: SocketAddr) -> RelayEndpoint {
    RelayEndpoint::new(
        Ipv4Addr::LOCALHOST.to_string(),
        addr.port(),
        "local",
        Environment::Production,
    )
    .expect("endpoint")
}

/// Connects and expects a refusal.
async fn connect_err(endpoint: &RelayEndpoint) -> ConnectError {
    match TcpTlsTelemetryTransport::new().connect(endpoint).await {
        Ok(_) => panic!("the connection to {endpoint} must be refused"),
        Err(e) => e,
    }
}

/// Asserts that `error` is a certificate rejection matching `expected`.
fn assert_untrusted(error: &ConnectError, expected: impl Fn(&CertificateError) -> bool) {
    match error {
        ConnectError::UntrustedCertificate {
            source: rustls::Error::InvalidCertificate(e),
        } if expected(e) => {
            assert_eq!(error.stage(), ConnectStage::Tls);
        }
        other => panic!("expected an untrusted certificate, got {other:?}"),
    }
}

/// Polls `condition` on the real clock.
async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + POLL_LIMIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        tokio::time::sleep(POLL_PERIOD).await;
    }
}

/// Asserts that the report failed at `stage` (error containing `error_contains`), with every
/// earlier stage passed and every later one skipped.
fn assert_failed_at(report: &ConnectivityReport, stage: Stage, error_contains: &str) {
    let stages: Vec<Stage> = report.stages.iter().map(|s| s.stage).collect();
    assert_eq!(stages, Stage::ALL.to_vec(), "{report:#?}");
    let failed = report
        .first_failure()
        .unwrap_or_else(|| panic!("no failed stage in {report:#?}"));
    assert_eq!(failed.stage, stage, "{report:#?}");
    match &failed.outcome {
        StageOutcome::Failed { error } => {
            assert!(error.contains(error_contains), "{stage}: {error}");
        }
        other => panic!("{stage}: {other:?}"),
    }
    let position = |s: Stage| Stage::ALL.iter().position(|x| *x == s);
    for result in &report.stages {
        match position(result.stage).cmp(&position(stage)) {
            std::cmp::Ordering::Less => assert!(
                matches!(result.outcome, StageOutcome::Passed { .. }),
                "{result:?}"
            ),
            std::cmp::Ordering::Greater => {
                assert_eq!(result.outcome, StageOutcome::Skipped, "{result:?}");
            }
            std::cmp::Ordering::Equal => {}
        }
    }
    assert!(!report.passed());
}

#[tokio::test]
async fn tls13_with_alpn_lmure_over_real_tcp_and_frames_flow() {
    let relay = FakeRelay::new(Environment::Development);
    let addr = relay.listen_tcp().await;
    let mut conn = TcpTlsTelemetryTransport::new()
        .connect(&dev_endpoint(addr, relay.ca_pem()))
        .await
        .expect("TLS 1.3 connection to a trusted relay");
    assert_eq!(conn.info.tls_version.as_deref(), Some("TLSv1_3"));
    let suite = conn.info.cipher_suite.clone().expect("cipher suite");
    assert!(suite.starts_with("TLS13_"), "{suite}");
    assert_eq!(conn.info.peer, Some(addr));

    // The stream carries protocol frames: HELLO → the relay's HELLO.
    let hello = Message::Hello(Hello {
        app_version: "0.1.0-test".to_owned(),
        protocol: ProtocolVersion::CURRENT,
        role: ClientRole::Driver,
        schema_versions: vec![telemetry_model::TELEMETRY_SCHEMA_VERSION],
        client_id: "tls-test".to_owned(),
    });
    let frame = hello
        .to_frame(&FrameMeta::control(NO_SESSION, 0))
        .expect("HELLO frame");
    conn.writer.send(&frame).await.expect("send");
    conn.writer.flush().await.expect("flush");
    let reply = recv_frame(&mut conn.reader).await;
    match Message::from_frame(&reply).expect("decodable reply") {
        Message::ServerHello(server) => assert_eq!(server.env, Environment::Development),
        other => panic!("expected the relay's HELLO, got {other:?}"),
    }
    assert_eq!(conn.stats.bytes_out(), frame.wire_len() as u64);
    assert!(conn.stats.bytes_in() >= reply.wire_len() as u64);
    close_writer(&mut conn.writer).await;
    assert_eq!(relay.tls_failures(), 0);
}

#[tokio::test]
async fn untrusted_certificate_is_refused_before_any_protocol_byte() {
    let relay = FakeRelay::new(Environment::Production);
    let addr = relay.listen_tcp().await;

    // A production client trusts only the public web PKI: the relay's test CA is unknown.
    let error = connect_err(&production_endpoint(addr)).await;
    assert_untrusted(&error, |e| matches!(e, CertificateError::UnknownIssuer));
    assert!(
        error.is_retryable(),
        "may be fixed relay-side, so it is retried"
    );

    // A development client trusting a different CA refuses it just the same. That CA has the same
    // subject name as the relay's, so webpki finds it as the issuer and rejects the signature.
    let other_ca = TestPki::new(&[TEST_HOST]).ca_pem.into_bytes();
    let error = connect_err(&dev_endpoint(addr, other_ca)).await;
    assert_untrusted(&error, |e| {
        matches!(
            e,
            CertificateError::BadSignature | CertificateError::UnknownIssuer
        )
    });

    wait_until("the relay saw both handshakes fail", || {
        relay.tls_failures() == 2
    })
    .await;
    assert!(
        relay.received().is_empty(),
        "no protocol frame was exchanged"
    );
}

#[tokio::test]
async fn certificate_for_another_host_name_is_refused() {
    // Trusted issuer, but the certificate names another host than the endpoint (127.0.0.1).
    let pki = TestPki::new(&["relay.other.example"]);
    let relay = FakeRelay::with_pki(&pki, Environment::Development, &[protocol::ALPN_PROTOCOL]);
    let addr = relay.listen_tcp().await;
    let error = connect_err(&dev_endpoint(addr, pki.ca_pem.clone().into_bytes())).await;
    assert_untrusted(&error, |e| {
        matches!(
            e,
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
        )
    });
    assert!(relay.received().is_empty());
}

#[tokio::test]
async fn server_without_alpn_lmure_is_refused() {
    let pki = TestPki::new(&[TEST_HOST, "127.0.0.1"]);
    // A TLS server that ignores ALPN (e.g. a web server or an intercepting proxy on the port):
    // the handshake completes without a protocol, and the client refuses the stream.
    let no_alpn = FakeRelay::with_pki(&pki, Environment::Development, &[]);
    let addr = no_alpn.listen_tcp().await;
    let error = connect_err(&dev_endpoint(addr, pki.ca_pem.clone().into_bytes())).await;
    assert!(matches!(error, ConnectError::Alpn), "{error:?}");
    assert_eq!(error.stage(), ConnectStage::Tls);

    // A server that speaks only other protocols aborts the handshake (no_application_protocol).
    let other = FakeRelay::with_pki(&pki, Environment::Development, &[b"h2"]);
    let addr = other.listen_tcp().await;
    let error = connect_err(&dev_endpoint(addr, pki.ca_pem.clone().into_bytes())).await;
    assert!(matches!(error, ConnectError::Tls { .. }), "{error:?}");
    assert!(no_alpn.received().is_empty() && other.received().is_empty());
}

#[tokio::test]
async fn a_server_speaking_only_tls12_is_refused() {
    let pki = TestPki::new(&[TEST_HOST, "127.0.0.1"]);
    // Trusted certificate and the right ALPN, but a legacy protocol version: TLS 1.3 only.
    let legacy = FakeRelay::with_acceptor(
        &pki,
        Environment::Development,
        pki.acceptor_with_versions(&[protocol::ALPN_PROTOCOL], &[&rustls::version::TLS12]),
    );
    let addr = legacy.listen_tcp().await;
    let error = connect_err(&dev_endpoint(addr, pki.ca_pem.clone().into_bytes())).await;
    assert!(matches!(error, ConnectError::Tls { .. }), "{error:?}");
    assert_eq!(error.stage(), ConnectStage::Tls);
    wait_until("the relay saw the handshake fail", || {
        legacy.tls_failures() == 1
    })
    .await;
    assert!(legacy.received().is_empty(), "no protocol byte exchanged");
}

#[tokio::test]
async fn driver_session_over_real_tcp_resumes_after_the_connection_is_dropped() {
    let relay = FakeRelay::new(Environment::Development);
    let addr = relay.listen_tcp().await;
    let config = config_for(dev_endpoint(addr, relay.ca_pem()));
    let (_store, auth) = new_app(&config);
    let transport = TcpTlsTelemetryTransport::new();
    login(
        &transport,
        &config,
        &auth,
        ClientRole::Driver,
        DRIVER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login over TCP + TLS");
    let (client, mut events) = DriverClient::spawn(config, transport, auth).expect("valid config");
    let mut status = client.status();
    let states = record_states(client.status());
    client.start(session_create()).await.expect("start");
    let session_id = match next_event(&mut events).await {
        DriverEvent::SessionReady {
            session_id,
            kind: SessionReadyKind::Created,
            ..
        } => session_id,
        other => panic!("expected a created session, got {other:?}"),
    };
    wait_state(&mut status, ConnectionState::Connected).await;

    // Abrupt drop (network change, relay restart): reconnect, re-authenticate, resume.
    relay.disconnect_all();
    loop {
        match next_event(&mut events).await {
            DriverEvent::SessionReady {
                session_id: id,
                kind,
                ..
            } => {
                assert_eq!((id, kind), (session_id, SessionReadyKind::Resumed));
                break;
            }
            DriverEvent::SnapshotNeeded { .. }
            | DriverEvent::RelayError { .. }
            | DriverEvent::SessionEnded { .. } => {}
        }
    }
    wait_state(&mut status, ConnectionState::Connected).await;
    let seen: Vec<ConnectionState> = states.lock().iter().map(|(s, _)| *s).collect();
    assert!(
        is_subsequence(
            &seen,
            &[
                ConnectionState::Connecting,
                ConnectionState::Authenticating,
                ConnectionState::Connected,
                ConnectionState::Reconnecting,
                ConnectionState::Connecting,
                ConnectionState::Connected,
            ]
        ),
        "{seen:?}"
    );
    assert_eq!(client.metrics().borrow().reconnects, 1);
    assert_eq!(relay.tls_failures(), 0);
    assert_eq!(relay.header_violations(), 0);
    client.shutdown().await;
}

#[tokio::test]
async fn connectivity_test_passes_every_stage_without_side_effects() {
    let relay = FakeRelay::new(Environment::Development);
    let addr = relay.listen_tcp().await;
    let config = config_for(dev_endpoint(addr, relay.ca_pem()));
    let (store, auth) = new_app(&config);
    let transport = TcpTlsTelemetryTransport::new();
    login(
        &transport,
        &config,
        &auth,
        ClientRole::Engineer,
        ENGINEER,
        Secret::new(PASSWORD),
    )
    .await
    .expect("login");

    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Engineer).await;
    assert!(report.passed(), "{report:#?}");
    let stages: Vec<Stage> = report.stages.iter().map(|s| s.stage).collect();
    assert_eq!(stages, Stage::ALL.to_vec());
    match report.stage(Stage::Tls) {
        Some(StageOutcome::Passed { detail }) => assert!(detail.contains("TLSv1_3"), "{detail}"),
        other => panic!("TLS stage: {other:?}"),
    }
    let rtt = report.rtt.expect("RTT summary");
    assert_eq!(rtt.samples, RTT_PROBES);
    assert!(rtt.min <= rtt.avg && rtt.avg <= rtt.max, "{rtt:?}");
    assert!(rtt.clock_offset_ns.is_some() && rtt.clock_uncertainty_ns.is_some());

    // No session was created, joined or resumed by the test.
    assert!(relay.sessions().is_empty());
    assert!(relay.received().iter().all(|r| !matches!(
        r.message.msg_type(),
        MsgType::SessionCreate | MsgType::SessionJoin | MsgType::SessionResume
    )));
    // The login survives: a second run passes, and so does one after an app restart (which
    // authenticates with the stored, rotated refresh token).
    let again = test_relay_connection(&transport, &config, &auth, ClientRole::Engineer).await;
    assert!(again.passed(), "{again:#?}");
    let restarted = restart_app(&store, &config);
    let after_restart =
        test_relay_connection(&transport, &config, &restarted, ClientRole::Engineer).await;
    assert!(after_restart.passed(), "{after_restart:#?}");
    assert!(restarted.is_logged_in(ClientRole::Engineer).expect("store"));
}

#[tokio::test]
async fn connectivity_test_reports_the_failing_stage_and_skips_the_rest() {
    let transport = TcpTlsTelemetryTransport::new();

    // TCP: nothing listens on the port.
    let free = {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        listener.local_addr().expect("address")
    };
    let config = config_for(production_endpoint(free));
    let (_store, auth) = new_app(&config);
    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Driver).await;
    assert_failed_at(&report, Stage::Tcp, "TCP");

    // TLS: a production client does not trust the relay's test CA.
    let relay = FakeRelay::new(Environment::Production);
    let addr = relay.listen_tcp().await;
    let config = config_for(production_endpoint(addr));
    let (_store, auth) = new_app(&config);
    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Driver).await;
    assert_failed_at(&report, Stage::Tls, "not trusted");

    // HELLO: the relay announces another environment; the stored credential is never offered.
    let relay = FakeRelay::new(Environment::Staging);
    let addr = relay.listen_tcp().await;
    let config = config_for(dev_endpoint(addr, relay.ca_pem()));
    let (store, auth) = new_app(&config);
    store
        .store(
            auth.namespace(),
            CredentialKind::RefreshToken,
            &Secret::new("refresh-token-of-another-environment"),
        )
        .expect("seed");
    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Driver).await;
    assert_failed_at(&report, Stage::Hello, "Staging");
    assert_eq!(relay.auth_attempts(), 0);

    // AUTH: never logged in, a clear re-login message (and nothing is sent).
    let relay = FakeRelay::new(Environment::Development);
    let addr = relay.listen_tcp().await;
    let config = config_for(dev_endpoint(addr, relay.ca_pem()));
    let (_store, auth) = new_app(&config);
    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Driver).await;
    assert_failed_at(&report, Stage::Auth, "Not logged in");
    assert_eq!(relay.auth_attempts(), 0);

    // AUTH: only a driver login exists; engineer mode never presents it (F7, logins are kept per
    // role), so the engineer test asks for a login.
    let (_store, auth) = new_app(&config);
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
    let attempts = relay.auth_attempts();
    let report = test_relay_connection(&transport, &config, &auth, ClientRole::Engineer).await;
    assert_failed_at(&report, Stage::Auth, "Not logged in");
    assert_eq!(
        relay.auth_attempts(),
        attempts,
        "the driver token is not sent"
    );
}

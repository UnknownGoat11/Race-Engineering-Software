//! Test support for the transport integration tests.
//!
//! * [`FakeRelay`]: an in-process relay speaking the real wire protocol (`protocol` crate framing
//!   and messages) over TLS 1.3 with an rcgen-generated test CA. It implements just enough of
//!   PROTOCOL.md §6 for the clients, modelled on the real relay's rules (`crates/relay`): HELLO
//!   with a configurable environment and limits; AUTH (password; refresh tokens rotated on every
//!   use, a rotated token accepted again within `REFRESH_REUSE_GRACE`, later reuse revoking the
//!   whole login family; rotating a live token **supersedes the family's other live tokens**, a
//!   token tolerated within the grace supersedes nothing; access tokens, whose AUTH_OK carries
//!   **no refresh token** (empty, expiry 0) and changes none; at most
//!   `MAX_ACTIVE_REFRESH_PER_FAMILY` refresh tokens per family); SESSION_CREATE / SESSION_RESUME
//!   (driver; the unused, unexpired connection code is **re-sent unchanged**); one-time connection
//!   codes (a redemption, kick or CONNECT_CODE_REQUEST issues a fresh one, a DISCONNECT or ticket
//!   expiry keeps the unused one); SESSION_JOIN / SESSION_RESUME (engineer) with a **new resume
//!   ticket in every SESSION_INFO** (join, resume, driver-restart notice), the newest ticket
//!   accepted and up to two **earlier ones, each within `PREVIOUS_TICKET_GRACE`** after the ticket
//!   that replaced it (configurable with [`FakeRelay::set_ticket_grace`]; a resume makes the
//!   presented ticket the only earlier one of the fresh ticket, a driver-restart notice adds the
//!   current one); ENGINEER_LIST / ENGINEER_KICK, SESSION_CLOSE, PING/PONG, forwarding with relay
//!   trailers and the join replay.
//!   Every client frame is checked with `protocol::frame::check_client_frame` (violations are
//!   counted, see [`FakeRelay::header_violations`]). Faults are injected with its methods: stall,
//!   slow reader, abrupt drop, down, token revocation, ticket loss, environment, held or lost
//!   session replies, payload limit.
//! * [`DuplexTransport`]: a [`TelemetryTransport`] that connects to the fake relay through an
//!   in-memory byte stream, running the production TLS client handshake
//!   ([`TcpTlsTelemetryTransport::handshake_tls`], certificate validation included). No OS socket
//!   is involved, so tests can run on Tokio's paused clock deterministically.
//!
//! Each test binary uses a different subset of this module.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use parking_lot::Mutex;
use protocol::Frame;
use protocol::frame::{
    Endpoint, FrameHeader, FrameMeta, FramePriority, MsgType, NO_SESSION, RelayTrailer,
    check_client_frame,
};
use protocol::messages::{
    AccountId, Auth, AuthOk, CONNECT_CODE_ALPHABET, CONNECT_CODE_LEN, ClientRole, ConnectCode,
    EngineerEntry, EngineerLink, EngineerList, ErrorCode, ErrorMsg, IssuedCode, Message, Pong,
    ResumeCredential, SchemaNegotiate, Secret, ServerHello, ServerLimits, SessionClose,
    SessionCloseReason, SessionCreate, SessionInfo, SessionResume, SnapshotReason, SnapshotRequest,
    TelemetryPayload,
};
use protocol::version::{Environment, ProtocolVersion};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use telemetry_model::enums::SessionType;
use telemetry_model::scoring::{CapabilityMap, SessionIdentity, VehicleCapabilities};
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadHalf, WriteHalf};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, timeout};
use tokio_rustls::TlsAcceptor;
use transport::{
    AuthManager, ClientConfig, ConnectError, ConnectionInfo, ConnectionState, ConnectionStatus,
    ExtraCa, FrameReader, FrameWriter, MemoryCredentialStore, RelayEndpoint, StreamFrameReader,
    StreamFrameWriter, TcpTlsTelemetryTransport, TelemetryTransport, TransportConnection,
    framed_split,
};

/// Password of every test account.
pub const PASSWORD: &str = "correct horse battery staple";
/// The driver account ("Alice Driver").
pub const DRIVER: &str = "alice";
/// Engineer accounts.
pub const ENGINEER: &str = "erin";
pub const ENGINEER2: &str = "eddie";
pub const ENGINEER3: &str = "emma";
/// Display name of [`DRIVER`].
pub const DRIVER_DISPLAY: &str = "Alice Driver";
/// Relay wall clock at the relay's epoch, ns since the Unix epoch (2026-09-21).
pub const RELAY_BASE_UNIX_NS: u64 = 1_790_000_000_000_000_000;
/// Version string of the fake relay.
pub const RELAY_VERSION: &str = "fake-relay/1";
/// Host name the test certificates are issued for.
pub const TEST_HOST: &str = "localhost";
/// Port the in-memory endpoints claim (never opened).
pub const TEST_PORT: u16 = 4433;
/// Buffer of each direction of an in-memory connection.
pub const DUPLEX_BUFFER: usize = 64 * 1024;
/// Generous bound for waiting on the paused clock (it auto-advances, so this never slows a test).
pub const WAIT: Duration = Duration::from_secs(120);
/// Access token lifetime the relay announces.
const ACCESS_TTL: Duration = Duration::from_secs(15 * 60);
/// Refresh token lifetime the relay announces.
const REFRESH_TTL: Duration = Duration::from_secs(90 * 24 * 3600);
/// A rotated refresh token presented again within this time is accepted (the client may have
/// missed the AUTH_OK with its successor); later reuse revokes the login family (real relay
/// default `refresh_reuse_grace`).
const REFRESH_REUSE_GRACE: Duration = Duration::from_secs(60);
/// Refresh tokens kept per login family; older ones are deleted (real relay default
/// `max_active_refresh_per_family`).
const MAX_ACTIVE_REFRESH_PER_FAMILY: usize = 4;
/// An engineer's previous resume ticket is accepted this long after a newer one was issued (real
/// relay default `previous_ticket_grace`).
pub const PREVIOUS_TICKET_GRACE: Duration = Duration::from_secs(60);
/// Outbound frames queued per relay connection.
const RELAY_OUT_CAPACITY: usize = 8_192;
/// Channel id used in test TLV blocks (not in the registry: receivers skip it).
const TEST_CHANNEL: u16 = 0xFFF0;
/// TLV tag byte of a VALID f64 (quality 0, value type 2).
const TAG_VALID_F64: u8 = 2;
/// Byte range of `capture_mono_ns` in a TLV header (used as the test frame's tag).
const TLV_TAG_RANGE: std::ops::Range<usize> = 2..10;

// -------------------------------------------------------------------------------------------------
// PKI
// -------------------------------------------------------------------------------------------------

/// A test CA and one server certificate signed by it.
#[derive(Clone)]
pub struct TestPki {
    pub ca_pem: String,
    pub server_chain: Vec<CertificateDer<'static>>,
    pub server_key_der: Vec<u8>,
}

impl TestPki {
    /// A fresh CA and a server certificate for `names` (DNS names or IP addresses).
    pub fn new(names: &[&str]) -> Self {
        let ca_key = KeyPair::generate().expect("CA key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "lmure transport test CA");
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_cert = ca_params.self_signed(&ca_key).expect("CA certificate");
        let issuer = Issuer::new(ca_params, ca_key);
        let key = KeyPair::generate().expect("server key");
        let mut params =
            CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>())
                .expect("server params");
        params.distinguished_name.push(
            DnType::CommonName,
            names.first().copied().unwrap_or(TEST_HOST),
        );
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let cert = params.signed_by(&key, &issuer).expect("server certificate");
        Self {
            ca_pem: ca_cert.pem(),
            server_chain: vec![cert.der().clone()],
            server_key_der: key.serialize_der(),
        }
    }

    /// TLS 1.3 acceptor presenting the server certificate, with the given ALPN protocols.
    pub fn acceptor(&self, alpn: &[&[u8]]) -> TlsAcceptor {
        self.acceptor_with_versions(alpn, &[&rustls::version::TLS13])
    }

    /// Acceptor speaking only the given TLS versions (e.g. a legacy TLS 1.2-only server).
    pub fn acceptor_with_versions(
        &self,
        alpn: &[&[u8]],
        versions: &[&'static rustls::SupportedProtocolVersion],
    ) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.server_key_der.clone()));
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .expect("supported TLS versions")
            .with_no_client_auth()
            .with_single_cert(self.server_chain.clone(), key)
            .expect("server certificate accepted");
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        TlsAcceptor::from(Arc::new(config))
    }
}

// -------------------------------------------------------------------------------------------------
// Fake relay
// -------------------------------------------------------------------------------------------------

/// One frame the relay received from a client (decoded).
#[derive(Debug, Clone)]
pub struct Received {
    pub conn: u64,
    pub role: Option<ClientRole>,
    pub username: Option<String>,
    pub header: FrameHeader,
    pub message: Message,
}

/// Something the relay sends on a connection's writer.
enum Out {
    Frame(Frame),
    /// Write everything queued before, then close (TLS close_notify) and end the connection.
    Close,
}

/// Control handle of one relay connection.
struct ConnCtl {
    out: mpsc::Sender<Out>,
    stalled: watch::Sender<bool>,
    read_paused: watch::Sender<bool>,
    /// The relay holds back what it writes on this connection (it keeps reading).
    write_held: watch::Sender<bool>,
    kill: watch::Sender<bool>,
}

impl ConnCtl {
    fn send(&self, frame: Frame) {
        // Bounded; a test relay never queues anywhere near the capacity.
        if self.out.try_send(Out::Frame(frame)).is_err() {
            tracing::warn!(component = "fake-relay", "outbound queue full or closed");
        }
    }

    fn close(&self) {
        let _ = self.out.try_send(Out::Close);
    }
}

#[derive(Clone)]
struct Account {
    id: [u8; 16],
    password: String,
    role: ClientRole,
    display_name: String,
}

struct Slot {
    username: String,
    display_name: String,
    since_ns: u64,
    /// The newest ticket (sent in the latest SESSION_INFO).
    ticket: String,
    /// Earlier tickets the engineer may still hold, oldest first, each with when the ticket that
    /// replaced it was issued (honoured only within [`Inner::ticket_grace`]); at most
    /// [`MAX_PREVIOUS_TICKETS`].
    previous: VecDeque<(String, Instant)>,
    /// The engineer's connection, `None` while it is dropped (link `Reconnecting`).
    conn: Option<u64>,
}

/// Earlier tickets an engineer slot keeps besides its newest one (real relay
/// `MAX_PREVIOUS_TICKETS`).
const MAX_PREVIOUS_TICKETS: usize = 2;

impl Slot {
    /// Replaces the ticket with `fresh` (every SESSION_INFO carries a new one) and returns it,
    /// like the real relay. After a resume (`presented`) that ticket becomes the only earlier one
    /// (using the newest ticket kills every older one). After a driver-restart notice
    /// (`presented` = `None`) the current ticket is added to the earlier ones still inside
    /// `grace` (the notice and a resume reply may both have been lost), the oldest dropped.
    fn rotate(
        &mut self,
        fresh: String,
        presented: Option<String>,
        grace: Option<Duration>,
    ) -> String {
        let old = std::mem::replace(&mut self.ticket, fresh);
        let now = Instant::now();
        match presented {
            Some(presented) => self.previous = VecDeque::from([(presented, now)]),
            None => {
                self.previous
                    .retain(|(_, at)| grace.is_some_and(|g| at.elapsed() < g));
                self.previous.push_back((old, now));
                while self.previous.len() > MAX_PREVIOUS_TICKETS {
                    self.previous.pop_front();
                }
            }
        }
        self.ticket.clone()
    }

    /// Whether `presented` is the current ticket, or an earlier one within `grace`.
    fn accepts(&self, presented: &str, grace: Option<Duration>) -> bool {
        self.ticket == presented
            || self
                .previous
                .iter()
                .any(|(old, at)| old == presented && grace.is_some_and(|g| at.elapsed() < g))
    }
}

/// One refresh token as the relay knows it.
struct RefreshRecord {
    username: String,
    family: u64,
    /// When it was presented (and rotated) the first time, or superseded by the rotation of
    /// another live token of its family.
    rotated_at: Option<Instant>,
}

struct Session {
    id: u64,
    driver: String,
    driver_display: String,
    token: String,
    create: SessionCreate,
    started_at_ns: u64,
    /// Unused code and its expiry (relay clock ns).
    code: Option<(String, u64)>,
    /// Used and replaced codes of this session.
    spent: HashSet<String>,
    engineers: BTreeMap<u64, Slot>,
    driver_conn: Option<u64>,
    /// Latest SCHEMA_NEGOTIATE / TELEMETRY_SNAPSHOT / SCORING_UPDATE for the join replay.
    cache: BTreeMap<u8, Frame>,
    /// Sequence space of frames the relay originates in this session (SNAPSHOT_REQUEST on join).
    relay_sequence: u64,
}

#[derive(Default)]
struct State {
    accounts: HashMap<String, Account>,
    refresh: HashMap<String, RefreshRecord>,
    /// Refresh tokens of each login family, oldest first.
    families: HashMap<u64, VecDeque<String>>,
    revoked_families: HashSet<u64>,
    expired_refresh: HashSet<String>,
    /// Access token → (username, login family).
    access: HashMap<String, (String, u64)>,
    sessions: BTreeMap<u64, Session>,
    joins: HashMap<String, u32>,
    next: u64,
}

impl State {
    fn next(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Issues a refresh token in `family`, deleting the oldest beyond the family cap.
    fn issue_refresh(&mut self, username: &str, family: u64) -> String {
        let token = format!("refresh-{}", self.next());
        self.refresh.insert(
            token.clone(),
            RefreshRecord {
                username: username.to_owned(),
                family,
                rotated_at: None,
            },
        );
        let tokens = self.families.entry(family).or_default();
        tokens.push_back(token.clone());
        while tokens.len() > MAX_ACTIVE_REFRESH_PER_FAMILY {
            if let Some(oldest) = tokens.pop_front() {
                self.refresh.remove(&oldest);
            }
        }
        token
    }

    /// Marks every live (not yet used) refresh token of `family` used or superseded now (the
    /// rotation of a live token): accepted again within `REFRESH_REUSE_GRACE`, reuse afterwards.
    fn supersede_family(&mut self, family: u64) {
        let Some(tokens) = self.families.get(&family) else {
            return;
        };
        for token in tokens {
            if let Some(record) = self.refresh.get_mut(token)
                && record.rotated_at.is_none()
            {
                record.rotated_at = Some(Instant::now());
            }
        }
    }

    /// Revokes a login family: its refresh and access tokens stop working.
    fn revoke_family(&mut self, family: u64) {
        self.revoked_families.insert(family);
        if let Some(tokens) = self.families.remove(&family) {
            for token in tokens {
                self.refresh.remove(&token);
            }
        }
    }
}

/// Protocol state of one relay connection.
#[derive(Debug, Default)]
struct ConnState {
    id: u64,
    role: Option<ClientRole>,
    user: Option<String>,
    bound: Option<u64>,
    /// Engineer slot (connection id of ENGINEER_LIST) held by this connection.
    slot: Option<u64>,
    said_goodbye: bool,
}

#[derive(Default)]
struct Outbox {
    frames: Vec<(u64, Frame)>,
    closes: Vec<u64>,
}

impl Outbox {
    fn push(&mut self, conn: u64, frame: Frame) {
        self.frames.push((conn, frame));
    }
}

struct Inner {
    acceptor: TlsAcceptor,
    ca_pem: String,
    env: Mutex<Environment>,
    limits: Mutex<ServerLimits>,
    epoch: Instant,
    down: AtomicBool,
    rotate_driver_tokens: AtomicBool,
    /// While set, AUTH_OK replies are computed (tokens rotated) but held back.
    hold_auth: AtomicBool,
    /// Held AUTH_OK replies: (connection, frame).
    held_auth: Mutex<Vec<(u64, Frame)>>,
    /// While set, the reply to SESSION_JOIN / engineer SESSION_RESUME is computed (slot taken,
    /// ticket issued) but the connection's writes are held back; reading continues.
    hold_session: AtomicBool,
    /// Engineer SESSION_RESUMEs still to be "lost": processed (ticket rotated), then the
    /// connection drops before the reply leaves the relay.
    lose_session_replies: AtomicU32,
    /// How long an engineer slot's previous ticket is still accepted (default
    /// [`PREVIOUS_TICKET_GRACE`] like the real relay; `None`: only the newest one).
    ticket_grace: Mutex<Option<Duration>>,
    /// A relay bug: SESSION_JOIN is answered with a SESSION_INFO carrying a driver token.
    join_reply_without_ticket: AtomicBool,
    join_limit: Mutex<Option<u32>>,
    state: Mutex<State>,
    conns: Mutex<BTreeMap<u64, Arc<ConnCtl>>>,
    log: Mutex<Vec<Received>>,
    accepted: AtomicU64,
    tls_failures: AtomicU64,
    header_violations: AtomicU64,
    next_conn: AtomicU64,
}

/// The fake relay. Cheap to clone (shared state).
#[derive(Clone)]
pub struct FakeRelay {
    inner: Arc<Inner>,
}

impl FakeRelay {
    /// A relay announcing `env`, with a certificate for `localhost` and 127.0.0.1.
    pub fn new(env: Environment) -> Self {
        Self::with_pki(
            &TestPki::new(&[TEST_HOST, "127.0.0.1"]),
            env,
            &[protocol::ALPN_PROTOCOL],
        )
    }

    /// A relay presenting `pki`'s certificate and offering `alpn`.
    pub fn with_pki(pki: &TestPki, env: Environment, alpn: &[&[u8]]) -> Self {
        Self::with_acceptor(pki, env, pki.acceptor(alpn))
    }

    /// A relay whose TLS side is `acceptor` (e.g. one speaking only TLS 1.2).
    pub fn with_acceptor(pki: &TestPki, env: Environment, acceptor: TlsAcceptor) -> Self {
        let mut state = State::default();
        for (name, role, display) in [
            (DRIVER, ClientRole::Driver, DRIVER_DISPLAY),
            (ENGINEER, ClientRole::Engineer, "Erin Engineer"),
            (ENGINEER2, ClientRole::Engineer, "Eddie Engineer"),
            (ENGINEER3, ClientRole::Engineer, "Emma Engineer"),
        ] {
            let n = state.next();
            state.accounts.insert(
                name.to_owned(),
                Account {
                    id: [u8::try_from(n).expect("few accounts"); 16],
                    password: PASSWORD.to_owned(),
                    role,
                    display_name: display.to_owned(),
                },
            );
        }
        Self {
            inner: Arc::new(Inner {
                acceptor,
                ca_pem: pki.ca_pem.clone(),
                env: Mutex::new(env),
                limits: Mutex::new(ServerLimits {
                    max_payload_bytes: u32::try_from(protocol::MAX_PAYLOAD_BYTES)
                        .expect("fits u32"),
                    heartbeat_interval_ms: 1_000,
                    heartbeat_timeout_ms: 5_000,
                    max_engineers_per_session: protocol::limits::MAX_ENGINEERS_PER_SESSION,
                    session_driver_grace_s: 600,
                }),
                epoch: Instant::now(),
                down: AtomicBool::new(false),
                rotate_driver_tokens: AtomicBool::new(false),
                hold_auth: AtomicBool::new(false),
                held_auth: Mutex::new(Vec::new()),
                hold_session: AtomicBool::new(false),
                lose_session_replies: AtomicU32::new(0),
                ticket_grace: Mutex::new(Some(PREVIOUS_TICKET_GRACE)),
                join_reply_without_ticket: AtomicBool::new(false),
                join_limit: Mutex::new(None),
                state: Mutex::new(state),
                conns: Mutex::new(BTreeMap::new()),
                log: Mutex::new(Vec::new()),
                accepted: AtomicU64::new(0),
                tls_failures: AtomicU64::new(0),
                header_violations: AtomicU64::new(0),
                next_conn: AtomicU64::new(0),
            }),
        }
    }

    /// PEM of the relay's CA (for the development-only extra CA of the client).
    pub fn ca_pem(&self) -> Vec<u8> {
        self.inner.ca_pem.clone().into_bytes()
    }

    /// In-memory transport to this relay.
    pub fn transport(&self) -> DuplexTransport {
        DuplexTransport {
            relay: self.clone(),
        }
    }

    /// Serves real TCP connections on 127.0.0.1 (ephemeral port).
    pub async fn listen_tcp(&self) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind 127.0.0.1");
        let addr = listener.local_addr().expect("local address");
        let relay = self.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if relay.inner.down.load(Ordering::SeqCst) {
                    continue;
                }
                relay.inner.accepted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(Arc::clone(&relay.inner), stream));
            }
        });
        addr
    }

    fn accept_stream(&self, stream: DuplexStream) {
        self.inner.accepted.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(serve(Arc::clone(&self.inner), stream));
    }

    /// Relay clock now (ns since the Unix epoch).
    pub fn now_ns(&self) -> u64 {
        self.inner.now_ns()
    }

    /// The clock offset (relay − client, ns) a client whose monotonic epoch is `client_epoch`
    /// must measure.
    pub fn expected_offset_ns(&self, client_epoch: Instant) -> i128 {
        let base = i128::from(RELAY_BASE_UNIX_NS);
        let diff = if client_epoch >= self.inner.epoch {
            i128::try_from(client_epoch.duration_since(self.inner.epoch).as_nanos()).expect("small")
        } else {
            -i128::try_from(self.inner.epoch.duration_since(client_epoch).as_nanos())
                .expect("small")
        };
        base + diff
    }

    pub fn set_env(&self, env: Environment) {
        *self.inner.env.lock() = env;
    }

    /// Refuse new connections (`true`), as if the relay were down or unreachable.
    pub fn set_down(&self, down: bool) {
        self.inner.down.store(down, Ordering::SeqCst);
    }

    pub fn is_down(&self) -> bool {
        self.inner.down.load(Ordering::SeqCst)
    }

    /// Every open connection stops reading and writing, keeping its socket open.
    pub fn stall_connections(&self) {
        for ctl in self.inner.conns.lock().values() {
            ctl.stalled.send_replace(true);
        }
    }

    /// Every open connection stops (`true`) or resumes (`false`) reading from its client.
    pub fn pause_reading(&self, paused: bool) {
        for ctl in self.inner.conns.lock().values() {
            ctl.read_paused.send_replace(paused);
        }
    }

    /// Drops every open connection abruptly (no DISCONNECT, no close_notify): a network blip.
    pub fn disconnect_all(&self) {
        for ctl in self.inner.conns.lock().values() {
            ctl.kill.send_replace(true);
        }
    }

    /// Number of open connections.
    pub fn open_connections(&self) -> usize {
        self.inner.conns.lock().len()
    }

    /// While `hold` is set, AUTH is processed (refresh tokens rotate at once) but the AUTH_OK replies
    /// are held back; clearing it sends the held replies to the connections that are still open.
    pub fn hold_auth_replies(&self, hold: bool) {
        self.inner.hold_auth.store(hold, Ordering::SeqCst);
        if !hold {
            let held = std::mem::take(&mut *self.inner.held_auth.lock());
            let conns = self.inner.conns.lock();
            for (conn, frame) in held {
                if let Some(ctl) = conns.get(&conn) {
                    ctl.send(frame);
                }
            }
        }
    }

    /// Sends only the newest held AUTH reply (if its connection is still open) and keeps holding
    /// the others, so overlapping AUTHs' AUTH_OKs reach the client out of order.
    pub fn release_newest_held_auth_reply(&self) {
        let newest = self.inner.held_auth.lock().pop();
        if let Some((conn, frame)) = newest
            && let Some(ctl) = self.inner.conns.lock().get(&conn)
        {
            ctl.send(frame);
        }
    }

    /// AUTH replies currently held back.
    pub fn held_auth_replies(&self) -> usize {
        self.inner.held_auth.lock().len()
    }

    /// While `hold` is set, SESSION_JOIN and engineer SESSION_RESUME are processed (the slot is
    /// taken, the ticket issued) but the replying connection's writes are held back, while the
    /// relay keeps reading it. Clearing it releases every held connection.
    pub fn hold_session_replies(&self, hold: bool) {
        self.inner.hold_session.store(hold, Ordering::SeqCst);
        if !hold {
            for ctl in self.inner.conns.lock().values() {
                ctl.write_held.send_replace(false);
            }
        }
    }

    /// The next `n` engineer SESSION_RESUMEs are processed (the ticket is rotated) but their
    /// connection drops before the reply leaves the relay: a SESSION_INFO lost in a second blip.
    pub fn lose_next_session_replies(&self, n: u32) {
        self.inner.lose_session_replies.store(n, Ordering::SeqCst);
    }

    /// How long an engineer slot's previous ticket stays valid (default [`PREVIOUS_TICKET_GRACE`];
    /// `None`: only the newest, like the relay before the grace existed).
    pub fn set_ticket_grace(&self, grace: Option<Duration>) {
        *self.inner.ticket_grace.lock() = grace;
    }

    /// Simulates a relay bug: joins are answered with a SESSION_INFO without an engineer ticket
    /// (the slot is still taken).
    pub fn set_join_reply_without_ticket(&self, broken: bool) {
        self.inner
            .join_reply_without_ticket
            .store(broken, Ordering::SeqCst);
    }

    /// The payload limit announced in HELLO and enforced on driver stream frames.
    pub fn set_max_payload_bytes(&self, limit: u32) {
        self.inner.limits.lock().max_payload_bytes = limit;
    }

    /// The newest resume ticket of `username`'s engineer slot.
    pub fn engineer_ticket(&self, username: &str) -> Option<String> {
        self.inner
            .state
            .lock()
            .sessions
            .values()
            .flat_map(|s| s.engineers.values())
            .find(|slot| slot.username == username)
            .map(|slot| slot.ticket.clone())
    }

    /// Engineer slots of every session (connected or reconnecting).
    pub fn engineer_slots(&self) -> usize {
        self.inner
            .state
            .lock()
            .sessions
            .values()
            .map(|s| s.engineers.len())
            .sum()
    }

    /// Revokes every login (refresh and access tokens of every family stop working).
    pub fn revoke_refresh_tokens(&self) {
        let mut st = self.inner.state.lock();
        let families: Vec<u64> = st.families.keys().copied().collect();
        for family in families {
            st.revoke_family(family);
        }
        st.refresh.clear();
    }

    /// Marks every refresh token as expired (AUTH with one gets `TokenExpired`).
    pub fn expire_refresh_tokens(&self) {
        let mut st = self.inner.state.lock();
        let tokens: Vec<String> = st.refresh.drain().map(|(t, _)| t).collect();
        st.expired_refresh.extend(tokens);
    }

    /// Revokes every access token.
    pub fn revoke_access_tokens(&self) {
        self.inner.state.lock().access.clear();
    }

    /// Every engineer resume ticket dies, superseded ones included; open engineer connections
    /// stay.
    pub fn kill_tickets(&self) {
        let mut st = self.inner.state.lock();
        for s in st.sessions.values_mut() {
            for slot in s.engineers.values_mut() {
                slot.ticket = format!("dead-{}", slot.ticket);
                slot.previous.clear();
            }
        }
    }

    /// The relay forgets every session (e.g. the grace period elapsed).
    pub fn forget_sessions(&self) {
        self.inner.state.lock().sessions.clear();
    }

    /// Every unused connection code expires now.
    pub fn expire_codes(&self) {
        let now = self.now_ns();
        let mut st = self.inner.state.lock();
        for s in st.sessions.values_mut() {
            if let Some((_, expires)) = s.code.as_mut() {
                *expires = now;
            }
        }
    }

    /// Allows at most `limit` SESSION_JOIN attempts per account (`RateLimited` afterwards).
    pub fn set_join_limit(&self, limit: Option<u32>) {
        *self.inner.join_limit.lock() = limit;
    }

    /// Rotate the driver's resume token on every driver resume.
    pub fn set_rotate_driver_tokens(&self, rotate: bool) {
        self.inner
            .rotate_driver_tokens
            .store(rotate, Ordering::SeqCst);
    }

    /// Rotates the driver's resume token of `session_id` now and announces it to the connected
    /// driver with a SESSION_INFO (§6: clients keep the credential of the newest SESSION_INFO).
    pub fn rotate_driver_token_now(&self, session_id: u64) -> String {
        let mut out = Outbox::default();
        let token = {
            let mut st = self.inner.state.lock();
            let token = format!("driver-token-{}", st.next());
            if let Some(s) = st.sessions.get_mut(&session_id) {
                s.token = token.clone();
                if let Some(d) = s.driver_conn {
                    let info = self
                        .inner
                        .session_info(s, ResumeCredential::DriverToken(Secret::new(token.clone())));
                    out.push(
                        d,
                        self.inner.control(Message::SessionInfo(info), session_id),
                    );
                }
            }
            token
        };
        self.inner.flush(out);
        token
    }

    /// Sends `frame` to every driver connection bound to a session.
    pub fn send_to_drivers(&self, frame: &Frame) {
        let ids: Vec<u64> = self
            .inner
            .state
            .lock()
            .sessions
            .values()
            .filter_map(|s| s.driver_conn)
            .collect();
        let conns = self.inner.conns.lock();
        for id in ids {
            if let Some(ctl) = conns.get(&id) {
                ctl.send(frame.clone());
            }
        }
    }

    /// The driver's resume token of `session_id` (as the relay knows it).
    pub fn driver_token(&self, session_id: u64) -> Option<String> {
        self.inner
            .state
            .lock()
            .sessions
            .get(&session_id)
            .map(|s| s.token.clone())
    }

    /// Ids of the live sessions.
    pub fn sessions(&self) -> Vec<u64> {
        self.inner.state.lock().sessions.keys().copied().collect()
    }

    /// Everything received so far.
    pub fn received(&self) -> Vec<Received> {
        self.inner.log.lock().clone()
    }

    /// Messages of type `msg_type` received from `username`'s connections.
    pub fn received_from(&self, username: &str, msg_type: MsgType) -> Vec<Received> {
        self.inner
            .log
            .lock()
            .iter()
            .filter(|r| r.username.as_deref() == Some(username) && r.message.msg_type() == msg_type)
            .cloned()
            .collect()
    }

    /// Number of client HELLOs received for `role` (one per connection attempt that got that far).
    pub fn hellos(&self, role: ClientRole) -> usize {
        self.inner
            .log
            .lock()
            .iter()
            .filter(|r| matches!(&r.message, Message::Hello(h) if h.role == role))
            .count()
    }

    /// Number of AUTH messages received (any account).
    pub fn auth_attempts(&self) -> usize {
        self.inner
            .log
            .lock()
            .iter()
            .filter(|r| r.message.msg_type() == MsgType::Auth)
            .count()
    }

    /// Connections accepted so far (TLS handshakes started).
    pub fn connections_accepted(&self) -> u64 {
        self.inner.accepted.load(Ordering::SeqCst)
    }

    /// TLS handshakes that failed on the relay side.
    pub fn tls_failures(&self) -> u64 {
        self.inner.tls_failures.load(Ordering::SeqCst)
    }

    /// Client frames that violated the relay-side header rules (`check_client_frame`).
    pub fn header_violations(&self) -> u64 {
        self.inner.header_violations.load(Ordering::SeqCst)
    }
}

impl Inner {
    fn now_ns(&self) -> u64 {
        let since = Instant::now().saturating_duration_since(self.epoch);
        RELAY_BASE_UNIX_NS + u64::try_from(since.as_nanos()).expect("test durations fit u64")
    }

    fn mono_ns(&self) -> u64 {
        u64::try_from(
            Instant::now()
                .saturating_duration_since(self.epoch)
                .as_nanos(),
        )
        .expect("test durations fit u64")
    }

    fn control(&self, message: Message, session_id: u64) -> Frame {
        message
            .to_frame(&FrameMeta::control(session_id, self.mono_ns()))
            .expect("relay control frame")
    }

    fn error(&self, code: ErrorCode, fatal: bool) -> Frame {
        self.control(
            Message::Error(ErrorMsg {
                code,
                message: format!("fake relay: {code:?}"),
                fatal,
            }),
            NO_SESSION,
        )
    }

    fn flush(&self, out: Outbox) {
        let conns = self.conns.lock();
        for (id, frame) in out.frames {
            if let Some(ctl) = conns.get(&id) {
                ctl.send(frame);
            }
        }
        for id in out.closes {
            if let Some(ctl) = conns.get(&id) {
                ctl.close();
            }
        }
    }

    fn new_code(st: &mut State) -> String {
        // Odd multiplier: a bijection on the low 40 bits, so codes never repeat.
        let mut x = st.next().wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (0..CONNECT_CODE_LEN)
            .map(|_| {
                let c = char::from(CONNECT_CODE_ALPHABET[usize::try_from(x & 31).expect("5 bits")]);
                x >>= 5;
                c
            })
            .collect()
    }

    fn session_info(&self, s: &Session, resume: ResumeCredential) -> SessionInfo {
        SessionInfo {
            session_id: s.id,
            driver_display_name: s.driver_display.clone(),
            driver_username: s.driver.clone(),
            car: s.create.identity.vehicle_name.clone(),
            vehicle_class: s.create.identity.class_name.clone(),
            track: s.create.identity.track_name.clone(),
            session_type: s.create.identity.session_type,
            started_at_ns: s.started_at_ns,
            schema_version: s.create.schema_version,
            resume,
        }
    }

    fn connect_code(&self, s: &Session) -> Message {
        Message::ConnectCode(ConnectCode {
            code: s.code.as_ref().map(|(code, expires)| IssuedCode {
                code: Secret::new(code.clone()),
                expires_at_ns: *expires,
            }),
            engineers_connected: u16::try_from(s.engineers.len()).expect("few engineers"),
            engineers_max: self.limits.lock().max_engineers_per_session,
        })
    }

    fn engineer_list(s: &Session) -> Message {
        Message::EngineerList(EngineerList {
            engineers: s
                .engineers
                .iter()
                .map(|(id, slot)| EngineerEntry {
                    connection_id: *id,
                    username: slot.username.clone(),
                    display_name: slot.display_name.clone(),
                    connected_since_ns: slot.since_ns,
                    link: if slot.conn.is_some() {
                        EngineerLink::Healthy
                    } else {
                        EngineerLink::Reconnecting
                    },
                })
                .collect(),
        })
    }

    /// CONNECT_CODE + ENGINEER_LIST to the session's driver (if connected).
    fn notify_driver(&self, s: &Session, out: &mut Outbox) {
        if let Some(d) = s.driver_conn {
            out.push(d, self.control(Self::engineer_list(s), s.id));
            out.push(d, self.control(self.connect_code(s), s.id));
        }
    }

    fn max_engineers(&self) -> usize {
        usize::from(self.limits.lock().max_engineers_per_session)
    }

    /// Issues a fresh code if there is none and a slot is free.
    fn refill_code(&self, st: &mut State, session_id: u64, now: u64) {
        let free = st
            .sessions
            .get(&session_id)
            .is_some_and(|s| s.code.is_none() && s.engineers.len() < self.max_engineers());
        if free {
            let code = Self::new_code(st);
            let expires = now + protocol::timing::CONNECT_CODE_TTL_S * 1_000_000_000;
            if let Some(s) = st.sessions.get_mut(&session_id) {
                s.code = Some((code, expires));
            }
        }
    }

    /// Cached frames replayed to an engineer after it joined or resumed (§6).
    fn replay(&self, s: &Session, to: u64, now: u64, out: &mut Outbox) {
        for t in [
            MsgType::SchemaNegotiate,
            MsgType::TelemetrySnapshot,
            MsgType::ScoringUpdate,
        ] {
            if let Some(frame) = s.cache.get(&t.id()) {
                out.push(
                    to,
                    frame.with_relay_trailer(&RelayTrailer {
                        relay_rx_ns: now,
                        relay_tx_ns: now,
                        relay_dropped: 0,
                        replayed: true,
                    }),
                );
            }
        }
    }

    /// Handles one client frame; `false` ends the connection.
    fn handle(&self, conn: &mut ConnState, frame: &Frame) -> bool {
        let frame = frame.detach();
        let from = conn.role.map_or(Endpoint::Engineer, Endpoint::from);
        if let Err(e) = check_client_frame(frame.header(), from, conn.bound) {
            tracing::warn!(component = "fake-relay", error = %e, "client frame violates header rules");
            self.header_violations.fetch_add(1, Ordering::SeqCst);
            return true;
        }
        let message = match Message::from_frame(&frame) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(component = "fake-relay", error = %e, "undecodable client frame");
                self.header_violations.fetch_add(1, Ordering::SeqCst);
                return true;
            }
        };
        self.log.lock().push(Received {
            conn: conn.id,
            role: conn.role,
            username: conn.user.clone(),
            header: *frame.header(),
            message: message.clone(),
        });
        let engineer_bind = matches!(
            &message,
            Message::SessionJoin(_) | Message::SessionResume(SessionResume::Engineer { .. })
        );
        let lose_reply = matches!(
            &message,
            Message::SessionResume(SessionResume::Engineer { .. })
        ) && self
            .lose_session_replies
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        let mut out = Outbox::default();
        let keep = self.dispatch(conn, &frame, message, &mut out);
        if lose_reply {
            // Processed, but the connection drops before the reply leaves the relay.
            out.frames.retain(|(to, _)| *to != conn.id);
            if let Some(ctl) = self.conns.lock().get(&conn.id) {
                ctl.kill.send_replace(true);
            }
        } else if engineer_bind
            && self.hold_session.load(Ordering::SeqCst)
            && let Some(ctl) = self.conns.lock().get(&conn.id)
        {
            ctl.write_held.send_replace(true);
        }
        self.flush(out);
        keep
    }

    fn dispatch(
        &self,
        conn: &mut ConnState,
        frame: &Frame,
        message: Message,
        out: &mut Outbox,
    ) -> bool {
        let me = conn.id;
        let now = self.now_ns();
        match message {
            Message::Hello(hello) => {
                conn.role = Some(hello.role);
                let reply = Message::ServerHello(ServerHello {
                    relay_version: RELAY_VERSION.to_owned(),
                    protocol: ProtocolVersion::CURRENT,
                    server_time_ns: now,
                    limits: *self.limits.lock(),
                    env: *self.env.lock(),
                });
                out.push(me, self.control(reply, NO_SESSION));
            }
            Message::Auth(auth) => match self.authenticate(&auth, now) {
                Ok(ok) => {
                    conn.user = Some(ok.username.clone());
                    let reply = self.control(Message::AuthOk(ok), NO_SESSION);
                    if self.hold_auth.load(Ordering::SeqCst) {
                        self.held_auth.lock().push((me, reply));
                    } else {
                        out.push(me, reply);
                    }
                }
                Err(code) => out.push(me, self.error(code, false)),
            },
            Message::SessionCreate(create) => self.session_create(conn, create, now, out),
            Message::SessionResume(SessionResume::Driver {
                session_id,
                resume_token,
                new_sequence_space,
            }) => self.resume_driver(
                conn,
                session_id,
                &resume_token,
                new_sequence_space,
                now,
                out,
            ),
            Message::SessionResume(SessionResume::Engineer { session_id, ticket }) => {
                self.resume_engineer(conn, session_id, &ticket, now, out);
            }
            Message::SessionJoin(join) => self.session_join(conn, join.code.expose(), now, out),
            Message::ConnectCodeRequest(_) => self.code_request(conn, now, out),
            Message::EngineerKick(kick) => self.kick(conn, kick.connection_id, now, out),
            Message::SessionClose(_) => self.end_session(conn, out),
            Message::Ping(ping) => {
                let pong = Message::Pong(Pong {
                    id: ping.id,
                    t0: ping.t0,
                    t1: now,
                    t2: self.now_ns(),
                });
                out.push(me, self.control(pong, conn.bound.unwrap_or(NO_SESSION)));
            }
            Message::Disconnect(_) => {
                conn.said_goodbye = true;
                return false;
            }
            Message::SnapshotRequest(_) if conn.role == Some(ClientRole::Engineer) => {
                let st = self.state.lock();
                if let Some(d) = conn
                    .bound
                    .and_then(|sid| st.sessions.get(&sid))
                    .and_then(|s| s.driver_conn)
                {
                    out.push(d, frame.clone());
                }
            }
            m if m.msg_type().is_stream() && conn.role == Some(ClientRole::Driver) => {
                self.driver_stream(conn, frame, now, out);
            }
            _ => {}
        }
        true
    }

    /// AUTH with the real relay's token rules (see the module docs). A password or refresh-token
    /// AUTH_OK carries a new refresh token of the login's family; an access-token AUTH_OK carries
    /// none (empty token, expiry 0) and changes no refresh token.
    fn authenticate(&self, auth: &Auth, now: u64) -> Result<AuthOk, ErrorCode> {
        let mut guard = self.state.lock();
        let st = &mut *guard;
        let (username, family, issue_refresh) = match auth {
            Auth::Password { username, password } => match st.accounts.get(username) {
                Some(a) if a.password == password.expose() => {
                    let family = st.next();
                    (username.clone(), family, true)
                }
                _ => return Err(ErrorCode::AuthFailed),
            },
            Auth::RefreshToken { token } => {
                if st.expired_refresh.contains(token.expose()) {
                    return Err(ErrorCode::TokenExpired);
                }
                let Some(record) = st.refresh.get(token.expose()) else {
                    return Err(ErrorCode::AuthFailed);
                };
                let (family, username) = (record.family, record.username.clone());
                if st.revoked_families.contains(&family) {
                    return Err(ErrorCode::AuthFailed);
                }
                match record.rotated_at {
                    // A live token (the client provably holds it): rotated, and every other live
                    // token of the family is superseded (one live refresh token per login).
                    None => st.supersede_family(family),
                    // The client missed the AUTH_OK with the successor: accepted again, nothing
                    // superseded (this may be a delayed duplicate).
                    Some(at) if at.elapsed() <= REFRESH_REUSE_GRACE => {}
                    // Reuse after the grace: theft detection revokes the whole login.
                    Some(_) => {
                        st.revoke_family(family);
                        return Err(ErrorCode::AuthFailed);
                    }
                }
                (username, family, true)
            }
            // No refresh token is issued and none is touched: the client keeps the one it holds.
            Auth::AccessToken { token } => match st.access.get(token.expose()) {
                Some((u, family)) if !st.revoked_families.contains(family) => {
                    (u.clone(), *family, false)
                }
                _ => return Err(ErrorCode::AuthFailed),
            },
        };
        let (refresh, refresh_expires_at_ns) = if issue_refresh {
            (
                st.issue_refresh(&username, family),
                now + duration_ns(REFRESH_TTL),
            )
        } else {
            (String::new(), 0)
        };
        let access = format!("access-{}", st.next());
        st.access.insert(access.clone(), (username.clone(), family));
        let account = st
            .accounts
            .get(&username)
            .cloned()
            .ok_or(ErrorCode::AuthFailed)?;
        Ok(AuthOk {
            account: AccountId(account.id),
            username,
            display_name: account.display_name,
            role: account.role,
            access_token: Secret::new(access),
            refresh_token: Secret::new(refresh),
            access_expires_at_ns: now + duration_ns(ACCESS_TTL),
            refresh_expires_at_ns,
        })
    }

    fn authenticated_as(&self, conn: &ConnState, role: ClientRole) -> Option<(String, Account)> {
        let user = conn.user.clone()?;
        let account = self.state.lock().accounts.get(&user).cloned()?;
        (account.role == role && conn.role == Some(role)).then_some((user, account))
    }

    fn session_create(
        &self,
        conn: &mut ConnState,
        create: SessionCreate,
        now: u64,
        out: &mut Outbox,
    ) {
        let me = conn.id;
        let Some((user, account)) = self.authenticated_as(conn, ClientRole::Driver) else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let mut st = self.state.lock();
        let id = st.next();
        let token = format!("driver-token-{}", st.next());
        st.sessions.insert(
            id,
            Session {
                id,
                driver: user,
                driver_display: account.display_name,
                token: token.clone(),
                create,
                started_at_ns: now,
                code: None,
                spent: HashSet::new(),
                engineers: BTreeMap::new(),
                driver_conn: Some(me),
                cache: BTreeMap::new(),
                relay_sequence: 0,
            },
        );
        self.refill_code(&mut st, id, now);
        conn.bound = Some(id);
        let s = st.sessions.get(&id).expect("just inserted");
        out.push(
            me,
            self.control(
                Message::SessionInfo(
                    self.session_info(s, ResumeCredential::DriverToken(Secret::new(token))),
                ),
                id,
            ),
        );
        out.push(me, self.control(self.connect_code(s), id));
        out.push(me, self.control(Self::engineer_list(s), id));
    }

    fn resume_driver(
        &self,
        conn: &mut ConnState,
        session_id: u64,
        token: &Secret,
        new_space: bool,
        now: u64,
        out: &mut Outbox,
    ) {
        let me = conn.id;
        let Some((user, _)) = self.authenticated_as(conn, ClientRole::Driver) else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let grace = *self.ticket_grace.lock();
        let mut st = self.state.lock();
        let rotated = self
            .rotate_driver_tokens
            .load(Ordering::SeqCst)
            .then(|| format!("driver-token-{}", st.next()));
        // A new sequence space is announced to every connected engineer with a SESSION_INFO, and
        // every SESSION_INFO carries a new ticket.
        let mut fresh_tickets = (0..self.max_engineers())
            .map(|_| format!("ticket-{}", st.next()))
            .collect::<Vec<_>>()
            .into_iter();
        let Some(s) = st.sessions.get_mut(&session_id) else {
            out.push(me, self.error(ErrorCode::SessionNotFound, false));
            return;
        };
        if s.driver != user || s.token != token.expose() {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        }
        if let Some(t) = rotated {
            s.token = t;
        }
        s.driver_conn = Some(me);
        conn.bound = Some(session_id);
        if new_space {
            // The relay resets the stream for every engineer before any frame of the new space.
            s.cache.clear();
            let connected: Vec<(u64, u64)> = s
                .engineers
                .iter()
                .filter_map(|(id, slot)| slot.conn.map(|c| (*id, c)))
                .collect();
            for (slot_id, c) in connected {
                let (Some(fresh), Some(slot)) =
                    (fresh_tickets.next(), s.engineers.get_mut(&slot_id))
                else {
                    continue;
                };
                // The engineer holds the current ticket, or an earlier one whose SESSION_INFO was
                // lost: all of them stay valid within their grace.
                let ticket = slot.rotate(fresh, None, grace);
                let info =
                    self.session_info(s, ResumeCredential::EngineerTicket(Secret::new(ticket)));
                out.push(c, self.control(Message::SessionInfo(info), session_id));
            }
        }
        // Like the real relay: the unused code is re-sent unchanged (same code, same expiry) while
        // it is unexpired; an expired one is replaced by a fresh code.
        if let Some(s) = st.sessions.get_mut(&session_id)
            && s.code.as_ref().is_some_and(|(_, expires)| *expires <= now)
            && let Some((expired, _)) = s.code.take()
        {
            s.spent.insert(expired);
        }
        self.refill_code(&mut st, session_id, now);
        let s = st.sessions.get(&session_id).expect("exists");
        let info = self.session_info(
            s,
            ResumeCredential::DriverToken(Secret::new(s.token.clone())),
        );
        out.push(me, self.control(Message::SessionInfo(info), session_id));
        out.push(me, self.control(self.connect_code(s), session_id));
        out.push(me, self.control(Self::engineer_list(s), session_id));
    }

    fn session_join(&self, conn: &mut ConnState, code: &str, now: u64, out: &mut Outbox) {
        let me = conn.id;
        let Some((user, account)) = self.authenticated_as(conn, ClientRole::Engineer) else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let mut st = self.state.lock();
        let attempts = {
            let a = st.joins.entry(user.clone()).or_default();
            *a += 1;
            *a
        };
        if self.join_limit.lock().is_some_and(|limit| attempts > limit) {
            out.push(me, self.error(ErrorCode::RateLimited, false));
            return;
        }
        let max = self.max_engineers();
        let found = st
            .sessions
            .values()
            .find(|s| s.code.as_ref().is_some_and(|(c, _)| c == code) || s.spent.contains(code));
        let Some(found) = found else {
            out.push(me, self.error(ErrorCode::CodeInvalid, false));
            return;
        };
        let session_id = found.id;
        if found.spent.contains(code) {
            let code = if found.engineers.len() >= max {
                ErrorCode::SessionFull
            } else {
                ErrorCode::CodeInvalid
            };
            out.push(me, self.error(code, false));
            return;
        }
        if found
            .code
            .as_ref()
            .is_some_and(|(_, expires)| *expires <= now)
        {
            out.push(me, self.error(ErrorCode::CodeExpired, false));
            return;
        }
        if found.engineers.len() >= max {
            out.push(me, self.error(ErrorCode::SessionFull, false));
            return;
        }
        let slot_id = st.next();
        let ticket = format!("ticket-{}", st.next());
        let s = st.sessions.get_mut(&session_id).expect("found");
        s.code = None;
        s.spent.insert(code.to_owned());
        s.engineers.insert(
            slot_id,
            Slot {
                username: user,
                display_name: account.display_name,
                since_ns: now,
                ticket: ticket.clone(),
                previous: VecDeque::new(),
                conn: Some(me),
            },
        );
        conn.bound = Some(session_id);
        conn.slot = Some(slot_id);
        self.refill_code(&mut st, session_id, now);
        let s = st.sessions.get_mut(&session_id).expect("found");
        let resume = if self.join_reply_without_ticket.load(Ordering::SeqCst) {
            ResumeCredential::DriverToken(Secret::new(ticket))
        } else {
            ResumeCredential::EngineerTicket(Secret::new(ticket))
        };
        let info = self.session_info(s, resume);
        out.push(me, self.control(Message::SessionInfo(info), session_id));
        self.replay(s, me, now, out);
        if let Some(d) = s.driver_conn {
            s.relay_sequence += 1;
            let request = Message::SnapshotRequest(SnapshotRequest {
                reason: SnapshotReason::EngineerJoined,
            })
            .to_frame(&FrameMeta::stream(
                FramePriority::Control,
                session_id,
                s.relay_sequence,
                self.mono_ns(),
            ))
            .expect("snapshot request frame");
            out.push(d, request);
        }
        self.notify_driver(s, out);
    }

    fn resume_engineer(
        &self,
        conn: &mut ConnState,
        session_id: u64,
        ticket: &Secret,
        now: u64,
        out: &mut Outbox,
    ) {
        let me = conn.id;
        let Some((user, _)) = self.authenticated_as(conn, ClientRole::Engineer) else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let grace = *self.ticket_grace.lock();
        let mut st = self.state.lock();
        let fresh = format!("ticket-{}", st.next());
        let Some(s) = st.sessions.get_mut(&session_id) else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let Some((&slot_id, slot)) = s
            .engineers
            .iter_mut()
            .find(|(_, slot)| slot.username == user && slot.accepts(ticket.expose(), grace))
        else {
            out.push(me, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        if let Some(old) = slot.conn.replace(me)
            && old != me
        {
            out.closes.push(old);
        }
        // Like the real relay: the reply carries a new ticket, and the presented one (current or
        // earlier) becomes its only earlier ticket, so a lost reply can be retried within the
        // grace; using the newer ticket kills the older ones.
        let ticket = slot.rotate(fresh, Some(ticket.expose().to_owned()), grace);
        conn.bound = Some(session_id);
        conn.slot = Some(slot_id);
        let info = self.session_info(s, ResumeCredential::EngineerTicket(Secret::new(ticket)));
        out.push(me, self.control(Message::SessionInfo(info), session_id));
        self.replay(s, me, now, out);
        self.notify_driver(s, out);
    }

    fn code_request(&self, conn: &mut ConnState, now: u64, out: &mut Outbox) {
        let (me, Some(sid)) = (conn.id, conn.bound) else {
            out.push(conn.id, self.error(ErrorCode::NotAuthorized, false));
            return;
        };
        let mut st = self.state.lock();
        let max = self.max_engineers();
        if let Some(s) = st.sessions.get_mut(&sid) {
            if let Some((old, _)) = s.code.take() {
                s.spent.insert(old);
            }
            if s.engineers.len() >= max {
                s.code = None;
            }
        }
        self.refill_code(&mut st, sid, now);
        if let Some(s) = st.sessions.get(&sid) {
            out.push(me, self.control(self.connect_code(s), sid));
        }
    }

    fn kick(&self, conn: &mut ConnState, connection_id: u64, now: u64, out: &mut Outbox) {
        let Some(sid) = conn.bound else {
            return;
        };
        let mut st = self.state.lock();
        let Some(s) = st.sessions.get_mut(&sid) else {
            return;
        };
        let Some(slot) = s.engineers.remove(&connection_id) else {
            return;
        };
        if let Some(c) = slot.conn {
            let close = Message::SessionClose(SessionClose {
                session_id: sid,
                reason: SessionCloseReason::RemovedByDriver,
            });
            out.push(c, self.control(close, sid));
            out.closes.push(c);
        }
        // Like the real relay: a kick may mean the code leaked, so the unused one is replaced by a
        // fresh code (a DISCONNECT keeps it, see `connection_closed`).
        if let Some((old, _)) = s.code.take() {
            s.spent.insert(old);
        }
        self.refill_code(&mut st, sid, now);
        if let Some(s) = st.sessions.get(&sid) {
            self.notify_driver(s, out);
        }
    }

    fn end_session(&self, conn: &mut ConnState, out: &mut Outbox) {
        let Some(sid) = conn.bound else {
            return;
        };
        let Some(s) = self.state.lock().sessions.remove(&sid) else {
            return;
        };
        for slot in s.engineers.values() {
            if let Some(c) = slot.conn {
                let close = Message::SessionClose(SessionClose {
                    session_id: sid,
                    reason: SessionCloseReason::DriverStopped,
                });
                out.push(c, self.control(close, sid));
                out.closes.push(c);
            }
        }
    }

    fn driver_stream(&self, conn: &ConnState, frame: &Frame, now: u64, out: &mut Outbox) {
        let Some(sid) = conn.bound else {
            return;
        };
        let mut st = self.state.lock();
        let Some(s) = st.sessions.get_mut(&sid) else {
            return;
        };
        if let Some(
            t @ (MsgType::SchemaNegotiate | MsgType::TelemetrySnapshot | MsgType::ScoringUpdate),
        ) = frame.msg_type()
        {
            s.cache.insert(t.id(), frame.detach());
        }
        let trailer = RelayTrailer {
            relay_rx_ns: now,
            relay_tx_ns: now,
            relay_dropped: 0,
            replayed: false,
        };
        for slot in s.engineers.values() {
            if let Some(c) = slot.conn {
                out.push(c, frame.with_relay_trailer(&trailer));
            }
        }
    }

    /// A connection ended (DISCONNECT, EOF, error or kill).
    fn connection_closed(&self, conn: &ConnState) {
        let mut out = Outbox::default();
        let now = self.now_ns();
        {
            let mut st = self.state.lock();
            if let (Some(sid), Some(slot_id)) = (conn.bound, conn.slot) {
                let mut changed = false;
                if let Some(s) = st.sessions.get_mut(&sid)
                    && let Some(slot) = s.engineers.get_mut(&slot_id)
                    && slot.conn == Some(conn.id)
                {
                    if conn.said_goodbye {
                        // An explicit disconnect frees the slot; the ticket dies with it.
                        s.engineers.remove(&slot_id);
                    } else {
                        // A drop keeps the slot for the ticket (link `Reconnecting`).
                        slot.conn = None;
                    }
                    changed = true;
                }
                if changed {
                    self.refill_code(&mut st, sid, now);
                    if let Some(s) = st.sessions.get(&sid) {
                        self.notify_driver(s, &mut out);
                    }
                }
            } else if let Some(sid) = conn.bound
                && let Some(s) = st.sessions.get_mut(&sid)
                && s.driver_conn == Some(conn.id)
            {
                s.driver_conn = None;
            }
        }
        self.flush(out);
    }
}

fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).expect("test durations fit u64")
}

/// Serves one client connection until it ends.
async fn serve<S>(inner: Arc<Inner>, stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let tls = match inner.acceptor.accept(stream).await {
        Ok(tls) => tls,
        Err(e) => {
            tracing::info!(component = "fake-relay", error = %e, "TLS handshake failed");
            inner.tls_failures.fetch_add(1, Ordering::SeqCst);
            return;
        }
    };
    let (mut reader, mut writer, _stats) = framed_split(tls);
    let id = inner.next_conn.fetch_add(1, Ordering::SeqCst) + 1_000;
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(RELAY_OUT_CAPACITY);
    let ctl = Arc::new(ConnCtl {
        out: out_tx,
        stalled: watch::channel(false).0,
        read_paused: watch::channel(false).0,
        write_held: watch::channel(false).0,
        kill: watch::channel(false).0,
    });
    inner.conns.lock().insert(id, Arc::clone(&ctl));

    let writer_task = {
        let ctl = Arc::clone(&ctl);
        let mut stalled = ctl.stalled.subscribe();
        let mut held = ctl.write_held.subscribe();
        let mut kill = ctl.kill.subscribe();
        tokio::spawn(async move {
            loop {
                let out = tokio::select! {
                    biased;
                    _ = kill.wait_for(|k| *k) => break,
                    out = out_rx.recv() => match out {
                        Some(out) => out,
                        None => break,
                    },
                };
                if *stalled.borrow_and_update() {
                    tokio::select! {
                        _ = kill.wait_for(|k| *k) => break,
                        _ = stalled.wait_for(|s| !*s) => {}
                    }
                }
                if *held.borrow_and_update() {
                    tokio::select! {
                        _ = kill.wait_for(|k| *k) => break,
                        _ = held.wait_for(|h| !*h) => {}
                    }
                }
                match out {
                    Out::Frame(frame) => {
                        if writer.send(&frame).await.is_err() || writer.flush().await.is_err() {
                            break;
                        }
                    }
                    Out::Close => {
                        let _ = writer.close().await;
                        ctl.kill.send_replace(true);
                        break;
                    }
                }
            }
        })
    };

    let mut conn = ConnState {
        id,
        ..ConnState::default()
    };
    let mut kill = ctl.kill.subscribe();
    let mut stalled = ctl.stalled.subscribe();
    let mut paused = ctl.read_paused.subscribe();
    loop {
        if *stalled.borrow_and_update() || *paused.borrow_and_update() {
            tokio::select! {
                _ = kill.wait_for(|k| *k) => break,
                _ = stalled.changed() => {}
                _ = paused.changed() => {}
            }
            continue;
        }
        let received = tokio::select! {
            _ = kill.wait_for(|k| *k) => break,
            _ = stalled.changed() => continue,
            _ = paused.changed() => continue,
            received = reader.recv() => received,
        };
        let Ok(Some(frame)) = received else {
            break;
        };
        if !inner.handle(&mut conn, &frame) {
            break;
        }
    }
    inner.conns.lock().remove(&id);
    inner.connection_closed(&conn);
    if *ctl.kill.borrow() {
        writer_task.abort();
    } else {
        // Write what is queued (e.g. a SESSION_CLOSE), then close.
        ctl.close();
        let _ = timeout(Duration::from_secs(1), writer_task).await;
    }
}

// -------------------------------------------------------------------------------------------------
// In-memory transport
// -------------------------------------------------------------------------------------------------

type DuplexTls = tokio_rustls::client::TlsStream<DuplexStream>;

/// [`TelemetryTransport`] to a [`FakeRelay`] over an in-memory byte stream, with the production
/// TLS client handshake (certificate validation and ALPN check included).
#[derive(Clone)]
pub struct DuplexTransport {
    relay: FakeRelay,
}

impl TelemetryTransport for DuplexTransport {
    type Reader = StreamFrameReader<ReadHalf<DuplexTls>>;
    type Writer = StreamFrameWriter<WriteHalf<DuplexTls>>;

    async fn connect(
        &self,
        endpoint: &RelayEndpoint,
    ) -> Result<TransportConnection<Self::Reader, Self::Writer>, ConnectError> {
        if self.relay.is_down() {
            return Err(ConnectError::Tcp {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port())),
                source: io::Error::from(io::ErrorKind::ConnectionRefused),
            });
        }
        let (client, server) = tokio::io::duplex(DUPLEX_BUFFER);
        self.relay.accept_stream(server);
        let tls = TcpTlsTelemetryTransport::new()
            .handshake_tls(endpoint, client)
            .await?;
        let (tls_version, cipher_suite) = TcpTlsTelemetryTransport::tls_details(&tls);
        let (reader, writer, stats) = framed_split(tls);
        Ok(TransportConnection {
            reader,
            writer,
            stats,
            info: ConnectionInfo {
                tls_version,
                cipher_suite,
                ..ConnectionInfo::default()
            },
        })
    }
}

// -------------------------------------------------------------------------------------------------
// Client helpers
// -------------------------------------------------------------------------------------------------

/// A development-environment client configuration trusting `relay`'s CA, with the monotonic epoch
/// taken from Tokio's (possibly paused) clock.
pub fn dev_config(relay: &FakeRelay) -> ClientConfig {
    let endpoint = RelayEndpoint::new(TEST_HOST, TEST_PORT, "test", Environment::Development)
        .expect("endpoint")
        .with_extra_ca(ExtraCa::Pem(relay.ca_pem()))
        .expect("development may trust a test CA");
    config_for(endpoint)
}

/// Client configuration for `endpoint` with the epoch from Tokio's clock.
pub fn config_for(endpoint: RelayEndpoint) -> ClientConfig {
    ClientConfig::new(
        endpoint,
        "0.1.0-test",
        "transport-test-client",
        Instant::now().into_std(),
    )
}

/// A fresh "app installation": its own credential store and auth manager.
pub fn new_app(config: &ClientConfig) -> (Arc<MemoryCredentialStore>, Arc<AuthManager>) {
    let store = Arc::new(MemoryCredentialStore::new());
    let auth = Arc::new(AuthManager::new(store.clone(), &config.endpoint));
    (store, auth)
}

/// The same installation after an app restart: the stored credentials, a new auth manager.
pub fn restart_app(store: &Arc<MemoryCredentialStore>, config: &ClientConfig) -> Arc<AuthManager> {
    Arc::new(AuthManager::new(store.clone(), &config.endpoint))
}

/// SESSION_CREATE data of the test driver.
pub fn session_create() -> SessionCreate {
    SessionCreate {
        identity: SessionIdentity {
            vehicle_name: Some("Porsche 963 #6".to_owned()),
            vehicle_model: Some("963".to_owned()),
            vehicle_class: None,
            class_name: Some("Hypercar".to_owned()),
            track_name: Some("Circuit de la Sarthe".to_owned()),
            session_type: Some(SessionType::Race(1)),
            game_version: Some(14_200),
        },
        lmu_version: Some("1.4.2.0".to_owned()),
        app_version: "0.1.0-test".to_owned(),
        schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        capabilities: VehicleCapabilities::default(),
    }
}

/// SCHEMA_NEGOTIATE with an empty capability map.
pub fn schema_negotiate() -> Message {
    Message::SchemaNegotiate(SchemaNegotiate {
        schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        capabilities: CapabilityMap::default(),
        vehicle_capabilities: VehicleCapabilities::default(),
    })
}

/// A valid TLV block (PROTOCOL.md §4) whose `capture_mono_ns` is `tag`, with `records` f64
/// records of a channel id receivers skip.
pub fn tlv(tag: u64, records: u16) -> TelemetryPayload {
    let mut buf =
        Vec::with_capacity(telemetry_model::codec::TLV_HEADER_BYTES + usize::from(records) * 11);
    buf.extend_from_slice(&telemetry_model::TELEMETRY_SCHEMA_VERSION.to_le_bytes());
    buf.extend_from_slice(&tag.to_le_bytes());
    buf.extend_from_slice(&telemetry_model::codec::RELAY_NS_UNKNOWN.to_le_bytes());
    buf.extend_from_slice(&f64::NAN.to_le_bytes());
    buf.extend_from_slice(&records.to_le_bytes());
    for i in 0..records {
        buf.extend_from_slice(&TEST_CHANNEL.to_le_bytes());
        buf.push(TAG_VALID_F64);
        buf.extend_from_slice(&f64::from(i).to_le_bytes());
    }
    TelemetryPayload::new(Bytes::from(buf)).expect("TLV block")
}

/// The tag of a TLV block made by [`tlv`].
pub fn tlv_tag(payload: &TelemetryPayload) -> u64 {
    u64::from_le_bytes(
        payload.as_bytes()[TLV_TAG_RANGE]
            .try_into()
            .expect("8 bytes"),
    )
}

/// The tag of a TELEMETRY_DELTA / TELEMETRY_SNAPSHOT / WEATHER_UPDATE message.
pub fn message_tag(message: &Message) -> Option<u64> {
    match message {
        Message::TelemetryDelta(p) | Message::TelemetrySnapshot(p) | Message::WeatherUpdate(p) => {
            Some(tlv_tag(p))
        }
        _ => None,
    }
}

/// Waits (on the paused clock) until the status satisfies `pred`.
pub async fn wait_status(
    rx: &mut watch::Receiver<ConnectionStatus>,
    what: &str,
    pred: impl FnMut(&ConnectionStatus) -> bool,
) -> ConnectionStatus {
    let outcome = match timeout(WAIT, rx.wait_for(pred)).await {
        Ok(Ok(status)) => Ok(status.clone()),
        Ok(Err(_)) => Err("status channel closed"),
        Err(_) => Err("timed out"),
    };
    match outcome {
        Ok(status) => status,
        Err(why) => panic!("{why} waiting for {what}; last status {:?}", *rx.borrow()),
    }
}

/// Waits for the connection state `state`.
pub async fn wait_state(
    rx: &mut watch::Receiver<ConnectionStatus>,
    state: ConnectionState,
) -> ConnectionStatus {
    wait_status(rx, state.name(), |s| s.state == state).await
}

/// Next event of a client, waiting on the paused clock.
pub async fn next_event<E: std::fmt::Debug>(rx: &mut mpsc::Receiver<E>) -> E {
    match timeout(WAIT, rx.recv()).await {
        Ok(Some(event)) => event,
        Ok(None) => panic!("event channel closed"),
        Err(_) => panic!("timed out waiting for an event"),
    }
}

/// Every event currently queued.
pub fn drain_events<E>(rx: &mut mpsc::Receiver<E>) -> Vec<E> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

/// Records every distinct connection state published (with the time it was observed).
pub fn record_states(
    mut rx: watch::Receiver<ConnectionStatus>,
) -> Arc<Mutex<Vec<(ConnectionState, Instant)>>> {
    let log = Arc::new(Mutex::new(vec![(
        rx.borrow_and_update().state,
        Instant::now(),
    )]));
    let sink = Arc::clone(&log);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let state = rx.borrow_and_update().state;
            let mut log = sink.lock();
            if log.last().map(|(s, _)| *s) != Some(state) {
                log.push((state, Instant::now()));
            }
        }
    });
    log
}

/// Whether `wanted` appears in `states` in this order (not necessarily adjacent).
pub fn is_subsequence(states: &[ConnectionState], wanted: &[ConnectionState]) -> bool {
    let mut it = states.iter();
    wanted.iter().all(|w| it.any(|s| s == w))
}

/// Advances the paused clock by `d`, letting every task run.
pub async fn advance(d: Duration) {
    tokio::time::sleep(d).await;
}

/// A frame the relay sends to the client, for injection tests.
pub fn relay_frame(message: &Message, session_id: u64, sequence: u64) -> Frame {
    let meta = if message.msg_type().is_stream() {
        FrameMeta::stream(
            if message.msg_type().allows_priority(FramePriority::P0) {
                FramePriority::P0
            } else {
                FramePriority::Control
            },
            session_id,
            sequence,
            0,
        )
    } else {
        FrameMeta::control(session_id, 0)
    };
    message.to_frame(&meta).expect("injected frame")
}

/// Shuts a reader/writer pair down (helper for raw-transport tests).
pub async fn close_writer<W: FrameWriter>(writer: &mut W) {
    let _ = writer.close().await;
}

/// Receives the next frame from a raw reader within [`WAIT`].
pub async fn recv_frame<R: FrameReader>(reader: &mut R) -> Frame {
    timeout(WAIT, reader.recv())
        .await
        .expect("frame in time")
        .expect("valid stream")
        .expect("open stream")
}

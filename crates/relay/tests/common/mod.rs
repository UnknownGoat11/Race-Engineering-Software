//! Test support: an in-process relay (in-memory store, rcgen development certificates written to a
//! temporary directory, real TLS 1.3 listener on 127.0.0.1) and a minimal protocol client speaking
//! the real wire format through `protocol`.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use lmu_relay::config::PasswordHashParams;
use lmu_relay::jwt::AccessTokenKeys;
use lmu_relay::store::AccountRole;
use lmu_relay::time::WallClock;
use lmu_relay::tls::{DEV_CA_FILE, DEV_CERT_FILE, DEV_KEY_FILE, write_dev_certificates};
use lmu_relay::{MemoryStore, RelayBuilder, RelayConfig, RelayHandle, ReloadingTls, Store};
use protocol::codec::FrameDecoder;
use protocol::frame::{Frame, FrameMeta, FramePriority, MsgType, NO_SESSION};
use protocol::messages::{
    Auth, AuthOk, ClientRole, ConnectCode, EngineerList, ErrorCode, ErrorMsg, Hello, Message,
    Secret, ServerHello, SessionCreate, SessionInfo, SessionJoin,
};
use protocol::version::ProtocolVersion;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use telemetry_model::enums::SessionType;
use telemetry_model::scoring::{SessionIdentity, VehicleCapabilities};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

/// Password of every test account.
pub const PASSWORD: &str = "correct horse battery staple";
/// Driver accounts.
pub const DRIVER: &str = "dave";
pub const DRIVER_B: &str = "bianca";
/// Engineer accounts.
pub const ENGINEER: &str = "erin";
pub const ENGINEER2: &str = "eddie";
pub const ENGINEER3: &str = "emma";
/// Upper bound for waiting on anything the relay should do promptly.
pub const WAIT: Duration = Duration::from_secs(10);
/// How long "nothing arrives" is observed.
pub const QUIET: Duration = Duration::from_millis(300);

/// A running relay with its certificates and the test accounts.
pub struct TestRelay {
    pub handle: RelayHandle,
    pub ca_pem: Vec<u8>,
    pub addr: SocketAddr,
    pub dir: PathBuf,
}

/// The configuration tests start from: development, ephemeral port, fast password hashing,
/// short housekeeping interval. Every test client connects from 127.0.0.1, so the per-IP join
/// and auth limits are raised (the guessing test restores the D-022 values), and link health
/// thresholds are relaxed so a slow CI machine never produces unexpected ENGINEER_LIST updates.
pub fn test_config() -> RelayConfig {
    let mut config = RelayConfig::development();
    config.auth.password_hash = PasswordHashParams::TEST_ONLY_FAST;
    config.timeouts.housekeeping_interval = Duration::from_millis(50);
    config.timeouts.heartbeat_degraded = Duration::from_secs(20);
    config.timeouts.heartbeat_timeout = Duration::from_secs(30);
    config.limits.code_redemptions_per_minute = 1_000;
    config.limits.code_redemptions_per_hour = 10_000;
    config.limits.auth_attempts_per_ip_per_minute = 1_000;
    config
}

/// A fresh temporary directory.
pub fn temp_dir(tag: &str) -> PathBuf {
    let suffix = lmu_relay::secrets::random_u64().expect("rng");
    let dir = std::env::temp_dir().join(format!("lmu-relay-test-{tag}-{suffix:016x}"));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

impl TestRelay {
    /// A relay with `test_config()` modified by `tweak` and the standard accounts.
    pub async fn start(tweak: impl FnOnce(&mut RelayConfig)) -> TestRelay {
        let mut config = test_config();
        tweak(&mut config);
        Self::start_with(config, Store::Memory(MemoryStore::new()), None, None, true).await
    }

    /// Full control: store (shared to simulate a restart), JWT keys, wall clock, accounts.
    pub async fn start_with(
        config: RelayConfig,
        store: Store,
        keys: Option<AccessTokenKeys>,
        clock: Option<WallClock>,
        create_accounts: bool,
    ) -> TestRelay {
        let dir = temp_dir("relay");
        write_dev_certificates(&dir, &["localhost".to_owned(), "127.0.0.1".to_owned()])
            .expect("dev certificates");
        let tls = Arc::new(
            ReloadingTls::load(&dir.join(DEV_CERT_FILE), &dir.join(DEV_KEY_FILE)).expect("tls"),
        );
        let mut builder = RelayBuilder::new(config, store, tls);
        if let Some(keys) = keys {
            builder = builder.jwt_keys(keys);
        }
        if let Some(clock) = clock {
            builder = builder.wall_clock(clock);
        }
        let handle = builder.start().await.expect("relay starts");
        let relay = TestRelay {
            addr: handle.local_addr(),
            ca_pem: std::fs::read(dir.join(DEV_CA_FILE)).expect("ca"),
            handle,
            dir,
        };
        if create_accounts {
            relay.create_accounts().await;
        }
        relay
    }

    /// Creates the standard accounts.
    pub async fn create_accounts(&self) {
        for (name, display, role) in [
            (DRIVER, "Dave Driver", AccountRole::Driver),
            (DRIVER_B, "Bianca Driver", AccountRole::Driver),
            (ENGINEER, "Erin Engineer", AccountRole::Engineer),
            (ENGINEER2, "Eddie Engineer", AccountRole::Engineer),
            (ENGINEER3, "Emma Engineer", AccountRole::Engineer),
        ] {
            self.handle
                .auth()
                .create_account(name, display, role, &Secret::new(PASSWORD))
                .await
                .expect("account");
        }
    }

    /// A connected (TLS only) client.
    pub async fn client(&self) -> Client {
        Client::connect(self.addr, &self.ca_pem).await
    }

    /// A driver logged in with a fresh session: (client, SESSION_INFO, first CONNECT_CODE).
    pub async fn driver_session(&self, username: &str) -> (Client, SessionInfo, ConnectCode) {
        let mut driver = self.client().await;
        driver.login(ClientRole::Driver, username).await;
        let (info, code, _) = driver.create_session().await;
        (driver, info, code)
    }

    /// An engineer logged in and joined with `code`.
    pub async fn engineer_joined(&self, username: &str, code: &Secret) -> (Client, SessionInfo) {
        let mut engineer = self.client().await;
        engineer.login(ClientRole::Engineer, username).await;
        let info = engineer.join(code).await.expect("joined");
        (engineer, info)
    }

    pub async fn shutdown(self) {
        self.handle.shutdown().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The code of a CONNECT_CODE (panics if there is none).
pub fn code_of(cc: &ConnectCode) -> Secret {
    cc.code.as_ref().expect("a code").code.clone()
}

/// A minimal protocol client.
pub struct Client {
    stream: TlsStream<TcpStream>,
    buf: BytesMut,
    decoder: FrameDecoder,
    /// Session bound to this connection (0 = none); used in sent headers.
    pub session_id: u64,
    seq: u64,
    pub eof: bool,
}

impl Client {
    /// TLS 1.3 connection trusting `ca_pem`, ALPN lmure/1.
    pub async fn connect(addr: SocketAddr, ca_pem: &[u8]) -> Client {
        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_slice_iter(ca_pem) {
            roots.add(cert.expect("ca pem")).expect("ca accepted");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("tls13")
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![protocol::ALPN_PROTOCOL.to_vec()];
        let tcp = TcpStream::connect(addr).await.expect("tcp connect");
        tcp.set_nodelay(true).expect("nodelay");
        let stream = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("localhost").expect("name"), tcp)
            .await
            .expect("tls handshake");
        Client {
            stream,
            buf: BytesMut::with_capacity(64 * 1024),
            decoder: FrameDecoder::new(),
            session_id: NO_SESSION,
            seq: 0,
            eof: false,
        }
    }

    /// Sends a control message carrying the bound session id (or 0).
    pub async fn send(&mut self, message: &Message) {
        let frame = message
            .to_frame(&FrameMeta::control(self.session_id, 0))
            .expect("encodable");
        self.send_frame(&frame).await;
    }

    pub async fn send_frame(&mut self, frame: &Frame) {
        self.send_raw(frame.as_bytes()).await;
    }

    pub async fn send_raw(&mut self, bytes: &[u8]) {
        // Writes to a connection the relay already closed may fail; tests check the outcome.
        let _ = self.stream.write_all(bytes).await;
        let _ = self.stream.flush().await;
    }

    /// Next stream sequence of this client.
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Restarts the stream sequence space (driver app restart).
    pub fn reset_seq(&mut self) {
        self.seq = 0;
    }

    /// Continues a sequence space after a reconnect (the next frame gets `last + 1`).
    pub fn continue_seq(&mut self, last: u64) {
        self.seq = last;
    }

    /// Sends SNAPSHOT_REQUEST (a reliable stream frame) with the next sequence.
    pub async fn send_snapshot_request(
        &mut self,
        reason: protocol::messages::SnapshotReason,
    ) -> u64 {
        let seq = self.next_seq();
        let frame = Message::SnapshotRequest(protocol::messages::SnapshotRequest { reason })
            .to_frame(&FrameMeta::stream(
                FramePriority::Control,
                self.session_id,
                seq,
                seq,
            ))
            .expect("snapshot request frame");
        self.send_frame(&frame).await;
        seq
    }

    /// A stream frame with the next sequence.
    pub fn stream_frame(
        &mut self,
        msg_type: MsgType,
        priority: FramePriority,
        payload: &[u8],
    ) -> Frame {
        let seq = self.next_seq();
        Frame::build(
            msg_type,
            &FrameMeta::stream(priority, self.session_id, seq, seq),
            payload,
        )
        .expect("stream frame")
    }

    /// Sends a stream frame with the next sequence; returns the sequence.
    pub async fn send_stream(
        &mut self,
        msg_type: MsgType,
        priority: FramePriority,
        payload: &[u8],
    ) -> u64 {
        let frame = self.stream_frame(msg_type, priority, payload);
        self.send_frame(&frame).await;
        frame.header().sequence
    }

    /// The next frame within `within`: `Ok(None)` on EOF / connection error, `Err(())` on
    /// timeout.
    pub async fn try_frame(&mut self, within: Duration) -> Result<Option<Frame>, ()> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match self.decoder.decode_frame(&mut self.buf) {
                Ok(Some(frame)) => return Ok(Some(frame)),
                Ok(None) => {}
                Err(e) => panic!("relay sent an invalid frame: {e}"),
            }
            if self.eof {
                return Ok(None);
            }
            self.buf.reserve(64 * 1024);
            match tokio::time::timeout_at(deadline, self.stream.read_buf(&mut self.buf)).await {
                Err(_) => return Err(()),
                Ok(Ok(0)) | Ok(Err(_)) => {
                    self.eof = true;
                }
                Ok(Ok(_)) => {}
            }
        }
    }

    /// The next frame (panics after [`WAIT`]); `None` on EOF.
    pub async fn frame(&mut self) -> Option<Frame> {
        self.try_frame(WAIT)
            .await
            .expect("a frame from the relay in time")
    }

    /// The next message other than PONG (panics on EOF or timeout). Stream frames whose test
    /// payload is not a real TLV / postcard body are skipped.
    pub async fn recv(&mut self) -> Message {
        loop {
            let frame = self.frame().await.expect("connection open");
            match Message::from_frame(&frame) {
                Ok(Message::Pong(_)) => {}
                Ok(message) => return message,
                Err(_) if frame.msg_type().is_some_and(MsgType::is_stream) => {}
                Err(e) => panic!("undecodable control message from the relay: {e}"),
            }
        }
    }

    /// Messages until one matches `pick` (others are skipped); panics on EOF or timeout.
    pub async fn recv_until<T>(&mut self, mut pick: impl FnMut(Message) -> Option<T>) -> T {
        loop {
            if let Some(t) = pick(self.recv().await) {
                return t;
            }
        }
    }

    /// The next ERROR (other messages are skipped).
    pub async fn error(&mut self) -> ErrorMsg {
        self.recv_until(|m| match m {
            Message::Error(e) => Some(e),
            _ => None,
        })
        .await
    }

    /// Waits until the relay closes the connection (other frames are skipped).
    pub async fn closed(&mut self) -> bool {
        loop {
            match self.try_frame(WAIT).await {
                Ok(None) => return true,
                Ok(Some(_)) => {}
                Err(()) => return false,
            }
        }
    }

    /// Whether no frame arrives within `within` (and the connection stays open).
    pub async fn quiet(&mut self, within: Duration) -> bool {
        matches!(self.try_frame(within).await, Err(()))
    }

    pub async fn hello(&mut self, role: ClientRole) -> ServerHello {
        self.send(&Message::Hello(Hello {
            app_version: "test/1".into(),
            protocol: ProtocolVersion::CURRENT,
            role,
            schema_versions: vec![1],
            client_id: "test-client".into(),
        }))
        .await;
        match self.recv().await {
            Message::ServerHello(h) => h,
            other => panic!("ServerHello expected, got {other:?}"),
        }
    }

    pub async fn auth(&mut self, auth: Auth) -> Result<AuthOk, ErrorMsg> {
        self.send(&Message::Auth(auth)).await;
        match self.recv().await {
            Message::AuthOk(ok) => Ok(ok),
            Message::Error(e) => Err(e),
            other => panic!("AUTH_OK or ERROR expected, got {other:?}"),
        }
    }

    /// HELLO + password AUTH.
    pub async fn login(&mut self, role: ClientRole, username: &str) -> AuthOk {
        self.hello(role).await;
        self.auth(Auth::Password {
            username: username.into(),
            password: Secret::new(PASSWORD),
        })
        .await
        .expect("login accepted")
    }

    /// SESSION_CREATE → (SESSION_INFO, CONNECT_CODE, ENGINEER_LIST).
    pub async fn create_session(&mut self) -> (SessionInfo, ConnectCode, EngineerList) {
        self.send(&Message::SessionCreate(session_create())).await;
        let info = match self.recv().await {
            Message::SessionInfo(info) => info,
            other => panic!("SESSION_INFO expected, got {other:?}"),
        };
        self.session_id = info.session_id;
        let code = match self.recv().await {
            Message::ConnectCode(c) => c,
            other => panic!("CONNECT_CODE expected, got {other:?}"),
        };
        let list = match self.recv().await {
            Message::EngineerList(l) => l,
            other => panic!("ENGINEER_LIST expected, got {other:?}"),
        };
        (info, code, list)
    }

    /// SESSION_JOIN{code}: SESSION_INFO or the ERROR.
    pub async fn join(&mut self, code: &Secret) -> Result<SessionInfo, ErrorMsg> {
        self.send(&Message::SessionJoin(SessionJoin { code: code.clone() }))
            .await;
        match self.recv().await {
            Message::SessionInfo(info) => {
                self.session_id = info.session_id;
                Ok(info)
            }
            Message::Error(e) => Err(e),
            other => panic!("SESSION_INFO or ERROR expected, got {other:?}"),
        }
    }

    /// The next CONNECT_CODE (other messages skipped).
    pub async fn connect_code(&mut self) -> ConnectCode {
        self.recv_until(|m| match m {
            Message::ConnectCode(c) => Some(c),
            _ => None,
        })
        .await
    }

    /// The next ENGINEER_LIST (other messages skipped).
    pub async fn engineer_list(&mut self) -> EngineerList {
        self.recv_until(|m| match m {
            Message::EngineerList(l) => Some(l),
            _ => None,
        })
        .await
    }

    /// The next stream frame of `msg_type` (control messages skipped).
    pub async fn stream_of(&mut self, msg_type: MsgType) -> Frame {
        loop {
            let frame = self.frame().await.expect("connection open");
            if frame.msg_type() == Some(msg_type) {
                return frame;
            }
        }
    }

    /// Drops the TCP connection without DISCONNECT (network loss / app crash).
    pub fn kill(self) {
        drop(self);
    }
}

/// A SESSION_CREATE describing a Le Mans hypercar session.
pub fn session_create() -> SessionCreate {
    SessionCreate {
        identity: SessionIdentity {
            vehicle_name: Some("Ferrari 499P #50".into()),
            vehicle_model: Some("499P".into()),
            vehicle_class: None,
            class_name: Some("Hypercar".into()),
            track_name: Some("Circuit de la Sarthe".into()),
            session_type: Some(SessionType::from_lmu(10)),
            game_version: Some(14_200),
        },
        lmu_version: Some("1.4.2.0".into()),
        app_version: "0.1.0-test".into(),
        schema_version: 1,
        capabilities: VehicleCapabilities::default(),
    }
}

/// Error code of an ERROR, for compact assertions.
pub fn code(e: &ErrorMsg) -> ErrorCode {
    e.code
}

/// A frame assembled byte by byte, bypassing the encoder's rule checks (for invalid input):
/// header fields as given, CRC-32C correct.
pub fn raw_frame(
    msg_type: u8,
    priority: u8,
    session_id: u64,
    sequence: u64,
    payload: &[u8],
) -> Vec<u8> {
    let header = protocol::frame::FrameHeader {
        minor: protocol::PROTOCOL_MINOR,
        msg_type,
        flags: protocol::frame::Flags::empty(),
        priority: FramePriority::from_u8(priority).expect("valid priority byte"),
        payload_len: u32::try_from(payload.len()).expect("small payload"),
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

/// Polls the store until session `id` has `status` (store writes follow registry changes
/// asynchronously).
pub async fn wait_store_status(
    store: &Store,
    id: u64,
    status: lmu_relay::store::SessionStatus,
) -> lmu_relay::store::SessionRecord {
    use lmu_relay::store::SessionStore as _;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let record = store.session(id).await.expect("store").expect("record");
        if record.status == status {
            return record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "session {id} did not reach {status:?} (is {:?})",
            record.status
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Polls `condition` every 10 ms until it holds (panics after [`WAIT`]).
pub async fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not reached in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Plain HTTP GET (admin listener); returns (status, body).
pub async fn http_get(addr: SocketAddr, path: &str, bearer: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("admin connect");
    let auth = bearer.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("request");
    let mut response = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut response))
        .await
        .expect("response in time")
        .expect("response");
    let text = String::from_utf8_lossy(&response).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("status code");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

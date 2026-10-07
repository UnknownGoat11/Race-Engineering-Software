//! One client connection: TLS handshake, the reader state machine (HELLO → AUTH → bound session)
//! and the writer task.
//!
//! Hot path (driver stream frames): the [`FrameDecoder`] validates framing and CRC, the header
//! rules are checked against the connection's authenticated role and session binding
//! ([`check_client_frame`]), the registry checks sequence monotonicity and fans the frame out. The
//! payload of telemetry frames is never decoded; the relay trailer is stamped by the writer.
//!
//! Invalid frames are skipped and charged to a per-connection budget (refilled 1/s); framing
//! errors are fatal (the byte stream is untrusted afterwards). Credentials, codes and tickets
//! never reach a log line.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use chrono::{DateTime, Utc};
use protocol::codec::{CodecError, FrameDecoder};
use protocol::frame::{
    Endpoint, Frame, FrameError, FrameHeader, FrameMeta, HeaderRuleError, MAX_FRAME_BYTES, MsgType,
    NO_SESSION, check_client_frame,
};
use protocol::messages::{
    Auth, ClientRole, Disconnect, DisconnectReason, ErrorCode, ErrorMsg, Message, Pong, Secret,
    ServerHello, ServerLimits, SessionCreate, SessionResume,
};
use protocol::version::{ProtocolVersion, negotiate_protocol};
use protocol::{ALPN_PROTOCOL, RelayTrailer};
use telemetry_model::enums::SessionType;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;

use crate::auth::{AuthError, AuthMethod, LoginOk};
use crate::outbox::{ConnHandle, Outgoing};
use crate::ratelimit::{RateMeter, TokenBucket};
use crate::registry::{
    ForwardReject, Identity, NewSession, RegistryError, ResumeError, SessionCell, SlotWrite,
    SnapshotRequestOutcome, StoredSession, StoredSlot,
};
use crate::secrets::{new_token, presented_token_hash, random_u64};
use crate::state::{PasswordAttempt, RelayState};
use crate::store::{
    Account, AccountRole, AccountStore as _, CLOSE_REASON_DRIVER_STOPPED, CLOSE_REASON_EXPIRED,
    CLOSE_REASON_RELAY_FULL, CLOSE_REASON_REPLACED, SessionMeta, SessionRecord, SessionStatus,
    SessionStore as _, StoreError,
};
use crate::time::{datetime_to_ns, duration_ms, mono_ns_since, now_utc, unix_now_ns};

/// Log component of this module.
const COMPONENT: &str = "relay.conn";
/// Minimum read reservation.
const READ_CHUNK: usize = 16 * 1024;
/// Bytes of reliable frames taken into one writer batch.
const MAX_BATCH_BYTES: usize = 256 * 1024;
/// A writer buffer that grew beyond this is released after the write.
const WRITE_BUFFER_KEEP: usize = 1024 * 1024;
/// Oldest client protocol minor accepted.
const MIN_CLIENT_MINOR: u8 = 0;
/// Client-supplied text is cut to this many characters in logs and session metadata.
const MAX_TEXT_CHARS: usize = 128;
/// Attempts to find an unused random session id.
const SESSION_ID_ATTEMPTS: usize = 4;
/// Time allowed for the TLS close_notify after the last write.
const SHUTDOWN_FLUSH: Duration = Duration::from_secs(2);
/// Payload limit until the connection is authenticated. HELLO, AUTH and ACCOUNT_REGISTER are
/// well under 1 KiB, so an unauthenticated peer cannot make the reader reserve and buffer large
/// frames; the configured (announced) limit applies from the first frame after AUTH_OK.
const PRE_AUTH_MAX_PAYLOAD: usize = crate::config::MIN_PAYLOAD_LIMIT;
/// Largest engineer SNAPSHOT_REQUEST payload accepted (the canonical encoding is 1 byte; a newer
/// minor may append a few). Larger payloads are violations and are never forwarded.
const MAX_SNAPSHOT_REQUEST_PAYLOAD: u32 = 16;

/// Why a connection ended (logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum End {
    PeerClosed,
    ClientDisconnect,
    PeerError,
    Io,
    IdleTimeout,
    HandshakeTimeout,
    Framing,
    Violations,
    Incompatible,
    HandshakeViolation,
    AuthAttempts,
    SlowConsumer,
    Closed,
    Shutdown,
}

impl End {
    const fn as_str(self) -> &'static str {
        match self {
            End::PeerClosed => "peer_closed",
            End::ClientDisconnect => "client_disconnect",
            End::PeerError => "peer_error",
            End::Io => "io_error",
            End::IdleTimeout => "idle_timeout",
            End::HandshakeTimeout => "handshake_timeout",
            End::Framing => "framing_error",
            End::Violations => "too_many_invalid_frames",
            End::Incompatible => "protocol_incompatible",
            End::HandshakeViolation => "handshake_violation",
            End::AuthAttempts => "auth_attempts_exhausted",
            End::SlowConsumer => "slow_consumer",
            End::Closed => "closed_by_relay",
            End::Shutdown => "relay_shutdown",
        }
    }
}

enum Flow {
    Continue,
    Close(End),
}

/// Authenticated connection identity.
#[derive(Debug, Clone)]
struct Authed {
    account: Account,
    role: ClientRole,
}

enum Phase {
    AwaitHello,
    AwaitAuth { requested: ClientRole },
    Ready(Authed),
}

/// Phase discriminant (for dispatch without borrowing the phase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseKind {
    AwaitHello,
    AwaitAuth,
    Ready,
}

/// Runs one accepted TCP connection to completion.
pub(crate) async fn serve_connection(
    state: Arc<RelayState>,
    tcp: TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
) {
    let id = state.next_conn_id();
    let opened = Instant::now();
    let handshake_deadline = opened + state.config.timeouts.handshake;
    let tls = match tokio::time::timeout_at(handshake_deadline.into(), acceptor.accept(tcp)).await {
        Ok(Ok(tls)) => tls,
        Ok(Err(e)) => {
            state.metrics.connections_rejected.inc();
            tracing::debug!(component = COMPONENT, conn = id, %peer, error = %e, "TLS handshake failed");
            return;
        }
        Err(_) => {
            state.metrics.connections_rejected.inc();
            tracing::debug!(component = COMPONENT, conn = id, %peer, "TLS handshake timed out");
            return;
        }
    };
    if tls.get_ref().1.alpn_protocol() != Some(ALPN_PROTOCOL) {
        state.metrics.connections_rejected.inc();
        tracing::warn!(component = COMPONENT, conn = id, %peer, "client did not negotiate ALPN lmure/1; closing");
        return;
    }
    let (rd, wr) = tokio::io::split(tls);
    let limits = &state.config.limits;
    let conn = Arc::new(ConnHandle::new(
        id,
        peer,
        state.epoch,
        limits.reliable_queue_frames,
        limits.reliable_queue_bytes,
    ));
    let writer = tokio::spawn(writer_loop(Arc::clone(&state), Arc::clone(&conn), wr));
    tracing::info!(component = COMPONENT, conn = id, %peer, "connection opened");
    let mut connection = Connection::new(Arc::clone(&state), Arc::clone(&conn), handshake_deadline);
    let end = connection.run(rd).await;
    connection.finish().await;
    state.logins.remove(id);
    conn.close();
    // The writer flushes what is queued (ERROR, SESSION_CLOSE, DISCONNECT) and closes.
    let flush = state
        .config
        .timeouts
        .write_stall
        .saturating_add(SHUTDOWN_FLUSH);
    if tokio::time::timeout(flush, writer).await.is_err() {
        tracing::warn!(
            component = COMPONENT,
            conn = id,
            "writer did not finish in time"
        );
    }
    conn.abort();
    tracing::info!(
        component = COMPONENT,
        conn = id,
        %peer,
        reason = end.as_str(),
        duration_ms = duration_ms(opened.elapsed()),
        "connection closed"
    );
}

/// Reader-side state of one connection.
struct Connection {
    state: Arc<RelayState>,
    conn: Arc<ConnHandle>,
    phase: Phase,
    binding: Option<Arc<SessionCell>>,
    rate: RateMeter,
    budget: TokenBucket,
    auth_attempts: u32,
    handshake_deadline: Instant,
    stale_warned: bool,
}

impl Connection {
    fn new(state: Arc<RelayState>, conn: Arc<ConnHandle>, handshake_deadline: Instant) -> Self {
        let now = Instant::now();
        let limits = &state.config.limits;
        let budget = u64::from(limits.malformed_budget);
        Self {
            rate: RateMeter::new(limits.connection_rate, now),
            // `malformed_budget` violations at once, refilled one per second.
            budget: TokenBucket::new(budget, Duration::from_secs(budget), now),
            state,
            conn,
            phase: Phase::AwaitHello,
            binding: None,
            auth_attempts: 0,
            handshake_deadline,
            stale_warned: false,
        }
    }

    fn phase_kind(&self) -> PhaseKind {
        match self.phase {
            Phase::AwaitHello => PhaseKind::AwaitHello,
            Phase::AwaitAuth { .. } => PhaseKind::AwaitAuth,
            Phase::Ready(_) => PhaseKind::Ready,
        }
    }

    fn authed(&self) -> Option<&Authed> {
        match &self.phase {
            Phase::Ready(a) => Some(a),
            _ => None,
        }
    }

    fn role(&self) -> Option<ClientRole> {
        self.authed().map(|a| a.role)
    }

    fn bound_id(&self) -> u64 {
        self.binding.as_ref().map_or(NO_SESSION, |c| c.id())
    }

    fn send(&self, message: &Message) {
        self.conn.send(message, self.bound_id());
    }

    fn reply_error(&self, code: ErrorCode, message: impl Into<String>) -> Flow {
        self.send(&Message::Error(ErrorMsg {
            code,
            message: message.into(),
            fatal: false,
        }));
        Flow::Continue
    }

    fn fatal(&self, code: ErrorCode, message: impl Into<String>, end: End) -> Flow {
        self.send(&Message::Error(ErrorMsg {
            code,
            message: message.into(),
            fatal: true,
        }));
        Flow::Close(end)
    }

    /// Skips an invalid frame and charges the budget; closes the connection when it is spent.
    fn violation(&mut self, now: Instant, what: &str, reply: Option<ErrorMsg>) -> Flow {
        if !self.budget.try_take(1.0, now) {
            tracing::warn!(
                component = COMPONENT,
                conn = self.conn.id,
                peer = %self.conn.peer,
                last = what,
                "too many invalid frames; closing the connection"
            );
            return self.fatal(
                ErrorCode::Malformed,
                "too many invalid frames",
                End::Violations,
            );
        }
        tracing::debug!(
            component = COMPONENT,
            conn = self.conn.id,
            what,
            "invalid frame skipped"
        );
        if let Some(reply) = reply {
            self.send(&Message::Error(reply));
        }
        Flow::Continue
    }

    fn read_deadline(&self) -> Instant {
        let idle = self.conn.last_inbound() + self.state.config.timeouts.heartbeat_timeout;
        if self.phase_kind() == PhaseKind::Ready {
            idle
        } else {
            idle.min(self.handshake_deadline)
        }
    }

    async fn run<R: AsyncRead + Unpin>(&mut self, mut rd: R) -> End {
        let mut buf = BytesMut::with_capacity(READ_CHUNK);
        let configured = self.state.config.limits.max_payload_bytes;
        let mut decoder = FrameDecoder::with_max_payload(PRE_AUTH_MAX_PAYLOAD.min(configured));
        loop {
            loop {
                match decoder.decode_frame(&mut buf) {
                    Ok(Some(frame)) => {
                        // The relay is ending this connection (removed, replaced, revoked, too
                        // slow, shutting down) and only flushes what is queued: nothing the client
                        // still sends is acted on, e.g. a SESSION_JOIN after a revocation.
                        if self.conn.outbox.is_closing() {
                            return End::Closed;
                        }
                        if let Flow::Close(end) = self.on_frame(frame).await {
                            return end;
                        }
                        // Authenticated: the announced limit applies from the next frame on. The
                        // decoder holds no stream state besides counters, so it is swapped.
                        if decoder.max_payload() < configured
                            && self.phase_kind() == PhaseKind::Ready
                        {
                            decoder = FrameDecoder::with_max_payload(configured);
                        }
                    }
                    Ok(None) => break,
                    Err(CodecError::Frame(e)) => return self.framing_error(&e),
                    Err(e) => {
                        tracing::warn!(component = COMPONENT, conn = self.conn.id, error = %e, "frame decoder failed");
                        return End::Framing;
                    }
                }
            }
            // The decoder validated what is buffered, so this is the size of the frame in progress.
            let needed = decoder.bytes_needed(&buf).unwrap_or(READ_CHUNK);
            buf.reserve(needed.clamp(READ_CHUNK, MAX_FRAME_BYTES));
            let deadline = self.read_deadline();
            let read = tokio::select! {
                biased;
                _ = self.conn.cancelled() => return End::Closed,
                _ = self.state.shutdown.cancelled() => return self.on_shutdown(),
                r = tokio::time::timeout_at(deadline.into(), rd.read_buf(&mut buf)) => r,
            };
            match read {
                Err(_) => return self.on_timeout(),
                Ok(Ok(0)) => return End::PeerClosed,
                Ok(Ok(n)) => {
                    self.state.metrics.bytes_received.add(n as u64);
                    self.conn.touch(Instant::now());
                }
                Ok(Err(e)) => {
                    tracing::debug!(component = COMPONENT, conn = self.conn.id, error = %e, "read failed");
                    return End::Io;
                }
            }
        }
    }

    fn on_timeout(&mut self) -> End {
        if self.phase_kind() != PhaseKind::Ready && Instant::now() >= self.handshake_deadline {
            tracing::info!(component = COMPONENT, conn = self.conn.id, peer = %self.conn.peer, "handshake not completed in time");
            return End::HandshakeTimeout;
        }
        tracing::info!(component = COMPONENT, conn = self.conn.id, peer = %self.conn.peer, "no inbound traffic within the heartbeat timeout");
        self.send(&Message::Disconnect(Disconnect {
            reason: DisconnectReason::IdleTimeout,
            message: Some("no traffic within the heartbeat timeout".into()),
        }));
        End::IdleTimeout
    }

    fn on_shutdown(&mut self) -> End {
        self.send(&Message::Disconnect(Disconnect {
            reason: DisconnectReason::ServerShutdown,
            message: Some("the relay is restarting".into()),
        }));
        End::Shutdown
    }

    fn framing_error(&mut self, e: &FrameError) -> End {
        self.state.metrics.packet_validation_failures.inc();
        tracing::warn!(
            component = COMPONENT,
            conn = self.conn.id,
            peer = %self.conn.peer,
            error = %e,
            "framing error; closing the connection"
        );
        self.send(&Message::Error(ErrorMsg {
            code: e.error_code(),
            message: e.to_string(),
            fatal: true,
        }));
        End::Framing
    }

    fn check_header(&self, header: &FrameHeader) -> Result<MsgType, HeaderRuleError> {
        let (from, bound) = match &self.phase {
            // The role is unknown before HELLO; only HELLO passes the dispatch below.
            Phase::AwaitHello => (Endpoint::Driver, None),
            Phase::AwaitAuth { requested } => (Endpoint::from(*requested), None),
            Phase::Ready(a) => (
                Endpoint::from(a.role),
                self.binding.as_ref().map(|c| c.id()),
            ),
        };
        check_client_frame(header, from, bound)
    }

    fn decode(&mut self, frame: &Frame, now: Instant) -> Result<Message, Flow> {
        Message::from_frame(frame).map_err(|e| {
            self.state.metrics.packet_validation_failures.inc();
            let msg_type =
                MsgType::from_id(frame.header().msg_type).map_or("unknown", MsgType::name);
            let reply = ErrorMsg {
                code: e.error_code(),
                message: format!("invalid {msg_type} payload"),
                fatal: false,
            };
            self.violation(now, "payload decode", Some(reply))
        })
    }

    async fn on_frame(&mut self, frame: Frame) -> Flow {
        let now = Instant::now();
        if !self.rate.admit(frame.wire_len(), now) {
            self.state.metrics.rate_limited.inc();
            let reply = ErrorMsg {
                code: ErrorCode::RateLimited,
                message: "inbound rate limit exceeded; frame dropped".into(),
                fatal: false,
            };
            return self.violation(now, "connection rate limit", Some(reply));
        }
        let msg_type = match self.check_header(frame.header()) {
            Ok(t) => t,
            Err(rule) => {
                self.state.metrics.packet_validation_failures.inc();
                let reply = match rule {
                    // Forward compatibility: unknown types are skipped silently (but counted).
                    HeaderRuleError::UnknownMsgType(_) => None,
                    HeaderRuleError::DirectionNotAllowed { .. }
                    | HeaderRuleError::SessionNotBound { .. }
                    | HeaderRuleError::SessionMismatch { .. } => Some(ErrorMsg {
                        code: ErrorCode::NotAuthorized,
                        message: rule.to_string(),
                        fatal: false,
                    }),
                    _ => Some(ErrorMsg {
                        code: ErrorCode::Malformed,
                        message: rule.to_string(),
                        fatal: false,
                    }),
                };
                return self.violation(now, "header rule", reply);
            }
        };
        let phase = self.phase_kind();
        match (msg_type, phase) {
            (MsgType::Disconnect, _) => self.on_disconnect(&frame, now).await,
            (MsgType::Error, _) => self.on_peer_error(&frame, now),
            (MsgType::Hello, PhaseKind::AwaitHello) => self.on_hello(&frame, now),
            (_, PhaseKind::AwaitHello) => self.fatal(
                ErrorCode::NotAuthorized,
                "HELLO expected first",
                End::HandshakeViolation,
            ),
            (MsgType::Auth, PhaseKind::AwaitAuth) => self.on_auth(&frame, now).await,
            (MsgType::AccountRegister, PhaseKind::AwaitAuth) => self.on_register(&frame, now).await,
            (MsgType::Ping, PhaseKind::AwaitAuth | PhaseKind::Ready) => self.on_ping(&frame, now),
            (_, PhaseKind::AwaitAuth) => self.fatal(
                ErrorCode::NotAuthorized,
                "AUTH expected",
                End::HandshakeViolation,
            ),
            (MsgType::SessionCreate, PhaseKind::Ready) => self.on_session_create(&frame, now).await,
            (MsgType::SessionJoin, PhaseKind::Ready) => self.on_session_join(&frame, now).await,
            (MsgType::SessionResume, PhaseKind::Ready) => self.on_session_resume(&frame, now).await,
            (MsgType::SessionClose, PhaseKind::Ready) => self.on_session_close(&frame, now).await,
            (MsgType::ConnectCodeRequest, PhaseKind::Ready) => self.on_code_request(&frame, now),
            (MsgType::EngineerKick, PhaseKind::Ready) => self.on_kick(&frame, now).await,
            (t, PhaseKind::Ready) if t.is_stream() => self.on_stream(&frame, t, now),
            (t, PhaseKind::Ready) => {
                self.state.metrics.packet_validation_failures.inc();
                let reply = ErrorMsg {
                    code: ErrorCode::NotAuthorized,
                    message: format!("{t} is not accepted on an authenticated connection"),
                    fatal: false,
                };
                self.violation(now, "unexpected message", Some(reply))
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Handshake
    // ---------------------------------------------------------------------------------------

    fn on_hello(&mut self, frame: &Frame, now: Instant) -> Flow {
        let hello = match self.decode(frame, now) {
            Ok(Message::Hello(hello)) => hello,
            Ok(_) => {
                return self.fatal(
                    ErrorCode::Malformed,
                    "client HELLO expected",
                    End::HandshakeViolation,
                );
            }
            Err(flow) => return flow,
        };
        if let Err(incompatible) =
            negotiate_protocol(ProtocolVersion::CURRENT, hello.protocol, MIN_CLIENT_MINOR)
        {
            tracing::warn!(
                component = COMPONENT,
                conn = self.conn.id,
                peer = %self.conn.peer,
                client_protocol = %hello.protocol,
                app_version = %truncated(&hello.app_version),
                error = %incompatible,
                "protocol version mismatch"
            );
            self.send(&Message::Error(incompatible.to_error_msg()));
            return Flow::Close(End::Incompatible);
        }
        let config = &self.state.config;
        let limits = ServerLimits {
            max_payload_bytes: u32::try_from(config.limits.max_payload_bytes).unwrap_or(u32::MAX),
            heartbeat_interval_ms: u32::try_from(duration_ms(config.timeouts.heartbeat_interval))
                .unwrap_or(u32::MAX),
            heartbeat_timeout_ms: u32::try_from(duration_ms(config.timeouts.heartbeat_timeout))
                .unwrap_or(u32::MAX),
            max_engineers_per_session: config.limits.max_engineers_per_session,
            session_driver_grace_s: u32::try_from(config.timeouts.driver_grace.as_secs())
                .unwrap_or(u32::MAX),
        };
        self.send(&Message::ServerHello(ServerHello {
            relay_version: self.state.relay_version.clone(),
            protocol: ProtocolVersion::CURRENT,
            server_time_ns: unix_now_ns(),
            limits,
            env: config.env,
        }));
        tracing::debug!(
            component = COMPONENT,
            conn = self.conn.id,
            role = ?hello.role,
            app_version = %truncated(&hello.app_version),
            client_protocol = %hello.protocol,
            "HELLO"
        );
        self.phase = Phase::AwaitAuth {
            requested: hello.role,
        };
        Flow::Continue
    }

    /// Charges one authentication attempt; `Some(flow)` if the connection or IP is over limit.
    fn charge_auth_attempt(&mut self, now: Instant) -> Option<Flow> {
        self.auth_attempts = self.auth_attempts.saturating_add(1);
        if self.auth_attempts > self.state.config.limits.auth_attempts_per_connection {
            self.state.metrics.rate_limited.inc();
            return Some(self.fatal(
                ErrorCode::RateLimited,
                "too many authentication attempts on this connection",
                End::AuthAttempts,
            ));
        }
        if !self.state.limiters.admit_auth_ip(self.conn.peer.ip(), now) {
            self.state.metrics.rate_limited.inc();
            self.state.metrics.auth_failures.inc();
            tracing::warn!(component = COMPONENT, conn = self.conn.id, peer = %self.conn.peer, "authentication rate limit reached for this IP");
            return Some(self.reply_error(
                ErrorCode::RateLimited,
                "too many authentication attempts; try again later",
            ));
        }
        None
    }

    async fn on_auth(&mut self, frame: &Frame, now: Instant) -> Flow {
        let Phase::AwaitAuth { requested } = self.phase else {
            return Flow::Continue;
        };
        if let Some(flow) = self.charge_auth_attempt(now) {
            return flow;
        }
        let auth = match self.decode(frame, now) {
            Ok(Message::Auth(auth)) => auth,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        let (method, result) = match &auth {
            Auth::Password { username, password } => {
                match self.state.limiters.admit_password_attempt(username, now) {
                    PasswordAttempt::Admitted => {}
                    PasswordAttempt::RateLimited => {
                        self.state.metrics.rate_limited.inc();
                        self.state.metrics.auth_failures.inc();
                        tracing::warn!(component = COMPONENT, conn = self.conn.id, peer = %self.conn.peer, "password attempts rate limit reached for an account");
                        return self.reply_error(
                            ErrorCode::RateLimited,
                            "too many password attempts; try again later",
                        );
                    }
                    // No account can have this name (the rules are public): refused without a
                    // password check, and the name never reaches the per-account limiter.
                    PasswordAttempt::InvalidUsername => {
                        return self
                            .auth_failed(AuthMethod::Password, &AuthError::InvalidCredentials);
                    }
                }
                (
                    AuthMethod::Password,
                    self.state.auth.login_password(username, password).await,
                )
            }
            Auth::AccessToken { token } => (
                AuthMethod::AccessToken,
                self.state.auth.login_access_token(token).await,
            ),
            Auth::RefreshToken { token } => (
                AuthMethod::RefreshToken,
                self.state.auth.login_refresh_token(token).await,
            ),
        };
        match result {
            Ok(login) => self.authenticated(login, requested),
            Err(e) => self.auth_failed(method, &e),
        }
    }

    async fn on_register(&mut self, frame: &Frame, now: Instant) -> Flow {
        let Phase::AwaitAuth { requested } = self.phase else {
            return Flow::Continue;
        };
        if let Some(flow) = self.charge_auth_attempt(now) {
            return flow;
        }
        let request = match self.decode(frame, now) {
            Ok(Message::AccountRegister(request)) => request,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        let role = match requested {
            ClientRole::Driver => AccountRole::Driver,
            ClientRole::Engineer => AccountRole::Engineer,
        };
        match self.state.auth.register(&request, role).await {
            Ok(login) => self.authenticated(login, requested),
            Err(e) => self.auth_failed(AuthMethod::Registration, &e),
        }
    }

    fn authenticated(&mut self, login: LoginOk, requested: ClientRole) -> Flow {
        let role = login.account.role.effective_role(requested);
        // Revoking this login (or disabling / deleting the account) ends this connection too.
        self.state.logins.register(&self.conn, login.family);
        self.send(&Message::AuthOk(login.auth_ok(requested)));
        tracing::info!(
            component = COMPONENT,
            conn = self.conn.id,
            peer = %self.conn.peer,
            account = %login.account.id,
            username = %login.account.username,
            role = ?role,
            method = login.method.as_str(),
            "authenticated"
        );
        self.phase = Phase::Ready(Authed {
            account: login.account,
            role,
        });
        Flow::Continue
    }

    fn auth_failed(&mut self, method: AuthMethod, e: &AuthError) -> Flow {
        self.state.metrics.auth_failures.inc();
        if e.is_internal() {
            tracing::error!(
                component = COMPONENT,
                conn = self.conn.id,
                method = method.as_str(),
                error = %e,
                cause = ?std::error::Error::source(e).map(ToString::to_string),
                "authentication failed (relay error)"
            );
        } else {
            tracing::warn!(
                component = COMPONENT,
                conn = self.conn.id,
                peer = %self.conn.peer,
                method = method.as_str(),
                reason = %e,
                "authentication failed"
            );
        }
        self.reply_error(e.error_code(), e.client_message())
    }

    // ---------------------------------------------------------------------------------------
    // Connection housekeeping
    // ---------------------------------------------------------------------------------------

    fn on_ping(&mut self, frame: &Frame, now: Instant) -> Flow {
        let ping = match self.decode(frame, now) {
            Ok(Message::Ping(ping)) => ping,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        let pong = Outgoing::Pong {
            id: ping.id,
            t0: ping.t0,
            t1: unix_now_ns(),
        };
        if self.conn.outbox.push_reliable(pong).is_err() {
            self.state.metrics.slow_consumer_disconnects.inc();
            return Flow::Close(End::SlowConsumer);
        }
        Flow::Continue
    }

    async fn on_disconnect(&mut self, frame: &Frame, now: Instant) -> Flow {
        let reason = match Message::from_frame(frame) {
            Ok(Message::Disconnect(d)) => Some(d.reason),
            _ => None,
        };
        if self.role() == Some(ClientRole::Engineer)
            && let Some(cell) = self.binding.take()
        {
            // An explicit disconnect ends the engineer's slot and kills its ticket (D-022), also
            // across relay restarts (its store row is deleted).
            self.state.registry.engineer_left(&cell, self.conn.id, now);
            flush_slot_writes(&self.state, &cell).await;
        }
        tracing::info!(component = COMPONENT, conn = self.conn.id, reason = ?reason, "client disconnected");
        Flow::Close(End::ClientDisconnect)
    }

    fn on_peer_error(&mut self, frame: &Frame, now: Instant) -> Flow {
        let error = match self.decode(frame, now) {
            Ok(Message::Error(error)) => error,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        tracing::warn!(
            component = COMPONENT,
            conn = self.conn.id,
            code = ?error.code,
            message = %truncated(&error.message),
            fatal = error.fatal,
            "client reported an error"
        );
        if error.fatal {
            Flow::Close(End::PeerError)
        } else {
            Flow::Continue
        }
    }

    // ---------------------------------------------------------------------------------------
    // Sessions
    // ---------------------------------------------------------------------------------------

    fn already_bound(&mut self, now: Instant) -> Flow {
        self.state.metrics.packet_validation_failures.inc();
        let reply = ErrorMsg {
            code: ErrorCode::NotAuthorized,
            message: "this connection already serves a session".into(),
            fatal: false,
        };
        self.violation(now, "already bound", Some(reply))
    }

    async fn on_session_create(&mut self, frame: &Frame, now: Instant) -> Flow {
        let create = match self.decode(frame, now) {
            Ok(Message::SessionCreate(create)) => create,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        if self.binding.is_some() {
            return self.already_bound(now);
        }
        let Some(authed) = self.authed() else {
            return Flow::Continue;
        };
        let driver = identity(&authed.account);
        if !self
            .state
            .limiters
            .admit_session_create(driver.account, now)
        {
            self.state.metrics.rate_limited.inc();
            tracing::warn!(component = COMPONENT, conn = self.conn.id, account = %driver.account, "SESSION_CREATE rate limit reached");
            return self.reply_error(
                ErrorCode::RateLimited,
                "too many sessions created; try again later",
            );
        }
        if self.state.registry.at_capacity() {
            tracing::warn!(
                component = COMPONENT,
                conn = self.conn.id,
                "session limit reached; SESSION_CREATE refused"
            );
            return self.reply_error(ErrorCode::ServerBusy, "the relay is at its session limit");
        }
        let (token, resume_hash) = match new_token() {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(component = COMPONENT, error = %e, "resume token not generated");
                return self.reply_error(ErrorCode::Internal, "internal relay error");
            }
        };
        let meta = session_meta(&create);
        // PostgreSQL keeps microseconds: use the stored precision everywhere.
        let started_at = truncate_to_micros(now_utc());
        let mut stored = None;
        for _ in 0..SESSION_ID_ATTEMPTS {
            let id = match random_u64() {
                Ok(v) => (v >> 1).max(1),
                Err(e) => {
                    tracing::error!(component = COMPONENT, error = %e, "session id not generated");
                    return self.reply_error(ErrorCode::Internal, "internal relay error");
                }
            };
            if self.state.registry.contains(id) {
                continue;
            }
            let record = SessionRecord {
                id,
                driver_account: driver.account,
                resume_hash: Some(resume_hash),
                meta: meta.clone(),
                started_at,
                status: SessionStatus::Live,
                close_reason: None,
                last_activity: started_at,
                ended_at: None,
            };
            match self.state.store.insert_session(&record).await {
                Ok(()) => {
                    stored = Some(record);
                    break;
                }
                Err(StoreError::Conflict(_)) => continue,
                Err(e) => {
                    tracing::error!(component = COMPONENT, conn = self.conn.id, error = %e, "session not stored");
                    return self
                        .reply_error(ErrorCode::Internal, "the session could not be stored");
                }
            }
        }
        let Some(record) = stored else {
            return self.reply_error(ErrorCode::Internal, "no free session id");
        };
        let new = NewSession {
            id: record.id,
            driver,
            meta,
            started_at_ns: datetime_to_ns(started_at),
            resume_token: token,
            resume_hash,
        };
        match self.state.registry.create_session(new, &self.conn, now) {
            Ok(created) => {
                self.binding = Some(created.cell);
                for id in created.replaced {
                    if let Err(e) = self
                        .state
                        .store
                        .close_session(id, CLOSE_REASON_REPLACED, now_utc())
                        .await
                    {
                        tracing::warn!(component = COMPONENT, session_id = id, error = %e, "replaced session not closed in the store");
                    }
                }
                Flow::Continue
            }
            Err(e) => {
                if let Err(se) = self
                    .state
                    .store
                    .close_session(record.id, CLOSE_REASON_RELAY_FULL, now_utc())
                    .await
                {
                    tracing::warn!(component = COMPONENT, session_id = record.id, error = %se, "refused session not closed in the store");
                }
                match e {
                    RegistryError::Full => {
                        self.reply_error(ErrorCode::ServerBusy, "the relay is at its session limit")
                    }
                    RegistryError::Random(e) => {
                        tracing::error!(component = COMPONENT, error = %e, "session not created");
                        self.reply_error(ErrorCode::Internal, "internal relay error")
                    }
                }
            }
        }
    }

    async fn on_session_join(&mut self, frame: &Frame, now: Instant) -> Flow {
        let join = match self.decode(frame, now) {
            Ok(Message::SessionJoin(join)) => join,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        if self.binding.is_some() {
            return self.already_bound(now);
        }
        let Some(authed) = self.authed() else {
            return Flow::Continue;
        };
        // The direction rules already refused SESSION_JOIN from driver connections.
        let engineer = identity(&authed.account);
        if !self
            .state
            .limiters
            .admit_join(engineer.account, self.conn.peer.ip(), now)
        {
            self.state.metrics.rate_limited.inc();
            self.state.metrics.join_failures.inc();
            tracing::warn!(
                component = COMPONENT,
                conn = self.conn.id,
                peer = %self.conn.peer,
                account = %engineer.account,
                "connection code redemption rate limit reached"
            );
            return self.reply_error(
                ErrorCode::RateLimited,
                "too many connection code attempts; try again later",
            );
        }
        let account = engineer.account;
        match self
            .state
            .registry
            .join(&join.code, engineer, &self.conn, now)
        {
            Ok(cell) => {
                flush_slot_writes(&self.state, &cell).await;
                self.binding = Some(cell);
                Flow::Continue
            }
            Err(e) => {
                self.state.metrics.join_failures.inc();
                tracing::info!(component = COMPONENT, conn = self.conn.id, account = %account, reason = %e, "SESSION_JOIN refused");
                self.send(&Message::Error(e.to_error_msg()));
                Flow::Continue
            }
        }
    }

    async fn on_session_resume(&mut self, frame: &Frame, now: Instant) -> Flow {
        let resume = match self.decode(frame, now) {
            Ok(Message::SessionResume(resume)) => resume,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        if self.binding.is_some() {
            return self.already_bound(now);
        }
        let Some(authed) = self.authed().cloned() else {
            return Flow::Continue;
        };
        let result = match (resume, authed.role) {
            (
                SessionResume::Driver {
                    session_id,
                    resume_token,
                    new_sequence_space,
                },
                ClientRole::Driver,
            ) => {
                self.resume_driver(
                    &authed.account,
                    session_id,
                    &resume_token,
                    new_sequence_space,
                    now,
                )
                .await
            }
            (SessionResume::Engineer { session_id, ticket }, ClientRole::Engineer) => {
                match self.state.registry.resume_engineer(
                    session_id,
                    &ticket,
                    authed.account.id,
                    &self.conn,
                    now,
                ) {
                    Err(ResumeError::NotInMemory) => {
                        match self.admit_store_lookup(&authed.account, now) {
                            Ok(()) => {
                                self.restore_engineer(&authed.account, session_id, &ticket, now)
                                    .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    other => other,
                }
            }
            _ => Err(ResumeError::NotAuthorized),
        };
        match result {
            Ok(cell) => {
                self.state.metrics.reconnects.inc();
                flush_slot_writes(&self.state, &cell).await;
                self.binding = Some(cell);
                Flow::Continue
            }
            Err(e) => {
                tracing::info!(component = COMPONENT, conn = self.conn.id, account = %authed.account.id, reason = %e, "SESSION_RESUME refused");
                match e {
                    // The relay's own failures are not charged to the client.
                    ResumeError::StoreUnavailable | ResumeError::Registry(_) => {
                        self.send(&Message::Error(e.to_error_msg()));
                        Flow::Continue
                    }
                    // A refused resume (unknown, closed or foreign session, dead credential, too
                    // many store lookups) is charged like an invalid frame, so probing session
                    // ids ends the connection.
                    _ => self.violation(now, "session resume refused", Some(e.to_error_msg())),
                }
            }
        }
    }

    /// Charges one store-backed resume lookup (the session is not in relay memory) to the
    /// account and source IP limits.
    fn admit_store_lookup(&self, account: &Account, now: Instant) -> Result<(), ResumeError> {
        if self
            .state
            .limiters
            .admit_resume_lookup(account.id, self.conn.peer.ip(), now)
        {
            Ok(())
        } else {
            self.state.metrics.rate_limited.inc();
            tracing::warn!(component = COMPONENT, conn = self.conn.id, peer = %self.conn.peer, account = %account.id, "session resume lookup rate limit reached");
            Err(ResumeError::RateLimited)
        }
    }

    async fn resume_driver(
        &mut self,
        account: &Account,
        session_id: u64,
        token: &Secret,
        new_sequence_space: bool,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let registry = &self.state.registry;
        if let Some(cell) = registry.cell(session_id) {
            // Registry change and store write in one order with the detach of the old connection.
            let _order = cell.store_order().await;
            let cell = registry.resume_driver(
                session_id,
                account.id,
                token,
                new_sequence_space,
                &self.conn,
                now,
            )?;
            self.store_status(&cell, SessionStatus::Live).await;
            return Ok(cell);
        }
        match registry.resume_driver(
            session_id,
            account.id,
            token,
            new_sequence_space,
            &self.conn,
            now,
        ) {
            Err(ResumeError::NotInMemory) => {
                self.admit_store_lookup(account, now)?;
                self.restore_driver(account, session_id, token, new_sequence_space, now)
                    .await
            }
            Ok(cell) => {
                {
                    let _order = cell.store_order().await;
                    self.store_status(&cell, SessionStatus::Live).await;
                }
                Ok(cell)
            }
            Err(e) => Err(e),
        }
    }

    /// Relay restart: the session is only in the store. Its metadata, driver account, resume
    /// digest and engineer slots survived (codes and ticket digests did not: relay memory only);
    /// the engineers come back with their tickets.
    async fn restore_driver(
        &mut self,
        account: &Account,
        session_id: u64,
        token: &Secret,
        new_sequence_space: bool,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let digest = presented_token_hash(token).ok_or(ResumeError::NotAuthorized)?;
        let record = self.stored_open_session(session_id).await?;
        let token_ok = record.resume_hash.is_some_and(|h| h.ct_eq(&digest));
        if !(token_ok && record.driver_account == account.id) {
            return Err(ResumeError::NotAuthorized);
        }
        self.driver_away_for(&record).await?;
        let stored = StoredSession {
            slots: self.stored_slots(session_id).await?,
            driver: identity(account),
            record,
        };
        let cell = self.state.registry.restore_session(
            &stored,
            token,
            new_sequence_space,
            &self.conn,
            now,
        )?;
        {
            let _order = cell.store_order().await;
            self.store_status(&cell, SessionStatus::Live).await;
        }
        Ok(cell)
    }

    /// Relay restart: an engineer app resumes with its in-memory ticket before (or without) the
    /// driver. The session, its driver and its engineer slots are read from the store; the ticket
    /// must carry a valid tag for the engineer's stored slot (`crate::tickets`).
    async fn restore_engineer(
        &mut self,
        account: &Account,
        session_id: u64,
        ticket: &Secret,
        now: Instant,
    ) -> Result<Arc<SessionCell>, ResumeError> {
        let record = self.stored_open_session(session_id).await?;
        let driver_away_for = self.driver_away_for(&record).await?;
        let slots = self.stored_slots(session_id).await?;
        // A stranger to the session learns nothing more than with a dead ticket.
        if !slots.iter().any(|s| s.record.account_id == account.id) {
            return Err(ResumeError::NotAuthorized);
        }
        let driver = match self.state.store.account_by_id(record.driver_account).await {
            Ok(Some(driver)) if !driver.disabled => identity(&driver),
            Ok(_) => return Err(ResumeError::Closed),
            Err(e) => {
                tracing::error!(component = COMPONENT, session_id, error = %e, "driver account lookup failed");
                return Err(ResumeError::StoreUnavailable);
            }
        };
        let stored = StoredSession {
            record,
            driver,
            slots,
        };
        self.state.registry.restore_for_engineer(
            &stored,
            driver_away_for,
            ticket,
            account.id,
            &self.conn,
            now,
        )
    }

    /// The stored record of an open session.
    async fn stored_open_session(&self, session_id: u64) -> Result<SessionRecord, ResumeError> {
        match self.state.store.session(session_id).await {
            Ok(Some(record)) if record.status != SessionStatus::Closed => Ok(record),
            Ok(Some(_)) => Err(ResumeError::Closed),
            Ok(None) => Err(ResumeError::NotFound),
            Err(e) => {
                tracing::error!(component = COMPONENT, session_id, error = %e, "session lookup failed");
                Err(ResumeError::StoreUnavailable)
            }
        }
    }

    /// How long the driver of a stored session has been away, measured as the registry measures
    /// grace: `last_activity` of a live session lags by up to one touch interval, so the window is
    /// grace + touch interval and the lag is not counted as absence. Past the window the session
    /// is closed in the store and `Closed` is returned.
    async fn driver_away_for(&self, record: &SessionRecord) -> Result<Duration, ResumeError> {
        let timeouts = &self.state.config.timeouts;
        let lag = timeouts.session_touch_interval;
        let window = timeouts.driver_grace.saturating_add(lag);
        let at = now_utc();
        // A last activity in the future (wall clock stepped back) counts as no absence.
        let since_activity = at
            .signed_duration_since(record.last_activity)
            .to_std()
            .unwrap_or(Duration::ZERO);
        if since_activity >= window {
            if let Err(e) = self
                .state
                .store
                .close_session(record.id, CLOSE_REASON_EXPIRED, at)
                .await
            {
                tracing::warn!(component = COMPONENT, session_id = record.id, error = %e, "expired session not closed in the store");
            }
            return Err(ResumeError::Closed);
        }
        Ok(since_activity.saturating_sub(lag))
    }

    /// The stored engineer slots of a session with their engineers' identities. Slots of missing
    /// or disabled accounts are left out (they cannot resume).
    async fn stored_slots(&self, session_id: u64) -> Result<Vec<StoredSlot>, ResumeError> {
        let unavailable = |e: StoreError| {
            tracing::error!(component = COMPONENT, session_id, error = %e, "engineer slots not read");
            ResumeError::StoreUnavailable
        };
        let records = self
            .state
            .store
            .engineer_slots(session_id)
            .await
            .map_err(unavailable)?;
        let mut slots = Vec::with_capacity(records.len());
        for record in records {
            match self
                .state
                .store
                .account_by_id(record.account_id)
                .await
                .map_err(unavailable)?
            {
                Some(engineer) if !engineer.disabled => slots.push(StoredSlot {
                    engineer: identity(&engineer),
                    record,
                }),
                _ => tracing::info!(
                    component = COMPONENT,
                    session_id,
                    connection_id = record.connection_id,
                    "stored engineer slot skipped: the account is gone or disabled"
                ),
            }
        }
        Ok(slots)
    }

    async fn store_status(&self, cell: &SessionCell, status: SessionStatus) {
        if let Err(e) = self
            .state
            .store
            .set_session_status(cell.id(), status, now_utc())
            .await
        {
            tracing::warn!(component = COMPONENT, session_id = cell.id(), error = %e, "session status not stored");
        }
    }

    async fn on_session_close(&mut self, frame: &Frame, now: Instant) -> Flow {
        let close = match self.decode(frame, now) {
            Ok(Message::SessionClose(close)) => close,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        let Some(cell) = self.binding.clone() else {
            return self.reply_error(
                ErrorCode::SessionNotFound,
                "no session is bound to this connection",
            );
        };
        if close.session_id != cell.id() {
            self.state.metrics.packet_validation_failures.inc();
            let reply = ErrorMsg {
                code: ErrorCode::NotAuthorized,
                message: "SESSION_CLOSE names another session".into(),
                fatal: false,
            };
            return self.violation(now, "session close mismatch", Some(reply));
        }
        {
            let _order = cell.store_order().await;
            if self
                .state
                .registry
                .close_by_driver(&cell, self.conn.id, now)
                && let Err(e) = self
                    .state
                    .store
                    .close_session(cell.id(), CLOSE_REASON_DRIVER_STOPPED, now_utc())
                    .await
            {
                tracing::warn!(component = COMPONENT, session_id = cell.id(), error = %e, "closed session not stored");
            }
        }
        self.binding = None;
        Flow::Continue
    }

    fn on_code_request(&mut self, frame: &Frame, now: Instant) -> Flow {
        if let Err(flow) = self.decode(frame, now) {
            return flow;
        }
        let Some(cell) = self.binding.clone() else {
            return self.reply_error(
                ErrorCode::SessionNotFound,
                "no session is bound to this connection",
            );
        };
        if !self.state.registry.request_code(&cell, self.conn.id, now) {
            return self.reply_error(
                ErrorCode::NotAuthorized,
                "this connection is not the session's driver",
            );
        }
        Flow::Continue
    }

    async fn on_kick(&mut self, frame: &Frame, now: Instant) -> Flow {
        let kick = match self.decode(frame, now) {
            Ok(Message::EngineerKick(kick)) => kick,
            Ok(_) => return Flow::Continue,
            Err(flow) => return flow,
        };
        let Some(cell) = self.binding.clone() else {
            return self.reply_error(
                ErrorCode::SessionNotFound,
                "no session is bound to this connection",
            );
        };
        // Unknown connection ids are ignored (§6). A removed slot's store row goes too, so its
        // ticket stays dead across a relay restart.
        if self
            .state
            .registry
            .kick(&cell, self.conn.id, kick.connection_id, now)
        {
            // The driver's reader never waits on the store: the slot deletion is written by a
            // tracked background task, in registry order (the session's store lock orders all
            // flushes); the driver connection's end flushes whatever is still queued.
            let state = Arc::clone(&self.state);
            self.state.tasks.spawn(async move {
                flush_slot_writes(&state, &cell).await;
            });
        }
        Flow::Continue
    }

    // ---------------------------------------------------------------------------------------
    // Stream frames
    // ---------------------------------------------------------------------------------------

    fn on_stream(&mut self, frame: &Frame, msg_type: MsgType, now: Instant) -> Flow {
        // The binding rules guarantee a bound session for stream frames.
        let Some(cell) = self.binding.clone() else {
            return Flow::Continue;
        };
        self.state.metrics.telemetry_packets_received.inc();
        match self.role() {
            Some(ClientRole::Driver) => {
                match self.state.registry.forward_driver_frame(
                    &cell,
                    self.conn.id,
                    frame,
                    msg_type,
                    unix_now_ns(),
                    now,
                ) {
                    Ok(out) => {
                        self.state
                            .metrics
                            .live_frames_dropped
                            .add(u64::from(out.dropped));
                        Flow::Continue
                    }
                    Err(reject) => self.forward_rejected(reject, now),
                }
            }
            Some(ClientRole::Engineer) => {
                // SNAPSHOT_REQUEST only (direction rules). An oversized payload is a violation;
                // otherwise the request is decoded and the registry re-encodes it, so the driver
                // never receives engineer bytes (a newer minor's appended bytes are dropped).
                if frame.header().payload_len > MAX_SNAPSHOT_REQUEST_PAYLOAD {
                    self.state.metrics.packet_validation_failures.inc();
                    let reply = ErrorMsg {
                        code: ErrorCode::Malformed,
                        message: format!(
                            "SNAPSHOT_REQUEST payload exceeds {MAX_SNAPSHOT_REQUEST_PAYLOAD} bytes"
                        ),
                        fatal: false,
                    };
                    return self.violation(now, "oversized snapshot request", Some(reply));
                }
                let request = match self.decode(frame, now) {
                    Ok(Message::SnapshotRequest(request)) => request,
                    // The header type is SNAPSHOT_REQUEST, so its decode yields nothing else.
                    Ok(_) => return Flow::Continue,
                    Err(flow) => return flow,
                };
                match self.state.registry.forward_snapshot_request(
                    &cell,
                    self.conn.id,
                    request,
                    frame.header(),
                    now,
                ) {
                    Ok(SnapshotRequestOutcome::Coalesced) => {
                        self.state.metrics.rate_limited.inc();
                        Flow::Continue
                    }
                    Ok(_) => Flow::Continue,
                    Err(reject) => self.forward_rejected(reject, now),
                }
            }
            None => Flow::Continue,
        }
    }

    fn forward_rejected(&mut self, reject: ForwardReject, now: Instant) -> Flow {
        match reject {
            // Replaced or closed meanwhile; this connection is being closed.
            ForwardReject::NotBound => {
                self.state.metrics.packet_validation_failures.inc();
                Flow::Continue
            }
            ForwardReject::RateLimited => {
                self.state.metrics.rate_limited.inc();
                let reply = ErrorMsg {
                    code: ErrorCode::RateLimited,
                    message: "session rate limit exceeded; frame dropped".into(),
                    fatal: false,
                };
                self.violation(now, "session rate limit", Some(reply))
            }
            ForwardReject::StaleSequence { last, got } => {
                self.state.metrics.packet_validation_failures.inc();
                if !self.stale_warned {
                    self.stale_warned = true;
                    tracing::warn!(
                        component = COMPONENT,
                        conn = self.conn.id,
                        session_id = self.bound_id(),
                        last,
                        got,
                        "stream frame with a non-increasing sequence dropped"
                    );
                }
                self.violation(now, "stale sequence", None)
            }
        }
    }

    /// The reader ended: a driver's session enters its grace period, an engineer's slot is held
    /// for its ticket.
    async fn finish(&mut self) {
        let Some(cell) = self.binding.take() else {
            return;
        };
        let now = Instant::now();
        match self.role() {
            Some(ClientRole::Driver) => {
                {
                    let _order = cell.store_order().await;
                    if self
                        .state
                        .registry
                        .driver_detached(&cell, self.conn.id, now)
                    {
                        self.store_status(&cell, SessionStatus::Grace).await;
                    }
                }
                // Slot deletions of kicks this driver made that a background flush has not
                // written yet (the flush takes the store lock itself).
                flush_slot_writes(&self.state, &cell).await;
            }
            Some(ClientRole::Engineer) => {
                self.state
                    .registry
                    .engineer_detached(&cell, self.conn.id, now);
                flush_slot_writes(&self.state, &cell).await;
            }
            None => {}
        }
    }
}

/// Writes the queued engineer-slot changes of a session to the store, in registry order: the
/// session's store lock is held while the queue is drained and applied, so concurrent flushes
/// cannot reorder them. A failed write is logged; the relay keeps working from memory, only a
/// later relay restart could restore that slot inaccurately.
pub(crate) async fn flush_slot_writes(state: &RelayState, cell: &SessionCell) {
    let _order = cell.store_order().await;
    for write in cell.take_slot_writes() {
        let result = match &write {
            SlotWrite::Put(slot) => state.store.put_engineer_slot(slot).await,
            SlotWrite::Delete {
                session_id,
                account,
                connection_id,
            } => {
                state
                    .store
                    .delete_engineer_slot(*session_id, *account, *connection_id)
                    .await
            }
        };
        if let Err(e) = result {
            tracing::error!(
                component = COMPONENT,
                session_id = cell.id(),
                error = %e,
                "engineer slot change not stored; a relay restart may restore this slot inaccurately"
            );
        }
    }
}

/// Writes queued frames until the outbox closes or the connection is aborted. Relay trailers and
/// PONG `t2` are stamped here, right before the bytes are handed to the socket.
pub(crate) async fn writer_loop<W: AsyncWrite + Unpin>(
    state: Arc<RelayState>,
    conn: Arc<ConnHandle>,
    mut wr: W,
) {
    let stall = state.config.timeouts.write_stall;
    let mut buf: Vec<u8> = Vec::with_capacity(READ_CHUNK);
    let mut live_rx: Vec<Instant> = Vec::new();
    loop {
        let batch = conn.outbox.take_batch(MAX_BATCH_BYTES);
        if batch.items.is_empty() {
            if batch.closing {
                break;
            }
            tokio::select! {
                _ = conn.outbox.notified() => continue,
                _ = conn.cancelled() => break,
                _ = state.hard_stop.cancelled() => break,
            }
        }
        buf.clear();
        live_rx.clear();
        let mut dropped = batch.dropped;
        let mut forwarded = 0u64;
        for item in batch.items {
            match item {
                Outgoing::Frame(frame) => buf.extend_from_slice(frame.as_bytes()),
                Outgoing::Forward { frame, replayed } => {
                    let relay_dropped = if replayed {
                        0
                    } else {
                        std::mem::take(&mut dropped)
                    };
                    let trailer = RelayTrailer {
                        relay_rx_ns: frame.rx_ns,
                        relay_tx_ns: unix_now_ns(),
                        relay_dropped,
                        replayed,
                    };
                    frame.encode_into(&mut buf, &trailer);
                    forwarded += 1;
                    if !replayed {
                        live_rx.push(frame.rx_at);
                    }
                }
                Outgoing::Pong { id, t0, t1 } => {
                    let pong = Message::Pong(Pong {
                        id,
                        t0,
                        t1,
                        t2: unix_now_ns(),
                    });
                    let meta = FrameMeta::control(NO_SESSION, mono_ns_since(state.epoch));
                    match pong.to_frame(&meta) {
                        Ok(frame) => buf.extend_from_slice(frame.as_bytes()),
                        Err(e) => {
                            tracing::error!(component = COMPONENT, conn = conn.id, error = %e, "PONG not encoded")
                        }
                    }
                }
            }
        }
        // `write_all` on a TLS stream may return while ciphertext is still buffered inside rustls
        // (the socket was full); `flush` drives it onto the socket. Without it the tail of a batch
        // could wait for the next write indefinitely.
        let send = async {
            wr.write_all(&buf).await?;
            wr.flush().await
        };
        let write = tokio::select! {
            r = tokio::time::timeout(stall, send) => r,
            _ = state.hard_stop.cancelled() => break,
        };
        match write {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::debug!(component = COMPONENT, conn = conn.id, error = %e, "write failed");
                break;
            }
            Err(_) => {
                state.metrics.slow_consumer_disconnects.inc();
                tracing::warn!(component = COMPONENT, conn = conn.id, peer = %conn.peer, "socket accepted no data within the write-stall limit; disconnecting");
                break;
            }
        }
        let written = Instant::now();
        state.metrics.bytes_forwarded.add(buf.len() as u64);
        state.metrics.telemetry_packets_forwarded.add(forwarded);
        for rx_at in &live_rx {
            let us = u64::try_from(written.saturating_duration_since(*rx_at).as_micros())
                .unwrap_or(u64::MAX);
            state.metrics.relay_forward_latency.observe_us(us);
        }
        if buf.capacity() > WRITE_BUFFER_KEEP {
            buf = Vec::with_capacity(READ_CHUNK);
        }
    }
    let _ = tokio::time::timeout(SHUTDOWN_FLUSH, wr.shutdown()).await;
    conn.abort();
}

fn identity(account: &Account) -> Identity {
    Identity {
        account: account.id,
        username: account.username.clone(),
        display_name: account.display_name.clone(),
    }
}

/// Client-supplied text cut to [`MAX_TEXT_CHARS`] characters.
fn truncated(text: &str) -> String {
    text.chars().take(MAX_TEXT_CHARS).collect()
}

fn bounded(text: Option<&str>) -> Option<String> {
    text.map(truncated).filter(|t| !t.is_empty())
}

/// Session metadata from SESSION_CREATE (identity only, never telemetry).
fn session_meta(create: &SessionCreate) -> SessionMeta {
    let id = &create.identity;
    SessionMeta {
        car: bounded(id.vehicle_name.as_deref().or(id.vehicle_model.as_deref())),
        vehicle_class: bounded(id.class_name.as_deref()),
        track: bounded(id.track_name.as_deref()),
        session_type: id.session_type.map(SessionType::raw),
        lmu_version: bounded(create.lmu_version.as_deref()),
        app_version: truncated(&create.app_version),
        schema_version: create.schema_version,
    }
}

fn truncate_to_micros(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(at.timestamp_micros()).unwrap_or(at)
}

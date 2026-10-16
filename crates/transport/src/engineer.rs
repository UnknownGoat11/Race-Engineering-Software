//! Engineer client: joins a driver with a one-time connection code and receives the live stream
//! (D-022, PROTOCOL.md §5–§6).
//!
//! * [`EngineerClient::join`] normalizes the code (malformed input never reaches the network),
//!   connects, authenticates silently and redeems the code with SESSION_JOIN. It returns the
//!   [`IdentityCard`] (driver, car, class, track, session type, start) or a typed [`JoinError`]
//!   (`CodeInvalid`, `CodeExpired`, `SessionFull`, `NotAnEngineer`, `RateLimited`, ...). A join is
//!   attempted once: codes are single use, so it is never retried automatically.
//! * The stream is shown only after [`EngineerClient::confirm`]. Frames received before that are
//!   tracked and gated but buffered (latest frame per latest-wins key, newest SCHEMA_NEGOTIATE,
//!   bounded FIFO of SESSION_EVENTs) and delivered in sequence order on confirmation.
//!   [`EngineerClient::cancel`] sends DISCONNECT (the code stays used), also while the join or a
//!   resume is still waiting for its reply: once SESSION_JOIN / SESSION_RESUME was sent, the client
//!   never leaves without DISCONNECT on cancel, shutdown or a failed join (timeout, invalid reply),
//!   so the relay frees a slot it may already have assigned instead of holding it for the ticket
//!   lifetime.
//! * After a network blip the client reconnects by itself with the in-memory resume ticket from the
//!   newest SESSION_INFO (`SESSION_RESUME::Engineer`), then sends SNAPSHOT_REQUEST{Reconnect}. The
//!   relay may issue a new ticket with every SESSION_INFO (join, resume, driver restart); the client
//!   always adopts it. The ticket is never written anywhere and is dropped on disconnect, cancel,
//!   removal by the driver, session end, shutdown, or [`crate::TimingConfig::engineer_ticket_ttl`]
//!   after the drop. Then the phase is [`EngineerPhase::Ended`] ("enter a new code from the
//!   driver"). Nothing about the driver or session survives an app restart: the engineer never
//!   auto-connects. A resume whose SESSION_INFO was lost (the link dropped again before it arrived)
//!   is retried with the previous ticket: that only works if the relay honours a superseded ticket
//!   for a grace period (PROTOCOL.md §6 network blips; a relay requirement).
//! * Inbound: only types the relay may send to an engineer are processed; stream frames go through
//!   `protocol::sequence::StreamReceiver` (loss accounting, per-key live gate, join replay,
//!   SESSION_INFO resets) and only live frames reach the application.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use protocol::frame::{Delivery, Endpoint, FrameHeader, FramePriority, MsgType, RelayTrailer};
use protocol::messages::{
    ClientRole, ConnectCodeError, DisconnectReason, ErrorCode, Message, ResumeCredential, Secret,
    ServerHello, SessionCloseReason, SessionInfo, SessionJoin, SessionResume, SnapshotReason,
    SnapshotRequest, normalize_connect_code,
};
use protocol::sequence::{Arrival, SequenceStats, StreamReceiver};
use protocol::version::Environment;
use protocol::{Frame, clock::ClockEstimate};
use telemetry_model::enums::SessionType;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::auth::AuthManager;
use crate::clock::{LinkClock, relay_time_of};
use crate::config::{ClientConfig, ConfigError, DEFAULT_COMMAND_CAPACITY};
use crate::driver::{ClientClosed, SessionReply, await_session_info};
use crate::handshake::{Disposition, HandshakeError, Link};
use crate::metrics::LinkMetrics;
use crate::queue::{Outbound, QueueError};
use crate::session::{
    ClientCore, CloseAction, ConnectionEnd, Ctx, EstablishError, Established, Flow, RoleLogic,
    disconnect, error_chain, establish, run_connection,
};
use crate::state::{
    ConnectionState, ConnectionStatus, DegradedReason, EngineerEndReason, FailureReason,
    ReloginCause,
};
use crate::transport::TelemetryTransport;

const COMPONENT: &str = "transport.engineer";
/// Time allowed for delivering an application event outside the receive loop.
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
/// Time `shutdown` waits for the client task before aborting it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The identity of a driver session as SESSION_INFO announces it, without the resume credential:
/// what the engineer confirms before monitoring (D-022, spec §247), and what the driver's
/// [`crate::DriverEvent::SessionReady`] carries. Safe to hand to the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct IdentityCard {
    pub session_id: u64,
    pub driver_display_name: String,
    pub driver_username: String,
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub track: Option<String>,
    pub session_type: Option<SessionType>,
    /// Session start, relay wall clock ns since the Unix epoch.
    pub started_at_ns: u64,
    pub schema_version: u16,
}

impl From<&SessionInfo> for IdentityCard {
    fn from(info: &SessionInfo) -> Self {
        Self {
            session_id: info.session_id,
            driver_display_name: info.driver_display_name.clone(),
            driver_username: info.driver_username.clone(),
            car: info.car.clone(),
            vehicle_class: info.vehicle_class.clone(),
            track: info.track.clone(),
            session_type: info.session_type,
            started_at_ns: info.started_at_ns,
            schema_version: info.schema_version,
        }
    }
}

/// Where the engineer is in the D-022 flow.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum EngineerPhase {
    /// Not connected to a driver: "Connect to driver" (enter a code).
    #[default]
    Idle,
    /// A code is being redeemed.
    Joining,
    /// Joined; the identity card is shown, the stream is held until confirmed.
    AwaitingConfirmation(IdentityCard),
    /// Confirmed; the stream is delivered.
    Monitoring(IdentityCard),
    /// The connection to the driver ended for good; only a new code connects again.
    Ended(EngineerEndReason),
}

/// Why a join failed. Every variant has a user-facing [`JoinError::message`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JoinError {
    #[error("the code is not a valid connection code")]
    CodeMalformed(#[source] ConnectCodeError),
    #[error("the code is not valid (wrong, already used or replaced)")]
    CodeInvalid,
    #[error("the code has expired")]
    CodeExpired,
    #[error("the driver already has {max} engineers connected")]
    SessionFull { max: u16 },
    #[error("the driver's session is no longer live")]
    SessionClosed,
    #[error("this account is not an engineer account")]
    NotAnEngineer,
    #[error("too many attempts, wait a minute")]
    RateLimited,
    #[error("log in required")]
    ReloginRequired(ReloginCause),
    #[error("cannot reach the relay: {detail}")]
    Unreachable { detail: String },
    #[error("the relay is incompatible: {detail}")]
    Incompatible { detail: String },
    #[error("the relay environment {announced:?} does not match {expected:?}")]
    EnvironmentMismatch {
        expected: Environment,
        announced: Environment,
    },
    #[error("the relay refused: {code:?} ({message})")]
    Refused { code: ErrorCode, message: String },
    #[error("already connected or connecting to a driver")]
    Busy,
    #[error("cancelled")]
    Cancelled,
    #[error("the client has shut down")]
    ClientClosed,
}

impl JoinError {
    /// User-facing text.
    pub fn message(&self) -> String {
        match self {
            JoinError::CodeMalformed(_) => {
                "That is not a connection code. It has 8 characters, e.g. 7KQ4-MZ9T.".to_owned()
            }
            JoinError::CodeInvalid => {
                "This code is not valid. Ask the driver for a new code.".to_owned()
            }
            JoinError::CodeExpired => {
                "This code has expired. Ask the driver for a new code.".to_owned()
            }
            JoinError::SessionFull { max } => {
                format!("Driver already has {max} engineers connected")
            }
            JoinError::SessionClosed => "The driver's session has ended.".to_owned(),
            JoinError::NotAnEngineer => "This account is not an engineer account.".to_owned(),
            JoinError::RateLimited => "Too many attempts. Wait a minute and try again.".to_owned(),
            JoinError::ReloginRequired(_) => "Please log in again.".to_owned(),
            other => other.to_string(),
        }
    }
}

/// Engineer-side errors of `confirm` / `request_snapshot`.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum EngineerError {
    #[error("not connected to a driver")]
    NotConnected,
    #[error("the snapshot request could not be queued")]
    Queue(#[from] QueueError),
    #[error("the client has shut down")]
    ClientClosed,
}

/// A live stream frame delivered to the application.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundFrame {
    pub message: Message,
    pub header: FrameHeader,
    pub trailer: Option<RelayTrailer>,
    /// Sequence classification; `None` for frames the relay replayed on join (not tracked, §5).
    pub arrival: Option<Arrival>,
    /// Received at, ns of the client's monotonic clock.
    pub received_at_ns: u64,
}

impl InboundFrame {
    /// Whether the relay replayed this frame from its cache on join (§6).
    pub fn replayed(&self) -> bool {
        self.trailer.is_some_and(|t| t.replayed)
    }
}

/// Events of the engineer client.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineerEvent {
    /// A live stream frame (only after confirmation; latest-wins types may be dropped if the
    /// application falls behind, counted in the metrics).
    Frame(Box<InboundFrame>),
    /// SESSION_INFO after a resume or a driver restart: the sequence space restarted, reset any
    /// stream-derived state (the replayed snapshot follows). No new confirmation is needed.
    StreamReset {
        /// The session as the relay announced it now.
        card: IdentityCard,
        /// After a network drop: the interval in which reliable events may be missing (§6). `None`
        /// when the link did not drop (the driver restarted its sequence space).
        gap: Option<StreamGap>,
    },
    /// The connection to the driver ended for good.
    Ended(EngineerEndReason),
    /// A non-fatal error reported by the relay.
    RelayError { code: ErrorCode, message: String },
    /// The driver disconnected; the relay holds the session for the grace period (H1).
    DriverAway {
        since_ns: u64,
        grace_expires_at_ns: u64,
    },
    /// The driver reconnected to the session (H1).
    DriverBack,
}

/// The interval of a network drop, in nanoseconds of the client's monotonic clock (the epoch of
/// [`InboundFrame::received_at_ns`] and [`EngineerClient::now_ns`]). Events of the driver that
/// happened in it may be missing (§6: the relay does not buffer reliable messages for a
/// disconnected engineer); the replayed snapshot restores the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamGap {
    /// When the client detected the link loss (the stream may have stopped somewhat earlier,
    /// up to the liveness timeout).
    pub dropped_at_ns: u64,
    /// When the resume's SESSION_INFO arrived.
    pub resumed_at_ns: u64,
}

type JoinReply = oneshot::Sender<Result<IdentityCard, JoinError>>;

enum EngineerCommand {
    Join {
        code: Secret,
        reply: JoinReply,
    },
    Confirm {
        reply: oneshot::Sender<Result<(), EngineerError>>,
    },
    Cancel,
    Disconnect,
    RequestSnapshot {
        reason: SnapshotReason,
        reply: oneshot::Sender<Result<(), EngineerError>>,
    },
    Shutdown,
}

impl std::fmt::Debug for EngineerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never prints the code.
        f.write_str(match self {
            EngineerCommand::Join { .. } => "Join",
            EngineerCommand::Confirm { .. } => "Confirm",
            EngineerCommand::Cancel => "Cancel",
            EngineerCommand::Disconnect => "Disconnect",
            EngineerCommand::RequestSnapshot { .. } => "RequestSnapshot",
            EngineerCommand::Shutdown => "Shutdown",
        })
    }
}

/// Handle of a running engineer client.
#[derive(Debug)]
pub struct EngineerClient {
    commands: mpsc::Sender<EngineerCommand>,
    clock: LinkClock,
    status: watch::Receiver<ConnectionStatus>,
    metrics: watch::Receiver<LinkMetrics>,
    phase: watch::Receiver<EngineerPhase>,
    task: JoinHandle<()>,
}

impl EngineerClient {
    /// Spawns the client task (idle until [`EngineerClient::join`]). It never connects by itself.
    /// Fails if the configuration is unusable ([`ClientConfig::validate`]).
    pub fn spawn<T: TelemetryTransport>(
        config: ClientConfig,
        transport: T,
        auth: Arc<AuthManager>,
    ) -> Result<(EngineerClient, mpsc::Receiver<EngineerEvent>), ConfigError> {
        config.validate()?;
        let event_capacity = config.event_capacity.max(1);
        let pending_capacity = config.pending_reliable_capacity.max(1);
        let (core, status, metrics) = ClientCore::new(config, auth, COMPONENT);
        let clock = core.clock;
        let (commands_tx, commands) = mpsc::channel(DEFAULT_COMMAND_CAPACITY);
        let (events, events_rx) = mpsc::channel(event_capacity);
        let (phase_tx, phase) = watch::channel(EngineerPhase::Idle);
        let status_pub = core.status.clone();
        let phase_watcher = Arc::new(phase_tx);
        let supervisor = Supervisor {
            core,
            transport,
            commands,
            events,
            phase: Arc::clone(&phase_watcher),
            pending_capacity,
        };
        let supervisor_task = tokio::spawn(supervisor.run());
        let task = tokio::spawn(async move {
            struct AbortGuard(Option<tokio::task::JoinHandle<()>>);
            impl Drop for AbortGuard {
                fn drop(&mut self) {
                    if let Some(h) = self.0.take() {
                        h.abort();
                    }
                }
            }
            let mut guard = AbortGuard(Some(supervisor_task));
            if let Some(h) = guard.0.as_mut() {
                let res = h.await;
                guard.0 = None;
                if let Err(e) = res
                    && e.is_panic()
                {
                    tracing::error!(component = COMPONENT, "engineer supervisor panicked");
                    status_pub.fail(FailureReason::Configuration {
                        detail: "internal error: supervisor panicked".to_owned(),
                    });
                    phase_watcher
                        .send_replace(EngineerPhase::Ended(EngineerEndReason::Configuration));
                }
            }
        });
        Ok((
            EngineerClient {
                commands: commands_tx,
                clock,
                status,
                metrics,
                phase,
                task,
            },
            events_rx,
        ))
    }

    /// Redeems a connection code typed by the user (`7kq4-mz9t` and `7KQ4MZ9T` both work) and
    /// returns the identity to confirm.
    pub async fn join(&self, code: &str) -> Result<IdentityCard, JoinError> {
        let code = normalize_connect_code(code).map_err(JoinError::CodeMalformed)?;
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(EngineerCommand::Join { code, reply })
            .await
            .map_err(|_| JoinError::ClientClosed)?;
        rx.await.unwrap_or(Err(JoinError::ClientClosed))
    }

    /// Confirms the identity card: the stream (including what arrived meanwhile) is delivered.
    pub async fn confirm(&self) -> Result<(), EngineerError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(EngineerCommand::Confirm { reply })
            .await
            .map_err(|_| EngineerError::ClientClosed)?;
        rx.await.unwrap_or(Err(EngineerError::ClientClosed))
    }

    /// Cancels at the identity card (or a join in progress): DISCONNECT, the code stays used.
    pub async fn cancel(&self) -> Result<(), ClientClosed> {
        self.command(EngineerCommand::Cancel).await
    }

    /// Disconnects from the driver; the ticket is dropped, a new code is needed to come back.
    /// Also acknowledges an ended connection (→ idle).
    pub async fn disconnect(&self) -> Result<(), ClientClosed> {
        self.command(EngineerCommand::Disconnect).await
    }

    /// Asks the driver for a fresh snapshot.
    pub async fn request_snapshot(&self, reason: SnapshotReason) -> Result<(), EngineerError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(EngineerCommand::RequestSnapshot { reason, reply })
            .await
            .map_err(|_| EngineerError::ClientClosed)?;
        rx.await.map_err(|_| EngineerError::ClientClosed)?
    }

    /// Disconnects and ends the client task (the ticket dies with it).
    pub async fn shutdown(self) {
        let EngineerClient {
            commands, mut task, ..
        } = self;
        let _ = commands.send(EngineerCommand::Shutdown).await;
        drop(commands);
        if timeout(SHUTDOWN_TIMEOUT, &mut task).await.is_err() {
            task.abort();
        }
    }

    async fn command(&self, command: EngineerCommand) -> Result<(), ClientClosed> {
        self.commands.send(command).await.map_err(|_| ClientClosed)
    }

    /// Connection status.
    pub fn status(&self) -> watch::Receiver<ConnectionStatus> {
        self.status.clone()
    }

    /// Link metrics (about once per second), including sequence statistics.
    pub fn metrics(&self) -> watch::Receiver<LinkMetrics> {
        self.metrics.clone()
    }

    /// The D-022 phase.
    pub fn phase(&self) -> watch::Receiver<EngineerPhase> {
        self.phase.clone()
    }

    /// Current clock offset to the relay (for sample ages, PROTOCOL.md §8). It applies to times on
    /// [`ClientConfig::mono_epoch`] (e.g. [`InboundFrame::received_at_ns`], [`Self::now_ns`]).
    pub fn clock(&self) -> Option<ClockEstimate> {
        self.metrics.borrow().clock
    }

    /// Nanoseconds of the client's monotonic clock now (the epoch of `received_at_ns`).
    pub fn now_ns(&self) -> u64 {
        self.clock.now_ns()
    }

    /// The relay clock (ns since the Unix epoch) at `at`, e.g. `Instant::now()` for the `now` of
    /// a sample age (PROTOCOL.md §8). `None` until the clock offset was measured.
    pub fn relay_time_of(&self, at: std::time::Instant) -> Option<i64> {
        relay_time_of(&self.clock, self.clock(), at)
    }
}

/// The engineer's session while joined (memory only).
struct Joined {
    session_id: u64,
    /// In-memory resume ticket. Never persisted (D-022).
    ticket: Secret,
    card: IdentityCard,
    confirmed: bool,
}

/// How a phase ended.
enum Exit {
    Idle,
    Shutdown,
}

/// A user command that interrupted connecting.
enum Interrupt {
    Cancel,
    Shutdown,
}

/// A connection bound to the driver's session.
struct BoundConn<R, W> {
    link: Link<R, W>,
    server: ServerHello,
    info: SessionInfo,
    /// Frames that arrived during the binding after SESSION_INFO.
    early: Vec<Frame>,
}

/// Result of a (cancellable) connect + bind.
enum Connected<R, W> {
    Bound(Box<BoundConn<R, W>>),
    Failed(BindFailure),
    Interrupted(Interrupt),
}

/// Why connect + bind failed.
enum BindFailure {
    Establish(EstablishError),
    Handshake(HandshakeError),
    /// The relay answered the SESSION_JOIN / SESSION_RESUME with an ERROR. `max_engineers` is the
    /// slot limit the relay announced in its HELLO (for "Driver already has N engineers").
    Refused {
        code: ErrorCode,
        message: String,
        max_engineers: u16,
    },
}

impl BindFailure {
    fn is_security_issue(&self) -> bool {
        match self {
            Self::Establish(e) => e.is_security_issue(),
            _ => false,
        }
    }
}

struct Supervisor<T> {
    core: ClientCore,
    transport: T,
    commands: mpsc::Receiver<EngineerCommand>,
    events: mpsc::Sender<EngineerEvent>,
    phase: Arc<watch::Sender<EngineerPhase>>,
    pending_capacity: usize,
}

/// What to send to bind the session.
enum BindRequest {
    Join(Secret),
    Resume { session_id: u64, ticket: Secret },
}

impl<T: TelemetryTransport> Supervisor<T> {
    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            match command {
                EngineerCommand::Join { code, reply } => {
                    if let Exit::Shutdown = self.join(code, reply).await {
                        break;
                    }
                }
                EngineerCommand::Confirm { reply } => {
                    let _ = reply.send(Err(EngineerError::NotConnected));
                }
                // Acknowledges FAILED / Ended.
                EngineerCommand::Cancel | EngineerCommand::Disconnect => self.idle(),
                EngineerCommand::RequestSnapshot { reply, .. } => {
                    let _ = reply.send(Err(EngineerError::NotConnected));
                }
                EngineerCommand::Shutdown => break,
            }
        }
        self.core.queue.clear_session();
        self.core.queue.close();
        self.core.status.disconnected();
        self.phase.send_replace(EngineerPhase::Idle);
        tracing::info!(component = COMPONENT, "engineer client stopped");
    }

    fn idle(&mut self) {
        self.core.queue.clear_session();
        self.core.status.disconnected();
        self.phase.send_replace(EngineerPhase::Idle);
    }

    async fn event(&mut self, event: EngineerEvent) {
        if !matches!(
            timeout(EVENT_TIMEOUT, self.events.send(event)).await,
            Ok(Ok(()))
        ) && !self.events.is_closed()
        {
            self.core.metrics.event_dropped();
            tracing::warn!(
                component = COMPONENT,
                "engineer event dropped: event channel full"
            );
        }
    }

    /// Redeems a code once (no automatic retry: codes are single use).
    async fn join(&mut self, code: Secret, reply: JoinReply) -> Exit {
        // A new code is a new user action: it acknowledges an ended connection (FAILED).
        self.core.status.acknowledge_failure();
        self.phase.send_replace(EngineerPhase::Joining);
        self.core.backoff.reset();
        match self.connect(BindRequest::Join(code), None).await {
            Connected::Interrupted(interrupt) => {
                let _ = reply.send(Err(JoinError::Cancelled));
                self.idle();
                match interrupt {
                    Interrupt::Cancel => Exit::Idle,
                    Interrupt::Shutdown => Exit::Shutdown,
                }
            }
            Connected::Failed(failure) => {
                let error = join_error(&failure);
                tracing::info!(component = COMPONENT, error = %error, "join failed");
                match &error {
                    JoinError::ReloginRequired(cause) => {
                        self.core
                            .status
                            .fail(FailureReason::ReloginRequired(*cause));
                    }
                    other => {
                        let text = other.to_string();
                        self.core.status.disconnected();
                        self.core.status.update(|s| s.last_error = Some(text));
                    }
                }
                self.phase.send_replace(EngineerPhase::Idle);
                let _ = reply.send(Err(error));
                Exit::Idle
            }
            Connected::Bound(bound) => {
                let BoundConn {
                    mut link,
                    server,
                    info,
                    early,
                } = *bound;
                let Some(ticket) = engineer_ticket(&info) else {
                    // Unusable without a ticket, but the relay bound the connection to a slot:
                    // release it explicitly.
                    link.goodbye(self.core.config.timing.close_timeout).await;
                    let _ = reply.send(Err(JoinError::Refused {
                        code: ErrorCode::Malformed,
                        message: "SESSION_INFO without an engineer ticket".to_owned(),
                    }));
                    self.idle();
                    return Exit::Idle;
                };
                let card = IdentityCard::from(&info);
                let joined = Joined {
                    session_id: info.session_id,
                    ticket,
                    card: card.clone(),
                    confirmed: false,
                };
                self.core.reset_sequence();
                self.core.queue.clear_session();
                self.phase
                    .send_replace(EngineerPhase::AwaitingConfirmation(card.clone()));
                tracing::info!(
                    component = COMPONENT,
                    session_id = info.session_id,
                    "joined, awaiting confirmation"
                );
                let _ = reply.send(Ok(card));
                self.monitor(joined, link, server, early).await
            }
        }
    }

    /// Runs the joined session across reconnects until it ends.
    async fn monitor(
        &mut self,
        mut joined: Joined,
        link: Link<T::Reader, T::Writer>,
        server: ServerHello,
        early: Vec<Frame>,
    ) -> Exit {
        let mut next = Some((link, server, early));
        loop {
            let Some((link, server, early)) = next.take() else {
                return Exit::Idle;
            };
            let mut logic = EngineerLogic {
                receiver: StreamReceiver::new(),
                confirmed: joined.confirmed,
                card: joined.card.clone(),
                pending: PendingBuffer::new(self.pending_capacity),
                phase: Arc::clone(&self.phase),
                exit: None,
                new_ticket: None,
                pending_arrival: None,
                stale_after: self.core.config.timing.liveness.stale_after,
            };
            let end = run_connection(
                &mut self.core,
                link,
                &server,
                joined.session_id,
                early,
                &mut logic,
                &mut self.commands,
                &self.events,
            )
            .await;
            joined.confirmed = logic.confirmed;
            joined.card = logic.card.clone();
            if let Some(ticket) = logic.new_ticket.take() {
                joined.ticket = ticket;
            }
            // The next connection starts a new stream epoch (fresh receiver): keep this one's
            // counters in the cumulative totals.
            self.core.metrics.sequence_reset(&logic.receiver.stats());
            match end {
                ConnectionEnd::Closed { flushed, .. } => {
                    if !flushed {
                        tracing::warn!(
                            component = COMPONENT,
                            "engineer disconnect was not flushed; slot may stay held up to ticket TTL"
                        );
                    }
                    // Cancel, disconnect or shutdown: the ticket dies with `joined`.
                    self.idle();
                    return match logic.exit {
                        Some(Interrupt::Cancel) => Exit::Idle,
                        Some(Interrupt::Shutdown) | None => Exit::Shutdown,
                    };
                }
                ConnectionEnd::Failed(reason) => {
                    let end_reason = end_reason_of(&reason);
                    return self.ended(end_reason, reason).await;
                }
                ConnectionEnd::Lost(error) => match self.resume(&mut joined, error).await {
                    Resumed::Connected(bound) => {
                        let BoundConn {
                            link,
                            server,
                            early,
                            ..
                        } = *bound;
                        next = Some((link, server, early));
                    }
                    Resumed::Exit(exit) => return exit,
                },
            }
        }
    }

    /// The connection to the driver ended for good.
    async fn ended(&mut self, reason: EngineerEndReason, failure: FailureReason) -> Exit {
        tracing::info!(
            component = COMPONENT,
            ?reason,
            "connection to the driver ended"
        );
        self.core.queue.clear_session();
        self.core.status.fail(failure);
        self.phase.send_replace(EngineerPhase::Ended(reason));
        self.event(EngineerEvent::Ended(reason)).await;
        Exit::Idle
    }

    /// Reconnects with the in-memory ticket until it works, the ticket is refused or expires, or
    /// the user disconnects.
    async fn resume(&mut self, joined: &mut Joined, first_error: String) -> Resumed<T> {
        let dropped_at = Instant::now();
        let dropped_at_ns = self.core.clock.now_ns();
        let ttl = self.core.config.timing.engineer_ticket_ttl;
        let mut error = first_error;
        loop {
            if dropped_at.elapsed() >= ttl {
                let (reason, failure) = if self.core.status.status().security_issue {
                    (
                        EngineerEndReason::Configuration,
                        FailureReason::Configuration {
                            detail: "resume aborted due to untrusted relay TLS certificate"
                                .to_owned(),
                        },
                    )
                } else {
                    let reason = EngineerEndReason::TicketExpired;
                    (reason, FailureReason::SessionEnded(reason))
                };
                return Resumed::Exit(self.ended(reason, failure).await);
            }
            let Some(delay) = self.core.after_failure(Disposition::Retry, error) else {
                return Resumed::Exit(Exit::Idle);
            };
            // Backoff, still serving commands.
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    () = &mut sleep => break,
                    command = self.commands.recv() => match command {
                        Some(EngineerCommand::Confirm { reply }) => {
                            confirm_offline(joined, &self.phase);
                            let _ = reply.send(Ok(()));
                        }
                        Some(EngineerCommand::Join { reply, .. }) => {
                            let _ = reply.send(Err(JoinError::Busy));
                        }
                        Some(EngineerCommand::RequestSnapshot { reply, .. }) => {
                            let _ = reply.send(Err(EngineerError::NotConnected));
                        }
                        Some(EngineerCommand::Cancel | EngineerCommand::Disconnect) => {
                            self.idle();
                            return Resumed::Exit(Exit::Idle);
                        }
                        None | Some(EngineerCommand::Shutdown) => {
                            self.idle();
                            return Resumed::Exit(Exit::Shutdown);
                        }
                    },
                }
            }
            let request = BindRequest::Resume {
                session_id: joined.session_id,
                ticket: joined.ticket.clone(),
            };
            match self.connect(request, Some(&mut *joined)).await {
                Connected::Bound(bound) => {
                    if let Some(ticket) = engineer_ticket(&bound.info) {
                        joined.ticket = ticket;
                    }
                    joined.card = IdentityCard::from(&bound.info);
                    self.core.metrics.reconnected();
                    // Ask for a fresh snapshot (the relay also replays its cache, §6).
                    if let Err(e) = self
                        .core
                        .queue
                        .try_push_reliable(snapshot_request(SnapshotReason::Reconnect))
                    {
                        tracing::warn!(component = COMPONENT, error = %e, "snapshot request after reconnect not queued");
                    }
                    if joined.confirmed {
                        let gap = StreamGap {
                            dropped_at_ns,
                            resumed_at_ns: self.core.clock.now_ns(),
                        };
                        self.event(EngineerEvent::StreamReset {
                            card: joined.card.clone(),
                            gap: Some(gap),
                        })
                        .await;
                    }
                    tracing::info!(
                        component = COMPONENT,
                        session_id = joined.session_id,
                        "resumed after a network drop"
                    );
                    return Resumed::Connected(bound);
                }
                Connected::Interrupted(interrupt) => {
                    self.idle();
                    return Resumed::Exit(match interrupt {
                        Interrupt::Cancel => Exit::Idle,
                        Interrupt::Shutdown => Exit::Shutdown,
                    });
                }
                Connected::Failed(BindFailure::Refused { code, message, .. }) => match code {
                    // The ticket is dead (expired, kicked meanwhile, unknown): only a new code
                    // connects (§6).
                    ErrorCode::NotAuthorized | ErrorCode::SessionNotFound => {
                        tracing::info!(component = COMPONENT, ?code, %message, "resume ticket refused");
                        let reason = EngineerEndReason::TicketExpired;
                        return Resumed::Exit(
                            self.ended(reason, FailureReason::SessionEnded(reason))
                                .await,
                        );
                    }
                    ErrorCode::SessionClosed => {
                        let reason = EngineerEndReason::DriverEndedSession;
                        return Resumed::Exit(
                            self.ended(reason, FailureReason::SessionEnded(reason))
                                .await,
                        );
                    }
                    // Retrying until the ticket expires cannot help: an update is required.
                    ErrorCode::Incompatible => {
                        tracing::warn!(component = COMPONENT, %message, "resume refused: incompatible");
                        self.core
                            .status
                            .update(|s| s.last_error = Some(message.clone()));
                        return Resumed::Exit(
                            self.ended(
                                EngineerEndReason::Incompatible,
                                FailureReason::Incompatible { detail: message },
                            )
                            .await,
                        );
                    }
                    other => error = format!("the relay refused the resume: {other:?} ({message})"),
                },
                Connected::Failed(failure) => {
                    if failure.is_security_issue() {
                        self.core.status.security_issue();
                    }
                    let (disposition, text) = failure_disposition(&failure);
                    match disposition {
                        Disposition::Retry => error = text,
                        Disposition::Fail(reason) => {
                            let end_reason = end_reason_of(&reason);
                            self.core.status.update(|s| s.last_error = Some(text));
                            return Resumed::Exit(self.ended(end_reason, reason).await);
                        }
                    }
                }
            }
        }
    }

    /// Connect, authenticate and bind, serving commands meanwhile. `joined` is the session being
    /// resumed (`None` while a code is redeemed): a confirmation arriving meanwhile applies to it.
    ///
    /// Once the bind request was sent the relay may have bound this connection (a join redeemed
    /// the code and took a slot, a resume re-attached the slot), so from then on the connection is
    /// never just dropped on cancel / shutdown: it says DISCONNECT first, which frees the slot. A
    /// failed join (timeout, invalid reply, refusal) does the same. A failed resume does not: its
    /// slot must stay held for the next attempt with the ticket.
    async fn connect(
        &mut self,
        request: BindRequest,
        mut joined: Option<&mut Joined>,
    ) -> Connected<T::Reader, T::Writer> {
        let close_timeout = self.core.config.timing.close_timeout;
        // 1. Connect and authenticate. Nothing session-related was sent yet, so an interrupted
        //    attempt is simply dropped (AUTH runs detached, see `establish`).
        let establishing = establish(&mut self.core, &self.transport, ClientRole::Engineer);
        let established = serve_while(
            &mut self.commands,
            &self.phase,
            joined.as_deref_mut(),
            establishing,
        )
        .await;
        let Established { mut link, server } = match established {
            Err(interrupt) => return Connected::Interrupted(interrupt),
            Ok(Err(e)) => return Connected::Failed(BindFailure::Establish(e)),
            Ok(Ok(established)) => established,
        };
        // 2. The bind request, not raced with commands: a frame interrupted half-way could not be
        //    followed by a DISCONNECT, and it is a few hundred bytes on a fresh connection.
        let resuming = matches!(request, BindRequest::Resume { .. });
        let (stage, message) = match request {
            BindRequest::Join(code) => ("SESSION_JOIN", Message::SessionJoin(SessionJoin { code })),
            BindRequest::Resume { session_id, ticket } => (
                "SESSION_RESUME",
                Message::SessionResume(SessionResume::Engineer { session_id, ticket }),
            ),
        };
        if let Err(e) = link.send(&message).await {
            return Connected::Failed(BindFailure::Handshake(e));
        }
        // 3. The reply.
        let reply = serve_while(
            &mut self.commands,
            &self.phase,
            joined,
            await_session_info(&mut link, stage),
        )
        .await;
        match reply {
            Ok(Ok(SessionReply::Info(info, early))) => Connected::Bound(Box::new(BoundConn {
                link,
                server,
                info,
                early,
            })),
            Err(interrupt) => {
                link.goodbye(close_timeout).await;
                Connected::Interrupted(interrupt)
            }
            Ok(Ok(SessionReply::Refused(code, message))) => {
                if !resuming {
                    link.goodbye(close_timeout).await;
                }
                Connected::Failed(BindFailure::Refused {
                    code,
                    message,
                    max_engineers: server.limits.max_engineers_per_session,
                })
            }
            Ok(Err(e)) => {
                if !resuming {
                    link.goodbye(close_timeout).await;
                }
                Connected::Failed(BindFailure::Handshake(e))
            }
        }
    }
}

/// Runs `work` while serving the user's commands. Returns `Err` if the user cancelled,
/// disconnected or shut down meanwhile; `work` is dropped then (it must be cancel-safe, or own
/// nothing that needs cleaning up). `joined` is the session being resumed, if any: a confirmation
/// arriving meanwhile applies to it.
async fn serve_while<F: std::future::Future>(
    commands: &mut mpsc::Receiver<EngineerCommand>,
    phase: &watch::Sender<EngineerPhase>,
    mut joined: Option<&mut Joined>,
    work: F,
) -> Result<F::Output, Interrupt> {
    tokio::pin!(work);
    loop {
        tokio::select! {
            output = &mut work => return Ok(output),
            command = commands.recv() => match command {
                Some(EngineerCommand::Join { reply, .. }) => {
                    let _ = reply.send(Err(JoinError::Busy));
                }
                Some(EngineerCommand::Confirm { reply }) => {
                    // While a joined session reconnects, the confirmation takes effect at once
                    // (the stream is delivered from the resumed connection on). While a code is
                    // being redeemed there is no identity to confirm yet.
                    let result = match joined.as_deref_mut() {
                        Some(joined) => {
                            confirm_offline(joined, phase);
                            Ok(())
                        }
                        None => Err(EngineerError::NotConnected),
                    };
                    let _ = reply.send(result);
                }
                Some(EngineerCommand::RequestSnapshot { reply, .. }) => {
                    let _ = reply.send(Err(EngineerError::NotConnected));
                }
                Some(EngineerCommand::Cancel | EngineerCommand::Disconnect) => {
                    return Err(Interrupt::Cancel);
                }
                None | Some(EngineerCommand::Shutdown) => return Err(Interrupt::Shutdown),
            },
        }
    }
}

/// The end reason shown for a FAILED engineer connection.
fn end_reason_of(reason: &FailureReason) -> EngineerEndReason {
    match reason {
        FailureReason::SessionEnded(r) => *r,
        FailureReason::ReloginRequired(_) => EngineerEndReason::ReloginRequired,
        FailureReason::Incompatible { .. } => EngineerEndReason::Incompatible,
        FailureReason::Configuration { .. } => EngineerEndReason::Configuration,
        FailureReason::EnvironmentMismatch { .. } => EngineerEndReason::EnvironmentMismatch,
        FailureReason::RoleMismatch { .. } => EngineerEndReason::NotAnEngineer,
        FailureReason::Replaced => EngineerEndReason::Replaced,
    }
}

/// Maps a failed join to the typed error shown by the UI.
fn join_error(failure: &BindFailure) -> JoinError {
    match failure {
        BindFailure::Refused {
            code,
            message,
            max_engineers,
        } => match code {
            ErrorCode::CodeInvalid => JoinError::CodeInvalid,
            ErrorCode::CodeExpired => JoinError::CodeExpired,
            ErrorCode::SessionFull => JoinError::SessionFull {
                max: *max_engineers,
            },
            ErrorCode::SessionClosed | ErrorCode::SessionNotFound => JoinError::SessionClosed,
            // The relay accepts SESSION_JOIN only from ENGINEER accounts (§6).
            ErrorCode::NotAuthorized => JoinError::NotAnEngineer,
            ErrorCode::RateLimited => JoinError::RateLimited,
            ErrorCode::Incompatible => JoinError::Incompatible {
                detail: message.clone(),
            },
            other => JoinError::Refused {
                code: *other,
                message: message.clone(),
            },
        },
        BindFailure::Establish(EstablishError::Handshake(e)) | BindFailure::Handshake(e) => {
            handshake_join_error(e)
        }
        BindFailure::Establish(EstablishError::Connect(e)) => JoinError::Unreachable {
            detail: error_chain(e),
        },
    }
}

/// Maps a handshake failure during a join.
fn handshake_join_error(e: &HandshakeError) -> JoinError {
    match e.disposition() {
        Disposition::Fail(FailureReason::ReloginRequired(cause)) => {
            JoinError::ReloginRequired(cause)
        }
        Disposition::Fail(FailureReason::RoleMismatch { .. }) => JoinError::NotAnEngineer,
        Disposition::Fail(FailureReason::Incompatible { detail }) => {
            JoinError::Incompatible { detail }
        }
        Disposition::Fail(FailureReason::EnvironmentMismatch {
            expected,
            announced,
        }) => JoinError::EnvironmentMismatch {
            expected,
            announced,
        },
        _ => match e {
            HandshakeError::Refused {
                code: ErrorCode::RateLimited,
                ..
            } => JoinError::RateLimited,
            other => JoinError::Unreachable {
                detail: error_chain(other),
            },
        },
    }
}

fn failure_disposition(failure: &BindFailure) -> (Disposition, String) {
    match failure {
        BindFailure::Establish(e) => (e.disposition(), e.describe()),
        BindFailure::Handshake(e) => (e.disposition(), error_chain(e)),
        BindFailure::Refused { code, message, .. } => (
            Disposition::Retry,
            format!("the relay refused: {code:?} ({message})"),
        ),
    }
}

/// Outcome of the resume loop after a network drop.
enum Resumed<T: TelemetryTransport> {
    Connected(Box<BoundConn<T::Reader, T::Writer>>),
    Exit(Exit),
}

fn engineer_ticket(info: &SessionInfo) -> Option<Secret> {
    match &info.resume {
        ResumeCredential::EngineerTicket(ticket) => Some(ticket.clone()),
        ResumeCredential::DriverToken(_) => None,
    }
}

fn snapshot_request(reason: SnapshotReason) -> Outbound {
    Outbound::Stream {
        message: Message::SnapshotRequest(SnapshotRequest { reason }),
        priority: FramePriority::Control,
    }
}

/// Confirmation while the link is down: takes effect immediately (buffered frames of the dropped
/// connection are superseded by the replay after the resume).
fn confirm_offline(joined: &mut Joined, phase: &watch::Sender<EngineerPhase>) {
    if !joined.confirmed {
        joined.confirmed = true;
        phase.send_replace(EngineerPhase::Monitoring(joined.card.clone()));
    }
}

/// Frames received before the identity was confirmed.
struct PendingBuffer {
    /// The newest SCHEMA_NEGOTIATE. Kept apart from the FIFO so that a long wait at the identity
    /// card can never evict it: the driver sends it only when the session is (re)bound, and
    /// without it the UI cannot tell unavailable channels from missing ones.
    schema: Option<InboundFrame>,
    /// Other reliable stream frames (SESSION_EVENT) in arrival order (bounded; the oldest is
    /// dropped and counted when full).
    reliable: VecDeque<InboundFrame>,
    capacity: usize,
    /// Newest frame per latest-wins key `(msg_type, priority)`.
    latest: BTreeMap<(u8, u8), InboundFrame>,
    dropped: u64,
}

impl PendingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            schema: None,
            reliable: VecDeque::new(),
            capacity,
            latest: BTreeMap::new(),
            dropped: 0,
        }
    }

    /// Buffers `frame`. Returns `true` if a reliable frame had to be dropped because the buffer
    /// is full (the user left the identity card open for a long time).
    fn push(&mut self, frame: InboundFrame) -> bool {
        let msg_type = frame.message.msg_type();
        if msg_type.delivery() == Delivery::LatestWins {
            // Same key: the newer frame supersedes the older one, exactly as live delivery would.
            self.latest
                .insert((msg_type.id(), frame.header.priority.as_u8()), frame);
            return false;
        }
        if msg_type == MsgType::SchemaNegotiate {
            // A newer schema / capability map replaces the older one (the newest one describes
            // the stream from here on).
            self.schema = Some(frame);
            return false;
        }
        let overflow = self.reliable.len() >= self.capacity;
        if overflow {
            self.reliable.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.reliable.push_back(frame);
        overflow
    }

    fn clear(&mut self) {
        self.schema = None;
        self.reliable.clear();
        self.latest.clear();
    }

    /// Everything, in sequence order (every frame passed the live gate, so applying them in
    /// sequence order reproduces a valid live order).
    fn drain(&mut self) -> Vec<InboundFrame> {
        let mut all: Vec<InboundFrame> = self.schema.take().into_iter().collect();
        all.extend(self.reliable.drain(..));
        all.extend(std::mem::take(&mut self.latest).into_values());
        all.sort_by_key(|f| f.header.sequence);
        all
    }
}

/// Whether a classification must reach the application even if its frame is not delivered.
fn is_significant(arrival: Arrival) -> bool {
    matches!(
        arrival,
        Arrival::First | Arrival::Gap { .. } | Arrival::Restart
    )
}

/// Combines the classification of an undelivered frame (`earlier`) with that of a later frame:
/// a restart outranks everything (old-space state must be reset), missing counts add up, and a
/// gap outranks the start of a stream (the loss is what the application must act on).
fn merge_arrival(earlier: Arrival, later: Arrival) -> Arrival {
    match (earlier, later) {
        (Arrival::Restart, _) | (_, Arrival::Restart) => Arrival::Restart,
        (Arrival::Gap { missing: a }, Arrival::Gap { missing: b }) => Arrival::Gap {
            missing: a.saturating_add(b),
        },
        (gap @ Arrival::Gap { .. }, _) | (_, gap @ Arrival::Gap { .. }) => gap,
        (Arrival::First, _) | (_, Arrival::First) => Arrival::First,
        (_, other) => other,
    }
}

/// Engineer behaviour on a bound connection.
struct EngineerLogic {
    receiver: StreamReceiver,
    confirmed: bool,
    card: IdentityCard,
    pending: PendingBuffer,
    phase: Arc<watch::Sender<EngineerPhase>>,
    exit: Option<Interrupt>,
    new_ticket: Option<Secret>,
    /// A significant classification (first frame, gap, restart) of a frame that was not
    /// delivered (an unknown stream type of a newer minor, an undecodable or non-live frame):
    /// merged into the next delivered frame's [`InboundFrame::arrival`] so the application still
    /// learns of the loss or restart.
    pending_arrival: Option<Arrival>,
    stale_after: Duration,
}

impl EngineerLogic {
    fn end(&mut self, reason: EngineerEndReason) -> Flow {
        Flow::Close(CloseAction::Fail(FailureReason::SessionEnded(reason)))
    }

    fn deliver(&mut self, ctx: &mut Ctx<'_, EngineerEvent>, frame: InboundFrame) {
        if !self.confirmed {
            if self.pending.push(frame) {
                ctx.metrics.event_dropped();
                tracing::warn!(
                    component = COMPONENT,
                    "pre-confirmation buffer full, oldest event dropped"
                );
            }
        } else if frame.message.msg_type().delivery() == Delivery::LatestWins {
            ctx.emit_droppable(EngineerEvent::Frame(Box::new(frame)));
        } else {
            ctx.emit(EngineerEvent::Frame(Box::new(frame)));
        }
    }

    /// Remembers the classification of a frame that is not delivered (see
    /// [`EngineerLogic::pending_arrival`]).
    fn carry(&mut self, arrival: Option<Arrival>) {
        if let Some(a) = arrival.filter(|a| is_significant(*a)) {
            self.pending_arrival = Some(match self.pending_arrival.take() {
                Some(earlier) => merge_arrival(earlier, a),
                None => a,
            });
        }
    }

    /// The classification reported for a delivered frame: `arrival` merged with the pending one.
    /// Replayed frames (`None`) are not tracked and leave the pending one for the next frame.
    fn with_pending(&mut self, arrival: Option<Arrival>) -> Option<Arrival> {
        let a = arrival?;
        Some(match self.pending_arrival.take() {
            Some(earlier) => merge_arrival(earlier, a),
            None => a,
        })
    }

    fn on_stream_frame(
        &mut self,
        ctx: &mut Ctx<'_, EngineerEvent>,
        frame: Frame,
        msg_type: MsgType,
    ) -> Flow {
        let header = *frame.header();
        if header.session_id != ctx.session_id {
            // The relay routes by the connection's binding; another session id is a relay bug.
            tracing::warn!(component = COMPONENT, %msg_type, "stream frame of another session ignored");
            ctx.metrics.ignored();
            return Flow::Continue;
        }
        let trailer = frame.relay_trailer().copied();
        let replayed = trailer.is_some_and(|t| t.replayed);
        let reception = self
            .receiver
            .receive(msg_type, header.priority, header.sequence, replayed);
        let arrival = reception.arrival;
        if let Some(t) = trailer.filter(|_| !replayed) {
            ctx.metrics.relay_dropped(t.relay_dropped);
        }
        if !reception.live {
            ctx.metrics.not_live();
            self.carry(arrival);
            return Flow::Continue;
        }
        if !replayed {
            ctx.metrics.stream_arrival(ctx.now_ns, header.source_ts_ns);
        }
        ctx.metrics.stream_frame_in(ctx.now);
        if !ctx.status.status().driver_away
            && ctx.status.status().degraded == Some(DegradedReason::NoStreamData)
        {
            ctx.status
                .transition(ConnectionState::Connected, |s| s.degraded = None);
        }
        // Kept beyond this dispatch: copy out of the read buffer (§1).
        let frame = frame.detach();
        let message = match Message::from_frame(&frame) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(component = COMPONENT, %msg_type, error = %e, "undecodable stream frame skipped");
                ctx.metrics.decode_error();
                self.carry(arrival);
                return Flow::Continue;
            }
        };
        let arrival = self.with_pending(arrival);
        self.deliver(
            ctx,
            InboundFrame {
                message,
                header,
                trailer,
                arrival,
                received_at_ns: ctx.now_ns,
            },
        );
        Flow::Continue
    }
}

impl RoleLogic for EngineerLogic {
    type Command = EngineerCommand;
    type Event = EngineerEvent;

    fn on_unknown(&mut self, ctx: &mut Ctx<'_, EngineerEvent>, frame: &Frame) {
        // A stream type of a newer minor still consumes a sequence number of the driver's space:
        // skipping it unseen would make the next known frame look like a loss (§5).
        let header = frame.header();
        if header.session_id != ctx.session_id {
            return;
        }
        let replayed = frame.relay_trailer().is_some_and(|t| t.replayed);
        // The frame is never applied: its classification travels with the next delivered frame.
        let arrival = self.receiver.observe_unknown(header, replayed);
        self.carry(arrival);
    }

    fn on_frame(
        &mut self,
        ctx: &mut Ctx<'_, EngineerEvent>,
        frame: Frame,
        msg_type: MsgType,
    ) -> Flow {
        if !msg_type.may_send(Endpoint::Relay, Endpoint::Engineer) {
            tracing::warn!(component = COMPONENT, %msg_type, "message type not sent to engineers, ignored");
            ctx.metrics.ignored();
            return Flow::Continue;
        }
        if msg_type.is_stream() {
            return self.on_stream_frame(ctx, frame, msg_type);
        }
        let message = match Message::from_frame(&frame) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(component = COMPONENT, %msg_type, error = %e, "undecodable message ignored");
                ctx.metrics.decode_error();
                return Flow::Continue;
            }
        };
        match message {
            Message::SessionInfo(info) => {
                // New stream epoch (resume, or the driver restarted its sequence space, §5/§6).
                ctx.metrics.sequence_reset(&self.receiver.stats());
                self.receiver.reset();
                self.pending.clear();
                self.pending_arrival = None;
                if let ResumeCredential::EngineerTicket(ticket) = &info.resume {
                    self.new_ticket = Some(ticket.clone());
                }
                self.card = IdentityCard::from(&info);
                if self.confirmed {
                    // On this connection: the link did not drop (a driver restart, or the
                    // resume's own SESSION_INFO, reported with its gap by `resume`).
                    ctx.emit(EngineerEvent::StreamReset {
                        card: self.card.clone(),
                        gap: None,
                    });
                } else {
                    self.phase
                        .send_replace(EngineerPhase::AwaitingConfirmation(self.card.clone()));
                }
                Flow::Continue
            }
            Message::DriverPresence(presence) => {
                if presence.present {
                    ctx.status.driver_away(false);
                    ctx.status
                        .transition(ConnectionState::Connected, |s| s.degraded = None);
                    ctx.emit(EngineerEvent::DriverBack);
                } else {
                    ctx.status.driver_away(true);
                    ctx.status.transition(ConnectionState::Degraded, |s| {
                        s.degraded = Some(DegradedReason::DriverAway);
                    });
                    ctx.emit(EngineerEvent::DriverAway {
                        since_ns: presence.away_since_ns,
                        grace_expires_at_ns: presence.grace_expires_at_ns,
                    });
                }
                Flow::Continue
            }
            Message::SessionClose(close) => match close.reason {
                SessionCloseReason::RemovedByDriver => self.end(EngineerEndReason::Kicked),
                SessionCloseReason::DriverStopped => {
                    self.end(EngineerEndReason::DriverEndedSession)
                }
                SessionCloseReason::Expired => self.end(EngineerEndReason::SessionExpired),
                // The relay restarts: the ticket may survive it, so try to resume.
                SessionCloseReason::RelayShutdown => Flow::Close(CloseAction::Reconnect(
                    "the relay is shutting down".to_owned(),
                )),
            },
            Message::Disconnect(d) => match d.reason {
                DisconnectReason::Replaced => self.end(EngineerEndReason::Replaced),
                DisconnectReason::SessionEnded => self.end(EngineerEndReason::DriverEndedSession),
                other => Flow::Close(CloseAction::Reconnect(format!(
                    "the relay disconnected ({other:?})"
                ))),
            },
            // Like the driver: an incompatibility is not retried (an update is required).
            Message::Error(e) if e.fatal && e.code == ErrorCode::Incompatible => {
                Flow::Close(CloseAction::Fail(FailureReason::Incompatible {
                    detail: e.message,
                }))
            }
            Message::Error(e) if e.fatal && e.code == ErrorCode::AuthFailed => {
                Flow::Close(CloseAction::Fail(FailureReason::ReloginRequired(
                    ReloginCause::RefreshTokenRejected,
                )))
            }
            Message::Error(e) if e.fatal && e.code == ErrorCode::TokenExpired => {
                Flow::Close(CloseAction::Fail(FailureReason::ReloginRequired(
                    ReloginCause::RefreshTokenExpired,
                )))
            }
            Message::Error(e) if e.fatal => Flow::Close(CloseAction::Reconnect(format!(
                "the relay reported a fatal error: {:?} ({})",
                e.code, e.message
            ))),
            Message::Error(e) => {
                tracing::warn!(component = COMPONENT, code = ?e.code, message = %e.message, "relay error");
                // Informational: never worth stalling the receive loop for.
                ctx.emit_droppable(EngineerEvent::RelayError {
                    code: e.code,
                    message: e.message,
                });
                Flow::Continue
            }
            _ => {
                ctx.metrics.ignored();
                Flow::Continue
            }
        }
    }

    fn on_command(&mut self, ctx: &mut Ctx<'_, EngineerEvent>, command: EngineerCommand) -> Flow {
        let bye = || disconnect(DisconnectReason::ClientShutdown);
        match command {
            EngineerCommand::Confirm { reply } => {
                if !self.confirmed {
                    self.confirmed = true;
                    for frame in self.pending.drain() {
                        self.deliver(ctx, frame);
                    }
                    self.phase
                        .send_replace(EngineerPhase::Monitoring(self.card.clone()));
                    tracing::info!(component = COMPONENT, "identity confirmed, monitoring");
                }
                let _ = reply.send(Ok(()));
                Flow::Continue
            }
            EngineerCommand::Join { reply, .. } => {
                let _ = reply.send(Err(JoinError::Busy));
                Flow::Continue
            }
            EngineerCommand::RequestSnapshot { reason, reply } => {
                let res = ctx
                    .queue
                    .try_push_reliable(snapshot_request(reason))
                    .map_err(EngineerError::Queue);
                if let Err(ref e) = res {
                    tracing::warn!(component = COMPONENT, error = %e, "snapshot request not queued");
                }
                let _ = reply.send(res);
                Flow::Continue
            }
            EngineerCommand::Cancel | EngineerCommand::Disconnect => {
                self.exit = Some(Interrupt::Cancel);
                Flow::Close(CloseAction::Graceful { last: vec![bye()] })
            }
            EngineerCommand::Shutdown => {
                self.exit = Some(Interrupt::Shutdown);
                Flow::Close(CloseAction::Graceful { last: vec![bye()] })
            }
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx<'_, EngineerEvent>) -> Flow {
        if self.confirmed
            && !ctx.status.status().driver_away
            && let Some(age) = ctx.metrics.last_stream_frame_age(ctx.now)
            && age >= self.stale_after
        {
            ctx.status.transition(ConnectionState::Degraded, |s| {
                s.degraded = Some(DegradedReason::NoStreamData);
            });
        }
        Flow::Continue
    }

    fn sequence(&self) -> Option<SequenceStats> {
        Some(self.receiver.stats())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use protocol::frame::Flags;
    use protocol::messages::TelemetryPayload;

    fn inbound(msg: Message, priority: FramePriority, seq: u64) -> InboundFrame {
        InboundFrame {
            header: FrameHeader {
                minor: 0,
                msg_type: msg.msg_type().id(),
                flags: Flags::empty(),
                priority,
                payload_len: 0,
                session_id: 1,
                sequence: seq,
                source_ts_ns: 0,
            },
            message: msg,
            trailer: None,
            arrival: None,
            received_at_ns: 0,
        }
    }

    fn delta(tag: u8) -> Message {
        Message::TelemetryDelta(TelemetryPayload::new(Bytes::from(vec![tag; 64])).expect("tlv"))
    }

    #[test]
    fn undelivered_classifications_merge_restart_first_and_gaps_add_up() {
        use Arrival as A;
        assert_eq!(
            merge_arrival(A::Gap { missing: 2 }, A::InOrder),
            A::Gap { missing: 2 }
        );
        assert_eq!(
            merge_arrival(A::Gap { missing: 2 }, A::Gap { missing: 3 }),
            A::Gap { missing: 5 }
        );
        assert_eq!(merge_arrival(A::Restart, A::Gap { missing: 1 }), A::Restart);
        assert_eq!(merge_arrival(A::Gap { missing: 1 }, A::Restart), A::Restart);
        assert_eq!(merge_arrival(A::Restart, A::InOrder), A::Restart);
        assert_eq!(merge_arrival(A::First, A::InOrder), A::First);
        assert_eq!(
            merge_arrival(A::First, A::Gap { missing: 4 }),
            A::Gap { missing: 4 }
        );
        assert!(!is_significant(A::InOrder));
        assert!(!is_significant(A::Duplicate));
        assert!(is_significant(A::Restart));
    }

    #[test]
    fn pending_buffer_keeps_latest_per_key_and_drains_in_sequence_order() {
        let mut p = PendingBuffer::new(2);
        p.push(inbound(delta(1), FramePriority::P0, 5));
        p.push(inbound(delta(2), FramePriority::P0, 9));
        p.push(inbound(delta(3), FramePriority::P1, 7));
        let event = |seq| {
            inbound(
                Message::SnapshotRequest(SnapshotRequest {
                    reason: SnapshotReason::Manual,
                }),
                FramePriority::Control,
                seq,
            )
        };
        // Reliable FIFO bounded at 2: the oldest is dropped.
        p.push(event(1));
        p.push(event(2));
        p.push(event(3));
        assert_eq!(p.dropped, 1);
        let seqs: Vec<u64> = p.drain().iter().map(|f| f.header.sequence).collect();
        assert_eq!(seqs, vec![2, 3, 7, 9]);
        assert!(p.drain().is_empty());
    }

    #[test]
    fn schema_negotiate_survives_a_full_pending_buffer() {
        use protocol::messages::SchemaNegotiate;
        use telemetry_model::scoring::{CapabilityMap, VehicleCapabilities};
        let schema = |seq| {
            inbound(
                Message::SchemaNegotiate(SchemaNegotiate {
                    schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
                    capabilities: CapabilityMap::default(),
                    vehicle_capabilities: VehicleCapabilities::default(),
                }),
                FramePriority::Control,
                seq,
            )
        };
        let event = |seq| {
            inbound(
                Message::SnapshotRequest(SnapshotRequest {
                    reason: SnapshotReason::Manual,
                }),
                FramePriority::Control,
                seq,
            )
        };
        let mut p = PendingBuffer::new(2);
        // The replayed schema arrives first, then more events than the buffer holds.
        assert!(!p.push(schema(1)));
        assert!(!p.push(event(2)));
        assert!(!p.push(event(3)));
        assert!(
            p.push(event(4)),
            "the oldest event is dropped, not the schema"
        );
        assert_eq!(p.dropped, 1);
        let drained: Vec<(MsgType, u64)> = p
            .drain()
            .iter()
            .map(|f| (f.message.msg_type(), f.header.sequence))
            .collect();
        assert_eq!(
            drained,
            vec![
                (MsgType::SchemaNegotiate, 1),
                (MsgType::SnapshotRequest, 3),
                (MsgType::SnapshotRequest, 4),
            ]
        );
        // A newer schema replaces an older one.
        p.push(schema(5));
        p.push(schema(9));
        let drained = p.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].header.sequence, 9);
    }

    #[test]
    fn join_error_messages_for_the_ui() {
        assert_eq!(
            JoinError::SessionFull { max: 2 }.message(),
            "Driver already has 2 engineers connected"
        );
        assert!(JoinError::CodeExpired.message().contains("new code"));
        assert!(
            JoinError::CodeMalformed(ConnectCodeError::InvalidCharacter)
                .message()
                .contains("8 characters")
        );
    }

    #[test]
    fn identity_card_has_no_ticket() {
        let info = SessionInfo {
            session_id: 4,
            driver_display_name: "Alice".into(),
            driver_username: "alice".into(),
            car: Some("Porsche 963".into()),
            vehicle_class: Some("Hypercar".into()),
            track: Some("Le Mans".into()),
            session_type: None,
            started_at_ns: 1,
            schema_version: 1,
            resume: ResumeCredential::EngineerTicket(Secret::new("ticket-secret")),
        };
        let card = IdentityCard::from(&info);
        assert_eq!(card.driver_username, "alice");
        assert!(!format!("{card:?}").contains("ticket-secret"));
        assert_eq!(engineer_ticket(&info), Some(Secret::new("ticket-secret")));
    }

    #[test]
    fn an_incompatible_refusal_maps_to_the_update_required_states() {
        let refused = BindFailure::Refused {
            code: ErrorCode::Incompatible,
            message: "minor too old".to_owned(),
            max_engineers: 2,
        };
        assert_eq!(
            join_error(&refused),
            JoinError::Incompatible {
                detail: "minor too old".to_owned()
            }
        );
        assert_eq!(
            end_reason_of(&FailureReason::Incompatible {
                detail: String::new()
            }),
            EngineerEndReason::Incompatible
        );
    }

    #[test]
    fn handshake_failures_map_to_join_errors() {
        assert_eq!(
            handshake_join_error(&HandshakeError::Relogin(ReloginCause::NotLoggedIn)),
            JoinError::ReloginRequired(ReloginCause::NotLoggedIn)
        );
        assert_eq!(
            handshake_join_error(&HandshakeError::RoleMismatch {
                requested: ClientRole::Engineer,
                granted: ClientRole::Driver
            }),
            JoinError::NotAnEngineer
        );
        assert!(matches!(
            handshake_join_error(&HandshakeError::Closed),
            JoinError::Unreachable { .. }
        ));
    }
}

//! Driver client: streams telemetry to the relay and manages engineer access (D-020, D-022).
//!
//! * [`DriverClient::start`] connects and binds the driver's session: on the first connection of
//!   an app run it resumes the session stored in the credential store
//!   (`SESSION_RESUME{new_sequence_space: true}`, the sequence restarts at 1) if the relay still
//!   has it, else it creates one (SESSION_CREATE). After a network loss it reconnects with
//!   exponential backoff, re-authenticates silently and resumes the same session continuing its
//!   sequence space. A resume refused by the relay (session expired or closed) falls back to a new
//!   session.
//! * Every time the session is (re)bound, the client discards stale queued telemetry and emits
//!   [`DriverEvent::SessionReady`] followed by [`DriverEvent::SnapshotNeeded`]: the application
//!   then sends SCHEMA_NEGOTIATE and a TELEMETRY_SNAPSHOT. Inbound SNAPSHOT_REQUESTs produce
//!   `SnapshotNeeded` at most once per `SNAPSHOT_REQUEST_MIN_INTERVAL_MS` (coalesced, §5).
//! * Reliable messages ([`DriverClient::send_reliable`]) are accepted from [`DriverClient::start`]
//!   until streaming stops (stop, end of session, FAILED, shutdown); then the pending ones are
//!   discarded and counted (`LinkMetrics::reliable_discarded`) and new ones are refused with
//!   [`QueueError::NotStreaming`]. Across a connection loss delivery is at most once (see
//!   [`crate::queue`]).
//! * Credentials never leave the transport: [`DriverEvent::SessionReady`] carries the session's
//!   [`IdentityCard`], not the SESSION_INFO with its resume token.
//! * Inbound traffic is limited to the engineer-direction whitelist (`protocol::driver_accepts`);
//!   any other message type is ignored and counted. No message can control LMU or the PC.
//! * Engineer access (D-022): [`DriverClient::connect_code`] publishes the current one-time code
//!   and slot usage, [`DriverClient::engineers`] the engineers holding a slot;
//!   [`DriverClient::request_connect_code`] rotates the code ("New code") and
//!   [`DriverClient::kick_engineer`] disconnects an engineer.
//! * Commands (stop, end session, shutdown) are served in every phase, including while a
//!   connection attempt is in progress.

use std::sync::Arc;
use std::time::Duration;

use protocol::Frame;
use protocol::clock::ClockEstimate;
use protocol::frame::{FramePriority, MsgType, driver_accepts};
use protocol::messages::{
    ClientRole, ConnectCode, ConnectCodeRequest, DisconnectReason, EngineerEntry, EngineerKick,
    ErrorCode, Message, ResumeCredential, Secret, ServerHello, SessionClose, SessionCloseReason,
    SessionCreate, SessionInfo, SessionResume, SnapshotReason,
};
use protocol::sequence::SequenceStats;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::auth::AuthManager;
use crate::clock::{LinkClock, relay_time_of};
use crate::config::{ClientConfig, ConfigError, DEFAULT_COMMAND_CAPACITY};
use crate::credentials::StoredDriverSession;
use crate::engineer::IdentityCard;
use crate::handshake::{self, Disposition, HandshakeError, Link, MAX_EARLY_FRAMES};
use crate::metrics::LinkMetrics;
use crate::queue::{LiveOutcome, Outbound, OutboundQueue, QueueError};
use crate::session::{
    ClientCore, CloseAction, ConnectionEnd, Ctx, Established, Flow, RoleLogic, disconnect,
    error_chain, establish, run_connection,
};
use crate::state::{ConnectionStatus, FailureReason};
use crate::transport::{FrameReader, FrameWriter, TelemetryTransport};

const COMPONENT: &str = "transport.driver";
/// Time allowed for delivering an application event outside the receive loop.
const EVENT_TIMEOUT: Duration = Duration::from_secs(2);
/// Time `shutdown` waits for the client task before aborting it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// How the driver's session was bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReadyKind {
    /// A new session was created (first start, or the previous one ended or expired).
    Created,
    /// The session was resumed after a reconnect within this app run (same sequence space).
    Resumed,
    /// The session stored by a previous app run was resumed; the sequence space restarted at 1.
    ResumedAfterRestart,
}

/// Why a fresh snapshot is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotTrigger {
    /// The session was (re)bound: send SCHEMA_NEGOTIATE and a full snapshot.
    SessionReady(SessionReadyKind),
    /// The relay forwarded a SNAPSHOT_REQUEST (rate-limited, §5).
    Requested(SnapshotReason),
}

/// Events of the driver client.
#[derive(Debug, Clone, PartialEq)]
pub enum DriverEvent {
    /// The session is bound (and the stream may flow).
    SessionReady {
        session_id: u64,
        kind: SessionReadyKind,
        /// The session as the relay announced it. The resume token stays inside the transport
        /// (which persists it), so the event can be forwarded to the UI as it is.
        identity: IdentityCard,
    },
    /// Send SCHEMA_NEGOTIATE (after `SessionReady`) and a TELEMETRY_SNAPSHOT now.
    SnapshotNeeded { trigger: SnapshotTrigger },
    /// A non-fatal error reported by the relay (e.g. a rate-limited code request).
    RelayError { code: ErrorCode, message: String },
}

/// The client has shut down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the client has shut down")]
pub struct ClientClosed;

enum DriverCommand {
    Start(Box<SessionCreate>),
    UpdateSession(Box<SessionCreate>),
    Stop,
    EndSession,
    Shutdown,
}

impl std::fmt::Debug for DriverCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DriverCommand::Start(_) => "Start",
            DriverCommand::UpdateSession(_) => "UpdateSession",
            DriverCommand::Stop => "Stop",
            DriverCommand::EndSession => "EndSession",
            DriverCommand::Shutdown => "Shutdown",
        })
    }
}

/// Formats a canonical connection code for display: `7KQ4MZ9T` → `7KQ4-MZ9T`.
pub fn display_connect_code(code: &Secret) -> String {
    let raw = code.expose();
    let half = raw.len() / 2;
    match (raw.get(..half), raw.get(half..)) {
        (Some(a), Some(b)) if !a.is_empty() => format!("{a}-{b}"),
        _ => raw.to_owned(),
    }
}

/// Time left before a connection code expires, from its relay-clock expiry, the clock estimate and
/// the local monotonic time (ns of the client's clock, [`DriverClient::now_ns`]). Expired codes
/// report zero.
pub fn code_expires_in(expires_at_ns: u64, clock: &ClockEstimate, now_mono_ns: u64) -> Duration {
    let relay_now = i128::from(now_mono_ns) + i128::from(clock.offset_ns);
    let left = i128::from(expires_at_ns) - relay_now;
    Duration::from_nanos(u64::try_from(left.max(0)).unwrap_or(u64::MAX))
}

/// Handle of a running driver client. Dropping it lets the client disconnect gracefully.
#[derive(Debug)]
pub struct DriverClient {
    commands: mpsc::Sender<DriverCommand>,
    queue: Arc<OutboundQueue>,
    clock: LinkClock,
    status: watch::Receiver<ConnectionStatus>,
    metrics: watch::Receiver<LinkMetrics>,
    connect_code: watch::Receiver<Option<ConnectCode>>,
    engineers: watch::Receiver<Vec<EngineerEntry>>,
    task: JoinHandle<()>,
}

impl DriverClient {
    /// Spawns the client task (DISCONNECTED until [`DriverClient::start`]). Share `auth` with
    /// login and the connectivity test. Must be called inside a Tokio runtime. Fails if the
    /// configuration is unusable ([`ClientConfig::validate`]).
    pub fn spawn<T: TelemetryTransport>(
        config: ClientConfig,
        transport: T,
        auth: Arc<AuthManager>,
    ) -> Result<(DriverClient, mpsc::Receiver<DriverEvent>), ConfigError> {
        config.validate()?;
        let event_capacity = config.event_capacity.max(1);
        let (core, status, metrics) = ClientCore::new(config, auth, COMPONENT);
        // Nothing carries reliable messages before `start`.
        core.queue.set_streaming(false);
        let (commands_tx, commands) = mpsc::channel(DEFAULT_COMMAND_CAPACITY);
        let (events, events_rx) = mpsc::channel(event_capacity);
        let (code_tx, connect_code) = watch::channel(None);
        let (engineers_tx, engineers) = watch::channel(Vec::new());
        let queue = Arc::clone(&core.queue);
        let clock = core.clock;
        let supervisor = Supervisor {
            core,
            transport,
            commands,
            events,
            watches: Arc::new(Watches {
                code: code_tx,
                engineers: engineers_tx,
            }),
            state: DriverState::default(),
        };
        let task = tokio::spawn(supervisor.run());
        Ok((
            DriverClient {
                commands: commands_tx,
                queue,
                clock,
                status,
                metrics,
                connect_code,
                engineers,
                task,
            },
            events_rx,
        ))
    }

    /// Starts streaming: connects, resumes or creates the session. `session` describes the
    /// vehicle / track / LMU version for SESSION_CREATE. Reliable messages are accepted from now
    /// on (they are written once the session is bound).
    pub async fn start(&self, session: SessionCreate) -> Result<(), ClientClosed> {
        self.queue.set_streaming(true);
        self.command(DriverCommand::Start(Box::new(session))).await
    }

    /// Updates the SESSION_CREATE data used for the next new session.
    pub async fn update_session(&self, session: SessionCreate) -> Result<(), ClientClosed> {
        self.command(DriverCommand::UpdateSession(Box::new(session)))
            .await
    }

    /// Stops streaming and disconnects, keeping the session for a resume (the relay keeps it for
    /// its grace period, engineers stay attached). Pending outbound messages are discarded
    /// (counted) and reliable messages are refused until the next start. Also acknowledges FAILED
    /// (→ DISCONNECTED).
    pub async fn stop(&self) -> Result<(), ClientClosed> {
        self.command(DriverCommand::Stop).await
    }

    /// Ends the session (SESSION_CLOSE: engineers see "driver ended the session"), forgets the
    /// stored resume token and disconnects.
    pub async fn end_session(&self) -> Result<(), ClientClosed> {
        self.command(DriverCommand::EndSession).await
    }

    /// Disconnects gracefully (the session is kept so the next app start resumes it) and ends the
    /// client task.
    pub async fn shutdown(self) {
        let DriverClient {
            commands,
            queue,
            mut task,
            ..
        } = self;
        let _ = commands.send(DriverCommand::Shutdown).await;
        drop(commands);
        if timeout(SHUTDOWN_TIMEOUT, &mut task).await.is_err() {
            task.abort();
        }
        queue.close();
    }

    async fn command(&self, command: DriverCommand) -> Result<(), ClientClosed> {
        self.commands.send(command).await.map_err(|_| ClientClosed)
    }

    /// Queues a latest-wins frame (TELEMETRY_DELTA / TELEMETRY_SNAPSHOT / SCORING_UPDATE /
    /// WEATHER_UPDATE / DRIVER_STATUS / RECORDING_STATUS). Never blocks; an unsent older frame of the
    /// same `(type, priority)` is replaced. A snapshot supersedes pending deltas. Enqueue in capture
    /// order.
    pub fn send_live(
        &self,
        message: Message,
        priority: FramePriority,
    ) -> Result<LiveOutcome, QueueError> {
        self.queue.push_live(message, priority, self.clock.now_ms())
    }

    /// Queues SCHEMA_NEGOTIATE or SESSION_EVENT on the reliable queue, waiting while it is full.
    ///
    /// * Fails at once with [`QueueError::NotStreaming`] while the client is not streaming (before
    ///   [`DriverClient::start`], after stop / end of session / FAILED), and a waiting call is
    ///   released with that error when streaming stops: nothing is queued for a later session.
    /// * While streaming, the queue drains only while connected, so during an outage a full queue
    ///   makes this wait until the link is back. Use [`DriverClient::try_send_reliable`] to never
    ///   wait.
    /// * Delivery is at most once across a connection loss: a message written shortly before the
    ///   link died silently is lost (the engineer sees a sequence gap) and is not re-sent;
    ///   messages queued while the link is down are delivered after the resume.
    pub async fn send_reliable(&self, message: Message) -> Result<(), QueueError> {
        self.queue.push_reliable(driver_reliable(message)?).await
    }

    /// Like [`DriverClient::send_reliable`] but fails with [`QueueError::Full`] instead of waiting.
    pub fn try_send_reliable(&self, message: Message) -> Result<(), QueueError> {
        self.queue.try_push_reliable(driver_reliable(message)?)
    }

    /// Asks the relay for a new connection code ("New code"; the unused one is invalidated).
    /// Fails with [`QueueError::NotStreaming`] while not streaming.
    pub fn request_connect_code(&self) -> Result<(), QueueError> {
        self.queue
            .try_push_reliable(Outbound::Control(Message::ConnectCodeRequest(
                ConnectCodeRequest,
            )))
    }

    /// Disconnects one engineer (its ticket dies; it needs a new code to come back). Also frees
    /// the slot of an engineer shown as `Reconnecting`.
    pub fn kick_engineer(&self, connection_id: u64) -> Result<(), QueueError> {
        self.queue
            .try_push_reliable(Outbound::Control(Message::EngineerKick(EngineerKick {
                connection_id,
            })))
    }

    /// Connection status.
    pub fn status(&self) -> watch::Receiver<ConnectionStatus> {
        self.status.clone()
    }

    /// Link metrics (about once per second).
    pub fn metrics(&self) -> watch::Receiver<LinkMetrics> {
        self.metrics.clone()
    }

    /// The current connection code and engineer slot usage (`None` while no session is bound).
    pub fn connect_code(&self) -> watch::Receiver<Option<ConnectCode>> {
        self.connect_code.clone()
    }

    /// Engineers holding a slot in the session.
    pub fn engineers(&self) -> watch::Receiver<Vec<EngineerEntry>> {
        self.engineers.clone()
    }

    /// Current clock offset to the relay (for `capture_relay_ns_est` and DRIVER_STATUS.clock). The
    /// offset applies to times on [`ClientConfig::mono_epoch`]; [`DriverClient::relay_time_of`]
    /// avoids that coupling.
    pub fn clock(&self) -> Option<ClockEstimate> {
        self.metrics.borrow().clock
    }

    /// Nanoseconds of the client's monotonic clock now (the epoch of frame headers and PINGs).
    pub fn now_ns(&self) -> u64 {
        self.clock.now_ns()
    }

    /// The relay clock (ns since the Unix epoch) at `at`, e.g. a sample's capture instant: its
    /// `capture_relay_ns_est` (PROTOCOL.md §8). Converts the instant itself, so it is correct
    /// whatever epoch the capture timestamps use. `None` until the clock offset was measured.
    pub fn relay_time_of(&self, at: std::time::Instant) -> Option<i64> {
        relay_time_of(&self.clock, self.clock(), at)
    }
}

fn driver_reliable(message: Message) -> Result<Outbound, QueueError> {
    match message.msg_type() {
        MsgType::SchemaNegotiate | MsgType::SessionEvent => Ok(Outbound::Stream {
            message,
            priority: FramePriority::Control,
        }),
        msg_type => Err(QueueError::NotAllowed { msg_type }),
    }
}

/// Watch senders shared by the supervisor and the connection logic.
#[derive(Debug)]
struct Watches {
    code: watch::Sender<Option<ConnectCode>>,
    engineers: watch::Sender<Vec<EngineerEntry>>,
}

impl Watches {
    fn clear(&self) {
        self.code.send_replace(None);
        self.engineers.send_replace(Vec::new());
    }
}

/// Driver state that outlives connections.
#[derive(Debug, Default)]
struct DriverState {
    create: Option<SessionCreate>,
    /// The driver's session (memory copy of the stored one).
    session: Option<StoredDriverSession>,
    stored_loaded: bool,
    /// Whether this app run already bound a session (else a resume announces a new sequence
    /// space, §5).
    sequence_space_announced: bool,
    /// The previous connection was lost (the next binding counts as a reconnect).
    lost_connection: bool,
    /// The session bound most recently in this app run. Reliable messages queued for it are
    /// discarded when a different session is bound (they are meaningless there).
    last_bound: Option<u64>,
}

impl DriverState {
    /// Forgets the session (memory and credential store).
    fn forget_session(&mut self, auth: &AuthManager) {
        self.session = None;
        self.stored_loaded = true;
        if let Err(e) = auth.clear_driver_session() {
            tracing::error!(component = COMPONENT, error = %e, "could not delete the stored session");
        }
    }

    /// Remembers (and persists) the session and its newest resume token.
    fn remember_session(&mut self, auth: &AuthManager, session_id: u64, token: Secret) {
        let session = StoredDriverSession {
            session_id,
            resume_token: token,
        };
        if self.session.as_ref() != Some(&session)
            && let Err(e) = auth.store_driver_session(&session)
        {
            tracing::error!(component = COMPONENT, error = %e, "could not persist the session resume token");
        }
        self.session = Some(session);
    }
}

/// How the active phase ended.
enum Exit {
    /// Back to DISCONNECTED or FAILED; wait for the next start.
    Idle,
    /// The client shuts down.
    Shutdown,
}

/// Outcome of one connection attempt.
enum Attempt {
    /// Stop retrying.
    Ended(Exit),
    /// Retry after the backoff.
    Retry(String),
}

/// A bound session.
struct Bound {
    info: SessionInfo,
    kind: SessionReadyKind,
    early: Vec<Frame>,
}

struct Supervisor<T> {
    core: ClientCore,
    transport: T,
    commands: mpsc::Receiver<DriverCommand>,
    events: mpsc::Sender<DriverEvent>,
    watches: Arc<Watches>,
    state: DriverState,
}

impl<T: TelemetryTransport> Supervisor<T> {
    async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            match command {
                DriverCommand::Start(create) => {
                    self.state.create = Some(*create);
                    // Starting again (e.g. after a new login) acknowledges FAILED.
                    self.core.status.acknowledge_failure();
                    // Also set by the handle; repeated here because an earlier stop or failure
                    // processed after that call cleared it.
                    self.core.queue.set_streaming(true);
                    let exit = self.active().await;
                    self.end_streaming();
                    if let Exit::Shutdown = exit {
                        break;
                    }
                }
                DriverCommand::UpdateSession(create) => self.state.create = Some(*create),
                // Acknowledges FAILED.
                DriverCommand::Stop => self.core.status.disconnected(),
                DriverCommand::EndSession => {
                    self.state.forget_session(&self.core.auth);
                    self.core.status.disconnected();
                }
                DriverCommand::Shutdown => break,
            }
        }
        self.core.queue.clear_session();
        self.core.queue.close();
        self.core.status.disconnected();
        self.watches.clear();
        tracing::info!(component = COMPONENT, "driver client stopped");
    }

    /// Connected or reconnecting until stopped, failed or shut down.
    async fn active(&mut self) -> Exit {
        self.core.backoff.reset();
        self.state.lost_connection = false;
        loop {
            let error = match self.attempt().await {
                Attempt::Ended(exit) => return exit,
                Attempt::Retry(error) => error,
            };
            let Some(delay) = self.core.after_failure(Disposition::Retry, error) else {
                return Exit::Idle;
            };
            // Wait for the backoff, still serving commands.
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);
            loop {
                tokio::select! {
                    () = &mut sleep => break,
                    command = self.commands.recv() => match command {
                        Some(DriverCommand::Start(c) | DriverCommand::UpdateSession(c)) => {
                            self.state.create = Some(*c);
                        }
                        other => return self.interrupted(other),
                    },
                }
            }
        }
    }

    /// A stop / end / shutdown command (or all handles dropped) outside a connection.
    fn interrupted(&mut self, command: Option<DriverCommand>) -> Exit {
        let exit = match command {
            Some(DriverCommand::EndSession) => {
                self.state.forget_session(&self.core.auth);
                Exit::Idle
            }
            Some(DriverCommand::Stop) => Exit::Idle,
            _ => Exit::Shutdown,
        };
        self.stopped(exit)
    }

    fn stopped(&mut self, exit: Exit) -> Exit {
        // Refuse first, then clear: a producer waiting for space must get NotStreaming, not slip
        // into the space the clear frees.
        self.core.queue.set_streaming(false);
        self.core.queue.clear_session();
        self.core.status.disconnected();
        self.watches.clear();
        exit
    }

    /// Leaves the streaming phase (stopped, ended, FAILED or shut down): reliable messages are
    /// refused from now on, and pending ones are discarded and counted instead of being delivered
    /// into a later session, possibly hours later.
    fn end_streaming(&mut self) {
        self.core.queue.set_streaming(false);
        let discarded = self.core.queue.clear_session();
        if discarded > 0 {
            tracing::info!(
                component = COMPONENT,
                discarded,
                "streaming ended: pending reliable messages discarded"
            );
        }
        self.core.publish_metrics(None);
    }

    async fn attempt(&mut self) -> Attempt {
        // Connect and bind, serving commands meanwhile (stop must not wait for a timeout).
        let connected = {
            let core = &mut self.core;
            let state = &mut self.state;
            let transport = &self.transport;
            let commands = &mut self.commands;
            let mut pending_create = None;
            let mut connect = Box::pin(async move {
                let Established {
                    mut link, server, ..
                } = establish(core, transport, ClientRole::Driver)
                    .await
                    .map_err(|e| (e.disposition(), e.describe()))?;
                let bound = bind(core, state, &mut link)
                    .await
                    .map_err(|e| (e.disposition(), error_chain(&e)))?;
                Ok::<_, (Disposition, String)>((link, server, bound))
            });
            let outcome = loop {
                tokio::select! {
                    result = &mut connect => break Ok(result),
                    command = commands.recv() => match command {
                        Some(DriverCommand::Start(c) | DriverCommand::UpdateSession(c)) => {
                            pending_create = Some(*c);
                        }
                        other => break Err(other),
                    },
                }
            };
            drop(connect);
            if let Some(create) = pending_create {
                self.state.create = Some(create);
            }
            match outcome {
                Ok(result) => result,
                Err(command) => return Attempt::Ended(self.interrupted(command)),
            }
        };
        let (link, server, bound) = match connected {
            Ok(c) => c,
            Err((Disposition::Retry, error)) => return Attempt::Retry(error),
            Err((fail, error)) => {
                self.core.after_failure(fail, error);
                self.watches.clear();
                return Attempt::Ended(Exit::Idle);
            }
        };
        self.session_ready(link, server, bound).await
    }

    async fn session_ready<R: FrameReader, W: FrameWriter>(
        &mut self,
        link: Link<R, W>,
        server: ServerHello,
        bound: Bound,
    ) -> Attempt {
        let session_id = bound.info.session_id;
        if std::mem::take(&mut self.state.lost_connection) {
            self.core.metrics.reconnected();
        }
        // Telemetry queued before this binding predates it; the snapshot requested below
        // replaces it.
        let stale = self.core.queue.clear_live();
        // Reliable messages still queued survive a resume of the same session (a lap event queued
        // while the link was down is delivered now; one already written into a link that died
        // silently is lost, see `crate::queue`), but those of a session that was replaced by a new
        // one are meaningless in it (events, code requests, kicks of the old session).
        let previous = self.state.last_bound.replace(session_id);
        let old_session_messages = match previous {
            Some(previous) if previous != session_id => self.core.queue.clear_session(),
            _ => 0,
        };
        tracing::info!(
            component = COMPONENT,
            session_id,
            kind = ?bound.kind,
            stale_frames_discarded = stale,
            old_session_messages_discarded = old_session_messages,
            "session ready"
        );
        self.event(DriverEvent::SessionReady {
            session_id,
            kind: bound.kind,
            identity: IdentityCard::from(&bound.info),
        })
        .await;
        self.event(DriverEvent::SnapshotNeeded {
            trigger: SnapshotTrigger::SessionReady(bound.kind),
        })
        .await;
        let mut logic = DriverLogic {
            watches: Arc::clone(&self.watches),
            auth: Arc::clone(&self.core.auth),
            exit: None,
            create_update: None,
            session_ended: false,
            rotated: None,
        };
        let end = run_connection(
            &mut self.core,
            link,
            &server,
            session_id,
            bound.early,
            &mut logic,
            &mut self.commands,
            &self.events,
        )
        .await;
        if let Some(create) = logic.create_update.take() {
            self.state.create = Some(create);
        }
        if let Some(session) = logic.rotated.take() {
            // Already persisted when it arrived; keep the memory copy in step.
            self.state.session = Some(session);
        }
        if logic.session_ended {
            self.state.forget_session(&self.core.auth);
        }
        match end {
            ConnectionEnd::Closed => {
                let exit = match logic.exit {
                    Some(UserExit::EndSession) => {
                        self.state.forget_session(&self.core.auth);
                        Exit::Idle
                    }
                    Some(UserExit::Stop) => Exit::Idle,
                    // Shutdown, or every handle dropped.
                    Some(UserExit::Shutdown) | None => Exit::Shutdown,
                };
                Attempt::Ended(self.stopped(exit))
            }
            ConnectionEnd::Lost(error) => {
                self.state.lost_connection = true;
                Attempt::Retry(error)
            }
            ConnectionEnd::Failed(reason) => {
                self.core.status.fail(reason);
                self.watches.clear();
                Attempt::Ended(Exit::Idle)
            }
        }
    }

    async fn event(&self, event: DriverEvent) {
        if !matches!(
            timeout(EVENT_TIMEOUT, self.events.send(event)).await,
            Ok(Ok(()))
        ) && !self.events.is_closed()
        {
            tracing::warn!(
                component = COMPONENT,
                "driver event dropped: event channel full"
            );
        }
    }
}

/// SESSION_RESUME (stored or current session) or SESSION_CREATE.
async fn bind<R: FrameReader, W: FrameWriter>(
    core: &mut ClientCore,
    state: &mut DriverState,
    link: &mut Link<R, W>,
) -> Result<Bound, HandshakeError> {
    if !state.stored_loaded {
        state.stored_loaded = true;
        match core.auth.load_driver_session() {
            Ok(stored) => state.session = stored,
            Err(e) => {
                tracing::warn!(component = COMPONENT, error = %e, "stored session unusable, a new one is created");
                state.forget_session(&core.auth);
            }
        }
    }
    if let Some(stored) = state.session.clone() {
        const STAGE: &str = "SESSION_RESUME";
        let new_space = !state.sequence_space_announced;
        link.send(&Message::SessionResume(SessionResume::Driver {
            session_id: stored.session_id,
            resume_token: stored.resume_token.clone(),
            new_sequence_space: new_space,
        }))
        .await?;
        match await_session_info(link, STAGE).await? {
            SessionReply::Info(info, early) => {
                let token = driver_token(&info, STAGE)?;
                if new_space {
                    core.reset_sequence();
                }
                state.sequence_space_announced = true;
                state.remember_session(&core.auth, info.session_id, token);
                let kind = if new_space {
                    SessionReadyKind::ResumedAfterRestart
                } else {
                    SessionReadyKind::Resumed
                };
                return Ok(Bound { info, kind, early });
            }
            SessionReply::Refused(code, message)
                if matches!(
                    code,
                    ErrorCode::SessionNotFound
                        | ErrorCode::SessionClosed
                        | ErrorCode::NotAuthorized
                ) =>
            {
                tracing::info!(component = COMPONENT, ?code, %message, "the stored session is gone, creating a new one");
                state.forget_session(&core.auth);
            }
            SessionReply::Refused(code, message) => {
                return Err(HandshakeError::Refused {
                    stage: STAGE,
                    code,
                    message,
                    fatal: false,
                });
            }
        }
    }
    const STAGE: &str = "SESSION_CREATE";
    // `start` always supplies the data; the fallback describes an unknown identity (all None).
    let create = state
        .create
        .clone()
        .unwrap_or_else(|| unknown_identity(&core.config.app_version));
    link.send(&Message::SessionCreate(create)).await?;
    match await_session_info(link, STAGE).await? {
        SessionReply::Info(info, early) => {
            let token = driver_token(&info, STAGE)?;
            core.reset_sequence();
            state.sequence_space_announced = true;
            state.remember_session(&core.auth, info.session_id, token);
            Ok(Bound {
                info,
                kind: SessionReadyKind::Created,
                early,
            })
        }
        SessionReply::Refused(code, message) => Err(HandshakeError::Refused {
            stage: STAGE,
            code,
            message,
            fatal: false,
        }),
    }
}

/// SESSION_CREATE data with nothing known yet (`None` everywhere, never invented values).
fn unknown_identity(app_version: &str) -> SessionCreate {
    SessionCreate {
        identity: Default::default(),
        lmu_version: None,
        app_version: app_version.to_owned(),
        schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        capabilities: Default::default(),
    }
}

/// Reply to a SESSION_CREATE / SESSION_RESUME / SESSION_JOIN.
pub(crate) enum SessionReply {
    Info(SessionInfo, Vec<Frame>),
    Refused(ErrorCode, String),
}

/// Waits for SESSION_INFO (or an ERROR) after a session request. Frames that arrive first are kept
/// for the receive loop (bounded). Shared with the engineer client.
pub(crate) async fn await_session_info<R: FrameReader, W: FrameWriter>(
    link: &mut Link<R, W>,
    stage: &'static str,
) -> Result<SessionReply, HandshakeError> {
    let mut early = Vec::new();
    loop {
        let (frame, msg_type) = link.recv_frame(stage).await?;
        match msg_type {
            MsgType::SessionInfo | MsgType::Error | MsgType::Disconnect | MsgType::SessionClose => {
                let message = Message::from_frame(&frame)
                    .map_err(|source| HandshakeError::Decode { msg_type, source })?;
                return match message {
                    Message::SessionInfo(info) => Ok(SessionReply::Info(info, early)),
                    Message::Error(e) => Ok(SessionReply::Refused(e.code, e.message)),
                    other => Err(handshake::unexpected(stage, other)),
                };
            }
            _ if early.len() < MAX_EARLY_FRAMES => early.push(frame.detach()),
            other => return Err(HandshakeError::Unexpected { stage, got: other }),
        }
    }
}

fn driver_token(info: &SessionInfo, stage: &'static str) -> Result<Secret, HandshakeError> {
    match &info.resume {
        ResumeCredential::DriverToken(token) => Ok(token.clone()),
        ResumeCredential::EngineerTicket(_) => Err(HandshakeError::Unexpected {
            stage,
            got: MsgType::SessionInfo,
        }),
    }
}

enum UserExit {
    Stop,
    EndSession,
    Shutdown,
}

/// Driver behaviour on a bound connection.
struct DriverLogic {
    watches: Arc<Watches>,
    auth: Arc<AuthManager>,
    exit: Option<UserExit>,
    create_update: Option<SessionCreate>,
    session_ended: bool,
    /// A rotated resume token received mid-connection (already persisted).
    rotated: Option<StoredDriverSession>,
}

impl RoleLogic for DriverLogic {
    type Command = DriverCommand;
    type Event = DriverEvent;

    fn on_frame(
        &mut self,
        ctx: &mut Ctx<'_, DriverEvent>,
        frame: Frame,
        msg_type: MsgType,
    ) -> Flow {
        // Engineer-direction whitelist (§3): anything else is ignored without decoding.
        if !driver_accepts(msg_type) {
            tracing::warn!(component = COMPONENT, %msg_type, "message type not accepted by a driver, ignored");
            ctx.metrics.ignored();
            return Flow::Continue;
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
                match info.resume {
                    // The relay rotated the resume token. Persist it at once: an app crash before
                    // this connection ends must still resume with the newest token (D-020).
                    ResumeCredential::DriverToken(token) if info.session_id == ctx.session_id => {
                        let session = StoredDriverSession {
                            session_id: info.session_id,
                            resume_token: token,
                        };
                        if let Err(e) = self.auth.store_driver_session(&session) {
                            tracing::error!(component = COMPONENT, error = %e, "could not persist the rotated session resume token");
                        }
                        self.rotated = Some(session);
                    }
                    ResumeCredential::DriverToken(_) => {
                        tracing::warn!(
                            component = COMPONENT,
                            "SESSION_INFO for another session ignored"
                        );
                        ctx.metrics.ignored();
                    }
                    ResumeCredential::EngineerTicket(_) => ctx.metrics.ignored(),
                }
                Flow::Continue
            }
            Message::ConnectCode(code) => {
                self.watches.code.send_replace(Some(code));
                Flow::Continue
            }
            Message::EngineerList(list) => {
                self.watches.engineers.send_replace(list.engineers);
                Flow::Continue
            }
            Message::SnapshotRequest(request) => {
                if ctx.queue.snapshot_request(request.reason, ctx.now_ms) {
                    ctx.emit(DriverEvent::SnapshotNeeded {
                        trigger: SnapshotTrigger::Requested(request.reason),
                    });
                } else {
                    ctx.metrics.snapshot_request_coalesced();
                }
                Flow::Continue
            }
            Message::Disconnect(d) => match d.reason {
                DisconnectReason::Replaced => {
                    Flow::Close(CloseAction::Fail(FailureReason::Replaced))
                }
                DisconnectReason::SessionEnded => {
                    self.session_ended = true;
                    Flow::Close(CloseAction::Reconnect(
                        "the relay ended the session".to_owned(),
                    ))
                }
                other => Flow::Close(CloseAction::Reconnect(format!(
                    "the relay disconnected ({other:?})"
                ))),
            },
            Message::Error(e) if e.fatal => match e.code {
                ErrorCode::Incompatible => {
                    Flow::Close(CloseAction::Fail(FailureReason::Incompatible {
                        detail: e.message,
                    }))
                }
                code => Flow::Close(CloseAction::Reconnect(format!(
                    "the relay reported a fatal error: {code:?} ({})",
                    e.message
                ))),
            },
            Message::Error(e) => {
                tracing::warn!(component = COMPONENT, code = ?e.code, message = %e.message, "relay error");
                // Informational: never worth stalling the receive loop for (a relay may send
                // many, e.g. while rate limiting).
                ctx.emit_droppable(DriverEvent::RelayError {
                    code: e.code,
                    message: e.message,
                });
                Flow::Continue
            }
            // Late handshake replies and the client form of HELLO carry nothing for a driver.
            _ => {
                ctx.metrics.ignored();
                Flow::Continue
            }
        }
    }

    fn on_command(&mut self, ctx: &mut Ctx<'_, DriverEvent>, command: DriverCommand) -> Flow {
        let bye = disconnect(DisconnectReason::ClientShutdown);
        match command {
            DriverCommand::Start(create) | DriverCommand::UpdateSession(create) => {
                self.create_update = Some(*create);
                Flow::Continue
            }
            DriverCommand::Stop => {
                self.exit = Some(UserExit::Stop);
                Flow::Close(CloseAction::Graceful { last: vec![bye] })
            }
            DriverCommand::EndSession => {
                self.exit = Some(UserExit::EndSession);
                let close = Message::SessionClose(SessionClose {
                    session_id: ctx.session_id,
                    reason: SessionCloseReason::DriverStopped,
                });
                Flow::Close(CloseAction::Graceful {
                    last: vec![close, bye],
                })
            }
            DriverCommand::Shutdown => {
                self.exit = Some(UserExit::Shutdown);
                Flow::Close(CloseAction::Graceful { last: vec![bye] })
            }
        }
    }

    fn on_tick(&mut self, ctx: &mut Ctx<'_, DriverEvent>) -> Flow {
        if let Some(reason) = ctx.queue.due_snapshot_request(ctx.now_ms) {
            ctx.emit(DriverEvent::SnapshotNeeded {
                trigger: SnapshotTrigger::Requested(reason),
            });
        }
        Flow::Continue
    }

    fn sequence(&self) -> Option<SequenceStats> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_code_display_form() {
        assert_eq!(display_connect_code(&Secret::new("7KQ4MZ9T")), "7KQ4-MZ9T");
        assert_eq!(display_connect_code(&Secret::new("")), "");
    }

    #[test]
    fn code_expiry_uses_the_relay_clock_offset() {
        let clock = ClockEstimate {
            offset_ns: 1_000_000_000_000,
            uncertainty_ns: 1_000,
            min_delay_ns: 2_000,
            samples: 3,
        };
        // Relay now = 5 s + offset; expiry 65 s + offset → 60 s left.
        let left = code_expires_in(1_000_000_000_000 + 65_000_000_000, &clock, 5_000_000_000);
        assert_eq!(left, Duration::from_secs(60));
        assert_eq!(
            code_expires_in(0, &clock, 5_000_000_000),
            Duration::ZERO,
            "expired codes report zero, never negative"
        );
    }

    #[test]
    fn only_schema_and_events_go_through_send_reliable() {
        assert!(driver_reliable(Message::ConnectCodeRequest(ConnectCodeRequest)).is_err());
        assert!(matches!(
            driver_reliable(Message::SnapshotRequest(
                protocol::messages::SnapshotRequest {
                    reason: SnapshotReason::Manual
                }
            )),
            Err(QueueError::NotAllowed { .. })
        ));
    }
}

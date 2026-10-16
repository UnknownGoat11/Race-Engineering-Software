//! Connection runtime shared by the driver and engineer clients.
//!
//! One client = one long-lived supervisor task (in [`crate::driver`] / [`crate::engineer`]) that
//! repeatedly: establishes a connection ([`establish`]: CONNECTING → AUTHENTICATING), binds the
//! session (role-specific), then runs it with [`run_connection`] until it ends, and decides between
//! FAILED, DISCONNECTED and RECONNECTING with backoff ([`ClientCore::after_failure`]).
//!
//! [`run_connection`] owns the receive loop and spawns one writer task per connection:
//! * the **writer** serves the connection's control lane (PING, DISCONNECT, SESSION_CLOSE), then
//!   the reliable queue, then latest-wins rounds ([`crate::queue`]); stream frames get their
//!   sequence number when written; every write has a timeout;
//! * the **receive loop** checks header rules, answers PONGs (RTT, clock sync with step detection),
//!   hands every other frame to the role logic, sends a PING every heartbeat interval, evaluates
//!   liveness (CONNECTED / DEGRADED / STALE, timeout → reconnect), resets the backoff after a
//!   healthy period and publishes metrics.
//!
//! Role logic is synchronous: it reacts to frames and commands by queueing control messages and
//! application events in a [`Ctx`], which the loop flushes afterwards.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use protocol::clock::{ClockEstimate, ClockSample, ClockSync};
use protocol::frame::{FrameMeta, FramePriority, MsgType};
use protocol::messages::{
    ClientRole, Disconnect, DisconnectReason, Message, Ping, Pong, ServerHello,
};
use protocol::sequence::SequenceStats;
use protocol::{Frame, HeaderRuleError};
use rand::SeedableRng;
use rand::rngs::StdRng;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval, timeout};

use crate::auth::AuthManager;
use crate::backoff::Backoff;
use crate::clock::LinkClock;
use crate::config::ClientConfig;
use crate::error::{ConnectError, TransportError};
use crate::handshake::{self, Disposition, HandshakeError, Link, MIN_RELAY_PAYLOAD_LIMIT};
use crate::liveness::{LinkHealth, LivenessMonitor};
use crate::metrics::{LinkMetrics, MetricsInputs, MetricsState, WriterCounters};
use crate::queue::{Outbound, OutboundQueue};
use crate::state::{
    ConnectionState, ConnectionStatus, DegradedReason, FailureReason, StatePublisher,
};
use crate::transport::{FrameReader, FrameWriter, TelemetryTransport};

/// First sequence number of a sequence space (§5).
pub(crate) const FIRST_SEQUENCE: u64 = 1;
/// Capacity of a connection's own control lane (PING, DISCONNECT, SESSION_CLOSE).
const CONTROL_LANE_CAPACITY: usize = 16;
/// Outstanding PINGs remembered for matching PONGs.
const MAX_OUTSTANDING_PINGS: usize = 8;
/// Shortest PING period, whatever the relay announces (a bogus tiny value must not flood it).
const MIN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
/// A reliable application event not taken within this time is dropped and counted. The
/// application must drain its event receiver; this only protects the network task.
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(2);
/// A clock sample whose offset differs from the current estimate by more than this, plus both
/// uncertainties, means the local monotonic clock jumped (e.g. Windows sleep/resume): the sample
/// window restarts instead of keeping a stale minimum-delay sample for up to 16 s.
const CLOCK_STEP_THRESHOLD_NS: u128 = 100_000_000;

/// Formats an error with its whole `source()` chain (never contains secrets: no error type of this
/// crate carries one).
pub(crate) fn error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(": ");
        text.push_str(&e.to_string());
        source = e.source();
    }
    text
}

/// Aborts a task when dropped (a connection's writer must never outlive its connection).
pub(crate) struct AbortOnDrop<T>(pub JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Failure to establish a connection.
#[derive(Debug)]
pub(crate) enum EstablishError {
    Connect(ConnectError),
    Handshake(HandshakeError),
}

impl EstablishError {
    pub(crate) fn disposition(&self) -> Disposition {
        match self {
            EstablishError::Connect(e) if !e.is_retryable() => {
                Disposition::Fail(FailureReason::Configuration {
                    detail: error_chain(e),
                })
            }
            EstablishError::Connect(_) => Disposition::Retry,
            EstablishError::Handshake(e) => e.disposition(),
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            EstablishError::Connect(e) => error_chain(e),
            EstablishError::Handshake(e) => error_chain(e),
        }
    }

    pub(crate) fn is_security_issue(&self) -> bool {
        match self {
            EstablishError::Connect(e) => e.is_security_issue(),
            _ => false,
        }
    }
}

impl From<HandshakeError> for EstablishError {
    fn from(e: HandshakeError) -> Self {
        EstablishError::Handshake(e)
    }
}

/// An authenticated connection, ready for the session binding.
pub(crate) struct Established<R, W> {
    pub link: Link<R, W>,
    pub server: ServerHello,
}

/// State of a client that outlives its connections.
pub(crate) struct ClientCore {
    pub config: ClientConfig,
    pub clock: LinkClock,
    pub queue: Arc<OutboundQueue>,
    pub status: StatePublisher,
    pub metrics_tx: watch::Sender<LinkMetrics>,
    pub metrics: MetricsState,
    pub writer_counters: Arc<WriterCounters>,
    pub clock_sync: ClockSync,
    /// Next stream sequence number of this client in the current session (written by the
    /// connection's writer task only).
    pub next_sequence: Arc<AtomicU64>,
    pub backoff: Backoff,
    pub rng: StdRng,
    pub auth: Arc<AuthManager>,
    pub component: &'static str,
}

impl ClientCore {
    /// New core in DISCONNECTED.
    pub(crate) fn new(
        config: ClientConfig,
        auth: Arc<AuthManager>,
        component: &'static str,
    ) -> (
        Self,
        watch::Receiver<ConnectionStatus>,
        watch::Receiver<LinkMetrics>,
    ) {
        let (status, status_rx) = StatePublisher::new(component);
        let (metrics_tx, metrics_rx) = watch::channel(LinkMetrics::default());
        let timing = config.timing;
        let core = Self {
            clock: LinkClock::with_epoch(config.mono_epoch),
            queue: Arc::new(OutboundQueue::new(config.reliable_queue_capacity)),
            status,
            metrics_tx,
            metrics: MetricsState::new(),
            writer_counters: Arc::new(WriterCounters::default()),
            clock_sync: ClockSync::new(),
            next_sequence: Arc::new(AtomicU64::new(FIRST_SEQUENCE)),
            backoff: Backoff::new(timing.reconnect_min, timing.reconnect_max),
            rng: StdRng::from_os_rng(),
            auth,
            component,
            config,
        };
        (core, status_rx, metrics_rx)
    }

    /// Restarts the stream sequence space at 1 (new session or new app run, §5).
    pub(crate) fn reset_sequence(&self) {
        self.next_sequence.store(FIRST_SEQUENCE, Ordering::Relaxed);
    }

    /// Publishes a metrics snapshot.
    pub(crate) fn publish_metrics(&mut self, sequence: Option<SequenceStats>) {
        let inputs = MetricsInputs {
            frames_out_total: self.writer_counters.frames_out(),
            oversize_dropped: self.writer_counters.oversize_dropped(),
            unencodable_dropped: self.writer_counters.unencodable_dropped(),
            dropped_obsolete: self.queue.dropped_obsolete(),
            reliable_discarded: self.queue.reliable_discarded(),
            send_queue_depth: self.queue.depth(),
            clock: self.clock_sync.estimate(),
            sequence,
            awaiting_schema: self.queue.awaiting_schema(),
        };
        let snapshot = self.metrics.snapshot(self.clock.now(), &inputs);
        self.metrics_tx.send_replace(snapshot);
    }

    /// After a failed attempt or a lost connection: FAILED (returns `None`), or RECONNECTING with
    /// the backoff delay to wait before the next attempt.
    pub(crate) fn after_failure(
        &mut self,
        disposition: Disposition,
        error: String,
    ) -> Option<Duration> {
        match disposition {
            Disposition::Fail(reason) => {
                self.status.update(|s| s.last_error = Some(error));
                self.status.fail(reason);
                None
            }
            Disposition::Retry => {
                let delay = self.backoff.next_delay(&mut self.rng);
                let attempt = self.backoff.attempt();
                tracing::info!(
                    component = self.component,
                    error = %error,
                    retry_in_ms = delay.as_millis() as u64,
                    attempt,
                    "connection attempt failed, will retry"
                );
                self.status.transition(ConnectionState::Reconnecting, |s| {
                    s.last_error = Some(error);
                    s.retry_in = Some(delay);
                    s.reconnect_attempt = attempt;
                    s.session_id = None;
                });
                // Nothing publishes during the backoff: the snapshot shown meanwhile must say "no
                // link" (rates None) and carry the current counters, not the last connected one.
                let sequence = self.metrics_tx.borrow().sequence;
                self.publish_metrics(sequence);
                Some(delay)
            }
        }
    }
}

/// Connects and runs HELLO + AUTH (CONNECTING → AUTHENTICATING).
pub(crate) async fn establish<T: TelemetryTransport>(
    core: &mut ClientCore,
    transport: &T,
    role: ClientRole,
) -> Result<Established<T::Reader, T::Writer>, EstablishError> {
    core.status.transition(ConnectionState::Connecting, |s| {
        s.retry_in = None;
    });
    let conn = match transport.connect(&core.config.endpoint).await {
        Ok(c) => {
            core.status.update(|s| s.security_issue = false);
            c
        }
        Err(e) => {
            let is_sec = e.is_security_issue();
            core.status.update(|s| s.security_issue = is_sec);
            return Err(EstablishError::Connect(e));
        }
    };
    core.status
        .transition(ConnectionState::Authenticating, |_| {});
    let mut link = Link {
        reader: conn.reader,
        writer: conn.writer,
        stats: conn.stats,
        info: conn.info,
        clock: core.clock,
        reply_timeout: core.config.timing.handshake_timeout,
    };
    // HELLO carries no credential and needs no protection against cancellation.
    let server = handshake::hello(&mut link, &core.config, role).await?;
    // Detached: a stop / cancel during AUTH must not lose a rotated refresh token.
    let (link, account) =
        handshake::authenticate_detached(link, Arc::clone(&core.auth), role, server.clone())
            .await?;
    tracing::info!(
        component = core.component,
        relay_version = %server.relay_version,
        tls = ?link.info.tls_version,
        role = ?account.role,
        "authenticated"
    );
    Ok(Established { link, server })
}

/// A command for a connection's writer task.
#[derive(Debug)]
pub(crate) enum WriterCommand {
    /// Write this control message next.
    Send(Box<Message>),
    /// Write these messages, flush and close the stream.
    Close { last: Vec<Message> },
}

/// What the receive loop does after a role callback.
#[derive(Debug)]
pub(crate) enum Flow {
    Continue,
    Close(CloseAction),
}

/// How a connection ends.
#[derive(Debug)]
pub(crate) enum CloseAction {
    /// On purpose: write `last` (e.g. SESSION_CLOSE, DISCONNECT), then close.
    Graceful { last: Vec<Message> },
    /// Lost or unusable: reconnect.
    Reconnect(String),
    /// Ended for good.
    Fail(FailureReason),
}

/// How [`run_connection`] ended.
#[derive(Debug)]
pub(crate) enum ConnectionEnd {
    /// Closed on purpose. `flushed`: the final messages (e.g. SESSION_CLOSE) were written and the
    /// stream closed without an error; `false` if the link failed or timed out first.
    Closed { flushed: bool },
    /// Lost; reconnect.
    Lost(String),
    /// Ended for good.
    Failed(FailureReason),
}

/// An application event queued by role logic.
enum Emitted<E> {
    /// Must arrive (waits up to [`EVENT_SEND_TIMEOUT`] for channel space).
    Reliable(E),
    /// Latest-wins data: dropped (and counted) if the channel is full.
    Droppable(E),
}

/// What role logic may do while handling a frame, command or tick.
pub(crate) struct Ctx<'a, E> {
    pub now: Instant,
    pub now_ns: u64,
    pub now_ms: u64,
    pub session_id: u64,
    pub metrics: &'a mut MetricsState,
    pub queue: &'a OutboundQueue,
    pub status: &'a StatePublisher,
    control: Vec<Message>,
    events: Vec<Emitted<E>>,
}

impl<E> Ctx<'_, E> {
    /// Sends a control message on this connection, ahead of all queued traffic.
    pub(crate) fn send_control(&mut self, message: Message) {
        self.control.push(message);
    }

    /// Emits an application event that must not be lost.
    pub(crate) fn emit(&mut self, event: E) {
        self.events.push(Emitted::Reliable(event));
    }

    /// Emits a latest-wins application event.
    pub(crate) fn emit_droppable(&mut self, event: E) {
        self.events.push(Emitted::Droppable(event));
    }
}

/// Role-specific behaviour inside [`run_connection`].
pub(crate) trait RoleLogic {
    type Command;
    type Event;

    /// One inbound frame that passed the header rules (PONG never reaches the role).
    fn on_frame(&mut self, ctx: &mut Ctx<'_, Self::Event>, frame: Frame, msg_type: MsgType)
    -> Flow;
    /// A frame of a message type this version does not know (skipped, counted as ignored). A
    /// receiver of the stream must still observe the sequence number of a stream-class id (§5
    /// "Unknown stream types"); the default does nothing.
    fn on_unknown(&mut self, _ctx: &mut Ctx<'_, Self::Event>, _frame: &Frame) {}
    /// One command from the application.
    fn on_command(&mut self, ctx: &mut Ctx<'_, Self::Event>, command: Self::Command) -> Flow;
    /// Periodic hook (every liveness check).
    fn on_tick(&mut self, ctx: &mut Ctx<'_, Self::Event>) -> Flow;
    /// Sequence counters of the inbound stream, if the role tracks one.
    fn sequence(&self) -> Option<SequenceStats>;
}

/// PINGs awaiting their PONG.
#[derive(Debug, Default)]
struct PingTracker {
    next_id: u64,
    outstanding: VecDeque<(u64, u64)>,
}

impl PingTracker {
    fn next(&mut self, t0: u64) -> Ping {
        self.next_id = self.next_id.wrapping_add(1);
        if self.outstanding.len() == MAX_OUTSTANDING_PINGS {
            self.outstanding.pop_front();
        }
        self.outstanding.push_back((self.next_id, t0));
        Ping {
            id: self.next_id,
            t0,
        }
    }

    /// The `t0` of PING `id`; older outstanding PINGs are forgotten (their PONGs are lost).
    fn take(&mut self, id: u64) -> Option<u64> {
        let pos = self.outstanding.iter().position(|(i, _)| *i == id)?;
        let (_, t0) = self.outstanding[pos];
        self.outstanding.drain(..=pos);
        Some(t0)
    }
}

/// What woke the receive loop.
enum Wake<C> {
    Early(Frame),
    WriterEnded(String),
    Command(Option<C>),
    Check,
    Ping,
    Received(Result<Option<Frame>, TransportError>),
}

/// Parameters of a connection's writer task.
struct WriterSetup {
    session_id: u64,
    next_sequence: Arc<AtomicU64>,
    clock: LinkClock,
    write_timeout: Duration,
    close_timeout: Duration,
    max_payload: usize,
    counters: Arc<WriterCounters>,
    component: &'static str,
}

/// Runs a bound session connection until it ends. `early` are frames that arrived during the
/// binding after the session reply (processed first).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_connection<R, W, L>(
    core: &mut ClientCore,
    link: Link<R, W>,
    server: &ServerHello,
    session_id: u64,
    early: Vec<Frame>,
    logic: &mut L,
    commands: &mut mpsc::Receiver<L::Command>,
    events: &mpsc::Sender<L::Event>,
) -> ConnectionEnd
where
    R: FrameReader,
    W: FrameWriter,
    L: RoleLogic,
{
    let Link {
        mut reader,
        writer,
        stats,
        ..
    } = link;
    let timing = core.config.timing;
    let (lane_tx, lane_rx) = mpsc::channel::<WriterCommand>(CONTROL_LANE_CAPACITY);
    // Validated in HELLO (a relay announcing an unusable limit never gets here).
    let max_payload =
        handshake::effective_payload_limit(&server.limits).unwrap_or(MIN_RELAY_PAYLOAD_LIMIT);
    // Producers are refused oversize messages up front from now on (the writer still drops and
    // counts what was queued under a larger limit of an earlier connection).
    core.queue.set_payload_limit(max_payload);
    let setup = WriterSetup {
        session_id,
        next_sequence: Arc::clone(&core.next_sequence),
        clock: core.clock,
        write_timeout: timing.write_timeout,
        close_timeout: timing.close_timeout,
        max_payload,
        counters: Arc::clone(&core.writer_counters),
        component: core.component,
    };
    let mut writer_task = AbortOnDrop(tokio::spawn(writer_loop(
        writer,
        lane_rx,
        Arc::clone(&core.queue),
        setup,
    )));

    let connected_at = core.clock.now();
    let mut liveness = LivenessMonitor::new(timing.liveness, connected_at);
    let mut pings = PingTracker::default();
    let mut backoff_reset = false;
    core.metrics
        .connection_started(Arc::clone(&stats), connected_at);
    core.status.transition(ConnectionState::Connected, |s| {
        s.session_id = Some(session_id);
        s.last_error = None;
    });
    core.publish_metrics(logic.sequence());

    let mut ping_tick = interval(heartbeat_period(
        timing.heartbeat_interval,
        server.limits.heartbeat_interval_ms,
    ));
    ping_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut check_tick = interval(timing.liveness_check_interval);
    check_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut metrics_published_at = connected_at;

    let mut pending_early: VecDeque<Frame> = early.into();
    // A JoinHandle must not be polled again once it returned its output.
    let mut writer_joined = false;

    let action = loop {
        let wake = match pending_early.pop_front() {
            Some(frame) => Wake::Early(frame),
            None => tokio::select! {
                biased;
                joined = &mut writer_task.0 => Wake::WriterEnded(match joined {
                    Ok(Ok(())) => "the writer stopped".to_owned(),
                    Ok(Err(e)) => error_chain(&e),
                    Err(e) => format!("the writer task failed: {e}"),
                }),
                command = commands.recv() => Wake::Command(command),
                _ = check_tick.tick() => Wake::Check,
                _ = ping_tick.tick() => Wake::Ping,
                received = reader.recv() => Wake::Received(received),
            },
        };
        // Timestamps are taken after the wait, when the event actually happened.
        let now = core.clock.now();
        let mut ctx = Ctx {
            now,
            now_ns: core.clock.ns_at(now),
            now_ms: core.clock.now_ms(),
            session_id,
            metrics: &mut core.metrics,
            queue: &core.queue,
            status: &core.status,
            control: Vec::new(),
            events: Vec::new(),
        };
        let flow = match wake {
            Wake::Early(frame) => dispatch(
                frame,
                &mut ctx,
                logic,
                &mut pings,
                &mut liveness,
                &mut core.clock_sync,
            ),
            Wake::WriterEnded(reason) => {
                writer_joined = true;
                Flow::Close(CloseAction::Reconnect(reason))
            }
            Wake::Command(Some(command)) => logic.on_command(&mut ctx, command),
            // Every handle was dropped: the application is shutting down.
            Wake::Command(None) => Flow::Close(CloseAction::Graceful {
                last: vec![disconnect(DisconnectReason::ClientShutdown)],
            }),
            Wake::Check => {
                liveness.observe_bytes(stats.bytes_in(), now);
                if let Some(seq) = logic.sequence() {
                    let totals = ctx.metrics.sequence_totals(Some(&seq));
                    liveness.observe_sequence(totals.received, totals.lost);
                }
                if !backoff_reset
                    && now.saturating_duration_since(connected_at) >= timing.backoff_reset_after
                {
                    core.backoff.reset();
                    backoff_reset = true;
                }
                let _ = core.auth.retry_pending_persists();
                if let Some(err) = core.auth.last_persist_error() {
                    core.status.update(|s| s.persist_error = Some(err));
                } else if core.status.status().persist_error.is_some() {
                    core.status.update(|s| s.persist_error = None);
                }
                match apply_health(&core.status, liveness.evaluate(now)) {
                    Some(action) => Flow::Close(action),
                    None => logic.on_tick(&mut ctx),
                }
            }
            Wake::Ping => {
                let ping = pings.next(ctx.now_ns);
                ctx.send_control(Message::Ping(ping));
                Flow::Continue
            }
            Wake::Received(Ok(Some(frame))) => {
                liveness.on_frame(now);
                ctx.metrics.frame_in();
                dispatch(
                    frame,
                    &mut ctx,
                    logic,
                    &mut pings,
                    &mut liveness,
                    &mut core.clock_sync,
                )
            }
            Wake::Received(Ok(None)) => Flow::Close(CloseAction::Reconnect(
                "the relay closed the connection".to_owned(),
            )),
            Wake::Received(Err(e)) => Flow::Close(CloseAction::Reconnect(error_chain(&e))),
        };
        let Ctx {
            control,
            events: emitted,
            ..
        } = ctx;
        for message in control {
            if lane_tx
                .send(WriterCommand::Send(Box::new(message)))
                .await
                .is_err()
            {
                break; // writer gone; its result is picked up next iteration
            }
        }
        // At most one wait of EVENT_SEND_TIMEOUT per iteration: once the application failed to
        // take an event in time, the rest of this batch is offered without waiting, so a stuck
        // consumer cannot stall heartbeats and liveness checks for long.
        let mut may_wait = true;
        for event in emitted {
            let reliable = matches!(event, Emitted::Reliable(_));
            if !deliver(events, event, &mut core.metrics, may_wait).await && reliable {
                may_wait = false;
                core.status.transition(ConnectionState::Degraded, |s| {
                    s.degraded = Some(DegradedReason::PacketLoss);
                });
            }
        }
        // The liveness tick guarantees an iteration at least every check interval.
        if now.saturating_duration_since(metrics_published_at) >= timing.metrics_interval {
            metrics_published_at = now;
            core.publish_metrics(logic.sequence());
        }
        if let Flow::Close(action) = flow {
            break action;
        }
    };

    let end = match action {
        CloseAction::Graceful { last } => {
            let close = lane_tx.send(WriterCommand::Close { last });
            // Wait for the writer to flush and close; it is aborted below otherwise.
            let mut flushed = false;
            if timeout(timing.close_timeout, close).await.is_ok()
                && let Ok(joined) =
                    timeout(timing.close_timeout.saturating_mul(2), &mut writer_task.0).await
            {
                writer_joined = true;
                // `last` reached the TLS stream only if the writer ended without an error.
                flushed = matches!(joined, Ok(Ok(())));
            }
            ConnectionEnd::Closed { flushed }
        }
        CloseAction::Reconnect(reason) => ConnectionEnd::Lost(reason),
        CloseAction::Fail(reason) => ConnectionEnd::Failed(reason),
    };
    if !writer_joined {
        // Stop the writer and wait until it is gone (an aborted task stops at its next await),
        // so that the reliable message it may have been writing is re-queued without racing it.
        writer_task.0.abort();
        let _ = (&mut writer_task.0).await;
    }
    drop(writer_task);
    if core.queue.requeue_in_flight() {
        tracing::debug!(
            component = core.component,
            "reliable message interrupted by the connection end, re-queued"
        );
    }
    core.metrics.connection_ended();
    core.publish_metrics(logic.sequence());
    end
}

/// PING period of a connection: the configured one, or the relay's announced
/// `heartbeat_interval_ms` if that is shorter (a relay may time out clients that ping more slowly
/// than it expects), never below [`MIN_HEARTBEAT_INTERVAL`]. `0` means "not announced".
fn heartbeat_period(configured: Duration, relay_ms: u32) -> Duration {
    let period = match relay_ms {
        0 => configured,
        ms => configured.min(Duration::from_millis(u64::from(ms))),
    };
    period.max(MIN_HEARTBEAT_INTERVAL)
}

/// Maps a liveness verdict to the published state; `Some` means the connection is dead.
fn apply_health(status: &StatePublisher, health: LinkHealth) -> Option<CloseAction> {
    match health {
        LinkHealth::Healthy => {
            let cur = status.status();
            if cur.driver_away {
                status.transition(ConnectionState::Degraded, |s| {
                    s.reconnect_attempt = 0;
                    s.degraded = Some(DegradedReason::DriverAway);
                });
            } else if let Some(reason) = cur.degraded {
                if matches!(
                    reason,
                    DegradedReason::DriverAway
                        | DegradedReason::NoStreamData
                        | DegradedReason::AwaitingSchema
                        | DegradedReason::SequenceDesync
                ) {
                    status.transition(ConnectionState::Degraded, |s| {
                        s.reconnect_attempt = 0;
                        s.degraded = Some(reason);
                    });
                } else {
                    status.transition(ConnectionState::Connected, |s| {
                        s.reconnect_attempt = 0;
                        s.degraded = None;
                    });
                }
            } else {
                status.transition(ConnectionState::Connected, |s| {
                    s.reconnect_attempt = 0;
                    s.degraded = None;
                });
            }
            None
        }
        LinkHealth::Degraded(reason) => {
            let cur = status.status();
            let final_reason = if cur.driver_away {
                DegradedReason::DriverAway
            } else {
                reason
            };
            status.transition(ConnectionState::Degraded, |s| {
                s.degraded = Some(final_reason)
            });
            None
        }
        LinkHealth::Stale => {
            status.transition(ConnectionState::Stale, |_| {});
            None
        }
        LinkHealth::TimedOut => Some(CloseAction::Reconnect(
            "no data from the relay within the heartbeat timeout".to_owned(),
        )),
    }
}

/// Header rules, PONG handling, then the role.
fn dispatch<L: RoleLogic>(
    frame: Frame,
    ctx: &mut Ctx<'_, L::Event>,
    logic: &mut L,
    pings: &mut PingTracker,
    liveness: &mut LivenessMonitor,
    clock_sync: &mut ClockSync,
) -> Flow {
    let msg_type = match frame.check_rules() {
        Ok(t) => t,
        Err(HeaderRuleError::UnknownMsgType(id)) => {
            // Forward compatibility: a newer relay's type is skipped (§1).
            tracing::debug!(
                component = "transport",
                msg_type = id,
                "unknown message type skipped"
            );
            ctx.metrics.ignored();
            logic.on_unknown(ctx, &frame);
            return Flow::Continue;
        }
        Err(e) => {
            tracing::warn!(component = "transport", error = %e, "inbound frame violates header rules, skipped");
            ctx.metrics.header_rule_violation();
            return Flow::Continue;
        }
    };
    if msg_type == MsgType::Pong {
        match Message::from_frame(&frame) {
            Ok(Message::Pong(pong)) => on_pong(ctx, pong, pings, liveness, clock_sync),
            _ => ctx.metrics.decode_error(),
        }
        return Flow::Continue;
    }
    logic.on_frame(ctx, frame, msg_type)
}

/// RTT, clock sync (with clock-step detection) and heartbeat bookkeeping for one PONG.
fn on_pong<E>(
    ctx: &mut Ctx<'_, E>,
    pong: Pong,
    pings: &mut PingTracker,
    liveness: &mut LivenessMonitor,
    clock_sync: &mut ClockSync,
) {
    let t3 = ctx.now_ns;
    let Some(t0) = pings.take(pong.id) else {
        tracing::debug!(
            component = "transport",
            id = pong.id,
            "PONG without outstanding PING"
        );
        ctx.metrics.ignored();
        return;
    };
    if pong.t0 != t0 {
        // Not the answer to our PING (§8): no RTT, no clock sample, no heartbeat credit.
        tracing::debug!(
            component = "transport",
            id = pong.id,
            "PONG echoes a foreign t0, ignored"
        );
        ctx.metrics.ignored();
        return;
    }
    if let (Ok(sample), Some(estimate)) = (
        ClockSample::from_timestamps(t0, pong.t1, pong.t2, t3),
        clock_sync.estimate(),
    ) && let Some(jump_ns) = clock_step(&estimate, &sample)
    {
        tracing::warn!(
            component = "transport",
            jump_ns = u64::try_from(jump_ns).unwrap_or(u64::MAX),
            "clock offset jumped (sleep/resume or clock step): restarting clock sync"
        );
        clock_sync.reset();
    }
    if let Err(e) = clock_sync.on_pong(pong.id, t0, &pong, t3) {
        tracing::debug!(component = "transport", error = %e, "clock sample rejected");
    }
    let rtt = Duration::from_nanos(t3.saturating_sub(t0));
    liveness.on_pong(ctx.now, rtt);
    ctx.metrics.rtt(rtt, liveness.srtt());
}

/// The offset jump of `sample` against the current `estimate`, if it exceeds what network delay can
/// explain ([`CLOCK_STEP_THRESHOLD_NS`] plus both uncertainties): the local monotonic clock or the
/// relay clock stepped, so the window of minimum-delay samples no longer describes the offset.
fn clock_step(estimate: &ClockEstimate, sample: &ClockSample) -> Option<u128> {
    let jump = (i128::from(sample.offset_ns) - i128::from(estimate.offset_ns)).unsigned_abs();
    let allowed = CLOCK_STEP_THRESHOLD_NS
        + u128::from(estimate.uncertainty_ns)
        + u128::from(sample.delay_ns / 2);
    (jump > allowed).then_some(jump)
}

/// Delivers one application event. A reliable event waits up to [`EVENT_SEND_TIMEOUT`] for channel
/// space if `may_wait`. Returns `false` if the event was dropped (counted) because the channel was
/// full; `true` if it was delivered or nobody listens any more.
async fn deliver<E>(
    events: &mpsc::Sender<E>,
    event: Emitted<E>,
    metrics: &mut MetricsState,
    may_wait: bool,
) -> bool {
    let delivered = match event {
        Emitted::Reliable(e) if may_wait => matches!(
            timeout(EVENT_SEND_TIMEOUT, events.send(e)).await,
            Ok(Ok(()))
        ),
        Emitted::Reliable(e) | Emitted::Droppable(e) => events.try_send(e).is_ok(),
    };
    if delivered || events.is_closed() {
        return true;
    }
    metrics.event_dropped();
    tracing::warn!(
        component = "transport",
        "application event dropped: event channel full"
    );
    false
}

/// `DISCONNECT{reason}`.
pub(crate) fn disconnect(reason: DisconnectReason) -> Message {
    Message::Disconnect(Disconnect {
        reason,
        message: None,
    })
}

enum LaneOutcome {
    Continue,
    Exit,
}

/// Live frames the writer took from the latest-wins slots for one round. Whatever is still in it
/// when the writer ends (connection lost, task aborted, graceful close) is counted as
/// `dropped_obsolete` on drop, so every live frame is either written or counted.
struct LiveRound {
    frames: VecDeque<(Message, FramePriority)>,
    /// A frame taken from `frames` whose write has not completed.
    writing: bool,
    queue: Arc<OutboundQueue>,
}

impl Drop for LiveRound {
    fn drop(&mut self) {
        let unsent = self.frames.len() + usize::from(self.writing);
        if unsent > 0 {
            self.queue
                .count_unsent_live(u64::try_from(unsent).unwrap_or(u64::MAX));
        }
    }
}

/// The writer task of one connection.
async fn writer_loop<W: FrameWriter>(
    mut writer: W,
    mut lane: mpsc::Receiver<WriterCommand>,
    queue: Arc<OutboundQueue>,
    setup: WriterSetup,
) -> Result<(), TransportError> {
    let mut round = LiveRound {
        frames: VecDeque::new(),
        writing: false,
        queue: Arc::clone(&queue),
    };
    let mut unflushed = false;
    loop {
        // 1. The connection's own control lane (PING, DISCONNECT, ...).
        match lane.try_recv() {
            Ok(command) => {
                unflushed = true;
                match handle_lane(&mut writer, command, &setup).await? {
                    LaneOutcome::Exit => return Ok(()),
                    LaneOutcome::Continue => continue,
                }
            }
            Err(TryRecvError::Disconnected) => return Ok(()),
            Err(TryRecvError::Empty) => {}
        }
        // 2. The reliable queue: never overtaken by telemetry (§2). The item stays in flight until
        //    its write completed; if this task fails or is aborted first, the connection's owner
        //    re-queues it (`OutboundQueue::requeue_in_flight`).
        if let Some(item) = queue.pop_reliable() {
            let written = match &item {
                Outbound::Control(message) => write_control(&mut writer, message, &setup).await?,
                Outbound::Stream { message, priority } => {
                    write_stream(&mut writer, message, *priority, &setup).await?
                }
            };
            queue.reliable_written();
            if item.msg_type() == MsgType::SchemaNegotiate {
                if written {
                    // The space's first stream frame is out: held telemetry may follow (§5).
                    queue.schema_written();
                } else if queue.schema_dropped() {
                    // The held stream waits for a schema that will never be written: the role
                    // logic asks the application for a new one (driver: `SnapshotNeeded`).
                    tracing::warn!(
                        component = setup.component,
                        "SCHEMA_NEGOTIATE dropped while the stream waits for it: a new one is needed"
                    );
                }
            }
            unflushed = true;
            continue;
        }
        // 3. Latest-wins frames, one round at a time.
        if round.frames.is_empty() {
            queue.take_live_round(&mut round.frames);
        }
        if let Some((message, priority)) = round.frames.pop_front() {
            round.writing = true;
            write_stream(&mut writer, &message, priority, &setup).await?;
            round.writing = false;
            unflushed = true;
            continue;
        }
        // 4. Idle: push buffered bytes out, then wait for work.
        if unflushed {
            timed(setup.write_timeout, writer.flush()).await?;
            unflushed = false;
            continue;
        }
        let notified = queue.notified();
        tokio::select! {
            command = lane.recv() => match command {
                None => return Ok(()),
                Some(command) => {
                    unflushed = true;
                    if let LaneOutcome::Exit = handle_lane(&mut writer, command, &setup).await? {
                        return Ok(());
                    }
                }
            },
            () = notified => {}
        }
    }
}

async fn handle_lane<W: FrameWriter>(
    writer: &mut W,
    command: WriterCommand,
    setup: &WriterSetup,
) -> Result<LaneOutcome, TransportError> {
    match command {
        WriterCommand::Send(message) => {
            write_control(writer, &message, setup).await?;
            Ok(LaneOutcome::Continue)
        }
        WriterCommand::Close { last } => {
            for message in &last {
                write_control(writer, message, setup).await?;
            }
            timed(setup.close_timeout, writer.flush()).await?;
            timed(setup.close_timeout, writer.close()).await?;
            Ok(LaneOutcome::Exit)
        }
    }
}

async fn timed<F>(limit: Duration, op: F) -> Result<(), TransportError>
where
    F: std::future::Future<Output = Result<(), TransportError>>,
{
    timeout(limit, op)
        .await
        .map_err(|_| TransportError::WriteTimeout { timeout: limit })?
}

/// Counts (and logs, at powers of two only) a message that could not be encoded.
fn unencodable(setup: &WriterSetup, msg_type: MsgType, error: &protocol::messages::MessageError) {
    let dropped_total = setup.counters.unencodable();
    // A message every frame fails on must not flood the log (the metrics count every drop).
    if dropped_total.is_power_of_two() {
        tracing::error!(
            component = setup.component,
            %msg_type,
            error = %error,
            dropped_total,
            "cannot encode message, dropped"
        );
    }
}

/// Writes a control-class message (sequence 0, the bound session id). Returns whether it was
/// written (`false`: dropped and counted).
async fn write_control<W: FrameWriter>(
    writer: &mut W,
    message: &Message,
    setup: &WriterSetup,
) -> Result<bool, TransportError> {
    if message.msg_type().is_stream() {
        // Stream-class messages need a sequence number (SESSION_EVENT etc. via the reliable queue).
        return write_stream(writer, message, FramePriority::Control, setup).await;
    }
    let frame = match message.to_frame(&FrameMeta::control(setup.session_id, setup.clock.now_ns()))
    {
        Ok(frame) => frame,
        Err(e) => {
            unencodable(setup, message.msg_type(), &e);
            return Ok(false);
        }
    };
    timed(setup.write_timeout, writer.send(&frame)).await?;
    setup.counters.frame_written();
    Ok(true)
}

/// Writes a stream-class message with the next sequence number. The number is consumed only when
/// the frame is written, so frames refused here leave no sequence gap. Returns whether it was
/// written (`false`: dropped and counted).
async fn write_stream<W: FrameWriter>(
    writer: &mut W,
    message: &Message,
    priority: FramePriority,
    setup: &WriterSetup,
) -> Result<bool, TransportError> {
    let sequence = setup.next_sequence.load(Ordering::Relaxed);
    let meta = FrameMeta::stream(priority, setup.session_id, sequence, setup.clock.now_ns());
    let frame = match message.to_frame(&meta) {
        Ok(frame) => frame,
        Err(e) => {
            unencodable(setup, message.msg_type(), &e);
            return Ok(false);
        }
    };
    if frame.payload().len() > setup.max_payload {
        let dropped_total = setup.counters.oversize();
        // Logged at powers of two only: a limit every frame exceeds must not flood the log (the
        // metrics count every drop).
        if dropped_total.is_power_of_two() {
            tracing::error!(
                component = setup.component,
                msg_type = %message.msg_type(),
                len = frame.payload().len(),
                limit = setup.max_payload,
                dropped_total,
                "payload exceeds the relay limit, dropped"
            );
        }
        return Ok(false);
    }
    timed(setup.write_timeout, writer.send(&frame)).await?;
    setup
        .next_sequence
        .store(sequence.wrapping_add(1), Ordering::Relaxed);
    setup.counters.frame_written();
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use bytes::Bytes;
    use parking_lot::Mutex;
    use protocol::frame::MAX_PAYLOAD_BYTES;
    use protocol::messages::{SnapshotReason, SnapshotRequest, TelemetryPayload};

    /// A link that accepts `capacity` frames and then stalls: every further write never completes.
    struct StallingWriter {
        written: Arc<Mutex<Vec<Frame>>>,
        attempts: Arc<AtomicUsize>,
        capacity: usize,
    }

    impl FrameWriter for StallingWriter {
        async fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.written.lock().len() >= self.capacity {
                std::future::pending::<()>().await;
            }
            self.written.lock().push(frame.clone());
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), TransportError> {
            Ok(())
        }

        async fn close(&mut self) -> Result<(), TransportError> {
            Ok(())
        }
    }

    struct WriterHarness {
        queue: Arc<OutboundQueue>,
        written: Arc<Mutex<Vec<Frame>>>,
        attempts: Arc<AtomicUsize>,
        next_sequence: Arc<AtomicU64>,
        counters: Arc<WriterCounters>,
        task: JoinHandle<Result<(), TransportError>>,
        _lane: mpsc::Sender<WriterCommand>,
    }

    /// Runs a writer over a link that stalls after `capacity` frames.
    fn writer_over_stalling_link(queue: Arc<OutboundQueue>, capacity: usize) -> WriterHarness {
        writer_with_limit(queue, capacity, MAX_PAYLOAD_BYTES)
    }

    /// Like [`writer_over_stalling_link`], with the relay's payload limit `max_payload`.
    fn writer_with_limit(
        queue: Arc<OutboundQueue>,
        capacity: usize,
        max_payload: usize,
    ) -> WriterHarness {
        let counters = Arc::new(WriterCounters::default());
        let written = Arc::new(Mutex::new(Vec::new()));
        let attempts = Arc::new(AtomicUsize::new(0));
        let next_sequence = Arc::new(AtomicU64::new(FIRST_SEQUENCE));
        let (lane, lane_rx) = mpsc::channel(CONTROL_LANE_CAPACITY);
        let setup = WriterSetup {
            session_id: 7,
            next_sequence: Arc::clone(&next_sequence),
            clock: LinkClock::with_epoch(std::time::Instant::now()),
            // Long enough never to fire: the tests end the writer by aborting it.
            write_timeout: Duration::from_secs(3_600),
            close_timeout: Duration::from_secs(1),
            max_payload,
            counters: Arc::clone(&counters),
            component: "transport.test",
        };
        let writer = StallingWriter {
            written: Arc::clone(&written),
            attempts: Arc::clone(&attempts),
            capacity,
        };
        let task = tokio::spawn(writer_loop(writer, lane_rx, Arc::clone(&queue), setup));
        WriterHarness {
            queue,
            written,
            attempts,
            next_sequence,
            counters,
            task,
            _lane: lane,
        }
    }

    /// Yields until `pred` holds.
    async fn until(what: &str, mut pred: impl FnMut() -> bool) {
        for _ in 0..10_000 {
            if pred() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("never happened: {what}");
    }

    fn schema() -> Message {
        Message::SchemaNegotiate(protocol::messages::SchemaNegotiate {
            schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
            capabilities: Default::default(),
            vehicle_capabilities: Default::default(),
        })
    }

    #[tokio::test]
    async fn the_writer_releases_held_telemetry_only_after_writing_the_schema() {
        let queue = Arc::new(OutboundQueue::new(4));
        queue.hold_until_schema();
        queue
            .push_live(delta(1), FramePriority::P0, 0)
            .expect("delta");
        let h = writer_over_stalling_link(Arc::clone(&queue), usize::MAX);
        for _ in 0..1_000 {
            tokio::task::yield_now().await;
        }
        assert!(h.written.lock().is_empty(), "held until the schema");
        queue
            .try_push_reliable(Outbound::Stream {
                message: schema(),
                priority: FramePriority::Control,
            })
            .expect("schema");
        until("two frames written", || h.written.lock().len() == 2).await;
        let written: Vec<(u64, Option<MsgType>)> = h
            .written
            .lock()
            .iter()
            .map(|f| (f.header().sequence, f.msg_type()))
            .collect();
        assert_eq!(
            written,
            vec![
                (1, Some(MsgType::SchemaNegotiate)),
                (2, Some(MsgType::TelemetryDelta))
            ]
        );
        assert!(!queue.awaiting_schema());
        h.task.abort();
    }

    #[tokio::test]
    async fn a_frame_queued_under_a_larger_limit_is_dropped_counted_and_leaves_no_gap() {
        // Queued while the limit was the protocol maximum; the new connection's relay announces
        // less: the writer drops it, counts it, and consumes no sequence number.
        let queue = Arc::new(OutboundQueue::new(4));
        queue
            .push_live(delta(1), FramePriority::P0, 0)
            .expect("delta");
        let h = writer_with_limit(Arc::clone(&queue), usize::MAX, 63);
        until("the oversize drop is counted", || {
            h.counters.oversize_dropped() == 1
        })
        .await;
        assert!(h.written.lock().is_empty());
        assert_eq!(h.next_sequence.load(Ordering::SeqCst), FIRST_SEQUENCE);
        assert_eq!(h.counters.unencodable_dropped(), 0);
        h.task.abort();
    }

    /// Yields until the writer attempted `n` writes (the last one stalls).
    async fn until_attempts(h: &WriterHarness, n: usize) {
        for _ in 0..10_000 {
            if h.attempts.load(Ordering::SeqCst) >= n {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the writer never attempted {n} writes");
    }

    fn delta(tag: u8) -> Message {
        Message::TelemetryDelta(TelemetryPayload::new(Bytes::from(vec![tag; 64])).expect("tlv"))
    }

    #[tokio::test]
    async fn live_frames_taken_by_the_writer_are_counted_when_its_connection_ends() {
        let queue = Arc::new(OutboundQueue::new(4));
        for (tag, priority) in [
            (1, FramePriority::P0),
            (2, FramePriority::P1),
            (3, FramePriority::P2),
        ] {
            queue.push_live(delta(tag), priority, 0).expect("delta");
        }
        // The writer takes all three as one round, writes the first and stalls on the second.
        let h = writer_over_stalling_link(queue, 1);
        until_attempts(&h, 2).await;
        h.task.abort();
        assert!(h.task.await.is_err(), "aborted");
        assert_eq!(h.written.lock().len(), 1);
        // The frame being written and the one still in the round are counted, not lost silently:
        // written + dropped_obsolete == pushed.
        assert_eq!(h.queue.dropped_obsolete(), 2);
        assert_eq!(h.queue.depth(), 0);
    }

    #[tokio::test]
    async fn a_reliable_message_whose_write_was_aborted_is_requeued_with_its_sequence_unused() {
        let queue = Arc::new(OutboundQueue::new(4));
        let request = Outbound::Stream {
            message: Message::SnapshotRequest(SnapshotRequest {
                reason: SnapshotReason::Manual,
            }),
            priority: FramePriority::Control,
        };
        queue.try_push_reliable(request.clone()).expect("queued");
        // The link stalls on the very first write: the request stays in flight.
        let h = writer_over_stalling_link(queue, 0);
        until_attempts(&h, 1).await;
        assert_eq!(h.queue.depth(), 0, "popped by the writer");
        h.task.abort();
        assert!(h.task.await.is_err(), "aborted");
        // What `run_connection` does once the writer is gone: the message goes back to the front,
        // and its sequence number was never consumed (no gap at the engineer).
        assert!(h.queue.requeue_in_flight());
        assert_eq!(h.next_sequence.load(Ordering::SeqCst), FIRST_SEQUENCE);
        assert_eq!(h.queue.pop_reliable(), Some(request));
        assert!(h.written.lock().is_empty());
    }

    #[test]
    fn ping_tracker_matches_and_forgets_older() {
        let mut p = PingTracker::default();
        let a = p.next(10);
        let b = p.next(20);
        let c = p.next(30);
        assert_eq!(p.take(b.id), Some(20));
        assert_eq!(p.take(a.id), None, "older than a matched PONG: lost");
        assert_eq!(p.take(c.id), Some(30));
        assert_eq!(p.take(c.id), None);
        for i in 0..20 {
            p.next(i);
        }
        assert_eq!(p.outstanding.len(), MAX_OUTSTANDING_PINGS);
    }

    #[test]
    fn clock_step_detects_jumps_beyond_network_delay_only() {
        const MS: u64 = 1_000_000;
        let estimate = ClockEstimate {
            offset_ns: 5_000 * MS as i64,
            uncertainty_ns: 10 * MS,
            min_delay_ns: 20 * MS,
            samples: 16,
        };
        // A sample 50 ms off with 40 ms delay is ordinary jitter (allowance 100 + 10 + 20 ms).
        let jitter = ClockSample {
            offset_ns: 5_050 * MS as i64,
            delay_ns: 40 * MS,
            at_client_ns: 0,
        };
        assert_eq!(clock_step(&estimate, &jitter), None);
        // After Windows sleep/resume the monotonic clock is 30 s behind the relay: a step.
        let resumed = ClockSample {
            offset_ns: 35_000 * MS as i64,
            delay_ns: 40 * MS,
            at_client_ns: 0,
        };
        assert_eq!(
            clock_step(&estimate, &resumed),
            Some(u128::from(30_000 * MS))
        );
        // Steps are detected in both directions.
        let backwards = ClockSample {
            offset_ns: -1_000 * MS as i64,
            delay_ns: 0,
            at_client_ns: 0,
        };
        assert!(clock_step(&estimate, &backwards).is_some());
    }

    #[test]
    fn heartbeat_follows_the_relay_when_it_asks_for_more_frequent_pings() {
        let configured = Duration::from_millis(1_000);
        assert_eq!(heartbeat_period(configured, 0), configured, "not announced");
        assert_eq!(heartbeat_period(configured, 1_000), configured);
        assert_eq!(
            heartbeat_period(configured, 5_000),
            configured,
            "never slower"
        );
        assert_eq!(
            heartbeat_period(configured, 500),
            Duration::from_millis(500)
        );
        assert_eq!(
            heartbeat_period(configured, 1),
            MIN_HEARTBEAT_INTERVAL,
            "floored"
        );
    }

    #[test]
    fn error_chain_includes_sources() {
        let e = TransportError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset by peer",
        ));
        assert_eq!(
            error_chain(&e),
            "I/O error on the relay connection: reset by peer"
        );
    }
}

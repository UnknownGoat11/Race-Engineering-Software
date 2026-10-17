//! **DEV / TEST ONLY.** A TCP proxy that injects network faults between a client and a server.
//!
//! In [`ProxyMode::FrameAware`] (plain protocol frames) every direction is parsed with the protocol's
//! [`FrameDecoder`] and the [`FaultModel`] decides per *whole frame*: forwarded after a latency (+ jitter,
//! order preserved), or dropped. The proxy never forwards a partial frame and never alters a byte of a
//! forwarded one; a framing error closes the connection. Loss and blackout drops only ever hit latest-wins frames
//! (telemetry, scoring, weather, status), which the relay may legitimately drop too; reliable frames are only
//! delayed, since dropping them would break the protocol instead of simulating loss.
//!
//! In [`ProxyMode::Opaque`] (e.g. TLS records the proxy cannot parse) bytes are forwarded in read-sized
//! chunks with latency, jitter and held blackouts only; lossy configurations are rejected, since removing
//! bytes from an opaque stream would corrupt it.
//!
//! Both modes support connection drops (on demand or after a fixed connection lifetime) and a restart
//! (all connections dropped, new ones refused for a downtime).

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use protocol::codec::FrameDecoder;
use protocol::frame::{Delivery, Frame};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::Instant;

use crate::error::SyntheticError;
use crate::fault::{DropReason, FaultConfig, FaultModel, MAX_BLACKOUT, Verdict};

/// Default capacity of each direction's delivery queue (units in flight).
pub const DEFAULT_QUEUE_UNITS: usize = 1_024;
/// Most simultaneous proxied connections; further clients are refused.
pub const MAX_CONNECTIONS: usize = 64;
/// Read size of one opaque chunk / one socket read (bytes).
const READ_CHUNK_BYTES: usize = 16 * 1_024;
/// Time allowed to connect to the upstream server.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Pause after a failed `accept` (e.g. out of file descriptors) before retrying.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(50);
/// Longest simulated server downtime of [`FaultProxy::restart`]; longer requests are capped.
pub const MAX_DOWNTIME: Duration = Duration::from_secs(3_600);
/// Odd constant mixing connection ids and directions into per-direction seeds (golden ratio).
const SEED_MIX: u64 = 0x9E37_79B9_7F4A_7C15;

/// How the proxy interprets the byte stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProxyMode {
    /// Plain protocol frames: whole-frame faults.
    FrameAware,
    /// Unparsed bytes (TLS): latency, jitter and held blackouts only.
    Opaque,
}

/// Proxy configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct FaultProxyConfig {
    /// Address to listen on (port 0 picks a free port).
    pub listen: SocketAddr,
    /// Server the proxy connects to for every client.
    pub upstream: SocketAddr,
    pub mode: ProxyMode,
    /// Faults of the client â†’ server direction.
    pub to_upstream: FaultConfig,
    /// Faults of the server â†’ client direction.
    pub to_client: FaultConfig,
    /// Seed of every fault decision (mixed with the connection index and direction).
    pub seed: u64,
    /// Every connection is dropped after this lifetime.
    pub drop_after: Option<Duration>,
    /// Capacity of each direction's delivery queue (bounded; a full queue back-pressures the sender).
    pub queue_units: usize,
}

impl FaultProxyConfig {
    /// A frame-aware proxy with perfect links.
    pub fn new(listen: SocketAddr, upstream: SocketAddr) -> Self {
        Self {
            listen,
            upstream,
            mode: ProxyMode::FrameAware,
            to_upstream: FaultConfig::default(),
            to_client: FaultConfig::default(),
            seed: 0,
            drop_after: None,
            queue_units: DEFAULT_QUEUE_UNITS,
        }
    }

    /// Checks the parameters.
    pub fn validate(&self) -> Result<(), SyntheticError> {
        validate_faults(self.mode, &self.to_upstream, &self.to_client)?;
        if self.queue_units == 0 {
            return Err(SyntheticError::config(
                "proxy.queue_units",
                "must be at least one",
            ));
        }
        if self.drop_after.is_some_and(|d| d.is_zero()) {
            return Err(SyntheticError::config(
                "proxy.drop_after",
                "must be positive",
            ));
        }
        Ok(())
    }
}

fn validate_faults(
    mode: ProxyMode,
    to_upstream: &FaultConfig,
    to_client: &FaultConfig,
) -> Result<(), SyntheticError> {
    to_upstream.validate()?;
    to_client.validate()?;
    if mode == ProxyMode::Opaque && !(to_upstream.is_lossless() && to_client.is_lossless()) {
        return Err(SyntheticError::config(
            "proxy.mode",
            "an opaque stream cannot lose data: use loss 0 and held blackouts",
        ));
    }
    Ok(())
}

/// Errors of the proxy.
#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("invalid proxy configuration")]
    Config(#[from] SyntheticError),
    #[error("cannot listen on {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("cannot read the proxy's listening address")]
    LocalAddr(#[source] io::Error),
}

/// Proxy counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProxyStats {
    pub connections_accepted: u64,
    pub connections_refused: u64,
    /// Connections closed by the proxy (drop, restart, lifetime, framing error).
    pub connections_dropped: u64,
    pub upstream_connect_failures: u64,
    pub framing_errors: u64,
    /// Frames (frame-aware) or chunks (opaque) offered by the senders.
    pub units_offered: u64,
    pub units_forwarded: u64,
    pub units_dropped_loss: u64,
    pub units_dropped_blackout: u64,
    pub bytes_forwarded: u64,
}

#[derive(Debug, Default)]
struct Counters {
    connections_accepted: AtomicU64,
    connections_refused: AtomicU64,
    connections_dropped: AtomicU64,
    upstream_connect_failures: AtomicU64,
    framing_errors: AtomicU64,
    units_offered: AtomicU64,
    units_forwarded: AtomicU64,
    units_dropped_loss: AtomicU64,
    units_dropped_blackout: AtomicU64,
    bytes_forwarded: AtomicU64,
}

fn bump(counter: &AtomicU64, by: u64) {
    counter.fetch_add(by, Ordering::Relaxed);
}

impl Counters {
    fn snapshot(&self) -> ProxyStats {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        ProxyStats {
            connections_accepted: get(&self.connections_accepted),
            connections_refused: get(&self.connections_refused),
            connections_dropped: get(&self.connections_dropped),
            upstream_connect_failures: get(&self.upstream_connect_failures),
            framing_errors: get(&self.framing_errors),
            units_offered: get(&self.units_offered),
            units_forwarded: get(&self.units_forwarded),
            units_dropped_loss: get(&self.units_dropped_loss),
            units_dropped_blackout: get(&self.units_dropped_blackout),
            bytes_forwarded: get(&self.bytes_forwarded),
        }
    }
}

/// Live-adjustable state.
#[derive(Debug)]
struct Control {
    to_upstream: FaultConfig,
    to_client: FaultConfig,
    refuse_until: Option<Instant>,
}

#[derive(Debug)]
struct Shared {
    epoch: Instant,
    mode: ProxyMode,
    seed: u64,
    upstream: SocketAddr,
    drop_after: Option<Duration>,
    queue_units: usize,
    control: Mutex<Control>,
    /// Bumped on every fault change so directions refresh their model.
    version: AtomicU64,
    /// Bumped to drop every live connection.
    kill: watch::Sender<u64>,
    counters: Counters,
}

impl Shared {
    fn control(&self) -> MutexGuard<'_, Control> {
        // The state stays consistent even if a holder panicked (plain field assignments).
        self.control.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn refusing(&self, now: Instant) -> bool {
        self.control().refuse_until.is_some_and(|until| now < until)
    }

    fn faults(&self, direction: Direction) -> FaultConfig {
        let control = self.control();
        match direction {
            Direction::ToUpstream => control.to_upstream.clone(),
            Direction::ToClient => control.to_client.clone(),
        }
    }

    fn drop_all(&self) {
        self.kill
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    ToUpstream,
    ToClient,
}

impl Direction {
    const fn index(self) -> u64 {
        match self {
            Direction::ToUpstream => 1,
            Direction::ToClient => 2,
        }
    }
}

/// How one half of a connection ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HalfEnd {
    /// Clean end of stream (propagated as a half-close).
    Eof,
    /// I/O failure or the peer half vanished.
    Failed,
    /// The sender violated the frame format.
    FramingError,
}

/// A running fault proxy. Dropping it stops accepting; [`FaultProxy::shutdown`] also closes connections.
#[derive(Debug)]
pub struct FaultProxy {
    local_addr: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    accept_task: JoinHandle<()>,
}

impl FaultProxy {
    /// Binds the listener and starts proxying. Must be called from within a tokio runtime.
    pub async fn start(config: FaultProxyConfig) -> Result<Self, ProxyError> {
        config.validate()?;
        let listener =
            TcpListener::bind(config.listen)
                .await
                .map_err(|source| ProxyError::Bind {
                    addr: config.listen,
                    source,
                })?;
        let local_addr = listener.local_addr().map_err(ProxyError::LocalAddr)?;
        let (kill, _) = watch::channel(0_u64);
        let shared = Arc::new(Shared {
            epoch: Instant::now(),
            mode: config.mode,
            seed: config.seed,
            upstream: config.upstream,
            drop_after: config.drop_after,
            queue_units: config.queue_units,
            control: Mutex::new(Control {
                to_upstream: config.to_upstream,
                to_client: config.to_client,
                refuse_until: None,
            }),
            version: AtomicU64::new(0),
            kill,
            counters: Counters::default(),
        });
        let (stop, stop_rx) = watch::channel(false);
        let accept_task = tokio::spawn(accept_loop(listener, Arc::clone(&shared), stop_rx));
        tracing::info!(component = "synthetic.proxy", %local_addr, upstream = %config.upstream, mode = ?config.mode, "fault proxy listening");
        Ok(Self {
            local_addr,
            shared,
            stop,
            accept_task,
        })
    }

    /// Address clients connect to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Counters so far.
    pub fn stats(&self) -> ProxyStats {
        self.shared.counters.snapshot()
    }

    /// Time since the proxy started: the time base of [`FaultConfig::blackouts`].
    pub fn elapsed(&self) -> Duration {
        self.shared.epoch.elapsed()
    }

    /// Replaces the faults of both directions (applies to frames read from now on).
    pub fn set_faults(
        &self,
        to_upstream: FaultConfig,
        to_client: FaultConfig,
    ) -> Result<(), SyntheticError> {
        validate_faults(self.shared.mode, &to_upstream, &to_client)?;
        {
            let mut control = self.shared.control();
            control.to_upstream = to_upstream;
            control.to_client = to_client;
        }
        self.shared.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// A blackout of both directions starting now, with each direction's blackout policy. The duration is
    /// capped at [`MAX_BLACKOUT`].
    pub fn blackout(&self, duration: Duration) {
        if duration > MAX_BLACKOUT {
            tracing::warn!(component = "synthetic.proxy", ?duration, cap = ?MAX_BLACKOUT, "blackout capped");
        }
        let duration = duration.min(MAX_BLACKOUT);
        let start = self.elapsed();
        {
            let mut control = self.shared.control();
            control.to_upstream.push_blackout(start, duration);
            control.to_client.push_blackout(start, duration);
        }
        self.shared.version.fetch_add(1, Ordering::Release);
        tracing::info!(component = "synthetic.proxy", ?duration, "blackout");
    }

    /// Drops every live connection abruptly (frames in flight are lost).
    pub fn drop_connections(&self) {
        tracing::info!(component = "synthetic.proxy", "dropping all connections");
        self.shared.drop_all();
    }

    /// Simulates a server restart: drops every connection and refuses new ones for `downtime` (capped at
    /// [`MAX_DOWNTIME`]).
    pub fn restart(&self, downtime: Duration) {
        if downtime > MAX_DOWNTIME {
            tracing::warn!(component = "synthetic.proxy", ?downtime, cap = ?MAX_DOWNTIME, "downtime capped");
        }
        let downtime = downtime.min(MAX_DOWNTIME);
        let now = Instant::now();
        self.shared.control().refuse_until = Some(now.checked_add(downtime).unwrap_or(now));
        tracing::info!(component = "synthetic.proxy", ?downtime, "restart");
        self.shared.drop_all();
    }

    /// Stops accepting and closes every connection.
    pub async fn shutdown(self) {
        // Send errors only mean the accept loop already ended.
        let _ = self.stop.send(true);
        self.shared.drop_all();
        if let Err(error) = self.accept_task.await
            && error.is_panic()
        {
            tracing::error!(component = "synthetic.proxy", %error, "accept loop panicked");
        }
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, mut stop: watch::Receiver<bool>) {
    let mut connections = JoinSet::new();
    let mut next_id: u64 = 0;
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    break;
                }
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            accepted = listener.accept() => match accepted {
                Ok((client, peer)) => {
                    if shared.refusing(Instant::now()) || connections.len() >= MAX_CONNECTIONS {
                        bump(&shared.counters.connections_refused, 1);
                        tracing::debug!(component = "synthetic.proxy", %peer, "connection refused");
                        drop(client);
                        continue;
                    }
                    // Subscribe before counting and spawning: a drop/restart issued after this accept but
                    // before the task first runs must still close the connection.
                    let kill = shared.kill.subscribe();
                    bump(&shared.counters.connections_accepted, 1);
                    connections.spawn(run_connection(client, Arc::clone(&shared), next_id, kill));
                    next_id = next_id.wrapping_add(1);
                }
                Err(error) => {
                    tracing::warn!(component = "synthetic.proxy", %error, "accept failed");
                    tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                }
            }
        }
    }
    connections.abort_all();
}

/// Proxies one accepted client. `kill` was subscribed when the client was accepted, so every
/// [`Shared::drop_all`] issued after the accept closes this connection.
async fn run_connection(
    client: TcpStream,
    shared: Arc<Shared>,
    id: u64,
    mut kill: watch::Receiver<u64>,
) {
    let connect = tokio::time::timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        TcpStream::connect(shared.upstream),
    );
    let connected = tokio::select! {
        connected = connect => connected,
        _ = kill.changed() => {
            bump(&shared.counters.connections_dropped, 1);
            return;
        }
    };
    let upstream = match connected {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            bump(&shared.counters.upstream_connect_failures, 1);
            tracing::warn!(component = "synthetic.proxy", %error, "upstream connect failed");
            return;
        }
        Err(_) => {
            bump(&shared.counters.upstream_connect_failures, 1);
            tracing::warn!(component = "synthetic.proxy", "upstream connect timed out");
            return;
        }
    };
    for stream in [&client, &upstream] {
        if let Err(error) = stream.set_nodelay(true) {
            tracing::debug!(component = "synthetic.proxy", %error, "set_nodelay failed");
        }
    }
    let (client_read, client_write) = client.into_split();
    let (upstream_read, upstream_write) = upstream.into_split();
    let mut halves = JoinSet::new();
    spawn_direction(
        &mut halves,
        client_read,
        upstream_write,
        &shared,
        Direction::ToUpstream,
        id,
    );
    spawn_direction(
        &mut halves,
        upstream_read,
        client_write,
        &shared,
        Direction::ToClient,
        id,
    );

    let lifetime = async {
        match shared.drop_after {
            Some(d) => tokio::time::sleep(d).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(lifetime);
    loop {
        tokio::select! {
            joined = halves.join_next() => match joined {
                None => return,
                Some(Ok(HalfEnd::Eof)) => {}
                Some(Ok(HalfEnd::Failed | HalfEnd::FramingError)) | Some(Err(_)) => {
                    // A broken half takes the whole connection down: never leave a stream half-corrupted.
                    bump(&shared.counters.connections_dropped, 1);
                    halves.abort_all();
                    return;
                }
            },
            // An error means the proxy itself is gone: close as well.
            _ = kill.changed() => {
                bump(&shared.counters.connections_dropped, 1);
                halves.abort_all();
                return;
            }
            () = &mut lifetime => {
                bump(&shared.counters.connections_dropped, 1);
                halves.abort_all();
                return;
            }
        }
    }
}

fn spawn_direction(
    halves: &mut JoinSet<HalfEnd>,
    src: OwnedReadHalf,
    dst: OwnedWriteHalf,
    shared: &Arc<Shared>,
    direction: Direction,
    connection: u64,
) {
    let (tx, rx) = mpsc::channel(shared.queue_units);
    let seed = shared.seed
        ^ connection.wrapping_add(1).wrapping_mul(SEED_MIX)
        ^ direction.index().wrapping_mul(SEED_MIX.rotate_left(17));
    let sender = Sender {
        tx,
        shared: Arc::clone(shared),
        direction,
        model: None,
        seed,
        version: u64::MAX,
    };
    halves.spawn(run_direction(src, dst, sender, rx, Arc::clone(shared)));
}

/// One direction: the reader decides and queues, the writer delivers on time. When the reader stops
/// (end of stream or framing error) the writer still flushes the whole frames already accepted.
async fn run_direction(
    src: OwnedReadHalf,
    dst: OwnedWriteHalf,
    sender: Sender,
    rx: mpsc::Receiver<(Instant, Bytes)>,
    shared: Arc<Shared>,
) -> HalfEnd {
    // The reader owns the queue's sender, so the queue closes as soon as the reader returns.
    let reader = read_half(src, sender);
    let writer = write_half(dst, rx, shared);
    tokio::pin!(reader, writer);
    tokio::select! {
        read_end = &mut reader => {
            let write_end = writer.await;
            match read_end {
                HalfEnd::Eof => write_end,
                other => other,
            }
        }
        // The writer only finishes first when it failed.
        write_end = &mut writer => write_end,
    }
}

/// The deciding side of one direction.
struct Sender {
    tx: mpsc::Sender<(Instant, Bytes)>,
    shared: Arc<Shared>,
    direction: Direction,
    model: Option<FaultModel>,
    seed: u64,
    version: u64,
}

impl Sender {
    /// Decides and queues one unit; `false` once the writer is gone.
    async fn offer(&mut self, unit: Bytes, droppable: bool) -> bool {
        let counters = &self.shared.counters;
        bump(&counters.units_offered, 1);
        let version = self.shared.version.load(Ordering::Acquire);
        if version != self.version || self.model.is_none() {
            let config = self.shared.faults(self.direction);
            self.version = version;
            match self.model.as_mut() {
                Some(model) => {
                    if let Err(error) = model.set_config(config) {
                        // Configurations are validated before they are stored; keep the previous one.
                        tracing::error!(component = "synthetic.proxy", %error, "fault update rejected");
                    }
                }
                None => match FaultModel::new(config, self.seed) {
                    Ok(model) => self.model = Some(model),
                    Err(error) => {
                        tracing::error!(component = "synthetic.proxy", %error, "invalid fault configuration");
                        return false;
                    }
                },
            }
        }
        let Some(model) = self.model.as_mut() else {
            return false;
        };
        let now = Instant::now();
        match model.decide(now.saturating_duration_since(self.shared.epoch), droppable) {
            Verdict::Deliver { at } => {
                let Some(deliver_at) = self.shared.epoch.checked_add(at) else {
                    // Unreachable with bounded latency/blackouts; close rather than panic.
                    tracing::error!(
                        component = "synthetic.proxy",
                        ?at,
                        "delivery time overflows the clock"
                    );
                    return false;
                };
                self.tx.send((deliver_at, unit)).await.is_ok()
            }
            Verdict::Drop(DropReason::Loss) => {
                bump(&counters.units_dropped_loss, 1);
                true
            }
            Verdict::Drop(DropReason::Blackout) => {
                bump(&counters.units_dropped_blackout, 1);
                true
            }
        }
    }
}

/// The bytes of a decoded frame as queued for delivery. A decoded frame shares the read buffer's
/// allocation; a queued unit outlives the dispatch, so it is detached (PROTOCOL §1) and never pins the
/// whole read buffer while it waits for its delivery time.
fn queued_unit(frame: &Frame) -> Bytes {
    frame.detach().into_bytes()
}

async fn read_half(mut src: OwnedReadHalf, mut sender: Sender) -> HalfEnd {
    let mode = sender.shared.mode;
    let mut decoder = FrameDecoder::new();
    let mut buf = BytesMut::with_capacity(READ_CHUNK_BYTES);
    loop {
        if mode == ProxyMode::FrameAware {
            loop {
                match decoder.decode_frame(&mut buf) {
                    Ok(Some(frame)) => {
                        let droppable = frame
                            .msg_type()
                            .is_some_and(|t| t.delivery() == Delivery::LatestWins);
                        if !sender.offer(queued_unit(&frame), droppable).await {
                            return HalfEnd::Failed;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        bump(&sender.shared.counters.framing_errors, 1);
                        tracing::warn!(component = "synthetic.proxy", %error, "framing error: closing the connection");
                        return HalfEnd::FramingError;
                    }
                }
            }
        }
        buf.reserve(READ_CHUNK_BYTES);
        match src.read_buf(&mut buf).await {
            Ok(0) => {
                if mode == ProxyMode::FrameAware && !buf.is_empty() {
                    // A partial frame at end of stream is never forwarded.
                    bump(&sender.shared.counters.framing_errors, 1);
                    tracing::warn!(
                        component = "synthetic.proxy",
                        pending = buf.len(),
                        "stream ended inside a frame"
                    );
                    return HalfEnd::FramingError;
                }
                return HalfEnd::Eof;
            }
            Ok(_) => {
                if mode == ProxyMode::Opaque {
                    let chunk = buf.split().freeze();
                    if !sender.offer(chunk, false).await {
                        return HalfEnd::Failed;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(component = "synthetic.proxy", %error, "read failed");
                return HalfEnd::Failed;
            }
        }
    }
}

async fn write_half(
    mut dst: OwnedWriteHalf,
    mut rx: mpsc::Receiver<(Instant, Bytes)>,
    shared: Arc<Shared>,
) -> HalfEnd {
    while let Some((at, unit)) = rx.recv().await {
        tokio::time::sleep_until(at).await;
        if let Err(error) = dst.write_all(&unit).await {
            tracing::debug!(component = "synthetic.proxy", %error, "write failed");
            return HalfEnd::Failed;
        }
        bump(&shared.counters.units_forwarded, 1);
        bump(&shared.counters.bytes_forwarded, unit.len() as u64);
    }
    // The sender finished cleanly: propagate the half-close.
    if let Err(error) = dst.shutdown().await {
        tracing::debug!(component = "synthetic.proxy", %error, "shutdown failed");
    }
    HalfEnd::Eof
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::frame::{FrameMeta, FramePriority, MsgType};

    /// Regression (review pass 2, F9): frames were queued with `into_bytes()`, pinning the read buffer.
    #[test]
    fn queued_frames_do_not_share_the_read_buffer() {
        const READ_CAPACITY: usize = 64 * 1_024;
        let meta = FrameMeta::stream(FramePriority::P0, 1, 1, 1);
        let frame = Frame::build(MsgType::TelemetryDelta, &meta, &[7; 100]).expect("frame");
        let mut buf = BytesMut::with_capacity(READ_CAPACITY);
        buf.extend_from_slice(frame.as_bytes());
        buf.extend_from_slice(frame.as_bytes());
        let base = buf.as_ptr() as usize;
        let read_range = base..base + READ_CAPACITY;
        let decoded = FrameDecoder::new()
            .decode_frame(&mut buf)
            .expect("valid")
            .expect("complete");
        assert!(read_range.contains(&(decoded.as_bytes().as_ptr() as usize)));
        let unit = queued_unit(&decoded);
        assert_eq!(&unit[..], frame.as_bytes());
        assert!(!read_range.contains(&(unit.as_ptr() as usize)));
    }
}

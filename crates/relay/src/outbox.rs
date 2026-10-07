//! Per-receiver outbound queues (PROTOCOL.md §2, D-009) and the connection handle shared by a
//! connection's reader task, its writer task and the session registry.
//!
//! An [`Outbox`] has two parts:
//! * a small **reliable** FIFO for control frames, relay replies, replayed join frames and the
//!   reliable stream types (SCHEMA_NEGOTIATE, SESSION_EVENT, SNAPSHOT_REQUEST). It is never
//!   dropped from; it is bounded by frame count and bytes, and a receiver that overflows it is
//!   disconnected as too slow;
//! * one **latest-wins** slot per `(msg_type, priority)` of the six live types × P0–P2 (18 slots).
//!   A newer frame of the same key replaces an unwritten older one, which is counted and reported
//!   to that receiver in the next forwarded frame's `relay_dropped`. A slow receiver therefore never
//!   grows memory: at most 18 live frames are pending, whatever the input rate.
//!
//! The writer serves the reliable queue first (§2), then the pending live frames in sequence order,
//! so a pending TELEMETRY_SNAPSHOT is always written before any delta that came after it. When a
//! snapshot is queued, pending deltas and weather updates older than it are dropped: the snapshot
//! carries every channel and supersedes them at the receiver (§5).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use protocol::frame::{CRC_BYTES, Frame, FrameMeta, FramePriority, MsgType, RELAY_TRAILER_BYTES};
use protocol::messages::Message;
use protocol::{RelayTrailer, TrailerStamper};
use tokio::sync::Notify;
use tokio_util::sync::{CancellationToken, WaitForCancellationFuture};

use crate::time::{duration_ms, mono_ns_since};

/// Latest-wins message types (§2), in slot order.
pub const LIVE_TYPES: [MsgType; 6] = [
    MsgType::TelemetryDelta,
    MsgType::TelemetrySnapshot,
    MsgType::ScoringUpdate,
    MsgType::WeatherUpdate,
    MsgType::DriverStatus,
    MsgType::RecordingStatus,
];
/// Priorities of live frames (P0–P2).
const LIVE_PRIORITIES: usize = 3;
/// Number of latest-wins slots per receiver.
pub const LIVE_SLOTS: usize = LIVE_TYPES.len() * LIVE_PRIORITIES;
/// Approximate encoded size of a relay-built PONG (header + small postcard payload + CRC).
const PONG_QUEUED_BYTES: usize = 64;

/// Slot index of a live frame, `None` for reliable types or the control priority.
pub fn live_slot(msg_type: MsgType, priority: FramePriority) -> Option<usize> {
    let t = LIVE_TYPES.iter().position(|&x| x == msg_type)?;
    let p = match priority {
        FramePriority::P0 => 0,
        FramePriority::P1 => 1,
        FramePriority::P2 => 2,
        FramePriority::Control => return None,
    };
    Some(t * LIVE_PRIORITIES + p)
}

/// A driver stream frame prepared once for every receiver: the payload is copied once into the
/// stamper (detached from the read buffer) and never decoded; each receiver gets its own trailer.
#[derive(Debug)]
pub struct Forwarded {
    stamper: TrailerStamper,
    pub msg_type: MsgType,
    pub priority: FramePriority,
    pub sequence: u64,
    /// Relay wall clock when the frame was received (trailer `relay_rx_ns`).
    pub rx_ns: u64,
    /// Monotonic receive time (forward-latency metric).
    pub rx_at: Instant,
}

impl Forwarded {
    /// Prepares `frame` (already validated) for forwarding.
    pub fn new(frame: &Frame, msg_type: MsgType, rx_ns: u64, rx_at: Instant) -> Self {
        Self {
            stamper: TrailerStamper::new(frame),
            msg_type,
            priority: frame.header().priority,
            sequence: frame.header().sequence,
            rx_ns,
            rx_at,
        }
    }

    /// Bytes on the wire with the relay trailer.
    pub fn wire_len(&self) -> usize {
        self.stamper.body().len() + RELAY_TRAILER_BYTES + CRC_BYTES
    }

    /// Appends the frame with `trailer` (header, payload, trailer, CRC) to `buf`.
    pub fn encode_into(&self, buf: &mut Vec<u8>, trailer: &RelayTrailer) {
        buf.extend_from_slice(self.stamper.body());
        buf.extend_from_slice(&self.stamper.tail(trailer));
    }
}

/// One queued outbound item.
#[derive(Debug, Clone)]
pub enum Outgoing {
    /// A complete frame written as is (relay-originated control frames, SNAPSHOT_REQUEST to the
    /// driver).
    Frame(Frame),
    /// A driver stream frame, written with a relay trailer stamped at write time.
    Forward {
        frame: Arc<Forwarded>,
        /// Replayed from the join cache (trailer flag REPLAYED, §6).
        replayed: bool,
    },
    /// PONG to a PING; `t2` is stamped when the frame is handed to the socket (§8).
    Pong { id: u64, t0: u64, t1: u64 },
}

impl Outgoing {
    fn queued_len(&self) -> usize {
        match self {
            Outgoing::Frame(f) => f.wire_len(),
            Outgoing::Forward { frame, .. } => frame.wire_len(),
            Outgoing::Pong { .. } => PONG_QUEUED_BYTES,
        }
    }
}

/// Why a push was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// The receiver is closing; nothing more is queued.
    Closed,
    /// The reliable queue is full: the receiver is too slow and is being disconnected.
    Overflow,
}

/// Result of a latest-wins push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePush {
    /// Older unwritten live frames dropped by this push (replaced or superseded).
    pub dropped: u32,
}

/// One writer batch.
#[derive(Debug, Default)]
pub struct Batch {
    pub items: Vec<Outgoing>,
    /// Live frames dropped since the last forwarded frame; belongs in the trailer of the first
    /// live (not replayed) forwarded frame of this batch (0 if the batch forwards none).
    pub dropped: u32,
    /// The outbox is closing: once this batch is written the writer ends.
    pub closing: bool,
}

/// Point-in-time view of an outbox (diagnostics, tests, link health).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboxStats {
    pub reliable_frames: usize,
    pub reliable_bytes: usize,
    pub live_pending: usize,
    pub dropped_total: u64,
    pub closing: bool,
    pub overflowed: bool,
}

#[derive(Debug)]
struct OutboxInner {
    reliable: VecDeque<Outgoing>,
    reliable_bytes: usize,
    live: [Option<Arc<Forwarded>>; LIVE_SLOTS],
    dropped_pending: u32,
    dropped_total: u64,
    last_drop: Option<Instant>,
    closing: bool,
    overflowed: bool,
}

/// Bounded outbound queue of one receiver.
#[derive(Debug)]
pub struct Outbox {
    inner: Mutex<OutboxInner>,
    notify: Notify,
    max_frames: usize,
    max_bytes: usize,
}

impl Outbox {
    /// An empty outbox whose reliable queue holds at most `max_frames` frames / `max_bytes` bytes.
    pub fn new(max_frames: usize, max_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(OutboxInner {
                reliable: VecDeque::new(),
                reliable_bytes: 0,
                live: Default::default(),
                dropped_pending: 0,
                dropped_total: 0,
                last_drop: None,
                closing: false,
                overflowed: false,
            }),
            notify: Notify::new(),
            max_frames: max_frames.max(1),
            max_bytes: max_bytes.max(1),
        }
    }

    /// Queues a reliable item. On overflow the outbox switches to closing (the writer flushes
    /// what is queued and disconnects the receiver).
    pub fn push_reliable(&self, item: Outgoing) -> Result<(), PushError> {
        let len = item.queued_len();
        let mut inner = self.inner.lock();
        if inner.closing {
            return Err(PushError::Closed);
        }
        if inner.reliable.len() >= self.max_frames
            || inner.reliable_bytes.saturating_add(len) > self.max_bytes
        {
            inner.overflowed = true;
            inner.closing = true;
            drop(inner);
            self.notify.notify_one();
            return Err(PushError::Overflow);
        }
        inner.reliable_bytes += len;
        inner.reliable.push_back(item);
        drop(inner);
        self.notify.notify_one();
        Ok(())
    }

    /// Puts a live frame into its latest-wins slot. Frames of a reliable type or the control
    /// priority belong in [`Outbox::push_reliable`] and are routed there.
    pub fn push_live(&self, frame: Arc<Forwarded>, now: Instant) -> Result<LivePush, PushError> {
        let Some(slot) = live_slot(frame.msg_type, frame.priority) else {
            return self
                .push_reliable(Outgoing::Forward {
                    frame,
                    replayed: false,
                })
                .map(|()| LivePush { dropped: 0 });
        };
        let mut inner = self.inner.lock();
        if inner.closing {
            return Err(PushError::Closed);
        }
        let mut dropped = 0u32;
        if frame.msg_type == MsgType::TelemetrySnapshot {
            // The snapshot supersedes every older delta and weather update (§5).
            for (i, pending) in inner.live.iter_mut().enumerate() {
                let superseded_type = matches!(
                    LIVE_TYPES[i / LIVE_PRIORITIES],
                    MsgType::TelemetryDelta | MsgType::WeatherUpdate
                );
                if superseded_type
                    && pending
                        .as_ref()
                        .is_some_and(|p| p.sequence < frame.sequence)
                {
                    *pending = None;
                    dropped += 1;
                }
            }
        }
        if inner.live[slot].replace(frame).is_some() {
            dropped += 1;
        }
        if dropped > 0 {
            inner.dropped_pending = inner.dropped_pending.saturating_add(dropped);
            inner.dropped_total = inner.dropped_total.saturating_add(u64::from(dropped));
            inner.last_drop = Some(now);
        }
        drop(inner);
        self.notify.notify_one();
        Ok(LivePush { dropped })
    }

    /// Drops every pending live frame (the driver started a new sequence space, §6). Returns how
    /// many were dropped; they are counted like latest-wins drops.
    pub fn clear_live(&self, now: Instant) -> u32 {
        let mut inner = self.inner.lock();
        let mut dropped = 0u32;
        for slot in &mut inner.live {
            if slot.take().is_some() {
                dropped += 1;
            }
        }
        if dropped > 0 {
            inner.dropped_pending = inner.dropped_pending.saturating_add(dropped);
            inner.dropped_total = inner.dropped_total.saturating_add(u64::from(dropped));
            inner.last_drop = Some(now);
        }
        dropped
    }

    /// Stops accepting items; the writer flushes what is queued and then closes the connection.
    pub fn close(&self) {
        self.inner.lock().closing = true;
        self.notify.notify_one();
    }

    /// Whether the outbox is closing.
    pub fn is_closing(&self) -> bool {
        self.inner.lock().closing
    }

    /// Takes the next batch: reliable items first (up to `max_bytes`, at least one), then, once
    /// the reliable queue is empty, every pending live frame in sequence order.
    pub fn take_batch(&self, max_bytes: usize) -> Batch {
        let mut inner = self.inner.lock();
        let mut items = Vec::new();
        let mut bytes = 0usize;
        while let Some(front) = inner.reliable.front() {
            let len = front.queued_len();
            if !items.is_empty() && bytes.saturating_add(len) > max_bytes {
                break;
            }
            if let Some(item) = inner.reliable.pop_front() {
                inner.reliable_bytes = inner.reliable_bytes.saturating_sub(len);
                bytes = bytes.saturating_add(len);
                items.push(item);
            }
        }
        if inner.reliable.is_empty() {
            let mut live: Vec<Arc<Forwarded>> =
                inner.live.iter_mut().filter_map(Option::take).collect();
            live.sort_by_key(|f| f.sequence);
            items.extend(live.into_iter().map(|frame| Outgoing::Forward {
                frame,
                replayed: false,
            }));
        }
        // Replayed join frames are outside the live sequence flow (§5), so the drop count waits
        // for the first live forwarded frame.
        let forwards = items.iter().any(|i| {
            matches!(
                i,
                Outgoing::Forward {
                    replayed: false,
                    ..
                }
            )
        });
        let dropped = if forwards {
            std::mem::take(&mut inner.dropped_pending)
        } else {
            0
        };
        Batch {
            items,
            dropped,
            closing: inner.closing,
        }
    }

    /// Waits until something was pushed or the outbox was closed (a notification sent while no
    /// one waited is kept, so no wake-up is lost).
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    /// Whether live frames were dropped for this receiver within `window` before `now`.
    pub fn dropped_recently(&self, now: Instant, window: Duration) -> bool {
        self.inner
            .lock()
            .last_drop
            .is_some_and(|t| now.saturating_duration_since(t) <= window)
    }

    /// Current queue state.
    pub fn stats(&self) -> OutboxStats {
        let inner = self.inner.lock();
        OutboxStats {
            reliable_frames: inner.reliable.len(),
            reliable_bytes: inner.reliable_bytes,
            live_pending: inner.live.iter().filter(|s| s.is_some()).count(),
            dropped_total: inner.dropped_total,
            closing: inner.closing,
            overflowed: inner.overflowed,
        }
    }
}

/// Handle of one client connection, shared by its reader and writer tasks and the registry.
#[derive(Debug)]
pub struct ConnHandle {
    /// Relay-wide unique connection id (logs; registry ownership checks).
    pub id: u64,
    pub peer: SocketAddr,
    pub outbox: Outbox,
    cancel: CancellationToken,
    epoch: Instant,
    last_inbound_ms: AtomicU64,
}

impl ConnHandle {
    /// A handle for connection `id`. `epoch` is the relay's monotonic epoch.
    pub fn new(
        id: u64,
        peer: SocketAddr,
        epoch: Instant,
        reliable_frames: usize,
        reliable_bytes: usize,
    ) -> Self {
        Self {
            id,
            peer,
            outbox: Outbox::new(reliable_frames, reliable_bytes),
            cancel: CancellationToken::new(),
            epoch,
            last_inbound_ms: AtomicU64::new(duration_ms(epoch.elapsed())),
        }
    }

    /// Records inbound bytes at `now`.
    pub fn touch(&self, now: Instant) {
        self.last_inbound_ms.store(
            duration_ms(now.saturating_duration_since(self.epoch)),
            Ordering::Relaxed,
        );
    }

    /// When the connection last received bytes.
    pub fn last_inbound(&self) -> Instant {
        self.epoch + Duration::from_millis(self.last_inbound_ms.load(Ordering::Relaxed))
    }

    /// Queues a relay-originated control message carrying `session_id` (0 = none) in its
    /// header. Returns `false` if it could not be queued (closing or overflow).
    pub fn send(&self, message: &Message, session_id: u64) -> bool {
        let meta = FrameMeta::control(session_id, mono_ns_since(self.epoch));
        match message.to_frame(&meta) {
            Ok(frame) => self.outbox.push_reliable(Outgoing::Frame(frame)).is_ok(),
            Err(e) => {
                // Only a relay bug (an oversized or rule-violating relay message) gets here.
                tracing::error!(
                    component = "relay.conn",
                    conn = self.id,
                    msg_type = %message.msg_type(),
                    error = %e,
                    "relay message could not be encoded"
                );
                false
            }
        }
    }

    /// Flushes what is queued, then closes the connection.
    pub fn close(&self) {
        self.outbox.close();
    }

    /// Ends the connection immediately (reader and writer stop; nothing more is written).
    pub fn abort(&self) {
        self.outbox.close();
        self.cancel.cancel();
    }

    /// Completes once the connection was aborted or its writer ended.
    pub fn cancelled(&self) -> WaitForCancellationFuture<'_> {
        self.cancel.cancelled()
    }

    /// Whether the connection was aborted or its writer ended.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::frame::FrameMeta;

    fn fwd(msg_type: MsgType, priority: FramePriority, seq: u64) -> Arc<Forwarded> {
        let meta = if msg_type.delivery() == protocol::frame::Delivery::Reliable {
            FrameMeta::stream(FramePriority::Control, 7, seq, seq)
        } else {
            FrameMeta::stream(priority, 7, seq, seq)
        };
        let frame = Frame::build(msg_type, &meta, &[0u8; 32]).expect("frame");
        Arc::new(Forwarded::new(&frame, msg_type, 1, Instant::now()))
    }

    fn seqs(batch: &Batch) -> Vec<u64> {
        batch
            .items
            .iter()
            .filter_map(|i| match i {
                Outgoing::Forward { frame, .. } => Some(frame.sequence),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn latest_wins_per_type_and_priority() {
        let now = Instant::now();
        let o = Outbox::new(16, 1 << 20);
        for seq in 1..=100 {
            o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P0, seq), now)
                .expect("push");
        }
        o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P1, 101), now)
            .expect("push");
        let stats = o.stats();
        assert_eq!(stats.live_pending, 2);
        assert_eq!(stats.dropped_total, 99);
        let batch = o.take_batch(usize::MAX);
        assert_eq!(seqs(&batch), vec![100, 101]);
        assert_eq!(
            batch.dropped, 99,
            "reported once, with the first forwarded frame"
        );
        assert_eq!(o.take_batch(usize::MAX).items.len(), 0);
    }

    #[test]
    fn memory_is_bounded_for_a_slow_receiver() {
        let now = Instant::now();
        let o = Outbox::new(16, 1 << 20);
        for seq in 1..=10_000u64 {
            let t = LIVE_TYPES[(seq % 6) as usize];
            let p = [FramePriority::P0, FramePriority::P1, FramePriority::P2][(seq % 3) as usize];
            o.push_live(fwd(t, p, seq), now).expect("push");
        }
        assert!(o.stats().live_pending <= LIVE_SLOTS);
    }

    #[test]
    fn snapshot_is_written_before_newer_deltas_and_supersedes_older_ones() {
        let now = Instant::now();
        let o = Outbox::new(16, 1 << 20);
        o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P1, 10), now)
            .expect("push");
        o.push_live(fwd(MsgType::WeatherUpdate, FramePriority::P2, 11), now)
            .expect("push");
        o.push_live(fwd(MsgType::TelemetrySnapshot, FramePriority::P1, 12), now)
            .expect("push");
        o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P0, 13), now)
            .expect("push");
        let batch = o.take_batch(usize::MAX);
        assert_eq!(seqs(&batch), vec![12, 13]);
        assert_eq!(batch.dropped, 2);
    }

    #[test]
    fn reliable_first_and_bounded() {
        let now = Instant::now();
        let o = Outbox::new(2, 1 << 20);
        o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P0, 5), now)
            .expect("push");
        o.push_reliable(Outgoing::Forward {
            frame: fwd(MsgType::SessionEvent, FramePriority::Control, 6),
            replayed: false,
        })
        .expect("push");
        o.push_reliable(Outgoing::Pong {
            id: 1,
            t0: 2,
            t1: 3,
        })
        .expect("push");
        // Third reliable item overflows: the receiver is too slow and gets closed.
        assert_eq!(
            o.push_reliable(Outgoing::Pong {
                id: 2,
                t0: 2,
                t1: 3
            }),
            Err(PushError::Overflow)
        );
        assert!(o.stats().overflowed);
        let batch = o.take_batch(usize::MAX);
        assert!(batch.closing);
        assert!(matches!(batch.items[0], Outgoing::Forward { .. }));
        assert!(matches!(batch.items[1], Outgoing::Pong { .. }));
        assert_eq!(seqs(&batch), vec![6, 5], "reliable first, then live");
        // Closed: nothing more is accepted.
        assert_eq!(
            o.push_live(fwd(MsgType::TelemetryDelta, FramePriority::P0, 7), now),
            Err(PushError::Closed)
        );
    }

    #[test]
    fn reliable_batches_respect_the_byte_budget() {
        let o = Outbox::new(100, 1 << 20);
        for i in 0..10 {
            o.push_reliable(Outgoing::Pong {
                id: i,
                t0: 0,
                t1: 0,
            })
            .expect("push");
        }
        let batch = o.take_batch(PONG_QUEUED_BYTES * 3);
        assert_eq!(batch.items.len(), 3);
        assert_eq!(o.stats().reliable_frames, 7);
    }

    #[test]
    fn clear_live_counts_drops() {
        let now = Instant::now();
        let o = Outbox::new(16, 1 << 20);
        o.push_live(fwd(MsgType::ScoringUpdate, FramePriority::P0, 1), now)
            .expect("push");
        o.push_live(fwd(MsgType::DriverStatus, FramePriority::P0, 2), now)
            .expect("push");
        assert_eq!(o.clear_live(now), 2);
        assert!(o.dropped_recently(now, Duration::from_secs(1)));
        assert_eq!(o.stats().live_pending, 0);
    }

    #[test]
    fn live_slots_cover_every_live_key() {
        let mut seen = std::collections::HashSet::new();
        for t in LIVE_TYPES {
            for p in [FramePriority::P0, FramePriority::P1, FramePriority::P2] {
                let slot = live_slot(t, p).expect("live key");
                assert!(slot < LIVE_SLOTS);
                assert!(seen.insert(slot));
            }
            assert_eq!(live_slot(t, FramePriority::Control), None);
        }
        assert_eq!(live_slot(MsgType::SessionEvent, FramePriority::P0), None);
    }
}

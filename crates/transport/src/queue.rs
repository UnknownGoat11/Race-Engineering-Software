//! Outbound queues of one client (PROTOCOL.md §2, D-009). Everything is bounded.
//!
//! * **Reliable queue**: a small FIFO for session-level messages that are never dropped *by the
//!   queue* (SCHEMA_NEGOTIATE, SESSION_EVENT, SNAPSHOT_REQUEST, CONNECT_CODE_REQUEST,
//!   ENGINEER_KICK). Bounded: a full queue makes [`OutboundQueue::push_reliable`] wait and
//!   [`OutboundQueue::try_push_reliable`] fail, never silently drop. It accepts messages only while
//!   the client streams ([`OutboundQueue::set_streaming`]; otherwise [`QueueError::NotStreaming`]),
//!   so nothing piles up while stopped and no waiting producer hangs. Messages queued while the
//!   link is down stay queued and are written after the reconnect (same session); when a new
//!   session starts or streaming stops, the pending ones are discarded and counted
//!   ([`OutboundQueue::reliable_discarded`]).
//!   **Delivery is at most once across a connection loss**: a message counts as written once its
//!   bytes were handed to the TLS stream. If the link then dies silently (detected only by the
//!   heartbeat or write timeout), the messages written meanwhile are lost with their sequence
//!   numbers (the engineer sees a gap) and are not re-sent: the protocol has no acknowledgement of
//!   stream frames. Only the message whose write had not completed when the connection died is
//!   "in flight": [`OutboundQueue::requeue_in_flight`] puts it back at the front (the relay never
//!   accepted the partial frame, and its sequence number was not consumed).
//! * **Latest-wins slots**: one slot per `(msg_type, priority)` for TELEMETRY_DELTA,
//!   TELEMETRY_SNAPSHOT, SCORING_UPDATE, WEATHER_UPDATE, DRIVER_STATUS and RECORDING_STATUS. A newer
//!   frame replaces an unsent older one of the same key (counted as `dropped_obsolete`).
//! * **Snapshots are never superseded by deltas**: a pending snapshot is written before every other
//!   live frame, and enqueueing a snapshot discards the pending TELEMETRY_DELTA / WEATHER_UPDATE
//!   frames, which it contains in newer form (counted as `dropped_obsolete`). Otherwise a delta
//!   captured *before* the snapshot would be written *after* it and roll channels back.
//!
//! The writer serves, in order: the connection's own control lane (PING, DISCONNECT), the reliable
//! queue, then latest-wins frames in **rounds**: a round takes every occupied slot at once —
//! snapshots first, then P0, P1, P2 — so a saturated link still sends every key at least once per
//! round instead of starving P2. Stream frames get their sequence number when they are written
//! (`crate::session`), so frames dropped here never show up as sequence gaps at the engineer.
//!
//! Producers must enqueue live frames in capture order (one producer task does).

use std::collections::VecDeque;

use parking_lot::Mutex;
use protocol::frame::{Delivery, FramePriority, MsgType};
use protocol::messages::{Message, SnapshotReason};
use protocol::sequence::SnapshotRequestLimiter;
use thiserror::Error;
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

/// Default capacity of the reliable queue.
pub const DEFAULT_RELIABLE_CAPACITY: usize = 256;

/// Latest-wins message types, in round order (snapshot first, §2).
const LIVE_TYPES: [MsgType; 6] = [
    MsgType::TelemetrySnapshot,
    MsgType::TelemetryDelta,
    MsgType::ScoringUpdate,
    MsgType::WeatherUpdate,
    MsgType::DriverStatus,
    MsgType::RecordingStatus,
];
/// Index of TELEMETRY_SNAPSHOT in [`LIVE_TYPES`].
const SNAPSHOT_INDEX: usize = 0;
/// Live priorities, in round order.
const LIVE_PRIORITIES: [FramePriority; 3] =
    [FramePriority::P0, FramePriority::P1, FramePriority::P2];

/// One queued outbound message.
#[derive(Debug, Clone, PartialEq)]
pub enum Outbound {
    /// Control-class message (sequence 0).
    Control(Message),
    /// Stream-class message; its sequence number is assigned when it is written.
    Stream {
        message: Message,
        priority: FramePriority,
    },
}

impl Outbound {
    /// Message type of the item.
    pub fn msg_type(&self) -> MsgType {
        match self {
            Outbound::Control(m) | Outbound::Stream { message: m, .. } => m.msg_type(),
        }
    }
}

/// Why a message was not queued.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum QueueError {
    #[error("{msg_type} is not a latest-wins message type")]
    NotLatestWins { msg_type: MsgType },
    #[error("{msg_type} cannot be queued as a {requested} message")]
    WrongClass {
        msg_type: MsgType,
        requested: &'static str,
    },
    #[error("priority {priority:?} is not allowed for {msg_type}")]
    PriorityNotAllowed {
        msg_type: MsgType,
        priority: FramePriority,
    },
    #[error("{msg_type} may not be sent by this client")]
    NotAllowed { msg_type: MsgType },
    #[error("the reliable queue is full ({capacity} messages)")]
    Full { capacity: usize },
    /// The client is not streaming (not started, stopped, ended or FAILED): nothing would carry
    /// the message, so it is refused instead of being queued for a later session.
    #[error("not streaming: the message was not queued")]
    NotStreaming,
    #[error("the client has shut down")]
    Closed,
}

/// Result of enqueueing a live frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveOutcome {
    /// An unsent frame of the same key was replaced.
    pub replaced: bool,
    /// Pending deltas / weather frames discarded because this is a newer snapshot.
    pub superseded: u64,
}

#[derive(Debug)]
struct Inner {
    reliable: VecDeque<Outbound>,
    /// Popped by the writer, write not completed yet.
    in_flight: Option<Outbound>,
    capacity: usize,
    slots: [[Option<Message>; LIVE_PRIORITIES.len()]; LIVE_TYPES.len()],
    occupied: usize,
    dropped_obsolete: u64,
    /// Reliable messages discarded unsent (new session, streaming stopped).
    reliable_discarded: u64,
    snapshot_limiter: SnapshotRequestLimiter,
    pending_snapshot_request: Option<SnapshotReason>,
    /// Reliable messages are accepted (see [`OutboundQueue::set_streaming`]).
    streaming: bool,
    closed: bool,
}

impl Inner {
    /// Why a reliable message cannot be queued now, if it cannot.
    fn refuse_reliable(&self) -> Option<QueueError> {
        if self.closed {
            Some(QueueError::Closed)
        } else if !self.streaming {
            Some(QueueError::NotStreaming)
        } else {
            None
        }
    }
}

/// The outbound queues of one client. Shared between the application (producer) and the
/// connection's writer task (consumer).
#[derive(Debug)]
pub struct OutboundQueue {
    inner: Mutex<Inner>,
    /// Wakes the writer.
    data: Notify,
    /// Wakes producers waiting for reliable-queue space.
    space: Notify,
}

fn live_index(msg_type: MsgType) -> Option<usize> {
    LIVE_TYPES.iter().position(|t| *t == msg_type)
}

fn priority_index(priority: FramePriority) -> Option<usize> {
    LIVE_PRIORITIES.iter().position(|p| *p == priority)
}

impl OutboundQueue {
    /// Queue with a reliable capacity of `capacity` messages (at least 1), streaming.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                reliable: VecDeque::new(),
                in_flight: None,
                capacity: capacity.max(1),
                slots: Default::default(),
                occupied: 0,
                dropped_obsolete: 0,
                reliable_discarded: 0,
                snapshot_limiter: SnapshotRequestLimiter::default(),
                pending_snapshot_request: None,
                streaming: true,
                closed: false,
            }),
            data: Notify::new(),
            space: Notify::new(),
        }
    }

    /// Whether reliable messages are accepted. While not streaming, [`Self::push_reliable`] and
    /// [`Self::try_push_reliable`] fail with [`QueueError::NotStreaming`] and producers waiting for
    /// space are released with that error. Messages already queued are kept; the owner discards
    /// them with [`Self::clear_session`].
    pub fn set_streaming(&self, streaming: bool) {
        self.inner.lock().streaming = streaming;
        if !streaming {
            self.space.notify_waiters();
        }
    }

    /// Puts a latest-wins frame into its `(msg_type, priority)` slot. `now_ms` is the client's
    /// monotonic clock (a snapshot counts for SNAPSHOT_REQUEST coalescing, §5).
    pub fn push_live(
        &self,
        message: Message,
        priority: FramePriority,
        now_ms: u64,
    ) -> Result<LiveOutcome, QueueError> {
        let msg_type = message.msg_type();
        if msg_type.delivery() != Delivery::LatestWins {
            return Err(QueueError::NotLatestWins { msg_type });
        }
        let (Some(t), Some(p)) = (live_index(msg_type), priority_index(priority)) else {
            return Err(QueueError::PriorityNotAllowed { msg_type, priority });
        };
        let mut inner = self.inner.lock();
        if inner.closed {
            return Err(QueueError::Closed);
        }
        let mut outcome = LiveOutcome::default();
        if t == SNAPSHOT_INDEX {
            // The snapshot carries every channel in newer form than any pending delta / weather
            // frame: those would roll channels back if written after it (§5).
            for partial in [MsgType::TelemetryDelta, MsgType::WeatherUpdate] {
                let Some(pt) = live_index(partial) else {
                    continue;
                };
                for slot in &mut inner.slots[pt] {
                    if slot.take().is_some() {
                        outcome.superseded += 1;
                    }
                }
            }
            inner.occupied -= outcome.superseded as usize;
            inner.snapshot_limiter.note_sent(now_ms);
            inner.pending_snapshot_request = None;
        }
        outcome.replaced = inner.slots[t][p].replace(message).is_some();
        if !outcome.replaced {
            inner.occupied += 1;
        }
        let dropped = outcome.superseded + u64::from(outcome.replaced);
        inner.dropped_obsolete = inner.dropped_obsolete.saturating_add(dropped);
        drop(inner);
        self.data.notify_one();
        Ok(outcome)
    }

    /// Appends to the reliable queue without waiting.
    pub fn try_push_reliable(&self, item: Outbound) -> Result<(), QueueError> {
        check_reliable(&item)?;
        let mut inner = self.inner.lock();
        if let Some(refused) = inner.refuse_reliable() {
            return Err(refused);
        }
        if inner.reliable.len() >= inner.capacity {
            return Err(QueueError::Full {
                capacity: inner.capacity,
            });
        }
        inner.reliable.push_back(item);
        drop(inner);
        self.data.notify_one();
        Ok(())
    }

    /// Appends to the reliable queue, waiting for space while it is full. The queue drains only
    /// while connected, so during an outage this waits until the link is back; it returns
    /// [`QueueError::NotStreaming`] / [`QueueError::Closed`] as soon as streaming stops or the
    /// client shuts down.
    pub async fn push_reliable(&self, item: Outbound) -> Result<(), QueueError> {
        check_reliable(&item)?;
        loop {
            let space = self.space.notified();
            tokio::pin!(space);
            // Register interest before checking, so a pop between the check and the await is
            // not missed.
            space.as_mut().enable();
            {
                let mut inner = self.inner.lock();
                if let Some(refused) = inner.refuse_reliable() {
                    return Err(refused);
                }
                if inner.reliable.len() < inner.capacity {
                    inner.reliable.push_back(item);
                    drop(inner);
                    self.data.notify_one();
                    return Ok(());
                }
            }
            space.await;
        }
    }

    /// Next reliable message (writer side). It stays in flight until
    /// [`OutboundQueue::reliable_written`].
    pub fn pop_reliable(&self) -> Option<Outbound> {
        let mut inner = self.inner.lock();
        let item = inner.reliable.pop_front()?;
        inner.in_flight = Some(item.clone());
        drop(inner);
        self.space.notify_one();
        Some(item)
    }

    /// The in-flight reliable message was written (or deliberately dropped, e.g. unencodable).
    pub fn reliable_written(&self) {
        self.inner.lock().in_flight = None;
    }

    /// Puts the in-flight reliable message back at the front of the queue (its connection died
    /// before the write completed). Call only after the connection's writer has stopped. The queue
    /// may then hold one message more than its capacity until the next pop. Returns whether there
    /// was one.
    pub fn requeue_in_flight(&self) -> bool {
        let mut inner = self.inner.lock();
        match inner.in_flight.take() {
            Some(item) => {
                inner.reliable.push_front(item);
                drop(inner);
                self.data.notify_one();
                true
            }
            None => false,
        }
    }

    /// Takes every pending live frame, in write order (writer side).
    pub fn take_live_round(&self, out: &mut VecDeque<(Message, FramePriority)>) {
        let mut inner = self.inner.lock();
        if inner.occupied == 0 {
            return;
        }
        for (p, priority) in LIVE_PRIORITIES.iter().enumerate() {
            if let Some(m) = inner.slots[SNAPSHOT_INDEX][p].take() {
                out.push_back((m, *priority));
            }
        }
        for (p, priority) in LIVE_PRIORITIES.iter().enumerate() {
            for t in (0..LIVE_TYPES.len()).filter(|t| *t != SNAPSHOT_INDEX) {
                if let Some(m) = inner.slots[t][p].take() {
                    out.push_back((m, *priority));
                }
            }
        }
        inner.occupied = 0;
    }

    /// Discards every pending live frame (they predate a reconnect; the fresh snapshot replaces
    /// them). Counted as `dropped_obsolete`. Returns how many were discarded.
    pub fn clear_live(&self) -> u64 {
        let mut inner = self.inner.lock();
        let mut n = 0u64;
        for row in &mut inner.slots {
            for slot in row.iter_mut() {
                if slot.take().is_some() {
                    n += 1;
                }
            }
        }
        inner.occupied = 0;
        inner.dropped_obsolete = inner.dropped_obsolete.saturating_add(n);
        n
    }

    /// Discards everything (a new session starts, or streaming stopped: the pending messages
    /// would otherwise be delivered into a later session). Live frames count as
    /// `dropped_obsolete`, reliable messages as [`Self::reliable_discarded`]. Returns how many
    /// reliable messages were discarded.
    pub fn clear_session(&self) -> usize {
        self.clear_live();
        let mut inner = self.inner.lock();
        let n = inner.reliable.len() + usize::from(inner.in_flight.is_some());
        inner.reliable.clear();
        inner.in_flight = None;
        inner.pending_snapshot_request = None;
        inner.reliable_discarded = inner
            .reliable_discarded
            .saturating_add(u64::try_from(n).unwrap_or(u64::MAX));
        drop(inner);
        self.space.notify_waiters();
        n
    }

    /// Counts live frames the writer had taken from the slots but could not write because its
    /// connection ended (counted as `dropped_obsolete`, like [`Self::clear_live`]).
    pub fn count_unsent_live(&self, n: u64) {
        let mut inner = self.inner.lock();
        inner.dropped_obsolete = inner.dropped_obsolete.saturating_add(n);
    }

    /// Reliable messages discarded unsent so far (see [`Self::clear_session`]).
    pub fn reliable_discarded(&self) -> u64 {
        self.inner.lock().reliable_discarded
    }

    /// Refuses further messages and wakes waiting producers (client shut down).
    pub fn close(&self) {
        self.inner.lock().closed = true;
        self.space.notify_waiters();
        self.data.notify_one();
    }

    /// Live frames dropped as obsolete so far.
    pub fn dropped_obsolete(&self) -> u64 {
        self.inner.lock().dropped_obsolete
    }

    /// Reliable messages plus occupied live slots.
    pub fn depth(&self) -> usize {
        let inner = self.inner.lock();
        inner.reliable.len() + inner.occupied
    }

    /// Whether anything is pending.
    pub fn is_empty(&self) -> bool {
        self.depth() == 0
    }

    /// Future completing when data may be available (writer side). A notification sent while
    /// nobody waits is stored, so none is lost.
    pub fn notified(&self) -> Notified<'_> {
        self.data.notified()
    }

    /// Driver: an inbound SNAPSHOT_REQUEST at `now_ms`. Returns `true` if a snapshot should be sent
    /// now; otherwise the request is remembered and served by [`Self::due_snapshot_request`] once
    /// the minimum interval passed, unless a snapshot is sent before (§5 coalescing).
    pub fn snapshot_request(&self, reason: SnapshotReason, now_ms: u64) -> bool {
        let mut inner = self.inner.lock();
        if inner.snapshot_limiter.permit(now_ms) {
            inner.pending_snapshot_request = None;
            true
        } else {
            inner.pending_snapshot_request = Some(reason);
            false
        }
    }

    /// Driver: a coalesced SNAPSHOT_REQUEST that is due now.
    pub fn due_snapshot_request(&self, now_ms: u64) -> Option<SnapshotReason> {
        let mut inner = self.inner.lock();
        let reason = inner.pending_snapshot_request?;
        if inner.snapshot_limiter.permit(now_ms) {
            inner.pending_snapshot_request = None;
            Some(reason)
        } else {
            None
        }
    }
}

fn check_reliable(item: &Outbound) -> Result<(), QueueError> {
    let msg_type = item.msg_type();
    if msg_type.delivery() != Delivery::Reliable {
        return Err(QueueError::WrongClass {
            msg_type,
            requested: "reliable",
        });
    }
    match item {
        Outbound::Stream { .. } if !msg_type.is_stream() => Err(QueueError::WrongClass {
            msg_type,
            requested: "stream",
        }),
        Outbound::Stream { priority, .. } if *priority != FramePriority::Control => {
            Err(QueueError::PriorityNotAllowed {
                msg_type,
                priority: *priority,
            })
        }
        Outbound::Control(_) if msg_type.is_stream() => Err(QueueError::WrongClass {
            msg_type,
            requested: "control",
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use protocol::messages::{SnapshotRequest, TelemetryPayload};

    fn tlv(tag: u8) -> TelemetryPayload {
        // A TLV header-sized blob is enough for queue tests (never decoded here).
        TelemetryPayload::new(Bytes::from(vec![tag; 64])).expect("payload")
    }

    fn delta(tag: u8) -> Message {
        Message::TelemetryDelta(tlv(tag))
    }

    fn snapshot(tag: u8) -> Message {
        Message::TelemetrySnapshot(tlv(tag))
    }

    fn weather(tag: u8) -> Message {
        Message::WeatherUpdate(tlv(tag))
    }

    fn drain(q: &OutboundQueue) -> Vec<(Message, FramePriority)> {
        let mut out = VecDeque::new();
        q.take_live_round(&mut out);
        out.into()
    }

    #[test]
    fn newer_frame_replaces_older_of_the_same_key_only() {
        let q = OutboundQueue::new(4);
        assert_eq!(
            q.push_live(delta(1), FramePriority::P0, 0),
            Ok(LiveOutcome::default())
        );
        assert_eq!(
            q.push_live(delta(2), FramePriority::P1, 0),
            Ok(LiveOutcome::default())
        );
        let out = q.push_live(delta(3), FramePriority::P0, 0).expect("queued");
        assert!(out.replaced);
        assert_eq!(q.dropped_obsolete(), 1);
        assert_eq!(q.depth(), 2);
        let round = drain(&q);
        assert_eq!(
            round,
            vec![(delta(3), FramePriority::P0), (delta(2), FramePriority::P1)]
        );
        assert!(q.is_empty());
    }

    #[test]
    fn snapshot_supersedes_pending_deltas_and_goes_first() {
        let q = OutboundQueue::new(4);
        q.push_live(delta(1), FramePriority::P0, 0).expect("delta");
        q.push_live(delta(2), FramePriority::P2, 0).expect("delta");
        q.push_live(weather(3), FramePriority::P2, 0)
            .expect("weather");
        q.push_live(Message::DriverStatus(sample_status()), FramePriority::P0, 0)
            .expect("status");
        let out = q
            .push_live(snapshot(9), FramePriority::P0, 0)
            .expect("snap");
        assert_eq!(out.superseded, 3, "two deltas and the weather frame");
        assert!(!out.replaced);
        assert_eq!(q.dropped_obsolete(), 3);
        // A delta captured after the snapshot is kept and written after it.
        q.push_live(delta(10), FramePriority::P0, 0).expect("delta");
        let round = drain(&q);
        let types: Vec<MsgType> = round.iter().map(|(m, _)| m.msg_type()).collect();
        assert_eq!(
            types,
            vec![
                MsgType::TelemetrySnapshot,
                MsgType::TelemetryDelta,
                MsgType::DriverStatus
            ]
        );
    }

    #[test]
    fn rounds_order_priorities_and_never_starve_p2() {
        let q = OutboundQueue::new(4);
        q.push_live(weather(1), FramePriority::P2, 0).expect("p2");
        q.push_live(delta(1), FramePriority::P1, 0).expect("p1");
        q.push_live(delta(2), FramePriority::P0, 0).expect("p0");
        let round = drain(&q);
        let priorities: Vec<FramePriority> = round.iter().map(|(_, p)| *p).collect();
        assert_eq!(
            priorities,
            vec![FramePriority::P0, FramePriority::P1, FramePriority::P2]
        );
    }

    #[test]
    fn live_validation() {
        let q = OutboundQueue::new(4);
        assert!(matches!(
            q.push_live(
                Message::SnapshotRequest(SnapshotRequest {
                    reason: SnapshotReason::Manual
                }),
                FramePriority::Control,
                0
            ),
            Err(QueueError::NotLatestWins { .. })
        ));
        assert!(matches!(
            q.push_live(delta(1), FramePriority::Control, 0),
            Err(QueueError::PriorityNotAllowed { .. })
        ));
    }

    #[test]
    fn reliable_queue_is_fifo_bounded_and_validated() {
        let q = OutboundQueue::new(2);
        let req = |r| Outbound::Stream {
            message: Message::SnapshotRequest(SnapshotRequest { reason: r }),
            priority: FramePriority::Control,
        };
        q.try_push_reliable(req(SnapshotReason::Manual)).expect("1");
        q.try_push_reliable(req(SnapshotReason::Reconnect))
            .expect("2");
        assert_eq!(
            q.try_push_reliable(req(SnapshotReason::SequenceGap)),
            Err(QueueError::Full { capacity: 2 })
        );
        assert_eq!(q.pop_reliable(), Some(req(SnapshotReason::Manual)));
        assert_eq!(q.pop_reliable(), Some(req(SnapshotReason::Reconnect)));
        assert_eq!(q.pop_reliable(), None);
        // Wrong class / priority.
        assert!(matches!(
            q.try_push_reliable(Outbound::Control(delta(1))),
            Err(QueueError::WrongClass { .. })
        ));
        assert!(matches!(
            q.try_push_reliable(Outbound::Control(Message::SnapshotRequest(
                SnapshotRequest {
                    reason: SnapshotReason::Manual
                }
            ))),
            Err(QueueError::WrongClass { .. })
        ));
        assert!(matches!(
            q.try_push_reliable(Outbound::Stream {
                message: Message::SnapshotRequest(SnapshotRequest {
                    reason: SnapshotReason::Manual
                }),
                priority: FramePriority::P0,
            }),
            Err(QueueError::PriorityNotAllowed { .. })
        ));
    }

    #[tokio::test]
    async fn push_reliable_waits_for_space_instead_of_dropping() {
        let q = std::sync::Arc::new(OutboundQueue::new(1));
        let item = Outbound::Control(Message::ConnectCodeRequest(
            protocol::messages::ConnectCodeRequest,
        ));
        q.try_push_reliable(item.clone()).expect("first");
        let q2 = std::sync::Arc::clone(&q);
        let item2 = item.clone();
        let waiter = tokio::spawn(async move { q2.push_reliable(item2).await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "must wait while full");
        assert_eq!(q.pop_reliable(), Some(item.clone()));
        waiter
            .await
            .expect("join")
            .expect("pushed after space freed");
        assert_eq!(q.pop_reliable(), Some(item));
        q.close();
        assert_eq!(
            q.try_push_reliable(Outbound::Control(Message::ConnectCodeRequest(
                protocol::messages::ConnectCodeRequest
            ))),
            Err(QueueError::Closed)
        );
    }

    #[test]
    fn a_message_whose_write_did_not_complete_is_requeued_first() {
        let q = OutboundQueue::new(2);
        let event = |r| Outbound::Stream {
            message: Message::SnapshotRequest(SnapshotRequest { reason: r }),
            priority: FramePriority::Control,
        };
        q.try_push_reliable(event(SnapshotReason::Manual))
            .expect("1");
        q.try_push_reliable(event(SnapshotReason::Reconnect))
            .expect("2");
        // Written completely: gone for good.
        assert_eq!(q.pop_reliable(), Some(event(SnapshotReason::Manual)));
        q.reliable_written();
        assert!(!q.requeue_in_flight());
        // The connection died during the write: back at the front, ahead of later messages.
        assert_eq!(q.pop_reliable(), Some(event(SnapshotReason::Reconnect)));
        q.try_push_reliable(event(SnapshotReason::SequenceGap))
            .expect("3");
        assert!(q.requeue_in_flight());
        assert!(!q.requeue_in_flight(), "requeued once");
        assert_eq!(q.pop_reliable(), Some(event(SnapshotReason::Reconnect)));
        q.reliable_written();
        assert_eq!(q.pop_reliable(), Some(event(SnapshotReason::SequenceGap)));
        // A new session discards the in-flight message too.
        assert_eq!(q.clear_session(), 1);
        assert!(!q.requeue_in_flight());
        assert!(q.is_empty());
        assert_eq!(q.reliable_discarded(), 1);
    }

    #[tokio::test]
    async fn reliable_messages_are_refused_while_not_streaming_and_waiters_are_released() {
        let q = std::sync::Arc::new(OutboundQueue::new(1));
        let item = || {
            Outbound::Control(Message::ConnectCodeRequest(
                protocol::messages::ConnectCodeRequest,
            ))
        };
        q.try_push_reliable(item()).expect("streaming by default");
        // A producer waits for space in the full queue ...
        let q2 = std::sync::Arc::clone(&q);
        let waiter = tokio::spawn(async move { q2.push_reliable(item()).await });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished(), "must wait while full");
        // ... and is released at once when streaming stops, instead of hanging until a later
        // session drains the queue.
        q.set_streaming(false);
        assert_eq!(waiter.await.expect("join"), Err(QueueError::NotStreaming));
        assert_eq!(q.try_push_reliable(item()), Err(QueueError::NotStreaming));
        assert_eq!(q.push_reliable(item()).await, Err(QueueError::NotStreaming));
        // What was queued before is discarded and counted by the owner.
        assert_eq!(q.clear_session(), 1);
        assert_eq!(q.reliable_discarded(), 1);
        q.set_streaming(true);
        q.try_push_reliable(item()).expect("streaming again");
        // Closed wins over not streaming.
        q.set_streaming(false);
        q.close();
        assert_eq!(q.try_push_reliable(item()), Err(QueueError::Closed));
    }

    #[test]
    fn clear_live_and_clear_session() {
        let q = OutboundQueue::new(4);
        q.push_live(delta(1), FramePriority::P0, 0).expect("delta");
        q.push_live(snapshot(1), FramePriority::P0, 0)
            .expect("snap");
        assert_eq!(q.clear_live(), 1);
        assert_eq!(q.dropped_obsolete(), 2);
        q.try_push_reliable(Outbound::Control(Message::ConnectCodeRequest(
            protocol::messages::ConnectCodeRequest,
        )))
        .expect("reliable");
        assert_eq!(q.clear_session(), 1);
        assert!(q.is_empty());
    }

    #[test]
    fn snapshot_requests_are_coalesced_until_the_interval_passed_or_a_snapshot_went_out() {
        let interval = protocol::timing::SNAPSHOT_REQUEST_MIN_INTERVAL_MS;
        let q = OutboundQueue::new(4);
        assert!(q.snapshot_request(SnapshotReason::EngineerJoined, 1_000));
        assert!(!q.snapshot_request(SnapshotReason::Reconnect, 1_000 + interval - 1));
        assert_eq!(q.due_snapshot_request(1_000 + interval - 1), None);
        assert_eq!(
            q.due_snapshot_request(1_000 + interval),
            Some(SnapshotReason::Reconnect)
        );
        assert_eq!(q.due_snapshot_request(5_000), None, "served once");
        // A snapshot sent meanwhile satisfies a coalesced request.
        assert!(!q.snapshot_request(SnapshotReason::Manual, 1_000 + interval + 1));
        q.push_live(snapshot(2), FramePriority::P0, 1_000 + interval + 2)
            .expect("snapshot");
        assert_eq!(q.due_snapshot_request(10_000), None);
    }

    fn sample_status() -> protocol::messages::DriverStatus {
        use protocol::messages::*;
        let ok = ComponentHealth {
            level: HealthLevel::Ok,
            detail: None,
        };
        DriverStatus {
            lmu_state: LmuSourceState::Live,
            lmu_source: ok.clone(),
            processing: ok.clone(),
            recording: ok.clone(),
            network: ok,
            rates: DriverRates {
                telemetry_read_hz: None,
                scoring_read_hz: None,
                frames_sent_hz: None,
                bytes_sent_per_s: None,
                frames_dropped_total: 0,
            },
            clock: None,
        }
    }
}

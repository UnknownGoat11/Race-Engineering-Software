//! Per-stream sequence tracking (docs/PROTOCOL.md §5).
//!
//! * [`SequenceTracker`] classifies every arriving sequence number and keeps the §5 loss /
//!   duplicate counters. It does **not** decide what is shown live: the relay delivers frames per
//!   `(msg_type, priority)` latest-wins slot plus a reliable control queue (§2), so frames
//!   legitimately arrive out of global sequence order.
//! * [`LiveGate`] decides liveness per latest-wins key: a frame is applied if it is newer than the
//!   last frame applied for its own `(msg_type, priority)`; an applied snapshot supersedes older
//!   deltas.
//! * [`StreamReceiver`] combines both for one inbound stream and handles the session lifecycle:
//!   frames replayed by the relay on join are applied but not tracked, SESSION_INFO resets the
//!   stream, and a driver restart (new sequence space) is detected as [`Arrival::Restart`].
//! * [`ReorderBuffer`] restores sequence order for archival consumers within a window of
//!   [`REORDER_WINDOW`] frames; anything that does not arrive within the window is declared lost.
//! * [`SnapshotRequestLimiter`] coalesces inbound SNAPSHOT_REQUESTs (driver) and rate-limits them
//!   per engineer (relay).
//!
//! "Seen" information is kept for the last [`REORDER_WINDOW`] sequence numbers below the highest one
//! (a 64-bit bitmap), so classification is O(1) and memory is constant.

use std::collections::BTreeMap;

use crate::frame::{
    CONTROL_SEQUENCE, Delivery, FIRST_SEQUENCE, Frame, FrameHeader, FramePriority, HeaderRuleError,
    MsgType, NO_SESSION, STREAM_TYPE_FIRST, STREAM_TYPE_LAST, is_stream_class_id,
};

/// Number of sequence numbers (ending at the highest seen) for which arrival is remembered, and the
/// capacity of the archival reorder buffer.
pub const REORDER_WINDOW: u64 = 64;

/// Priorities a latest-wins frame may use: P0, P1, P2 (§2).
const LIVE_PRIORITIES: u32 = 3;

/// Number of latest-wins keys `(msg_type, priority)` a stream can use (§2): every latest-wins type
/// times the three live priorities.
pub const LATEST_WINS_KEYS: u32 = {
    let mut keys = 0;
    let mut i = 0;
    while i < MsgType::ALL.len() {
        if matches!(MsgType::ALL[i].delivery(), Delivery::LatestWins) {
            keys += LIVE_PRIORITIES;
        }
        i += 1;
    }
    keys
};

/// Number of consecutive frames that are all stale (duplicate or older than the window) and carry
/// strictly increasing sequence numbers after which [`SequenceTracker`] concludes that the sender
/// restarted its sequence space (defence in depth; the relay's SESSION_INFO is the normative signal,
/// §6).
///
/// A conforming relay never produces such a run: it serves the reliable queue first (§2), so a
/// reliable frame is never overtaken, and after any fresh frame at most one frame per *other*
/// latest-wins key can follow before the next fresh one, i.e. at most `LATEST_WINS_KEYS - 1` stale
/// frames in a row. Twice the key count leaves a margin and still detects a restart within a few
/// dozen frames.
pub const RESTART_DETECT_RUN: u32 = 2 * LATEST_WINS_KEYS;
const _: () = assert!(RESTART_DETECT_RUN > LATEST_WINS_KEYS);

/// Classification of one arriving sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrival {
    /// First frame of the stream (or after a reset).
    First,
    /// Exactly the next expected sequence.
    InOrder,
    /// Newer than expected: `missing` sequence numbers were skipped (counted as lost for now).
    Gap { missing: u64 },
    /// Older than the highest seen and not seen before: fills an earlier gap. Not used for live state.
    Late,
    /// Already received.
    Duplicate,
    /// Older than the tracking window; whether it is a duplicate can no longer be told.
    TooOld,
    /// The sender restarted its sequence space ([`RESTART_DETECT_RUN`] consecutive stale frames
    /// with increasing sequences). The tracker was reset and this frame is the first of the new
    /// space; live state keyed on the old space must be reset too.
    Restart,
}

impl Arrival {
    /// Whether the frame advanced the stream's highest sequence (or started a new one). This is the
    /// loss-accounting view only; whether a frame updates live display is decided per
    /// `(msg_type, priority)` by [`LiveGate`].
    pub const fn advances_highest(self) -> bool {
        matches!(
            self,
            Arrival::First | Arrival::InOrder | Arrival::Gap { .. } | Arrival::Restart
        )
    }
}

/// Counters of one stream (§5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SequenceStats {
    /// Next sequence number expected (`highest + 1`), `None` before the first frame.
    pub expected: Option<u64>,
    /// Highest sequence number seen.
    pub highest: Option<u64>,
    /// Distinct frames received (including late ones).
    pub received: u64,
    /// Sequence numbers currently missing (gaps not filled by late frames).
    pub lost: u64,
    /// Frames received more than once.
    pub duplicated: u64,
    /// Frames that arrived after a newer one (late gap fills).
    pub out_of_order: u64,
    /// Frames older than the tracking window.
    pub too_old: u64,
    /// Sequence-space restarts detected by the tracker ([`Arrival::Restart`]). Survives the reset
    /// that a detected restart performs; cleared by [`SequenceTracker::reset`].
    pub restarts: u64,
}

/// Sequence tracker for one (sender, session) stream.
#[derive(Debug, Clone, Default)]
pub struct SequenceTracker {
    stats: SequenceStats,
    /// Bit `i` set ⇔ sequence `highest - i` has been received (bit 0 is the highest itself).
    seen: u64,
    /// Length of the current run of consecutive stale (Duplicate / TooOld) frames whose sequences
    /// strictly increase, and the last sequence of that run.
    stale_run: u32,
    stale_run_last: u64,
}

impl SequenceTracker {
    /// New tracker with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets all history (new sequence space, e.g. after SESSION_INFO / driver restart).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Current counters.
    pub const fn stats(&self) -> SequenceStats {
        self.stats
    }

    /// Records the arrival of `seq` and classifies it.
    ///
    /// Restart detection (defence in depth behind the relay's SESSION_INFO, §5):
    /// * [`FIRST_SEQUENCE`] that does not advance the highest sequence is an immediate
    ///   [`Arrival::Restart`]. A sender emits seq 1 once per sequence space and the stream runs over
    ///   one ordered TCP connection whose replayed frames are never tracked; with the reliable
    ///   SCHEMA_NEGOTIATE as a space's first frame (§5) nothing of the same space overtakes it, so a
    ///   tracked seq 1 below the highest can only be the first frame of a new space. Without this, a restart
    ///   after fewer than [`RESTART_DETECT_RUN`] frames read the new space's first frames as
    ///   duplicates and then silently continued the old space.
    /// * Otherwise [`RESTART_DETECT_RUN`] consecutive stale frames with strictly increasing
    ///   sequences (a restart whose first frame was lost or overtaken).
    pub fn observe(&mut self, seq: u64) -> Arrival {
        let arrival = self.classify(seq);
        if seq == FIRST_SEQUENCE
            && matches!(
                arrival,
                Arrival::Late | Arrival::Duplicate | Arrival::TooOld
            )
        {
            return self.restart_at(seq);
        }
        match arrival {
            Arrival::Duplicate | Arrival::TooOld => {
                let continues = self.stale_run > 0 && seq > self.stale_run_last;
                self.stale_run = if continues {
                    self.stale_run.saturating_add(1)
                } else {
                    1
                };
                self.stale_run_last = seq;
                if self.stale_run >= RESTART_DETECT_RUN {
                    return self.restart_at(seq);
                }
            }
            _ => self.stale_run = 0,
        }
        arrival
    }

    /// Resets for a new sequence space whose first observed frame is `seq`, keeping the restart
    /// count. The counters of the classification that preceded this call are discarded with the
    /// old space.
    fn restart_at(&mut self, seq: u64) -> Arrival {
        let restarts = self.stats.restarts.saturating_add(1);
        self.reset();
        self.classify(seq);
        self.stats.restarts = restarts;
        Arrival::Restart
    }

    fn classify(&mut self, seq: u64) -> Arrival {
        let Some(highest) = self.stats.highest else {
            self.stats.highest = Some(seq);
            self.stats.expected = seq.checked_add(1);
            self.stats.received = 1;
            self.seen = 1;
            return Arrival::First;
        };
        if seq > highest {
            let advance = seq - highest;
            let missing = advance - 1;
            self.seen = if advance >= REORDER_WINDOW {
                0
            } else {
                self.seen << advance
            };
            self.seen |= 1;
            self.stats.highest = Some(seq);
            self.stats.expected = seq.checked_add(1);
            self.stats.received = self.stats.received.saturating_add(1);
            self.stats.lost = self.stats.lost.saturating_add(missing);
            return if missing == 0 {
                Arrival::InOrder
            } else {
                Arrival::Gap { missing }
            };
        }
        let back = highest - seq;
        if back >= REORDER_WINDOW {
            self.stats.too_old = self.stats.too_old.saturating_add(1);
            return Arrival::TooOld;
        }
        let bit = 1u64 << back;
        if self.seen & bit != 0 {
            self.stats.duplicated = self.stats.duplicated.saturating_add(1);
            return Arrival::Duplicate;
        }
        self.seen |= bit;
        self.stats.received = self.stats.received.saturating_add(1);
        self.stats.out_of_order = self.stats.out_of_order.saturating_add(1);
        self.stats.lost = self.stats.lost.saturating_sub(1);
        Arrival::Late
    }
}

/// Priority slots per stream type in [`LiveGate`]: P0, P1, P2 and control.
const PRIORITY_SLOTS: usize = 4;
/// The single [`LiveGate`] slot shared by every priority of a whole-state stream type (every
/// stream type except TELEMETRY_DELTA).
const SHARED_SLOT: usize = 0;
/// Number of stream (telemetry-class) message types, 0x20–0x28.
const STREAM_TYPES: usize = (STREAM_TYPE_LAST - STREAM_TYPE_FIRST + 1) as usize;

/// Live-state gate for one inbound stream (§5): "accept only if newer" **per live key**, matching
/// the relay's per-key slots and reliable queue (§2).
///
/// * The live key of TELEMETRY_DELTA is `(msg_type, priority)`: its frames of different priorities
///   carry different channels, so P1 delta 201 is still applied after P0 delta 202.
/// * Every other stream type carries its whole state at any priority (TELEMETRY_SNAPSHOT,
///   SCORING_UPDATE, WEATHER_UPDATE, DRIVER_STATUS, RECORDING_STATUS, and the reliable types), so
///   its live key is the type alone: a P1 snapshot 250 arriving after a P0 snapshot 300 (the relay
///   serves P0 first) is refused instead of rolling every channel back.
/// * A frame is accepted if its sequence is above the last one accepted for its key. Frames of one
///   relay key never overtake each other on the way, so for the reliable stream types
///   (SCHEMA_NEGOTIATE, SESSION_EVENT, which travel in the FIFO control queue) this rejects
///   exactly the duplicates.
/// * An accepted TELEMETRY_SNAPSHOT carries every channel (weather channels included), so it
///   supersedes every channel frame — TELEMETRY_DELTA of any priority and WEATHER_UPDATE — with a
///   sequence at or below its own.
/// * A TELEMETRY_SNAPSHOT older than an already applied channel frame is refused (applying it
///   would roll those channels back). §2 makes the relay send a pending snapshot first, so this
///   only happens with a misbehaving relay; the next periodic snapshot restores the full state.
/// * Control-class types and SNAPSHOT_REQUEST (which reaches the driver from several engineers'
///   sequence spaces) are not sequence-gated: always accepted.
#[derive(Debug, Clone, Default)]
pub struct LiveGate {
    /// Last accepted sequence per `[stream type][priority slot]` (whole-state types use only
    /// [`SHARED_SLOT`]).
    applied: [[Option<u64>; PRIORITY_SLOTS]; STREAM_TYPES],
    /// Sequence of the newest accepted TELEMETRY_SNAPSHOT.
    snapshot_floor: Option<u64>,
}

/// TLV channel frames that a TELEMETRY_SNAPSHOT supersedes (it carries every channel, §3/§4).
const fn is_partial_channel_frame(msg_type: MsgType) -> bool {
    matches!(msg_type, MsgType::TelemetryDelta | MsgType::WeatherUpdate)
}

impl LiveGate {
    /// Gate with nothing applied.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets everything (new sequence space).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Whether a frame of `msg_type` / `priority` / `seq` updates live state. An accepted frame
    /// becomes the newest of its key.
    pub fn accept(&mut self, msg_type: MsgType, priority: FramePriority, seq: u64) -> bool {
        if msg_type == MsgType::SnapshotRequest {
            return true;
        }
        let Some((t, p)) = Self::slot(msg_type, priority) else {
            return true;
        };
        if self.applied[t][p].is_some_and(|applied| seq <= applied) {
            return false;
        }
        if is_partial_channel_frame(msg_type) && self.snapshot_floor.is_some_and(|f| seq <= f) {
            return false;
        }
        if msg_type == MsgType::TelemetrySnapshot
            && self
                .newest_partial_channel_frame()
                .is_some_and(|newest| newest > seq)
        {
            return false;
        }
        self.applied[t][p] = Some(seq);
        if msg_type == MsgType::TelemetrySnapshot {
            self.snapshot_floor = Some(self.snapshot_floor.map_or(seq, |floor| floor.max(seq)));
        }
        true
    }

    /// Newest accepted TELEMETRY_DELTA or WEATHER_UPDATE, over all priorities.
    fn newest_partial_channel_frame(&self) -> Option<u64> {
        [MsgType::TelemetryDelta, MsgType::WeatherUpdate]
            .into_iter()
            .filter_map(|t| Self::slot(t, FramePriority::P0))
            .flat_map(|(t, _)| self.applied[t].iter().flatten().copied())
            .max()
    }

    /// `[stream type][priority slot]` index of a stream type, `None` for control-class types.
    ///
    /// Only TELEMETRY_DELTA has one slot per priority: its frames of different priorities carry
    /// different channels. Every other stream type carries its whole state at any priority, so all
    /// its priorities share one slot: a frame older than one already applied at *another* priority
    /// would roll that state back (§5).
    fn slot(msg_type: MsgType, priority: FramePriority) -> Option<(usize, usize)> {
        if !msg_type.is_stream() {
            return None;
        }
        let t = usize::from(msg_type.id() - STREAM_TYPE_FIRST);
        if msg_type != MsgType::TelemetryDelta {
            return Some((t, SHARED_SLOT));
        }
        let p = match priority {
            FramePriority::P0 => 0,
            FramePriority::P1 => 1,
            FramePriority::P2 => 2,
            FramePriority::Control => 3,
        };
        Some((t, p))
    }
}

/// What a [`StreamReceiver`] concluded about one inbound frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reception {
    /// Sequence classification, `None` for frames that are not sequence-tracked: control-class
    /// frames, inbound SNAPSHOT_REQUEST and frames the relay replayed on join.
    pub arrival: Option<Arrival>,
    /// Whether the frame is applied to live state (or, for reliable types, delivered).
    pub live: bool,
}

/// Receive-side sequencing of one inbound connection's stream (§5, §6): loss accounting with
/// [`SequenceTracker`], liveness with [`LiveGate`], and the session lifecycle:
/// * SESSION_INFO resets both (new session, join, or the relay announcing a driver restart);
/// * frames whose relay trailer has the REPLAYED flag (join replay of the relay's cache) are gated
///   but never tracked, so they cause neither false loss nor a false "late" verdict;
/// * a detected sequence-space restart ([`Arrival::Restart`]) resets the gate as well.
#[derive(Debug, Clone, Default)]
pub struct StreamReceiver {
    tracker: SequenceTracker,
    gate: LiveGate,
}

impl StreamReceiver {
    /// Receiver with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets all history.
    pub fn reset(&mut self) {
        self.tracker.reset();
        self.gate.reset();
    }

    /// Loss / duplicate counters of the tracked (non-replayed) frames.
    pub const fn stats(&self) -> SequenceStats {
        self.tracker.stats()
    }

    /// Classifies one frame given its header fields. `replayed` is the relay trailer's REPLAYED
    /// flag. Header rules must have been checked by the caller.
    pub fn receive(
        &mut self,
        msg_type: MsgType,
        priority: FramePriority,
        sequence: u64,
        replayed: bool,
    ) -> Reception {
        if msg_type == MsgType::SessionInfo {
            self.reset();
        }
        if !msg_type.is_stream() || msg_type == MsgType::SnapshotRequest {
            return Reception {
                arrival: None,
                live: true,
            };
        }
        if replayed {
            return Reception {
                arrival: None,
                live: self.gate.accept(msg_type, priority, sequence),
            };
        }
        let arrival = self.tracker.observe(sequence);
        if arrival == Arrival::Restart {
            self.gate.reset();
        }
        Reception {
            arrival: Some(arrival),
            live: self.gate.accept(msg_type, priority, sequence),
        }
    }

    /// Sequence-observes a frame whose type this version does not know but whose id lies in the
    /// reserved stream-class range 0x20–0x2F ([`is_stream_class_id`]): a stream type added by a
    /// later minor still consumes a sequence number of the sender's space, so skipping it unseen
    /// would count one false loss per frame (§5, §10). The frame is never applied.
    ///
    /// Returns the classification, or `None` (receiver unchanged) if the id is known, outside the
    /// range, the frame was replayed by the relay, or its header breaks the stream rules (sequence 0
    /// or no session id). Call it where an unknown type is skipped; [`Self::receive_frame`] does.
    pub fn observe_unknown(&mut self, header: &FrameHeader, replayed: bool) -> Option<Arrival> {
        if header.known_msg_type().is_some()
            || !is_stream_class_id(header.msg_type)
            || replayed
            || header.sequence == CONTROL_SEQUENCE
            || header.session_id == NO_SESSION
        {
            return None;
        }
        let arrival = self.tracker.observe(header.sequence);
        if arrival == Arrival::Restart {
            self.gate.reset();
        }
        Some(arrival)
    }

    /// Checks the header rules of `frame` and classifies it. A rule violation is returned (skip and
    /// count the frame) and leaves the receiver unchanged, except that an unknown type in the
    /// reserved stream-class range is still sequence-observed ([`Self::observe_unknown`]) before
    /// its [`HeaderRuleError::UnknownMsgType`] is returned.
    pub fn receive_frame(&mut self, frame: &Frame) -> Result<Reception, HeaderRuleError> {
        let header = frame.header();
        let replayed = frame.relay_trailer().is_some_and(|t| t.replayed);
        let msg_type = match frame.check_rules() {
            Ok(msg_type) => msg_type,
            Err(err @ HeaderRuleError::UnknownMsgType(_)) => {
                self.observe_unknown(header, replayed);
                return Err(err);
            }
            Err(err) => return Err(err),
        };
        Ok(self.receive(msg_type, header.priority, header.sequence, replayed))
    }
}

/// Minimum-interval limiter for SNAPSHOT_REQUEST handling (§5):
/// * the driver serves at most one request-triggered snapshot per interval and records every
///   snapshot it sends ([`SnapshotRequestLimiter::note_sent`]); a request inside the interval is
///   coalesced into the next periodic snapshot (≤ `SNAPSHOT_INTERVAL_MS` away);
/// * the relay keeps one limiter per engineer and drops (and counts) requests above the rate.
///
/// Times are monotonic milliseconds supplied by the caller. A clock that goes backwards permits
/// (one extra snapshot is harmless; a stuck limiter is not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotRequestLimiter {
    min_interval_ms: u64,
    last_ms: Option<u64>,
}

impl Default for SnapshotRequestLimiter {
    fn default() -> Self {
        Self::new(crate::timing::SNAPSHOT_REQUEST_MIN_INTERVAL_MS)
    }
}

impl SnapshotRequestLimiter {
    /// Limiter allowing one event per `min_interval_ms`.
    pub const fn new(min_interval_ms: u64) -> Self {
        Self {
            min_interval_ms,
            last_ms: None,
        }
    }

    /// Whether a request arriving at `now_ms` is served now. A permitted request is recorded.
    pub fn permit(&mut self, now_ms: u64) -> bool {
        let within_interval = self
            .last_ms
            .is_some_and(|last| now_ms >= last && now_ms - last < self.min_interval_ms);
        if !within_interval {
            self.last_ms = Some(now_ms);
        }
        !within_interval
    }

    /// Records a snapshot sent for another reason (periodic) at `now_ms`.
    pub fn note_sent(&mut self, now_ms: u64) {
        self.last_ms = Some(now_ms);
    }
}

/// Outcome of offering an item to the [`ReorderBuffer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReorderOutcome<T> {
    /// Buffered (and possibly released together with others).
    Accepted,
    /// The sequence was already released or skipped; the item is returned.
    TooLate(T),
    /// The sequence is already buffered; the item is returned.
    Duplicate(T),
}

/// Archival reorder buffer: releases items strictly in sequence order, waiting up to
/// [`REORDER_WINDOW`] frames for a missing sequence before skipping it.
#[derive(Debug, Clone)]
pub struct ReorderBuffer<T> {
    next: Option<u64>,
    pending: BTreeMap<u64, T>,
    skipped: u64,
}

impl<T> Default for ReorderBuffer<T> {
    fn default() -> Self {
        Self {
            next: None,
            pending: BTreeMap::new(),
            skipped: 0,
        }
    }
}

impl<T> ReorderBuffer<T> {
    /// Empty buffer; the first pushed sequence starts the order.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sequence numbers given up on so far (never arrived within the window).
    pub const fn skipped(&self) -> u64 {
        self.skipped
    }

    /// Items currently held back.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Offers `item` with sequence `seq`; every item that becomes releasable is appended to `out`
    /// in sequence order.
    pub fn push(
        &mut self,
        seq: u64,
        item: T,
        out: &mut impl Extend<(u64, T)>,
    ) -> ReorderOutcome<T> {
        let next = *self.next.get_or_insert(seq);
        if seq < next {
            return ReorderOutcome::TooLate(item);
        }
        if self.pending.contains_key(&seq) {
            return ReorderOutcome::Duplicate(item);
        }
        self.pending.insert(seq, item);
        self.release(out);
        ReorderOutcome::Accepted
    }

    /// Starts a new sequence space: everything still buffered is released first (in order, gaps
    /// counted as skipped, exactly like [`ReorderBuffer::flush`]), then the order is forgotten, so
    /// the next pushed sequence starts the new order. Call it on every SESSION_INFO and on
    /// [`Arrival::Restart`] (§5); otherwise every frame of the new space (seq 1, 2, …) would be
    /// refused as [`ReorderOutcome::TooLate`]. [`ReorderBuffer::skipped`] keeps counting across
    /// resets.
    pub fn reset(&mut self, out: &mut impl Extend<(u64, T)>) {
        self.flush(out);
        self.next = None;
    }

    /// Releases everything still buffered, in order, skipping the gaps (end of stream).
    pub fn flush(&mut self, out: &mut impl Extend<(u64, T)>) {
        while let Some((seq, item)) = self.pending.pop_first() {
            if let Some(next) = self.next {
                self.skipped = self.skipped.saturating_add(seq.saturating_sub(next));
            }
            self.next = seq.checked_add(1);
            out.extend(std::iter::once((seq, item)));
        }
    }

    fn release(&mut self, out: &mut impl Extend<(u64, T)>) {
        loop {
            let Some(next) = self.next else { return };
            let Some((&first, _)) = self.pending.first_key_value() else {
                return;
            };
            let Some((&last, _)) = self.pending.last_key_value() else {
                return;
            };
            if first != next {
                // Wait for `next` until the window is exceeded, then give up on the gap.
                let window_exceeded =
                    last - next >= REORDER_WINDOW || self.pending.len() as u64 >= REORDER_WINDOW;
                if !window_exceeded {
                    return;
                }
                self.skipped = self.skipped.saturating_add(first - next);
            }
            let Some((seq, item)) = self.pending.pop_first() else {
                return;
            };
            self.next = seq.checked_add(1);
            out.extend(std::iter::once((seq, item)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seqs: &[u64]) -> (SequenceTracker, Vec<Arrival>) {
        let mut t = SequenceTracker::new();
        let arrivals = seqs.iter().map(|s| t.observe(*s)).collect();
        (t, arrivals)
    }

    #[test]
    fn gap_501_502_504_reports_503_lost() {
        let (t, a) = run(&[501, 502, 504]);
        assert_eq!(
            a,
            vec![
                Arrival::First,
                Arrival::InOrder,
                Arrival::Gap { missing: 1 }
            ]
        );
        let s = t.stats();
        assert_eq!(s.lost, 1);
        assert_eq!(s.received, 3);
        assert_eq!(s.expected, Some(505));
        assert_eq!(s.highest, Some(504));
    }

    #[test]
    fn ordering_100_101_103_102_104() {
        let (t, a) = run(&[100, 101, 103, 102, 104]);
        assert_eq!(
            a,
            vec![
                Arrival::First,
                Arrival::InOrder,
                Arrival::Gap { missing: 1 },
                Arrival::Late,
                Arrival::InOrder
            ]
        );
        let live: Vec<u64> = [100, 101, 103, 102, 104]
            .into_iter()
            .zip(&a)
            .filter(|(_, a)| a.advances_highest())
            .map(|(s, _)| s)
            .collect();
        assert_eq!(
            live,
            vec![100, 101, 103, 104],
            "102 must not roll live state back"
        );
        let s = t.stats();
        assert_eq!(
            (s.received, s.lost, s.out_of_order, s.duplicated),
            (5, 0, 1, 0)
        );

        // Archival order is restored.
        let mut buf = ReorderBuffer::new();
        let mut out = Vec::new();
        for s in [100u64, 101, 103, 102, 104] {
            assert_eq!(buf.push(s, s, &mut out), ReorderOutcome::Accepted);
        }
        assert_eq!(
            out.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec![100, 101, 102, 103, 104]
        );
        assert_eq!(buf.skipped(), 0);
    }

    #[test]
    fn duplicates_are_counted_and_not_live() {
        // Starts above FIRST_SEQUENCE: a repeated seq 1 is a restart, not a duplicate (§5).
        let (t, a) = run(&[5, 6, 6, 7, 5, 7]);
        assert_eq!(a[2], Arrival::Duplicate);
        assert_eq!(a[4], Arrival::Duplicate);
        assert_eq!(a[5], Arrival::Duplicate);
        assert!(!Arrival::Duplicate.advances_highest());
        let s = t.stats();
        assert_eq!((s.received, s.duplicated, s.lost), (3, 3, 0));
        // A late frame seen twice: first Late, then Duplicate.
        let (t, a) = run(&[10, 12, 11, 11]);
        assert_eq!(&a[2..], &[Arrival::Late, Arrival::Duplicate]);
        assert_eq!(t.stats().lost, 0);
    }

    #[test]
    fn window_edges() {
        let mut t = SequenceTracker::new();
        t.observe(1000);
        t.observe(1000 + REORDER_WINDOW); // big jump: 63 missing
        assert_eq!(t.stats().lost, REORDER_WINDOW - 1);
        assert_eq!(t.observe(1001), Arrival::Late);
        assert_eq!(t.observe(1000), Arrival::TooOld);
        assert_eq!(t.stats().too_old, 1);
        // Huge jump does not overflow.
        assert_eq!(
            t.observe(u64::MAX),
            Arrival::Gap {
                missing: u64::MAX - 1000 - REORDER_WINDOW - 1
            }
        );
        assert_eq!(t.stats().expected, None);
        t.reset();
        assert_eq!(t.observe(5), Arrival::First);
    }

    #[test]
    fn reorder_buffer_skips_after_window() {
        let mut buf = ReorderBuffer::new();
        let mut out = Vec::new();
        buf.push(1, 'a', &mut out);
        // 2 never arrives.
        for s in 3..(3 + REORDER_WINDOW) {
            buf.push(s, 'x', &mut out);
        }
        assert_eq!(buf.skipped(), 1);
        assert_eq!(out.first().map(|(s, _)| *s), Some(1));
        assert_eq!(out.get(1).map(|(s, _)| *s), Some(3));
        assert_eq!(buf.push(2, 'z', &mut out), ReorderOutcome::TooLate('z'));
        let released: Vec<u64> = out.iter().map(|(s, _)| *s).collect();
        assert!(
            released.windows(2).all(|w| w[0] < w[1]),
            "strictly increasing"
        );
        assert!(buf.pending() < REORDER_WINDOW as usize);
    }

    #[test]
    fn reorder_buffer_duplicate_and_flush() {
        let mut buf = ReorderBuffer::new();
        let mut out = Vec::new();
        buf.push(10, 1, &mut out);
        buf.push(12, 2, &mut out);
        assert_eq!(buf.push(12, 3, &mut out), ReorderOutcome::Duplicate(3));
        assert_eq!(buf.push(10, 4, &mut out), ReorderOutcome::TooLate(4));
        buf.flush(&mut out);
        assert_eq!(out, vec![(10, 1), (12, 2)]);
        assert_eq!(buf.skipped(), 1);
    }

    #[test]
    fn reorder_buffer_reset_starts_a_new_sequence_space() {
        // Review finding: without a reset every frame of a restarted driver (seq 1, 2, …) came
        // back as TooLate for good.
        let mut buf = ReorderBuffer::new();
        let mut out = Vec::new();
        buf.push(500, 'a', &mut out);
        buf.push(502, 'c', &mut out);
        assert_eq!(buf.push(3, 'x', &mut out), ReorderOutcome::TooLate('x'));
        buf.reset(&mut out);
        assert_eq!(
            out,
            vec![(500, 'a'), (502, 'c')],
            "buffered items are released"
        );
        assert_eq!(buf.skipped(), 1, "the gap at 501 stays counted");
        assert_eq!(buf.pending(), 0);
        out.clear();
        for (s, c) in [(1, 'n'), (3, 'p'), (2, 'o')] {
            assert_eq!(buf.push(s, c, &mut out), ReorderOutcome::Accepted);
        }
        assert_eq!(out, vec![(1, 'n'), (2, 'o'), (3, 'p')]);
        assert_eq!(buf.skipped(), 1);
    }

    use crate::frame::{FrameMeta, RelayTrailer};
    use FramePriority::{Control, P0, P1, P2};
    use MsgType::{
        SchemaNegotiate, ScoringUpdate, SessionEvent, SessionInfo, TelemetryDelta,
        TelemetrySnapshot, WeatherUpdate,
    };

    fn live(rx: &mut StreamReceiver, t: MsgType, p: FramePriority, seq: u64) -> bool {
        rx.receive(t, p, seq, false).live
    }

    #[test]
    fn priority_reordering_keeps_the_newest_frame_of_every_key() {
        let mut rx = StreamReceiver::new();
        assert!(live(&mut rx, TelemetryDelta, P0, 200));
        // The relay serves the P0 slot first: 202 overtakes P1 201.
        assert!(live(&mut rx, TelemetryDelta, P0, 202));
        assert!(
            live(&mut rx, TelemetryDelta, P1, 201),
            "newest tyre/brake values must not be discarded"
        );
        // Sustained congestion: P1 always arrives after a newer P0.
        for k in 0..10u64 {
            let base = 203 + 2 * k;
            assert!(live(&mut rx, TelemetryDelta, P0, base + 1));
            assert!(live(&mut rx, TelemetryDelta, P1, base));
        }
        // A reliable event overtaken by a delta is still delivered, once.
        assert!(live(&mut rx, TelemetryDelta, P0, 224));
        assert!(live(&mut rx, SessionEvent, Control, 223));
        assert!(!live(&mut rx, SessionEvent, Control, 223), "duplicate");
        // An older frame of the same key never rolls live state back.
        assert!(!live(&mut rx, TelemetryDelta, P1, 210));
        let s = rx.stats();
        assert_eq!((s.lost, s.duplicated), (0, 2));
    }

    #[test]
    fn snapshot_supersedes_older_deltas_of_every_priority() {
        let mut gate = LiveGate::new();
        assert!(gate.accept(TelemetryDelta, P0, 290));
        assert!(gate.accept(TelemetrySnapshot, P0, 300));
        assert!(!gate.accept(TelemetryDelta, P1, 299), "snapshot is newer");
        assert!(!gate.accept(TelemetryDelta, P2, 295));
        assert!(
            gate.accept(ScoringUpdate, P0, 297),
            "scoring is not part of the TLV snapshot: own key"
        );
        assert!(gate.accept(TelemetryDelta, P2, 301));
        // A snapshot older than an applied delta would roll channels back.
        assert!(!gate.accept(TelemetrySnapshot, P0, 250));
        assert!(gate.accept(TelemetrySnapshot, P0, 302));
        // Control-class types are not gated.
        assert!(gate.accept(MsgType::Ping, Control, 0));
        assert!(gate.accept(MsgType::SnapshotRequest, Control, 1));
        assert!(gate.accept(MsgType::SnapshotRequest, Control, 1));
    }

    #[test]
    fn snapshot_supersedes_older_weather_updates() {
        // The snapshot carries every channel, weather channels included: a weather update the
        // relay sends after a newer snapshot (pending snapshots go first, §2) is older data.
        let mut gate = LiveGate::new();
        assert!(gate.accept(WeatherUpdate, P2, 280));
        assert!(gate.accept(TelemetrySnapshot, P0, 300));
        assert!(
            !gate.accept(WeatherUpdate, P2, 298),
            "would roll weather channels back"
        );
        assert!(gate.accept(WeatherUpdate, P2, 301));
        assert!(!gate.accept(TelemetrySnapshot, P0, 300), "duplicate");
        // A snapshot older than an applied weather update would roll weather channels back.
        let mut gate = LiveGate::new();
        assert!(gate.accept(WeatherUpdate, P2, 310));
        assert!(!gate.accept(TelemetrySnapshot, P0, 305));
        assert!(gate.accept(TelemetrySnapshot, P0, 311));
        assert!(gate.accept(WeatherUpdate, P2, 312));
    }

    #[test]
    fn whole_state_types_are_gated_across_priorities() {
        // Review finding: a P1 snapshot 250 arriving after a P0 snapshot 300 (the relay serves P0
        // first) found its own (type, P1) slot empty and rolled every channel back.
        let mut gate = LiveGate::new();
        assert!(gate.accept(TelemetrySnapshot, P0, 300));
        assert!(!gate.accept(TelemetrySnapshot, P1, 250), "older snapshot");
        assert!(!gate.accept(TelemetrySnapshot, P2, 300), "same snapshot");
        assert!(gate.accept(TelemetrySnapshot, P1, 301));
        for t in [
            ScoringUpdate,
            WeatherUpdate,
            MsgType::DriverStatus,
            MsgType::RecordingStatus,
        ] {
            let mut gate = LiveGate::new();
            assert!(gate.accept(t, P0, 300), "{t}");
            assert!(
                !gate.accept(t, P1, 250),
                "{t}: older state at another priority"
            );
            assert!(!gate.accept(t, P2, 299), "{t}");
            assert!(gate.accept(t, P2, 301), "{t}");
        }
        // Deltas keep one key per priority: their priorities carry different channels.
        let mut gate = LiveGate::new();
        assert!(gate.accept(TelemetryDelta, P0, 300));
        assert!(gate.accept(TelemetryDelta, P1, 250));
        assert!(gate.accept(TelemetryDelta, P2, 260));
        assert!(!gate.accept(TelemetryDelta, P1, 249));
    }

    #[test]
    fn join_replay_is_applied_but_not_tracked() {
        for order in [
            [
                (SchemaNegotiate, Control, 1),
                (TelemetrySnapshot, P0, 99_990),
                (ScoringUpdate, P0, 99_985),
            ],
            [
                (TelemetrySnapshot, P0, 99_990),
                (SchemaNegotiate, Control, 1),
                (ScoringUpdate, P0, 99_985),
            ],
        ] {
            let mut rx = StreamReceiver::new();
            rx.receive(SessionInfo, Control, 0, false);
            for (t, p, seq) in order {
                let r = rx.receive(t, p, seq, true);
                assert_eq!(
                    r,
                    Reception {
                        arrival: None,
                        live: true
                    },
                    "{t} replay"
                );
            }
            assert_eq!(rx.stats(), SequenceStats::default(), "no false loss");
            let r = rx.receive(TelemetryDelta, P0, 100_051, false);
            assert_eq!(
                r,
                Reception {
                    arrival: Some(Arrival::First),
                    live: true
                }
            );
            assert_eq!(
                rx.receive(TelemetryDelta, P1, 100_052, false).arrival,
                Some(Arrival::InOrder)
            );
            assert_eq!(rx.stats().lost, 0);
        }
    }

    /// `frame` re-encoded with raw type id `id` (CRC recomputed), e.g. a type of a later minor.
    fn with_raw_type(frame: &Frame, id: u8) -> Frame {
        const MSG_TYPE_OFFSET: usize = 4;
        let mut raw = frame.as_bytes().to_vec();
        raw[MSG_TYPE_OFFSET] = id;
        let body = raw.len() - crate::frame::CRC_BYTES;
        let crc = crc32c::crc32c(&raw[..body]);
        raw[body..].copy_from_slice(&crc.to_le_bytes());
        Frame::decode(bytes::Bytes::from(raw)).expect("valid framing")
    }

    #[test]
    fn unknown_stream_class_types_are_sequence_observed_but_not_applied() {
        // Review finding: a 1.1 stream type (e.g. 0x29) consumes a driver sequence number; a 1.0
        // engineer that skipped it unseen counted one false loss per frame, forever.
        let meta = |seq| FrameMeta::stream(P0, 7, seq, 0);
        let delta = |seq| Frame::build(TelemetryDelta, &meta(seq), &[]).expect("build");
        let mut rx = StreamReceiver::new();
        for seq in 1..=30 {
            if seq % 3 == 0 {
                let future = with_raw_type(&delta(seq), 0x29);
                assert_eq!(
                    rx.receive_frame(&future),
                    Err(HeaderRuleError::UnknownMsgType(0x29)),
                    "skipped, never applied"
                );
            } else {
                assert!(rx.receive_frame(&delta(seq)).expect("rules").live);
            }
        }
        let s = rx.stats();
        assert_eq!((s.lost, s.received, s.highest), (0, 30, Some(30)));

        // Outside the reserved range, replayed, or breaking the stream rules: not observed.
        let before = rx.stats();
        for id in [0x19, 0x30, 0x40] {
            assert_eq!(
                rx.observe_unknown(with_raw_type(&delta(31), id).header(), false),
                None
            );
        }
        assert_eq!(
            rx.observe_unknown(with_raw_type(&delta(31), 0x2F).header(), true),
            None
        );
        let mut header = *with_raw_type(&delta(31), 0x2F).header();
        header.sequence = CONTROL_SEQUENCE;
        assert_eq!(rx.observe_unknown(&header, false), None);
        header.sequence = 31;
        header.session_id = NO_SESSION;
        assert_eq!(rx.observe_unknown(&header, false), None);
        assert_eq!(
            rx.observe_unknown(delta(31).header(), false),
            None,
            "known types go through receive"
        );
        assert_eq!(rx.stats(), before);
        header.session_id = 7;
        assert_eq!(rx.observe_unknown(&header, false), Some(Arrival::InOrder));
    }

    #[test]
    fn replay_flag_is_read_from_the_relay_trailer() {
        let frame =
            Frame::build(TelemetrySnapshot, &FrameMeta::stream(P0, 7, 500, 0), &[]).expect("build");
        let replayed = frame.with_relay_trailer(&RelayTrailer {
            replayed: true,
            ..RelayTrailer::default()
        });
        let mut rx = StreamReceiver::new();
        let r = rx.receive_frame(&replayed).expect("rules");
        assert_eq!(
            r,
            Reception {
                arrival: None,
                live: true
            }
        );
        let r = rx
            .receive_frame(&frame.with_relay_trailer(&RelayTrailer::default()))
            .expect("rules");
        assert_eq!(
            r,
            Reception {
                arrival: Some(Arrival::First),
                live: false
            },
            "already applied"
        );
        let ping = Frame::build(MsgType::Ping, &FrameMeta::control(0, 0), &[]).expect("build");
        assert_eq!(
            rx.receive_frame(&ping),
            Ok(Reception {
                arrival: None,
                live: true
            })
        );
    }

    #[test]
    fn driver_restart_is_detected_without_session_info() {
        let mut rx = StreamReceiver::new();
        for seq in 249_990..=250_000 {
            assert!(live(&mut rx, TelemetryDelta, P0, seq));
        }
        // The new space's seq 1 was lost: the run detector has to find the restart.
        let run = u64::from(RESTART_DETECT_RUN);
        let first_seen = FIRST_SEQUENCE + 1;
        for seq in first_seen..first_seen + run - 1 {
            let r = rx.receive(TelemetryDelta, P0, seq, false);
            assert_eq!(
                r,
                Reception {
                    arrival: Some(Arrival::TooOld),
                    live: false
                }
            );
        }
        let last = first_seen + run - 1;
        let r = rx.receive(TelemetryDelta, P0, last, false);
        assert_eq!(
            r,
            Reception {
                arrival: Some(Arrival::Restart),
                live: true
            }
        );
        assert_eq!(
            rx.receive(TelemetryDelta, P0, last + 1, false).arrival,
            Some(Arrival::InOrder)
        );
        assert!(live(&mut rx, TelemetryDelta, P0, last + 2));
        assert_eq!(rx.stats().restarts, 1);
        assert_eq!(rx.stats().highest, Some(last + 2));
    }

    #[test]
    fn first_sequence_below_the_highest_is_an_immediate_restart() {
        // Review finding: the driver restarts after about 20 frames and the relay fails to send
        // SESSION_INFO first. The new space's 1..=20 used to be duplicates (a run shorter than
        // RESTART_DETECT_RUN), 21 InOrder, and the new SCHEMA_NEGOTIATE (seq 1) and early events
        // were dropped by the gate as duplicates of the old space's.
        let old_highest = 20;
        let mut rx = StreamReceiver::new();
        assert!(live(&mut rx, SchemaNegotiate, Control, 1));
        for seq in 2..=old_highest {
            assert!(live(&mut rx, SessionEvent, Control, seq));
        }
        let r = rx.receive(SchemaNegotiate, Control, FIRST_SEQUENCE, false);
        assert_eq!(
            r,
            Reception {
                arrival: Some(Arrival::Restart),
                live: true
            },
            "the new capability map is applied"
        );
        assert!(
            live(&mut rx, SessionEvent, Control, 2),
            "the new space's events are delivered"
        );
        let s = rx.stats();
        assert_eq!(
            (s.restarts, s.highest, s.duplicated, s.lost),
            (1, Some(2), 0, 0)
        );
        // The same through the TooOld path (old highest far above the window) and the Late path
        // (tracking of the old space began above seq 1, e.g. after a join replay).
        for first_tracked in [250_000, 5] {
            let mut t = SequenceTracker::new();
            t.observe(first_tracked);
            t.observe(first_tracked + 15);
            assert_eq!(t.observe(FIRST_SEQUENCE), Arrival::Restart);
            assert_eq!(t.observe(FIRST_SEQUENCE + 1), Arrival::InOrder);
            assert_eq!(t.stats().restarts, 1);
        }
    }

    #[test]
    fn restart_of_a_short_stream_is_detected_through_duplicates() {
        // The new space's seq 1 was lost; its next frames are duplicates of the old space's.
        let run = u64::from(RESTART_DETECT_RUN);
        let old_highest = REORDER_WINDOW - 1;
        let first_seen = FIRST_SEQUENCE + 1;
        assert!(
            first_seen + run - 1 <= old_highest,
            "the new space must stay inside the seen window"
        );
        let mut rx = StreamReceiver::new();
        for seq in 1..=old_highest {
            assert!(live(&mut rx, TelemetryDelta, P0, seq));
        }
        for seq in first_seen..first_seen + run - 1 {
            assert_eq!(
                rx.receive(TelemetryDelta, P0, seq, false).arrival,
                Some(Arrival::Duplicate)
            );
        }
        assert_eq!(
            rx.receive(TelemetryDelta, P0, first_seen + run - 1, false)
                .arrival,
            Some(Arrival::Restart)
        );
    }

    #[test]
    fn draining_every_latest_wins_key_never_looks_like_a_restart() {
        // 6 latest-wins types × P0–P2 (§2); RESTART_DETECT_RUN > LATEST_WINS_KEYS is a const
        // assertion next to the constants.
        assert_eq!(LATEST_WINS_KEYS, 18);
        let mut t = SequenceTracker::new();
        for round in 0..4u64 {
            // After a stall the relay writes one fresh frame, then the newest frame of every other
            // key. Rarely sent keys are far older than the window; increasing order is the worst
            // case for the run detector.
            let fresh = 100_000 + round * 10_000;
            assert!(t.observe(fresh).advances_highest());
            for k in 1..u64::from(LATEST_WINS_KEYS) {
                assert_eq!(t.observe(fresh - 1_000 + k), Arrival::TooOld);
            }
        }
        assert_eq!(t.stats().restarts, 0);
    }

    #[test]
    fn session_info_resets_the_stream() {
        let mut rx = StreamReceiver::new();
        assert!(live(&mut rx, TelemetryDelta, P0, 250_000));
        rx.receive(SessionInfo, Control, 0, false);
        let r = rx.receive(TelemetryDelta, P0, 1, false);
        assert_eq!(
            r,
            Reception {
                arrival: Some(Arrival::First),
                live: true
            }
        );
    }

    #[test]
    fn no_false_restart_from_isolated_stale_frames() {
        let mut t = SequenceTracker::new();
        t.observe(1_000);
        // (Stale sequences start at 2: a stale seq 1 is a restart by definition.)
        for k in 0..u64::from(RESTART_DETECT_RUN) * 2 {
            // A stale frame followed by a fresh one never forms a run.
            assert_eq!(t.observe(2 + k), Arrival::TooOld);
            assert!(t.observe(1_001 + k).advances_highest());
        }
        // Decreasing stale sequences do not form a run either.
        for seq in (2..=u64::from(RESTART_DETECT_RUN) * 2).rev() {
            assert_ne!(t.observe(seq), Arrival::Restart);
        }
        assert_eq!(t.stats().restarts, 0);
    }

    #[test]
    fn snapshot_requests_are_not_tracked_and_are_rate_limited() {
        let mut rx = StreamReceiver::new();
        // Two engineers, two sequence spaces, both starting at 1.
        for _ in 0..2 {
            let r = rx.receive(MsgType::SnapshotRequest, Control, 1, false);
            assert_eq!(
                r,
                Reception {
                    arrival: None,
                    live: true
                }
            );
        }
        let mut lim = SnapshotRequestLimiter::default();
        let min = crate::timing::SNAPSHOT_REQUEST_MIN_INTERVAL_MS;
        assert!(lim.permit(0));
        assert!(!lim.permit(1));
        assert!(!lim.permit(min - 1));
        assert!(lim.permit(min));
        lim.note_sent(10 * min);
        assert!(
            !lim.permit(10 * min + 1),
            "coalesced into the periodic snapshot"
        );
        assert!(lim.permit(11 * min));
        assert!(lim.permit(5), "clock going backwards permits");
    }
}

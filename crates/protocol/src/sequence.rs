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
    Delivery, Frame, FramePriority, HeaderRuleError, MsgType, STREAM_TYPE_FIRST, STREAM_TYPE_LAST,
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
    pub fn observe(&mut self, seq: u64) -> Arrival {
        let arrival = self.classify(seq);
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
                    let restarts = self.stats.restarts.saturating_add(1);
                    self.reset();
                    self.classify(seq);
                    self.stats.restarts = restarts;
                    return Arrival::Restart;
                }
            }
            _ => self.stale_run = 0,
        }
        arrival
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
/// Number of stream (telemetry-class) message types, 0x20–0x28.
const STREAM_TYPES: usize = (STREAM_TYPE_LAST - STREAM_TYPE_FIRST + 1) as usize;

/// Live-state gate for one inbound stream (§5): "accept only if newer" **per latest-wins key**
/// `(msg_type, priority)`, matching the relay's per-key slots and reliable queue (§2).
///
/// * A frame is accepted if its sequence is above the last one accepted for its key. Frames of one
///   key never overtake each other on the way, so for the reliable stream types (SCHEMA_NEGOTIATE,
///   SESSION_EVENT, which travel in the FIFO control queue) this rejects exactly the duplicates.
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
    /// Last accepted sequence per `[stream type][priority slot]`.
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
    fn slot(msg_type: MsgType, priority: FramePriority) -> Option<(usize, usize)> {
        if !msg_type.is_stream() {
            return None;
        }
        let t = usize::from(msg_type.id() - STREAM_TYPE_FIRST);
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

    /// Checks the header rules of `frame` and classifies it. A rule violation is returned (skip and
    /// count the frame) and leaves the receiver unchanged.
    pub fn receive_frame(&mut self, frame: &Frame) -> Result<Reception, HeaderRuleError> {
        let msg_type = frame.check_rules()?;
        let header = frame.header();
        let replayed = frame.relay_trailer().is_some_and(|t| t.replayed);
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
        let (t, a) = run(&[1, 2, 2, 3, 1, 3]);
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
        let run = u64::from(RESTART_DETECT_RUN);
        for seq in 1..run {
            let r = rx.receive(TelemetryDelta, P0, seq, false);
            assert_eq!(
                r,
                Reception {
                    arrival: Some(Arrival::TooOld),
                    live: false
                }
            );
        }
        let r = rx.receive(TelemetryDelta, P0, run, false);
        assert_eq!(
            r,
            Reception {
                arrival: Some(Arrival::Restart),
                live: true
            }
        );
        assert_eq!(
            rx.receive(TelemetryDelta, P0, run + 1, false).arrival,
            Some(Arrival::InOrder)
        );
        assert!(live(&mut rx, TelemetryDelta, P0, run + 2));
        assert_eq!(rx.stats().restarts, 1);
        assert_eq!(rx.stats().highest, Some(run + 2));
    }

    #[test]
    fn restart_of_a_short_stream_is_detected_through_duplicates() {
        let run = u64::from(RESTART_DETECT_RUN);
        let old_highest = REORDER_WINDOW - 1;
        assert!(
            run <= old_highest,
            "the new space must stay inside the seen window"
        );
        let mut rx = StreamReceiver::new();
        for seq in 1..=old_highest {
            assert!(live(&mut rx, TelemetryDelta, P0, seq));
        }
        for seq in 1..run {
            assert_eq!(
                rx.receive(TelemetryDelta, P0, seq, false).arrival,
                Some(Arrival::Duplicate)
            );
        }
        assert_eq!(
            rx.receive(TelemetryDelta, P0, run, false).arrival,
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
        for k in 0..u64::from(RESTART_DETECT_RUN) * 2 {
            // A stale frame followed by a fresh one never forms a run.
            assert_eq!(t.observe(1 + k), Arrival::TooOld);
            assert!(t.observe(1_001 + k).advances_highest());
        }
        // Decreasing stale sequences do not form a run either.
        for seq in (1..=u64::from(RESTART_DETECT_RUN) * 2).rev() {
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

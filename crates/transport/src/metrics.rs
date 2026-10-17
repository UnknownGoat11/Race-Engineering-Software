//! Link metrics (bytes/s, frames/s, RTT, jitter, clock offset and uncertainty, reconnects,
//! dropped_obsolete, sequence statistics), published once per second through a `watch` channel.
//!
//! Every rate is `None` until it was measured over a full interval: "not measured" is never shown
//! as 0. Totals are cumulative over the lifetime of the client (across reconnects); sequence totals
//! are cumulative across the stream resets that every SESSION_INFO causes (PROTOCOL.md §5).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use protocol::clock::ClockEstimate;
use protocol::sequence::SequenceStats;
use tokio::time::Instant;

use crate::transport::TransportStats;

/// Jitter smoothing divisor (RFC 3550 §6.4.1: J += (|D| − J) / 16).
const JITTER_DIVISOR: f64 = 16.0;
/// Shortest interval over which a rate is computed.
const MIN_RATE_INTERVAL: Duration = Duration::from_millis(100);

/// Cumulative sequence counters of the inbound stream across stream resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SequenceTotals {
    pub received: u64,
    /// Sequence numbers missing at the end of each stream epoch plus those missing now.
    pub lost: u64,
    pub duplicated: u64,
    pub out_of_order: u64,
    pub too_old: u64,
    pub restarts: u64,
}

impl SequenceTotals {
    fn add(&self, s: &SequenceStats) -> SequenceTotals {
        SequenceTotals {
            received: self.received.saturating_add(s.received),
            lost: self.lost.saturating_add(s.lost),
            duplicated: self.duplicated.saturating_add(s.duplicated),
            out_of_order: self.out_of_order.saturating_add(s.out_of_order),
            too_old: self.too_old.saturating_add(s.too_old),
            restarts: self.restarts.saturating_add(s.restarts),
        }
    }
}

/// Snapshot of the link metrics.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkMetrics {
    pub bytes_in_per_s: Option<f64>,
    pub bytes_out_per_s: Option<f64>,
    pub frames_in_per_s: Option<f64>,
    pub frames_out_per_s: Option<f64>,
    pub bytes_in_total: u64,
    pub bytes_out_total: u64,
    pub frames_in_total: u64,
    pub frames_out_total: u64,
    /// Round-trip time of the latest PING/PONG (t3 − t0).
    pub rtt: Option<Duration>,
    /// Smoothed RTT (RFC 6298 α = 1/8).
    pub rtt_smoothed: Option<Duration>,
    /// Smallest RTT seen.
    pub rtt_min: Option<Duration>,
    /// Smoothed variation of consecutive RTT samples (RFC 3550 style).
    pub rtt_jitter: Option<Duration>,
    /// Inter-arrival jitter of the inbound telemetry stream (RFC 3550 §6.4.1; engineer only).
    pub interarrival_jitter: Option<Duration>,
    /// Clock offset to the relay and its uncertainty (δ_min / 2), PROTOCOL.md §8.
    pub clock: Option<ClockEstimate>,
    /// Connections re-established after a loss.
    pub reconnects: u64,
    /// Live frames replaced in a latest-wins slot or superseded by a snapshot before they were
    /// written (D-009), or discarded unsent because their connection ended (a fresh snapshot
    /// follows every reconnect).
    pub dropped_obsolete: u64,
    /// Reliable messages (SCHEMA_NEGOTIATE, SESSION_EVENT, code requests, kicks) discarded unsent
    /// because streaming stopped or a new session replaced the old one.
    pub reliable_discarded: u64,
    /// Frames waiting in the outbound queue (reliable queue plus occupied latest-wins slots).
    pub send_queue_depth: usize,
    /// Sequence counters of the current inbound stream (engineer), `None` for the driver.
    pub sequence: Option<SequenceStats>,
    /// Sequence counters accumulated across stream resets (engineer).
    pub sequence_totals: SequenceTotals,
    /// Live frames the relay dropped for this receiver (sum of `relay_dropped` trailers).
    pub relay_dropped: u64,
    /// Inbound frames ignored by role filtering (e.g. the driver's engineer-direction whitelist).
    pub ignored_frames: u64,
    /// Inbound frames skipped for header-rule violations (PROTOCOL.md §1).
    pub header_rule_violations: u64,
    /// Inbound frames whose payload could not be decoded.
    pub decode_errors: u64,
    /// Inbound stream frames refused by the live gate (older than what was applied).
    pub not_live: u64,
    /// Outbound frames refused because they exceed the relay's payload limit.
    pub oversize_dropped: u64,
    /// Outbound messages dropped by the writer because they could not be encoded (producers are
    /// normally told up front with `QueueError::Unencodable` / `QueueError::TooLarge`).
    pub unencodable_dropped: u64,
    /// Application events dropped because the event channel was full, or (engineer) because the
    /// buffer of reliable stream messages held before the identity was confirmed overflowed.
    pub events_dropped: u64,
    /// SNAPSHOT_REQUESTs coalesced by the minimum interval (driver).
    pub snapshot_requests_coalesced: u64,
    /// How long the current session connection has been up.
    pub connected_for: Option<Duration>,
    /// How long since the latest live telemetry stream frame arrived (engineer only; H1).
    pub last_stream_frame_age: Option<Duration>,
    /// Whether the writer is holding stream frames until SCHEMA_NEGOTIATE is written (H2).
    pub awaiting_schema: bool,
}

/// Counters shared with the writer task.
#[derive(Debug, Default)]
pub struct WriterCounters {
    frames_out: AtomicU64,
    oversize_dropped: AtomicU64,
    unencodable_dropped: AtomicU64,
}

impl WriterCounters {
    /// Counts one written frame.
    pub fn frame_written(&self) {
        self.frames_out.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts one frame refused for its size; returns the new total.
    pub fn oversize(&self) -> u64 {
        self.oversize_dropped
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    /// Counts one message that could not be encoded; returns the new total.
    pub fn unencodable(&self) -> u64 {
        self.unencodable_dropped
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    /// Messages that could not be encoded so far.
    pub fn unencodable_dropped(&self) -> u64 {
        self.unencodable_dropped.load(Ordering::Relaxed)
    }

    /// Frames written so far.
    pub fn frames_out(&self) -> u64 {
        self.frames_out.load(Ordering::Relaxed)
    }

    /// Frames refused for their size so far.
    pub fn oversize_dropped(&self) -> u64 {
        self.oversize_dropped.load(Ordering::Relaxed)
    }
}

/// Rate of a cumulative counter, measured between two observations.
#[derive(Debug, Clone, Copy, Default)]
struct RateMeter {
    previous: Option<(Instant, u64)>,
    rate: Option<f64>,
}

impl RateMeter {
    fn observe(&mut self, now: Instant, total: u64) -> Option<f64> {
        match self.previous {
            Some((at, count)) => {
                let elapsed = now.saturating_duration_since(at);
                if elapsed >= MIN_RATE_INTERVAL {
                    self.rate = Some(total.saturating_sub(count) as f64 / elapsed.as_secs_f64());
                    self.previous = Some((now, total));
                }
            }
            None => self.previous = Some((now, total)),
        }
        self.rate
    }
}

/// RFC 3550-style jitter: J += (|D| − J) / 16.
#[derive(Debug, Clone, Copy, Default)]
struct Jitter {
    j_ns: Option<f64>,
}

impl Jitter {
    fn add(&mut self, d_ns: f64) {
        let d = d_ns.abs();
        self.j_ns = Some(match self.j_ns {
            None => d,
            Some(j) => j + (d - j) / JITTER_DIVISOR,
        });
    }

    fn value(&self) -> Option<Duration> {
        self.j_ns.map(|j| Duration::from_nanos(j.max(0.0) as u64))
    }
}

/// Metrics bookkeeping of one client (owned by its supervisor task).
#[derive(Debug, Default)]
pub struct MetricsState {
    m: LinkMetrics,
    connection: Option<Arc<TransportStats>>,
    connection_since: Option<Instant>,
    /// Bytes of earlier connections.
    base_in: u64,
    base_out: u64,
    bytes_in_rate: RateMeter,
    bytes_out_rate: RateMeter,
    frames_in_rate: RateMeter,
    frames_out_rate: RateMeter,
    rtt_jitter: Jitter,
    interarrival_jitter: Jitter,
    last_arrival: Option<(u64, u64)>,
    last_stream_frame_at: Option<Instant>,
    sequence_base: SequenceTotals,
}

impl MetricsState {
    /// Empty metrics.
    pub fn new() -> Self {
        Self::default()
    }

    /// A connection's session became usable.
    pub fn connection_started(&mut self, stats: Arc<TransportStats>, now: Instant) {
        self.connection_ended();
        self.connection = Some(stats);
        self.connection_since = Some(now);
        self.last_arrival = None;
    }

    /// The current connection ended: its byte counts move into the totals, and the link rates go
    /// back to "not measured" (`None`) until the next connection measured them: a rate of the
    /// lost connection must not look current during a reconnect or backoff.
    pub fn connection_ended(&mut self) {
        if let Some(stats) = self.connection.take() {
            self.base_in = self.base_in.saturating_add(stats.bytes_in());
            self.base_out = self.base_out.saturating_add(stats.bytes_out());
        }
        self.connection_since = None;
        self.bytes_in_rate = RateMeter::default();
        self.bytes_out_rate = RateMeter::default();
        self.frames_in_rate = RateMeter::default();
        self.frames_out_rate = RateMeter::default();
        // L1: clear RTT and jitter of the lost connection so stale values don't look current.
        self.m.rtt = None;
        self.m.rtt_smoothed = None;
        self.m.rtt_jitter = None;
    }

    /// Counts a reconnect.
    pub fn reconnected(&mut self) {
        self.m.reconnects = self.m.reconnects.saturating_add(1);
    }

    /// Counts one inbound frame.
    pub fn frame_in(&mut self) {
        self.m.frames_in_total = self.m.frames_in_total.saturating_add(1);
    }

    /// Records a PONG round trip.
    pub fn rtt(&mut self, rtt: Duration, smoothed: Option<Duration>) {
        if let Some(previous) = self.m.rtt {
            self.rtt_jitter
                .add(rtt.as_nanos() as f64 - previous.as_nanos() as f64);
        }
        self.m.rtt = Some(rtt);
        self.m.rtt_smoothed = smoothed;
        self.m.rtt_min = Some(self.m.rtt_min.map_or(rtt, |m| m.min(rtt)));
        self.m.rtt_jitter = self.rtt_jitter.value();
    }

    /// Records the arrival of a live stream frame: local receive time and the sender's
    /// `source_ts_ns`, both in ns (different clocks; only differences are used).
    pub fn stream_arrival(&mut self, received_ns: u64, source_ns: u64) {
        if let Some((r0, s0)) = self.last_arrival {
            let d = (i128::from(received_ns) - i128::from(r0))
                - (i128::from(source_ns) - i128::from(s0));
            self.interarrival_jitter.add(d as f64);
            self.m.interarrival_jitter = self.interarrival_jitter.value();
        }
        self.last_arrival = Some((received_ns, source_ns));
    }

    /// Records the arrival of a live telemetry stream frame at `now` (H1).
    pub fn stream_frame_in(&mut self, now: Instant) {
        self.last_stream_frame_at = Some(now);
    }

    /// How long since the latest live telemetry stream frame arrived at `now` (H1).
    pub fn last_stream_frame_age(&self, now: Instant) -> Option<Duration> {
        self.last_stream_frame_at
            .map(|at| now.saturating_duration_since(at))
    }

    /// The inbound stream was reset (SESSION_INFO): folds its counters into the totals.
    pub fn sequence_reset(&mut self, ended: &SequenceStats) {
        self.sequence_base = self.sequence_base.add(ended);
        self.last_arrival = None;
    }

    /// Adds relay-dropped frames from a trailer.
    pub fn relay_dropped(&mut self, n: u32) {
        self.m.relay_dropped = self.m.relay_dropped.saturating_add(u64::from(n));
    }

    /// Counts an ignored inbound frame.
    pub fn ignored(&mut self) {
        self.m.ignored_frames = self.m.ignored_frames.saturating_add(1);
    }

    /// Counts a header-rule violation.
    pub fn header_rule_violation(&mut self) {
        self.m.header_rule_violations = self.m.header_rule_violations.saturating_add(1);
    }

    /// Counts an undecodable payload.
    pub fn decode_error(&mut self) {
        self.m.decode_errors = self.m.decode_errors.saturating_add(1);
    }

    /// Counts a frame refused by the live gate.
    pub fn not_live(&mut self) {
        self.m.not_live = self.m.not_live.saturating_add(1);
    }

    /// Counts a dropped application event.
    pub fn event_dropped(&mut self) {
        self.m.events_dropped = self.m.events_dropped.saturating_add(1);
    }

    /// Counts a coalesced SNAPSHOT_REQUEST.
    pub fn snapshot_request_coalesced(&mut self) {
        self.m.snapshot_requests_coalesced = self.m.snapshot_requests_coalesced.saturating_add(1);
    }

    /// Sequence totals across stream resets, including the current stream's counters.
    pub fn sequence_totals(&self, current: Option<&SequenceStats>) -> SequenceTotals {
        match current {
            Some(current) => self.sequence_base.add(current),
            None => self.sequence_base,
        }
    }

    /// Builds the snapshot to publish.
    pub fn snapshot(&mut self, now: Instant, inputs: &MetricsInputs) -> LinkMetrics {
        let (cur_in, cur_out) = self
            .connection
            .as_ref()
            .map_or((0, 0), |s| (s.bytes_in(), s.bytes_out()));
        self.m.bytes_in_total = self.base_in.saturating_add(cur_in);
        self.m.bytes_out_total = self.base_out.saturating_add(cur_out);
        self.m.frames_out_total = inputs.frames_out_total;
        if self.connection.is_some() {
            self.m.bytes_in_per_s = self.bytes_in_rate.observe(now, self.m.bytes_in_total);
            self.m.bytes_out_per_s = self.bytes_out_rate.observe(now, self.m.bytes_out_total);
            self.m.frames_in_per_s = self.frames_in_rate.observe(now, self.m.frames_in_total);
            self.m.frames_out_per_s = self.frames_out_rate.observe(now, self.m.frames_out_total);
        } else {
            // No link: no rate (not a measured 0, and not the last connection's value). The
            // meters start afresh with the next connection.
            self.m.bytes_in_per_s = None;
            self.m.bytes_out_per_s = None;
            self.m.frames_in_per_s = None;
            self.m.frames_out_per_s = None;
        }
        self.m.clock = inputs.clock;
        self.m.dropped_obsolete = inputs.dropped_obsolete;
        self.m.reliable_discarded = inputs.reliable_discarded;
        self.m.send_queue_depth = inputs.send_queue_depth;
        self.m.oversize_dropped = inputs.oversize_dropped;
        self.m.unencodable_dropped = inputs.unencodable_dropped;
        self.m.sequence = inputs.sequence;
        self.m.sequence_totals = self.sequence_totals(inputs.sequence.as_ref());
        self.m.connected_for = self
            .connection_since
            .map(|since| now.saturating_duration_since(since));
        self.m.last_stream_frame_age = self
            .last_stream_frame_at
            .map(|at| now.saturating_duration_since(at));
        self.m.awaiting_schema = inputs.awaiting_schema;
        self.m.clone()
    }
}

/// Values owned elsewhere that go into a [`LinkMetrics`] snapshot.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetricsInputs {
    pub frames_out_total: u64,
    pub oversize_dropped: u64,
    pub unencodable_dropped: u64,
    pub dropped_obsolete: u64,
    pub reliable_discarded: u64,
    pub send_queue_depth: usize,
    pub clock: Option<ClockEstimate>,
    pub sequence: Option<SequenceStats>,
    pub awaiting_schema: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn rates_are_none_until_measured_then_per_second() {
        let mut m = MetricsState::new();
        let t0 = Instant::now();
        let stats = Arc::new(TransportStats::default());
        m.connection_started(Arc::clone(&stats), t0);
        let first = m.snapshot(t0, &MetricsInputs::default());
        assert_eq!(first.bytes_in_per_s, None, "not measured is not zero");
        for _ in 0..50 {
            m.frame_in();
        }
        let second = m.snapshot(t0 + Duration::from_millis(500), &MetricsInputs::default());
        assert_eq!(second.frames_in_per_s, Some(100.0));
        assert_eq!(second.bytes_in_per_s, Some(0.0));
        assert_eq!(second.connected_for, Some(Duration::from_millis(500)));
    }

    #[tokio::test(start_paused = true)]
    async fn rates_of_a_lost_connection_do_not_stay_current_while_reconnecting() {
        let mut m = MetricsState::new();
        let t0 = Instant::now();
        m.connection_started(Arc::new(TransportStats::default()), t0);
        m.snapshot(t0, &MetricsInputs::default());
        for _ in 0..60 {
            m.frame_in();
        }
        m.rtt(Duration::from_millis(25), Some(Duration::from_millis(25)));
        m.stream_frame_in(t0);
        let live = m.snapshot(t0 + Duration::from_secs(1), &MetricsInputs::default());
        assert_eq!(live.frames_in_per_s, Some(60.0));
        assert_eq!(live.rtt, Some(Duration::from_millis(25)));
        assert_eq!(live.last_stream_frame_age, Some(Duration::from_secs(1)));
        // The link drops: the snapshot published at the end (and kept during the backoff) shows
        // no rate instead of the last measured one, and clears RTT (L1).
        m.connection_ended();
        let lost = m.snapshot(t0 + Duration::from_secs(2), &MetricsInputs::default());
        assert_eq!(lost.frames_in_per_s, None);
        assert_eq!(lost.bytes_out_per_s, None);
        assert_eq!(lost.connected_for, None);
        assert_eq!(lost.rtt, None);
        assert_eq!(lost.rtt_smoothed, None);
        assert_eq!(lost.rtt_jitter, None);
        assert_eq!(lost.frames_in_total, 60, "totals are kept");
        assert_eq!(lost.last_stream_frame_age, Some(Duration::from_secs(2)));
        // Later snapshots during the outage (each backoff publishes one) stay unmeasured.
        let later = m.snapshot(t0 + Duration::from_secs(10), &MetricsInputs::default());
        assert_eq!(later.frames_in_per_s, None);
        assert_eq!(later.bytes_in_per_s, None);
    }

    #[test]
    fn jitter_follows_rfc3550_smoothing() {
        let mut j = Jitter::default();
        j.add(16_000.0);
        assert_eq!(j.value(), Some(Duration::from_nanos(16_000)));
        j.add(0.0);
        assert_eq!(j.value(), Some(Duration::from_nanos(15_000)));
    }

    #[tokio::test(start_paused = true)]
    async fn rtt_min_and_jitter() {
        let mut m = MetricsState::new();
        m.rtt(Duration::from_millis(30), Some(Duration::from_millis(30)));
        m.rtt(Duration::from_millis(50), Some(Duration::from_millis(32)));
        m.rtt(Duration::from_millis(20), Some(Duration::from_millis(31)));
        let s = m.snapshot(Instant::now(), &MetricsInputs::default());
        assert_eq!(s.rtt, Some(Duration::from_millis(20)));
        assert_eq!(s.rtt_min, Some(Duration::from_millis(20)));
        assert!(s.rtt_jitter.expect("jitter") > Duration::ZERO);
    }

    #[test]
    fn interarrival_jitter_is_zero_for_a_perfectly_paced_stream() {
        let mut m = MetricsState::new();
        for i in 0..10u64 {
            m.stream_arrival(5_000 + i * 10_000_000, 77 + i * 10_000_000);
        }
        assert_eq!(m.m.interarrival_jitter, Some(Duration::ZERO));
    }

    #[tokio::test(start_paused = true)]
    async fn sequence_totals_accumulate_across_resets() {
        let mut m = MetricsState::new();
        let epoch = SequenceStats {
            received: 10,
            lost: 2,
            ..SequenceStats::default()
        };
        m.sequence_reset(&epoch);
        let current = SequenceStats {
            received: 5,
            lost: 1,
            ..SequenceStats::default()
        };
        let s = m.snapshot(
            Instant::now(),
            &MetricsInputs {
                sequence: Some(current),
                ..MetricsInputs::default()
            },
        );
        assert_eq!(s.sequence_totals.received, 15);
        assert_eq!(s.sequence_totals.lost, 3);
    }
}

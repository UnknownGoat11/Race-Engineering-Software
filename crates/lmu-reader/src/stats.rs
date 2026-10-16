//! Reader statistics: measured rates, frame-gate and lock timing, duplicates and drops.
//!
//! Everything is driven by injected monotonic timestamps (nanoseconds of a [`crate::MonoClock`]), so
//! the logic is deterministic and testable. Rates are never assumed: LMU's update rates are
//! UNVERIFIED (A-LMU-001/002) and these meters are how Gate 1 measures them.

use serde::Serialize;

use crate::offsets::EVENT_SLOTS;

/// Nanoseconds per second.
const NS_PER_SEC: u64 = 1_000_000_000;

/// Length of one rate-measurement window.
pub const RATE_WINDOW_NS: u64 = NS_PER_SEC;

/// Lock hold-time budget per frame (research §5.4: ≤ 50 µs).
pub const LOCK_HOLD_BUDGET_NS: u64 = 50_000;

/// Lock hold times above this are logged as warnings (research §5.4: > 1 ms).
pub const LOCK_HOLD_WARN_NS: u64 = 1_000_000;

/// Counts events per fixed window and publishes the rate of the last closed window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct RateMeter {
    total: u64,
    window_start_ns: Option<u64>,
    window_count: u64,
    rate_hz: Option<f64>,
}

impl RateMeter {
    /// Records one event at `now_ns`.
    pub fn record(&mut self, now_ns: u64) {
        self.roll(now_ns);
        if self.window_start_ns.is_none() {
            self.window_start_ns = Some(now_ns);
        }
        self.window_count += 1;
        self.total += 1;
    }

    /// Closes the current window once it is at least [`RATE_WINDOW_NS`] old, so the rate decays to
    /// zero while no events arrive. Call regularly (the reader calls it on every poll).
    pub fn roll(&mut self, now_ns: u64) {
        let Some(start) = self.window_start_ns else {
            return;
        };
        let elapsed = now_ns.saturating_sub(start);
        if elapsed >= RATE_WINDOW_NS {
            self.rate_hz = Some(self.window_count as f64 * NS_PER_SEC as f64 / elapsed as f64);
            self.window_start_ns = Some(now_ns);
            self.window_count = 0;
        }
    }

    /// Events per second over the last closed window (`None` until one window has closed).
    pub fn rate_hz(&self) -> Option<f64> {
        self.rate_hz
    }

    /// Total events recorded.
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Upper bounds (inclusive, ns) of the [`DurationStats`] histogram buckets; one more open-ended
/// bucket follows the last bound.
pub const DURATION_BUCKET_BOUNDS_NS: [u64; 15] = [
    1_000,
    2_000,
    5_000,
    10_000,
    25_000,
    50_000,
    100_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    10_000_000,
    50_000_000,
    100_000_000,
];

/// Number of histogram buckets (bounded ones plus the open-ended one).
const DURATION_BUCKETS: usize = DURATION_BUCKET_BOUNDS_NS.len() + 1;

/// Count, min / mean / max and a fixed log-scale histogram of durations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct DurationStats {
    count: u64,
    total_ns: u64,
    min_ns: Option<u64>,
    max_ns: Option<u64>,
    last_ns: Option<u64>,
    buckets: [u64; DURATION_BUCKETS],
}

impl DurationStats {
    /// Records one duration.
    pub fn record(&mut self, ns: u64) {
        self.count += 1;
        self.total_ns = self.total_ns.saturating_add(ns);
        self.min_ns = Some(self.min_ns.map_or(ns, |m| m.min(ns)));
        self.max_ns = Some(self.max_ns.map_or(ns, |m| m.max(ns)));
        self.last_ns = Some(ns);
        let bucket = DURATION_BUCKET_BOUNDS_NS
            .iter()
            .position(|&bound| ns <= bound)
            .unwrap_or(DURATION_BUCKET_BOUNDS_NS.len());
        if let Some(slot) = self.buckets.get_mut(bucket) {
            *slot += 1;
        }
    }

    /// Number of recorded durations.
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Mean in nanoseconds.
    pub fn mean_ns(&self) -> Option<f64> {
        (self.count > 0).then(|| self.total_ns as f64 / self.count as f64)
    }

    /// Shortest recorded duration.
    pub fn min_ns(&self) -> Option<u64> {
        self.min_ns
    }

    /// Longest recorded duration.
    pub fn max_ns(&self) -> Option<u64> {
        self.max_ns
    }

    /// Most recent duration.
    pub fn last_ns(&self) -> Option<u64> {
        self.last_ns
    }

    /// `(inclusive upper bound, count)` per bucket; the last bucket has no bound.
    pub fn buckets(&self) -> impl Iterator<Item = (Option<u64>, u64)> + '_ {
        self.buckets
            .iter()
            .enumerate()
            .map(|(i, &count)| (DURATION_BUCKET_BOUNDS_NS.get(i).copied(), count))
    }

    /// Approximate quantile: the upper bound of the bucket that contains quantile `q` (0..=1),
    /// capped at the maximum seen. `None` while empty.
    pub fn quantile_upper_ns(&self, q: f64) -> Option<u64> {
        let max = self.max_ns?;
        let q = if q.is_finite() {
            q.clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Smallest rank whose cumulative count reaches q of all samples (at least the first sample).
        let target = ((self.count as f64 * q).ceil() as u64).max(1);
        let mut cumulative = 0u64;
        for (bound, count) in self.buckets() {
            cumulative += count;
            if cumulative >= target {
                return Some(bound.map_or(max, |b| b.min(max)));
            }
        }
        Some(max)
    }
}

/// Counters and meters exposed by [`crate::LmuReader::stats`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ReaderStats {
    /// Successful attaches to `LMU_Data`.
    pub connects: u64,
    /// Detaches (LMU exit, stale data, incompatibility).
    pub disconnects: u64,
    /// Attach attempts that failed with an unexpected OS error (not "LMU not running").
    pub connect_failures: u64,
    /// Frame-gate waits that returned "new frame".
    pub gate_wakeups: u64,
    /// Frame-gate waits that timed out.
    pub gate_timeouts: u64,
    /// Frame-gate waits that failed (`WAIT_FAILED`).
    pub gate_failures: u64,
    /// Time spent in successful frame-gate waits.
    pub gate_wait: DurationStats,
    /// New frames delivered after a frame-gate signal.
    pub gate_new_frames: u64,
    /// New frames found by a witness read while the gate stayed silent (A-LMU-003: the gate missed
    /// a publish; the frame was copied anyway).
    pub gate_missed_updates: u64,
    /// Connections switched from the frame gate to polling because the gate delivered fewer than
    /// half of the new frames (A-LMU-003).
    pub gate_degradations: u64,
    /// Frame gate or SDK lock opened after attaching (they did not exist yet at attach time).
    pub object_upgrades: u64,
    /// Polls in fallback mode (no frame gate).
    pub polls: u64,
    /// Polls that skipped the copy (and the SDK lock) because the lock-free witness was unchanged
    /// and the keep-alive copy was not due ([`crate::reader::POLL_KEEPALIVE`]).
    pub polls_unchanged: u64,
    /// Frames copied out of `LMU_Data`.
    pub copies: u64,
    /// SDK lock acquisitions.
    pub lock_acquired: u64,
    /// Lock acquisitions abandoned after the bounded spin + wait (the frame was skipped).
    pub lock_timeouts: u64,
    /// Lock waits that failed (`WAIT_FAILED`).
    pub lock_failures: u64,
    /// Time from starting to acquire the lock until it was held.
    pub lock_wait: DurationStats,
    /// Time the lock was held (copy + release). Budget: [`LOCK_HOLD_BUDGET_NS`].
    pub lock_hold: DurationStats,
    /// Lock holds longer than [`LOCK_HOLD_BUDGET_NS`].
    pub lock_hold_over_budget: u64,
    /// Copies made without the lock (witness-verified).
    pub lock_free_copies: u64,
    /// Extra lock-free copy attempts caused by a witness change.
    pub copy_retries: u64,
    /// Lock-free copies whose witness changed on every attempt (frame dropped).
    pub torn_copies: u64,
    /// Copies that failed because a read fell outside the view (frame dropped).
    pub copy_errors: u64,
    /// Frames whose decode failed (frame dropped; indicates a bug).
    pub decode_errors: u64,
    /// Copies byte-identical to the previous frame (not returned).
    pub duplicate_frames: u64,
    /// Frames that failed a structural compatibility check (not decoded).
    pub rejected_frames: u64,
    /// Snapshots returned.
    pub snapshots: u64,
    /// Snapshots with a new player telemetry `mElapsedTime`.
    pub telemetry_updates: u64,
    /// Snapshots with new scoring data.
    pub scoring_updates: u64,
    /// Snapshots in which neither telemetry nor scoring changed (only generic bytes did).
    pub same_et_frames: u64,
    /// Player `mElapsedTime` went backwards (session restart or bad data).
    pub telemetry_et_regressions: u64,
    /// Player ET steps longer than 1.5 × `mDeltaTime` (samples published by LMU but never seen).
    pub telemetry_gaps: u64,
    /// Telemetry samples skipped inside those gaps (`round(step / mDeltaTime) - 1`; gaps of more
    /// than 1,000 samples are discontinuities and only counted in `telemetry_gaps`).
    pub telemetry_samples_skipped: u64,
    /// Distribution of the player `mDeltaTime` (as nanoseconds) of fresh samples (research §6).
    pub delta_time: DurationStats,
    /// Snapshots whose player was found by `mID` in another slot than `playerVehicleIdx` named.
    pub player_found_by_id_frames: u64,
    /// Snapshots whose player could not be verified (player UNAVAILABLE, research §11.4).
    pub player_unverified_frames: u64,
    /// Distinct player telemetry samples per second.
    pub telemetry_rate: RateMeter,
    /// Scoring updates per second.
    pub scoring_rate: RateMeter,
    /// Frame-gate wake-ups per second.
    pub gate_rate: RateMeter,
    /// Snapshots per second.
    pub snapshot_rate: RateMeter,
    /// Occurrences of each `SharedMemoryEvent` code in the `generic.events[]` queue, summed over the
    /// slots of every returned frame (A-LMU-009).
    pub event_code_occurrences: [u64; EVENT_SLOTS],
    /// Unused queue slots (`SME_MAX`, verified at Gate 1 with LMU 1.4.2.0).
    pub empty_event_slots: u64,
    /// Occurrences of codes > `SME_MAX`.
    pub unknown_event_codes: u64,
    /// Capture time of the last returned snapshot.
    pub last_snapshot_ns: Option<u64>,
    /// Capture time of the last telemetry update.
    pub last_telemetry_update_ns: Option<u64>,
    /// Capture time of the last scoring update.
    pub last_scoring_update_ns: Option<u64>,
}

impl ReaderStats {
    /// Rolls every rate meter (call on every poll so rates decay while idle).
    pub fn tick(&mut self, now_ns: u64) {
        self.telemetry_rate.roll(now_ns);
        self.scoring_rate.roll(now_ns);
        self.gate_rate.roll(now_ns);
        self.snapshot_rate.roll(now_ns);
    }

    /// Age of the newest snapshot at `now_ns`.
    pub fn sample_age_ns(&self, now_ns: u64) -> Option<u64> {
        self.last_snapshot_ns.map(|t| now_ns.saturating_sub(t))
    }

    /// Records one lock hold time.
    pub(crate) fn record_lock_hold(&mut self, ns: u64) {
        self.lock_hold.record(ns);
        if ns > LOCK_HOLD_BUDGET_NS {
            self.lock_hold_over_budget += 1;
        }
    }

    /// Counts the codes in one frame's event queue.
    pub(crate) fn record_events(&mut self, events: &[u32]) {
        for &code in events {
            if code == crate::offsets::sme::MAX {
                self.empty_event_slots += 1;
                continue;
            }
            match usize::try_from(code)
                .ok()
                .and_then(|i| self.event_code_occurrences.get_mut(i))
            {
                Some(slot) => *slot += 1,
                None => self.unknown_event_codes += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn rate_meter_counts_per_window_and_decays_when_idle() {
        let mut m = RateMeter::default();
        assert_eq!(m.rate_hz(), None);
        for i in 0..100 {
            m.record(i * 10 * MS); // 100 Hz for one second
        }
        assert_eq!(m.rate_hz(), None, "no window closed yet");
        m.roll(1_000 * MS);
        assert_eq!(m.rate_hz(), Some(100.0));
        m.roll(2_000 * MS);
        assert_eq!(m.rate_hz(), Some(0.0), "an idle window reads 0 Hz");
        assert_eq!(m.total(), 100);
    }

    #[test]
    fn rate_meter_uses_the_actual_window_length() {
        let mut m = RateMeter::default();
        m.record(0);
        for i in 1..=5 {
            m.record(i * 400 * MS); // 2.5 Hz
        }
        // The window closed at 1.2 s with the events at 0, 0.4, 0.8 inside it: 3 / 1.2 s.
        assert_eq!(m.rate_hz(), Some(2.5));
    }

    #[test]
    fn duration_stats_track_extremes_and_quantiles() {
        let mut d = DurationStats::default();
        assert_eq!(d.quantile_upper_ns(0.5), None);
        for ns in [3_000, 4_000, 20_000, 40_000, 2_000_000] {
            d.record(ns);
        }
        assert_eq!(d.count(), 5);
        assert_eq!(d.min_ns(), Some(3_000));
        assert_eq!(d.max_ns(), Some(2_000_000));
        assert_eq!(d.last_ns(), Some(2_000_000));
        assert_eq!(d.mean_ns(), Some(2_067_000.0 / 5.0));
        assert_eq!(d.quantile_upper_ns(0.4), Some(5_000));
        assert_eq!(d.quantile_upper_ns(0.8), Some(50_000));
        assert_eq!(d.quantile_upper_ns(1.0), Some(2_000_000));
        let total: u64 = d.buckets().map(|(_, c)| c).sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn open_ended_bucket_reports_the_maximum() {
        let mut d = DurationStats::default();
        d.record(5_000_000_000);
        assert_eq!(d.quantile_upper_ns(0.99), Some(5_000_000_000));
        assert_eq!(d.buckets().last(), Some((None, 1)));
    }

    #[test]
    fn lock_budget_and_event_counting() {
        let mut s = ReaderStats::default();
        s.record_lock_hold(LOCK_HOLD_BUDGET_NS);
        s.record_lock_hold(LOCK_HOLD_BUDGET_NS + 1);
        assert_eq!(s.lock_hold_over_budget, 1);
        s.record_events(&[11, 0, 0, 99, 16, 16]);
        assert_eq!(s.event_code_occurrences[11], 1);
        assert_eq!(s.event_code_occurrences[0], 2);
        assert_eq!(s.empty_event_slots, 2, "SME_MAX = unused slot (Gate 1)");
        assert_eq!(s.unknown_event_codes, 1);
        s.last_snapshot_ns = Some(10);
        assert_eq!(s.sample_age_ns(25), Some(15));
    }
}

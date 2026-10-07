//! Monotonic time base of the transport.
//!
//! Frame headers (`source_ts_ns`) and PING `t0` carry "sender monotonic ns since the process epoch"
//! (PROTOCOL.md §1, §8). The driver later maps capture times of the LMU reader into relay time with
//! the clock offset measured here (`capture_relay_ns_est = capture_mono_ns + θ`), so both must use
//! the **same epoch**: pass the reader's epoch (`lmu_reader::MonoClock::epoch()`) as
//! [`crate::ClientConfig::mono_epoch`] (a required argument of [`crate::ClientConfig::new`]).
//! [`relay_time_of`] converts an [`std::time::Instant`] itself and needs no shared epoch.
//!
//! The clock reads `tokio::time::Instant`, which is `std::time::Instant` in production and the
//! pausable test clock under `tokio::time::pause`, so heartbeat and backoff logic can be tested
//! deterministically.

use std::time::Duration;

use protocol::clock::ClockEstimate;
use telemetry_model::codec::RELAY_NS_UNKNOWN;
use tokio::time::Instant;

/// Monotonic clock with an explicit epoch; values are nanoseconds since the epoch.
#[derive(Debug, Clone, Copy)]
pub struct LinkClock {
    epoch: Instant,
}

impl LinkClock {
    /// Clock whose epoch is the given `std` instant (e.g. the LMU reader's `MonoClock` epoch).
    pub fn with_epoch(epoch: std::time::Instant) -> Self {
        Self {
            epoch: Instant::from_std(epoch),
        }
    }

    /// Current instant.
    pub fn now(&self) -> Instant {
        Instant::now()
    }

    /// Nanoseconds since the epoch, now.
    pub fn now_ns(&self) -> u64 {
        self.ns_at(Instant::now())
    }

    /// Nanoseconds from the epoch to `t` (0 if `t` precedes the epoch, saturating at `u64::MAX`).
    pub fn ns_at(&self, t: Instant) -> u64 {
        duration_ns(t.saturating_duration_since(self.epoch))
    }

    /// Milliseconds since the epoch, now (for millisecond-based limiters).
    pub fn now_ms(&self) -> u64 {
        u64::try_from(
            Instant::now()
                .saturating_duration_since(self.epoch)
                .as_millis(),
        )
        .unwrap_or(u64::MAX)
    }

    /// Signed nanoseconds from the epoch to `t` (negative if `t` precedes the epoch).
    pub fn signed_ns_at(&self, t: std::time::Instant) -> i128 {
        let t = Instant::from_std(t);
        if t >= self.epoch {
            i128::from(duration_ns(t.duration_since(self.epoch)))
        } else {
            -i128::from(duration_ns(self.epoch.duration_since(t)))
        }
    }
}

/// Relay wall-clock time (ns since the Unix epoch) at the instant `at`, from the clock offset
/// `estimate` measured on `clock` (PROTOCOL.md §8). Works for any instant, before or after the
/// clock's epoch, so callers never have to share that epoch. `None` without an estimate, or if the
/// result does not fit an `i64` or would equal the "unknown" sentinel
/// ([`telemetry_model::codec::RELAY_NS_UNKNOWN`]).
pub fn relay_time_of(
    clock: &LinkClock,
    estimate: Option<ClockEstimate>,
    at: std::time::Instant,
) -> Option<i64> {
    let estimate = estimate?;
    let relay = clock.signed_ns_at(at) + i128::from(estimate.offset_ns);
    i64::try_from(relay)
        .ok()
        .filter(|ns| *ns != RELAY_NS_UNKNOWN)
}

/// A duration in whole nanoseconds, saturating at `u64::MAX` (about 584 years).
pub fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn paused_clock_advances_with_tokio_time() {
        // The epoch must come from the (paused) tokio clock: a fresh `std::time::Instant` is ahead
        // of the paused clock, and times before the epoch saturate at 0.
        let clock = LinkClock::with_epoch(Instant::now().into_std());
        let a = clock.now_ns();
        tokio::time::advance(Duration::from_millis(1_500)).await;
        let b = clock.now_ns();
        assert!(b - a >= 1_500_000_000, "{a} -> {b}");
        assert!(clock.now_ms() >= 1_500);
    }

    #[tokio::test(start_paused = true)]
    async fn relay_time_of_an_instant_is_independent_of_the_clock_epoch() {
        const OFFSET_NS: i64 = 1_790_000_000_000_000_000;
        let estimate = ClockEstimate {
            offset_ns: OFFSET_NS,
            uncertainty_ns: 0,
            min_delay_ns: 0,
            samples: 1,
        };
        let reader_epoch = Instant::now();
        tokio::time::advance(Duration::from_secs(40)).await;
        // The transport's epoch was taken 40 s after the reader's (e.g. when "Start" was pressed).
        let transport_epoch = Instant::now();
        let clock = LinkClock::with_epoch(transport_epoch.into_std());
        tokio::time::advance(Duration::from_secs(1)).await;
        // A capture 5 s after the reader's epoch, i.e. 35 s before the transport's epoch.
        let capture = reader_epoch + Duration::from_secs(5);
        assert_eq!(clock.signed_ns_at(capture.into_std()), -35_000_000_000);
        assert_eq!(
            relay_time_of(&clock, Some(estimate), capture.into_std()),
            Some(OFFSET_NS - 35_000_000_000)
        );
        assert_eq!(
            relay_time_of(&clock, Some(estimate), transport_epoch.into_std()),
            Some(OFFSET_NS)
        );
        assert_eq!(relay_time_of(&clock, None, capture.into_std()), None);
        // The sentinel and out-of-range results are never produced.
        let sentinel = ClockEstimate {
            offset_ns: i64::MIN,
            ..estimate
        };
        assert_eq!(
            relay_time_of(&clock, Some(sentinel), transport_epoch.into_std()),
            None
        );
    }

    #[test]
    fn duration_ns_saturates() {
        assert_eq!(duration_ns(Duration::from_secs(1)), 1_000_000_000);
        assert_eq!(duration_ns(Duration::MAX), u64::MAX);
    }
}

//! Monotonic capture timestamps.
//!
//! Every snapshot carries `capture_mono_ns`: nanoseconds since a [`MonoClock`] epoch, taken from
//! [`std::time::Instant`] right after the bytes were copied. Share one `MonoClock` (it is `Copy`)
//! between the reader and its consumers so that sample ages can be computed in the same time base.

use std::time::{Duration, Instant};

/// A monotonic clock with an explicit `Instant` epoch.
#[derive(Debug, Clone, Copy)]
pub struct MonoClock {
    epoch: Instant,
}

impl MonoClock {
    /// A clock whose epoch is "now".
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }

    /// A clock with the given epoch (e.g. the application start).
    pub fn with_epoch(epoch: Instant) -> Self {
        Self { epoch }
    }

    /// The epoch.
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Nanoseconds since the epoch.
    pub fn now_ns(&self) -> u64 {
        self.ns_at(Instant::now())
    }

    /// Nanoseconds from the epoch to `t` (0 if `t` precedes the epoch, saturating at `u64::MAX`).
    pub fn ns_at(&self, t: Instant) -> u64 {
        duration_ns(t.saturating_duration_since(self.epoch))
    }
}

impl Default for MonoClock {
    fn default() -> Self {
        Self::new()
    }
}

/// A duration in whole nanoseconds, saturating at `u64::MAX` (about 584 years).
pub fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_monotonic_and_relative_to_the_epoch() {
        let clock = MonoClock::new();
        let a = clock.now_ns();
        let b = clock.now_ns();
        assert!(b >= a);
        let later = clock.epoch() + Duration::from_millis(5);
        assert_eq!(clock.ns_at(later), 5_000_000);
        let earlier = MonoClock::with_epoch(later);
        assert_eq!(
            earlier.ns_at(clock.epoch()),
            0,
            "before the epoch saturates to 0"
        );
    }

    #[test]
    fn duration_ns_saturates() {
        assert_eq!(duration_ns(Duration::from_secs(1)), 1_000_000_000);
        assert_eq!(duration_ns(Duration::MAX), u64::MAX);
    }
}

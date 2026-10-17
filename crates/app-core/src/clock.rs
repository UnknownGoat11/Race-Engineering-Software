//! `AppClock`: the single monotonic time base of a run.
//!
//! Wraps `lmu_reader::MonoClock`. Its epoch is passed to transport's `ClientConfig::new(.., mono_epoch)` and
//! to the recorder's `SessionOptions::with_capture_clock`, so capture timestamps from the reader, the network
//! headers and the recording all share one clock. Also hosts the wall-versus-monotonic skew helper used by
//! the sleep/resume check (a skew above 5 s is a discontinuity).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lmu_reader::MonoClock;
use recorder::CaptureClock;

/// Default skew threshold between wall-clock and monotonic time that indicates
/// an OS sleep/resume or clock jump (5 seconds, per blueprint §4).
pub const SKEW_DISCONTINUITY_THRESHOLD: Duration = Duration::from_secs(5);

/// The single monotonic time base of an app-core run.
#[derive(Debug, Clone, Copy)]
pub struct AppClock {
    mono: MonoClock,
}

impl AppClock {
    /// Creates a clock whose epoch is "now".
    pub fn new() -> Self {
        Self {
            mono: MonoClock::new(),
        }
    }

    /// Creates a clock with the given monotonic epoch instant.
    pub fn with_epoch(epoch: Instant) -> Self {
        Self {
            mono: MonoClock::with_epoch(epoch),
        }
    }

    /// Returns the monotonic epoch instant.
    pub fn epoch(&self) -> Instant {
        self.mono.epoch()
    }

    /// Monotonic nanoseconds since the epoch.
    pub fn now_ns(&self) -> u64 {
        self.mono.now_ns()
    }

    /// Monotonic nanoseconds from the epoch to `t`.
    pub fn ns_at(&self, t: Instant) -> u64 {
        self.mono.ns_at(t)
    }

    /// The underlying [`lmu_reader::MonoClock`].
    pub fn mono_clock(&self) -> MonoClock {
        self.mono
    }

    /// Current wall-clock Unix timestamp in seconds.
    pub fn now_unix_s(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Current wall-clock Unix timestamp in nanoseconds.
    pub fn now_unix_ns(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(0)
    }
}

impl Default for AppClock {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureClock for AppClock {
    fn now_ns(&self) -> u64 {
        self.now_ns()
    }
}

/// Outcome of checking wall-versus-monotonic clock alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkewStatus {
    /// Monotonic and wall clocks are advancing normally within tolerance.
    Synchronized {
        /// Absolute detected skew between wall and monotonic elapsed time.
        skew: Duration,
    },
    /// A wall-versus-monotonic drift exceeding the threshold was detected,
    /// indicating an OS sleep/resume or NTP/manual time step.
    Discontinuity {
        /// Magnitude of the detected skew.
        skew: Duration,
        /// Elapsed wall-clock duration since the last sample.
        wall_elapsed: Duration,
        /// Elapsed monotonic duration since the last sample.
        mono_elapsed: Duration,
    },
}

/// Tracks wall-clock versus monotonic drift over time to detect sleep/resume
/// cycles or clock changes.
#[derive(Debug, Clone)]
pub struct SkewTracker {
    last_sample: Option<(SystemTime, Instant)>,
    threshold: Duration,
}

impl SkewTracker {
    /// Creates a new skew tracker with the default 5-second threshold.
    pub fn new() -> Self {
        Self::with_threshold(SKEW_DISCONTINUITY_THRESHOLD)
    }

    /// Creates a new skew tracker with a custom threshold.
    pub fn with_threshold(threshold: Duration) -> Self {
        Self {
            last_sample: None,
            threshold,
        }
    }

    /// Threshold duration above which a drift is classified as a discontinuity.
    pub fn threshold(&self) -> Duration {
        self.threshold
    }

    /// Resets the tracker baseline to "now".
    pub fn reset(&mut self) {
        self.last_sample = Some((SystemTime::now(), Instant::now()));
    }

    /// Evaluates current skew between wall and monotonic clocks.
    ///
    /// The first call establishes the baseline and reports `Synchronized { skew: Duration::ZERO }`.
    /// Subsequent calls compare elapsed wall-clock time against elapsed monotonic time.
    pub fn check(&mut self) -> SkewStatus {
        self.check_at(SystemTime::now(), Instant::now())
    }

    /// Evaluates skew at explicitly provided timestamps (useful for deterministic testing).
    pub fn check_at(&mut self, now_wall: SystemTime, now_mono: Instant) -> SkewStatus {
        let Some((last_wall, last_mono)) = self.last_sample else {
            self.last_sample = Some((now_wall, now_mono));
            return SkewStatus::Synchronized {
                skew: Duration::ZERO,
            };
        };

        // Monotonic elapsed time is always non-negative:
        let mono_elapsed = now_mono.saturating_duration_since(last_mono);

        // Wall elapsed time may be forward or backward (NTP step / DST adjustment):
        let (wall_elapsed, is_wall_forward) = if now_wall >= last_wall {
            (
                now_wall.duration_since(last_wall).unwrap_or(Duration::ZERO),
                true,
            )
        } else {
            (
                last_wall.duration_since(now_wall).unwrap_or(Duration::ZERO),
                false,
            )
        };

        // Update the baseline sample:
        self.last_sample = Some((now_wall, now_mono));

        // Calculate absolute drift between wall progression and monotonic progression:
        let skew = if is_wall_forward {
            wall_elapsed.abs_diff(mono_elapsed)
        } else {
            // Wall moved backward: total skew is mono_elapsed + backward wall jump
            mono_elapsed.saturating_add(wall_elapsed)
        };

        if skew >= self.threshold {
            SkewStatus::Discontinuity {
                skew,
                wall_elapsed,
                mono_elapsed,
            }
        } else {
            SkewStatus::Synchronized { skew }
        }
    }
}

impl Default for SkewTracker {
    fn default() -> Self {
        Self::new()
    }
}

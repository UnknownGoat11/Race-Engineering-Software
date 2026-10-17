//! Relay clocks.
//!
//! Two clocks are used, never mixed:
//! * **wall clock** (`unix_now_ns`, ns since the Unix epoch) for every timestamp that goes on the wire
//!   (PONG t1/t2, relay trailer rx/tx, `expires_at_ns`, `started_at_ns`) and into PostgreSQL;
//! * **monotonic** `std::time::Instant` for every expiry decision inside the relay (code TTL, ticket
//!   TTL, driver grace, rate limiters), so a wall-clock step never expires or revives anything.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};

/// Nanoseconds per second.
pub const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Injectable wall clock (ns since the Unix epoch) for decisions on persisted timestamps (token
/// expiry). Production uses [`WallClock::system`]; tests substitute a manual clock so expiry is
/// tested without sleeping.
#[derive(Clone)]
pub struct WallClock(Arc<dyn Fn() -> u64 + Send + Sync>);

impl WallClock {
    /// The operating system wall clock ([`unix_now_ns`]).
    pub fn system() -> Self {
        Self(Arc::new(unix_now_ns))
    }

    /// A clock reading `f` (tests).
    pub fn from_fn(f: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Current time, ns since the Unix epoch.
    pub fn now_ns(&self) -> u64 {
        (self.0)()
    }

    /// Current time as `DateTime<Utc>`.
    pub fn now_utc(&self) -> DateTime<Utc> {
        ns_to_datetime(self.now_ns())
    }

    /// Current time in whole seconds since the Unix epoch.
    pub fn now_s(&self) -> u64 {
        self.now_ns() / NANOS_PER_SEC
    }
}

impl fmt::Debug for WallClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WallClock({})", self.now_ns())
    }
}

/// Relay wall clock in ns since the Unix epoch. A clock set before 1970 reads as 0.
pub fn unix_now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, duration_ns)
}

/// A duration in ns, saturating at `u64::MAX` (≈ 584 years).
pub fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// A duration in ms, saturating at `u64::MAX`.
pub fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Wall-clock ns → `DateTime<Utc>` (saturating at chrono's maximum).
pub fn ns_to_datetime(ns: u64) -> DateTime<Utc> {
    let secs = i64::try_from(ns / NANOS_PER_SEC).unwrap_or(i64::MAX);
    let sub = u32::try_from(ns % NANOS_PER_SEC).unwrap_or(0);
    DateTime::from_timestamp(secs, sub).unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// `DateTime<Utc>` → wall-clock ns (0 before 1970, saturating far in the future).
pub fn datetime_to_ns(dt: DateTime<Utc>) -> u64 {
    match dt.timestamp_nanos_opt() {
        Some(ns) => u64::try_from(ns).unwrap_or(0),
        None if dt.timestamp() > 0 => u64::MAX,
        None => 0,
    }
}

/// The current wall clock as `DateTime<Utc>`.
pub fn now_utc() -> DateTime<Utc> {
    ns_to_datetime(unix_now_ns())
}

/// Monotonic process clock in ns since `epoch` (used as `source_ts_ns` of relay-originated frames).
pub fn mono_ns_since(epoch: Instant) -> u64 {
    duration_ns(epoch.elapsed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_roundtrip_keeps_nanoseconds() {
        let ns = 1_790_000_000_123_456_789u64;
        assert_eq!(datetime_to_ns(ns_to_datetime(ns)), ns);
    }

    #[test]
    fn wall_clock_is_after_2020() {
        // 2020-01-01T00:00:00Z
        assert!(unix_now_ns() > 1_577_836_800 * NANOS_PER_SEC);
    }

    #[test]
    fn saturating_conversions() {
        assert_eq!(duration_ns(Duration::MAX), u64::MAX);
        assert_eq!(duration_ms(Duration::from_millis(1500)), 1500);
        assert_eq!(datetime_to_ns(DateTime::<Utc>::MIN_UTC), 0);
    }
}

//! Clock helpers: wall clock, the producer's capture clock, and compact UTC formatting for directory
//! names.

use std::time::{SystemTime, UNIX_EPOCH};

/// The producer's capture clock (the clock behind every `capture_mono_ns`). When supplied, each segment
/// header stores `(created_unix_ns, created_mono_ns)` read back-to-back, anchoring capture times to wall
/// time. The recorder never assumes which clock the producer uses.
pub trait CaptureClock: Send + Sync {
    /// Current capture-clock reading in nanoseconds.
    fn now_ns(&self) -> u64;
}

impl<F> CaptureClock for F
where
    F: Fn() -> u64 + Send + Sync,
{
    fn now_ns(&self) -> u64 {
        self()
    }
}

const NANOS_PER_SEC: i64 = 1_000_000_000;
const SECS_PER_DAY: i64 = 86_400;
const SECS_PER_HOUR: i64 = 3_600;
const SECS_PER_MINUTE: i64 = 60;

/// Wall clock in ns since the Unix epoch (negative if the system clock is before 1970; saturating).
pub(crate) fn unix_now_ns() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos())
            .map(|n| -n)
            .unwrap_or(i64::MIN),
    }
}

/// Formats a Unix time as `YYYYMMDDTHHMMSSZ` (UTC), e.g. `20261001T123005Z`.
pub(crate) fn format_utc_compact(unix_ns: i64) -> String {
    let secs = unix_ns.div_euclid(NANOS_PER_SEC);
    let days = secs.div_euclid(SECS_PER_DAY);
    let sod = secs.rem_euclid(SECS_PER_DAY);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        sod / SECS_PER_HOUR,
        (sod % SECS_PER_HOUR) / SECS_PER_MINUTE,
        sod % SECS_PER_MINUTE
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian calendar.
/// Howard Hinnant's `civil_from_days` (public domain): eras of 400 years, March-based years.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    const DAYS_FROM_0000_03_01_TO_EPOCH: i64 = 719_468;
    const DAYS_PER_ERA: i64 = 146_097;
    let z = days + DAYS_FROM_0000_03_01_TO_EPOCH;
    let era = z.div_euclid(DAYS_PER_ERA);
    let doe = z.rem_euclid(DAYS_PER_ERA); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc_compact(0), "19700101T000000Z");
        // 2026-10-01T00:00:00Z = 20 727 days after the epoch.
        assert_eq!(
            format_utc_compact(1_790_812_800 * NANOS_PER_SEC),
            "20261001T000000Z"
        );
        assert_eq!(
            format_utc_compact((1_790_812_800 + 12 * 3600 + 30 * 60 + 5) * NANOS_PER_SEC + 999),
            "20261001T123005Z"
        );
        // Leap day and a pre-epoch instant.
        assert_eq!(
            format_utc_compact(951_782_400 * NANOS_PER_SEC),
            "20000229T000000Z"
        );
        assert_eq!(format_utc_compact(-NANOS_PER_SEC), "19691231T235959Z");
    }

    #[test]
    fn closures_are_capture_clocks() {
        let c = || 42u64;
        assert_eq!(CaptureClock::now_ns(&c), 42);
        assert!(unix_now_ns() > 0);
    }
}

//! Exponential reconnect backoff with jitter (PROTOCOL.md §7: `RECONNECT_MIN_MS` / `RECONNECT_MAX_MS`).
//!
//! Attempt `n` (0-based) waits a uniformly random time in `[d/2, d]` with
//! `d = min(max, min · 2ⁿ)`, never less than `min`. The jitter spreads reconnects of many clients
//! after a relay restart; the cap keeps recovery quick after long outages (sleep/resume, Wi-Fi
//! changes) without hammering the relay.

use std::time::Duration;

use rand::Rng;

/// Highest exponent used; `2^16 · min` exceeds any sensible cap, so larger attempts saturate.
const MAX_EXPONENT: u32 = 16;

/// Backoff state of one reconnect sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    attempt: u32,
}

impl Backoff {
    /// Backoff between `min` and `max` (`max` is raised to `min` if smaller).
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max: max.max(min),
            attempt: 0,
        }
    }

    /// Attempts made since the last [`Backoff::reset`].
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Upper bound `d` of the delay before attempt `attempt`.
    pub fn ceiling(&self, attempt: u32) -> Duration {
        let factor = 1u32 << attempt.min(MAX_EXPONENT);
        self.min.saturating_mul(factor).min(self.max)
    }

    /// Delay before the next attempt, and counts the attempt.
    pub fn next_delay<R: Rng + ?Sized>(&mut self, rng: &mut R) -> Duration {
        let ceiling = self.ceiling(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        let low = ceiling / 2;
        let span = ceiling.saturating_sub(low);
        let jitter = span.mul_f64(rng.random::<f64>());
        (low + jitter).clamp(self.min, self.max)
    }

    /// Starts over (after a connection was healthy for a while).
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    const MIN: Duration = Duration::from_millis(protocol::timing::RECONNECT_MIN_MS);
    const MAX: Duration = Duration::from_millis(protocol::timing::RECONNECT_MAX_MS);

    #[test]
    fn delays_grow_exponentially_within_jitter_bounds_and_cap() {
        let mut rng = StdRng::seed_from_u64(7);
        let mut backoff = Backoff::new(MIN, MAX);
        for attempt in 0..20u32 {
            let ceiling = backoff.ceiling(attempt);
            let d = backoff.next_delay(&mut rng);
            assert!(d >= MIN, "attempt {attempt}: {d:?} < min");
            assert!(d <= MAX, "attempt {attempt}: {d:?} > max");
            assert!(
                d <= ceiling,
                "attempt {attempt}: {d:?} > ceiling {ceiling:?}"
            );
            assert!(
                d >= (ceiling / 2).max(MIN),
                "attempt {attempt}: {d:?} below half the ceiling"
            );
        }
        assert_eq!(backoff.ceiling(0), MIN);
        assert_eq!(backoff.ceiling(1), MIN * 2);
        assert_eq!(backoff.ceiling(4), MIN * 16);
        assert_eq!(
            backoff.ceiling(5),
            MAX,
            "250 ms · 32 = 8 s is capped at 5 s"
        );
        assert_eq!(backoff.ceiling(u32::MAX), MAX);
        assert_eq!(backoff.attempt(), 20);
        backoff.reset();
        assert_eq!(backoff.attempt(), 0);
    }

    #[test]
    fn jitter_actually_varies() {
        let mut rng = StdRng::seed_from_u64(1);
        let delays: Vec<Duration> = (0..32)
            .map(|_| {
                let mut b = Backoff::new(MIN, MAX);
                b.reset();
                for _ in 0..6 {
                    b.next_delay(&mut rng);
                }
                b.next_delay(&mut rng)
            })
            .collect();
        let first = delays[0];
        assert!(delays.iter().any(|d| *d != first), "no jitter: {delays:?}");
    }
}

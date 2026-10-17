//! Deterministic rate limiting (token buckets). Every function takes the current monotonic time
//! as an argument, so the logic is testable without sleeping.
//!
//! * [`TokenBucket`]: `capacity` tokens, refilled continuously at `capacity / period`.
//! * [`WindowLimit`]: several buckets that must all admit (e.g. 5/min **and** 20/hour, D-022).
//! * [`KeyedLimiter`]: one [`WindowLimit`] per key (account, IP, username) in a bounded map.
//! * [`RateMeter`]: packets/s and bytes/s of a connection or session.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

/// A continuously refilled token bucket.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_sec: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// A full bucket holding `capacity` tokens that refills `capacity` tokens per `period`.
    pub fn new(capacity: u64, period: Duration, now: Instant) -> Self {
        // Precision: capacities are far below 2^52, so the f64 conversion is exact.
        let capacity = capacity as f64;
        let secs = period.as_secs_f64();
        let refill_per_sec = if secs > 0.0 {
            capacity / secs
        } else {
            f64::INFINITY
        };
        Self {
            capacity,
            refill_per_sec,
            tokens: capacity,
            last: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        // Callers pass a `now` read before taking their lock, so another thread may already have
        // refilled up to a later instant. An earlier `now` adds nothing and never moves `last`
        // back (that would credit the same interval twice on the next refill).
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last = self.last.max(now);
    }

    /// Whether `cost` tokens are available at `now` (refills first, takes nothing).
    pub fn can_take(&mut self, cost: f64, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= cost
    }

    /// Takes `cost` tokens if available.
    pub fn try_take(&mut self, cost: f64, now: Instant) -> bool {
        if self.can_take(cost, now) {
            self.tokens -= cost;
            true
        } else {
            false
        }
    }

    /// Takes `cost` tokens unconditionally (may go negative: debt is paid back by the refill).
    pub fn take_forced(&mut self, cost: f64, now: Instant) {
        self.refill(now);
        self.tokens -= cost;
    }

    /// Whether the bucket is full (an idle key that can be forgotten).
    pub fn is_full(&mut self, now: Instant) -> bool {
        self.refill(now);
        self.tokens >= self.capacity
    }
}

/// One window of a [`WindowLimit`]: `count` events per `period`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub count: u64,
    pub period: Duration,
}

impl Window {
    /// `count` events per minute.
    pub const fn per_minute(count: u64) -> Self {
        Self {
            count,
            period: Duration::from_secs(60),
        }
    }

    /// `count` events per hour.
    pub const fn per_hour(count: u64) -> Self {
        Self {
            count,
            period: Duration::from_secs(3_600),
        }
    }
}

/// Several token buckets that must all admit an event (e.g. 5 per minute and 20 per hour).
#[derive(Debug, Clone)]
pub struct WindowLimit {
    buckets: Vec<TokenBucket>,
}

impl WindowLimit {
    /// A limit with all buckets full.
    pub fn new(windows: &[Window], now: Instant) -> Self {
        Self {
            buckets: windows
                .iter()
                .map(|w| TokenBucket::new(w.count, w.period, now))
                .collect(),
        }
    }

    /// Admits one event if **every** window has room; takes from all of them only then.
    pub fn try_admit(&mut self, now: Instant) -> bool {
        if self.buckets.iter_mut().all(|b| b.can_take(1.0, now)) {
            for b in &mut self.buckets {
                b.try_take(1.0, now);
            }
            true
        } else {
            false
        }
    }

    fn is_idle(&mut self, now: Instant) -> bool {
        self.buckets.iter_mut().all(|b| b.is_full(now))
    }
}

/// One [`WindowLimit`] per key in a map bounded to `max_keys` entries. When the map is full, idle
/// keys (all buckets full) are evicted; if none is idle the new key is refused (fail closed).
#[derive(Debug)]
pub struct KeyedLimiter<K> {
    windows: Vec<Window>,
    max_keys: usize,
    entries: HashMap<K, WindowLimit>,
}

impl<K: Eq + Hash + Clone> KeyedLimiter<K> {
    /// A limiter applying `windows` to each key, tracking at most `max_keys` keys.
    pub fn new(windows: &[Window], max_keys: usize) -> Self {
        Self {
            windows: windows.to_vec(),
            max_keys: max_keys.max(1),
            entries: HashMap::new(),
        }
    }

    /// Admits one event for `key` at `now`.
    pub fn try_admit(&mut self, key: &K, now: Instant) -> bool {
        if let Some(limit) = self.entries.get_mut(key) {
            return limit.try_admit(now);
        }
        if self.entries.len() >= self.max_keys {
            self.sweep(now);
            if self.entries.len() >= self.max_keys {
                return false;
            }
        }
        let mut limit = WindowLimit::new(&self.windows, now);
        let admitted = limit.try_admit(now);
        self.entries.insert(key.clone(), limit);
        admitted
    }

    /// Forgets idle keys (every bucket refilled).
    pub fn sweep(&mut self, now: Instant) {
        self.entries.retain(|_, limit| !limit.is_idle(now));
    }

    /// Keys currently tracked.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no key is tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Packet-rate and byte-rate limit of one sender (connection or session).
#[derive(Debug, Clone)]
pub struct RateMeter {
    packets: TokenBucket,
    bytes: TokenBucket,
}

/// Rate limit configuration: sustained rates plus a burst allowance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateSpec {
    pub packets_per_sec: u64,
    pub bytes_per_sec: u64,
    /// Bucket capacity in seconds of sustained rate (≥ 1).
    pub burst_secs: u64,
}

impl RateMeter {
    /// A full meter for `spec`.
    pub fn new(spec: RateSpec, now: Instant) -> Self {
        let burst = spec.burst_secs.max(1);
        let period = Duration::from_secs(burst);
        Self {
            packets: TokenBucket::new(spec.packets_per_sec.saturating_mul(burst), period, now),
            bytes: TokenBucket::new(spec.bytes_per_sec.saturating_mul(burst), period, now),
        }
    }

    /// Admits one packet of `bytes` bytes. Both rates must have room.
    pub fn admit(&mut self, bytes: usize, now: Instant) -> bool {
        // Precision: frame sizes are ≤ MAX_FRAME_BYTES, exact in f64.
        let cost = bytes as f64;
        if self.packets.can_take(1.0, now) && self.bytes.can_take(cost, now) {
            self.packets.try_take(1.0, now);
            self.bytes.try_take(cost, now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_refills_continuously() {
        let t0 = Instant::now();
        let mut b = TokenBucket::new(5, Duration::from_secs(60), t0);
        for _ in 0..5 {
            assert!(b.try_take(1.0, t0));
        }
        assert!(!b.try_take(1.0, t0));
        // One token per 12 s.
        assert!(!b.try_take(1.0, t0 + Duration::from_secs(11)));
        assert!(b.try_take(1.0, t0 + Duration::from_secs(12)));
        assert!(b.is_full(t0 + Duration::from_secs(3_600)));
    }

    #[test]
    fn window_limit_needs_every_window() {
        let t0 = Instant::now();
        let windows = [Window::per_minute(5), Window::per_hour(20)];
        // Burst: the minute window admits 5 at once.
        let mut l = WindowLimit::new(&windows, t0);
        assert_eq!((0..10).filter(|_| l.try_admit(t0)).count(), 5);

        // One attempt every 13 s never exhausts the minute window (it refills 13/12 per step),
        // so the hour window decides: 20 at once, then one per 180 s. Over 507 s that admits
        // 22 (the 21st at 260 s, the 22nd at 364 s).
        let mut l = WindowLimit::new(&windows, t0);
        let admitted = (0..40u64)
            .filter(|k| l.try_admit(t0 + Duration::from_secs(k * 13)))
            .count();
        assert_eq!(admitted, 22);
    }

    #[test]
    fn keyed_limiter_is_bounded_and_evicts_idle_keys() {
        let t0 = Instant::now();
        let mut l = KeyedLimiter::new(&[Window::per_minute(1)], 2);
        assert!(l.try_admit(&"a", t0));
        assert!(l.try_admit(&"b", t0));
        // Map full, nobody idle: refused.
        assert!(!l.try_admit(&"c", t0));
        // After a minute both are idle and get evicted.
        assert!(l.try_admit(&"c", t0 + Duration::from_secs(61)));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn rate_meter_limits_packets_and_bytes() {
        let t0 = Instant::now();
        let spec = RateSpec {
            packets_per_sec: 10,
            bytes_per_sec: 1_000,
            burst_secs: 1,
        };
        let mut m = RateMeter::new(spec, t0);
        assert!(m.admit(900, t0));
        // Byte budget exhausted although packets remain.
        assert!(!m.admit(200, t0));
        // One second later both buckets are full again: 10 packets pass, the 11th does not.
        let t1 = t0 + Duration::from_secs(1);
        for _ in 0..10 {
            assert!(m.admit(1, t1));
        }
        assert!(!m.admit(1, t1));
    }

    #[test]
    fn backwards_clock_adds_nothing() {
        let t0 = Instant::now() + Duration::from_secs(10);
        let mut b = TokenBucket::new(1, Duration::from_secs(1), t0);
        assert!(b.try_take(1.0, t0));
        assert!(!b.try_take(1.0, t0 - Duration::from_secs(5)));
        // The stale reading did not move the refill origin back: 0.5 s after t0 is half a token.
        assert!(!b.try_take(1.0, t0 + Duration::from_millis(500)));
        assert!(b.try_take(1.0, t0 + Duration::from_secs(1)));
    }
}

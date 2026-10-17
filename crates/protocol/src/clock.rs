//! NTP-style clock synchronization against the relay clock (docs/PROTOCOL.md §8, D-013).
//!
//! For one PING/PONG exchange: `t0` client send (client clock), `t1` relay receive and `t2` relay
//! send (relay wall clock, ns since the Unix epoch), `t3` client receive (client clock).
//!
//! * offset θ = ((t1 − t0) + (t2 − t3)) / 2 — relay clock minus client clock;
//! * delay  δ = (t3 − t0) − (t2 − t1) — round trip minus relay processing time.
//!
//! [`ClockSync`] keeps the last [`CLOCK_SAMPLE_WINDOW`] samples and uses the one with the minimum
//! δ (least queuing, so least asymmetry error); its uncertainty is δ_min / 2. All arithmetic is done in
//! `i128`, so arbitrary `u64` timestamps cannot overflow.

use std::collections::VecDeque;

use thiserror::Error;

use crate::messages::{ClockSummary, Pong};

/// Number of recent samples kept for the minimum-delay selection.
pub const CLOCK_SAMPLE_WINDOW: usize = 16;

/// Why a PING/PONG exchange was rejected as a clock sample.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ClockSampleError {
    #[error("PONG id {received} does not match the outstanding PING id {expected}")]
    IdMismatch { expected: u64, received: u64 },
    #[error("PONG echoes t0 {received}, the PING carried {expected}")]
    T0Mismatch { expected: u64, received: u64 },
    #[error("client receive time t3 {t3} is before send time t0 {t0}")]
    ClientTimeReversed { t0: u64, t3: u64 },
    #[error("relay send time t2 {t2} is before receive time t1 {t1}")]
    RelayTimeReversed { t1: u64, t2: u64 },
    #[error("relay processing time exceeds the round trip (delay {delay_ns} ns)")]
    NegativeDelay { delay_ns: i128 },
    #[error("offset {offset_ns} ns does not fit in i64")]
    OffsetOverflow { offset_ns: i128 },
}

/// One accepted exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    /// θ in ns (relay − client).
    pub offset_ns: i64,
    /// δ in ns.
    pub delay_ns: u64,
    /// Client clock at receipt (t3), for ageing out.
    pub at_client_ns: u64,
}

impl ClockSample {
    /// Computes θ and δ from the four timestamps, rejecting physically impossible exchanges.
    pub fn from_timestamps(t0: u64, t1: u64, t2: u64, t3: u64) -> Result<Self, ClockSampleError> {
        if t3 < t0 {
            return Err(ClockSampleError::ClientTimeReversed { t0, t3 });
        }
        if t2 < t1 {
            return Err(ClockSampleError::RelayTimeReversed { t1, t2 });
        }
        let (t0, t1, t2, t3_) = (
            i128::from(t0),
            i128::from(t1),
            i128::from(t2),
            i128::from(t3),
        );
        let delay = (t3_ - t0) - (t2 - t1);
        let delay_ns = u64::try_from(delay)
            .map_err(|_| ClockSampleError::NegativeDelay { delay_ns: delay })?;
        // Floor division keeps the result deterministic for odd sums.
        let offset = ((t1 - t0) + (t2 - t3_)).div_euclid(2);
        let offset_ns = i64::try_from(offset)
            .map_err(|_| ClockSampleError::OffsetOverflow { offset_ns: offset })?;
        Ok(Self {
            offset_ns,
            delay_ns,
            at_client_ns: t3,
        })
    }
}

/// Current best estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockEstimate {
    /// θ of the minimum-delay sample: add to a client timestamp to get relay time.
    pub offset_ns: i64,
    /// δ_min / 2.
    pub uncertainty_ns: u64,
    /// δ_min.
    pub min_delay_ns: u64,
    /// Samples in the window.
    pub samples: usize,
}

impl ClockEstimate {
    /// Converts a client clock value to the relay clock (`capture_relay_ns_est`, §8). Saturates at the
    /// `i64` range instead of wrapping.
    pub fn to_relay_ns(&self, client_ns: u64) -> i64 {
        let v = i128::from(client_ns) + i128::from(self.offset_ns);
        i64::try_from(v).unwrap_or(if v < 0 { i64::MIN } else { i64::MAX })
    }

    /// Summary for DRIVER_STATUS.
    pub const fn summary(&self) -> ClockSummary {
        ClockSummary {
            offset_ns: self.offset_ns,
            uncertainty_ns: self.uncertainty_ns,
        }
    }
}

/// Age of a driver sample as seen by an engineer (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgeEstimate {
    /// `(now_engineer + θ_engineer) − capture_relay_ns_est`. Can be slightly negative within the
    /// uncertainty.
    pub age_ns: i64,
    /// δ_min,driver / 2 + δ_min,engineer / 2.
    pub uncertainty_ns: u64,
}

/// Computes the age of a sample captured at `capture_relay_ns_est` (driver's estimate of relay
/// time) given the engineer's local clock `now_engineer_ns`, the engineer's clock estimate and the
/// driver's reported uncertainty. Returns `None` when the capture time is unknown
/// (`telemetry_model::codec::RELAY_NS_UNKNOWN`).
pub fn sample_age(
    capture_relay_ns_est: i64,
    now_engineer_ns: u64,
    engineer: &ClockEstimate,
    driver_uncertainty_ns: u64,
) -> Option<AgeEstimate> {
    if capture_relay_ns_est == telemetry_model::codec::RELAY_NS_UNKNOWN {
        return None;
    }
    let now_relay = i128::from(now_engineer_ns) + i128::from(engineer.offset_ns);
    let age = now_relay - i128::from(capture_relay_ns_est);
    let age_ns = i64::try_from(age).ok()?;
    Some(AgeEstimate {
        age_ns,
        uncertainty_ns: engineer
            .uncertainty_ns
            .saturating_add(driver_uncertainty_ns),
    })
}

/// Sliding-window clock-offset estimator for one connection.
#[derive(Debug, Clone, Default)]
pub struct ClockSync {
    samples: VecDeque<ClockSample>,
    rejected: u64,
}

impl ClockSync {
    /// Empty estimator.
    pub fn new() -> Self {
        Self {
            samples: VecDeque::with_capacity(CLOCK_SAMPLE_WINDOW),
            rejected: 0,
        }
    }

    /// Adds one exchange; the oldest sample leaves the window when it is full.
    pub fn add(
        &mut self,
        t0: u64,
        t1: u64,
        t2: u64,
        t3: u64,
    ) -> Result<ClockSample, ClockSampleError> {
        match ClockSample::from_timestamps(t0, t1, t2, t3) {
            Ok(sample) => {
                if self.samples.len() == CLOCK_SAMPLE_WINDOW {
                    self.samples.pop_front();
                }
                self.samples.push_back(sample);
                Ok(sample)
            }
            Err(e) => {
                self.rejected = self.rejected.saturating_add(1);
                tracing::debug!(component = "protocol", error = %e, "clock sample rejected");
                Err(e)
            }
        }
    }

    /// Adds the exchange of a PONG answering the PING `(ping_id, ping_t0)`, received at client time
    /// `t3`. Mismatched id or echoed `t0` are rejected (a stale or foreign PONG).
    pub fn on_pong(
        &mut self,
        ping_id: u64,
        ping_t0: u64,
        pong: &Pong,
        t3: u64,
    ) -> Result<ClockSample, ClockSampleError> {
        if pong.id != ping_id {
            self.rejected = self.rejected.saturating_add(1);
            return Err(ClockSampleError::IdMismatch {
                expected: ping_id,
                received: pong.id,
            });
        }
        if pong.t0 != ping_t0 {
            self.rejected = self.rejected.saturating_add(1);
            return Err(ClockSampleError::T0Mismatch {
                expected: ping_t0,
                received: pong.t0,
            });
        }
        self.add(pong.t0, pong.t1, pong.t2, t3)
    }

    /// Best estimate (minimum-delay sample of the window), `None` before the first sample.
    pub fn estimate(&self) -> Option<ClockEstimate> {
        // Ties: the most recent sample wins (newest reflects current drift).
        let best = self.samples.iter().rev().min_by_key(|s| s.delay_ns)?;
        Some(ClockEstimate {
            offset_ns: best.offset_ns,
            uncertainty_ns: best.delay_ns / 2,
            min_delay_ns: best.delay_ns,
            samples: self.samples.len(),
        })
    }

    /// Samples rejected so far.
    pub const fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Clears the window (e.g. after reconnecting to a different relay instance).
    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;
    /// Relay clock is ~1.7e18 ns (Unix time); the client clock starts near zero (process epoch).
    const RELAY_EPOCH: u64 = 1_790_000_000_000_000_000;

    /// Simulates one exchange with a client clock that lags the relay by `true_offset` and the given
    /// one-way delays and relay processing time.
    fn exchange(
        client_send: u64,
        true_offset: u64,
        up: u64,
        proc_: u64,
        down: u64,
    ) -> (u64, u64, u64, u64) {
        let t0 = client_send;
        let t1 = t0 + true_offset + up;
        let t2 = t1 + proc_;
        let t3 = t2 - true_offset + down;
        (t0, t1, t2, t3)
    }

    #[test]
    fn symmetric_path_recovers_exact_offset() {
        let offset = RELAY_EPOCH;
        let (t0, t1, t2, t3) = exchange(5 * MS, offset, 10 * MS, MS, 10 * MS);
        let s = ClockSample::from_timestamps(t0, t1, t2, t3).expect("valid");
        assert_eq!(s.offset_ns, offset as i64);
        assert_eq!(s.delay_ns, 20 * MS);
    }

    #[test]
    fn skewed_clocks_min_delay_sample_wins() {
        let offset = RELAY_EPOCH + 123_456_789;
        let mut sync = ClockSync::new();
        // Jittery, asymmetric samples; the 4th has the least queuing (2 ms + 2 ms).
        let paths = [(30, 10), (12, 25), (40, 5), (2, 2), (18, 9), (50, 50)];
        for (i, (up, down)) in paths.iter().enumerate() {
            let (t0, t1, t2, t3) =
                exchange(i as u64 * 1000 * MS, offset, up * MS, MS / 2, down * MS);
            sync.add(t0, t1, t2, t3).expect("valid sample");
        }
        let est = sync.estimate().expect("estimate");
        assert_eq!(est.min_delay_ns, 4 * MS);
        assert_eq!(est.uncertainty_ns, 2 * MS);
        // Symmetric 2/2 ms path → exact offset.
        assert_eq!(est.offset_ns, offset as i64);
        assert_eq!(est.samples, paths.len());
        assert_eq!(est.to_relay_ns(1_000), offset as i64 + 1_000);
    }

    #[test]
    fn asymmetric_error_is_within_uncertainty() {
        let offset = RELAY_EPOCH;
        let (t0, t1, t2, t3) = exchange(0, offset, 3 * MS, 0, 9 * MS);
        let mut sync = ClockSync::new();
        sync.add(t0, t1, t2, t3).expect("valid");
        let est = sync.estimate().expect("estimate");
        let err = (i128::from(est.offset_ns) - i128::from(offset)).unsigned_abs();
        assert!(
            err <= u128::from(est.uncertainty_ns),
            "error {err} > uncertainty {}",
            est.uncertainty_ns
        );
    }

    #[test]
    fn client_ahead_of_relay_gives_negative_offset() {
        // Client clock far ahead: relay − client is negative.
        let client = RELAY_EPOCH + 10 * MS;
        let relay_t1 = RELAY_EPOCH + 5 * MS;
        let s = ClockSample::from_timestamps(client, relay_t1, relay_t1, client + 10 * MS)
            .expect("valid");
        assert!(s.offset_ns < 0);
    }

    #[test]
    fn window_keeps_last_16() {
        let mut sync = ClockSync::new();
        // A very good sample first, then 16 worse ones: the good one ages out.
        sync.add(0, 100, 100, 2).expect("valid");
        for i in 0..CLOCK_SAMPLE_WINDOW as u64 {
            let t0 = (i + 1) * 1000;
            sync.add(t0, t0 + 50, t0 + 50, t0 + 100).expect("valid");
        }
        let est = sync.estimate().expect("estimate");
        assert_eq!(est.samples, CLOCK_SAMPLE_WINDOW);
        assert_eq!(est.min_delay_ns, 100);
    }

    #[test]
    fn impossible_samples_are_rejected() {
        let mut sync = ClockSync::new();
        assert_eq!(
            sync.add(10, 0, 0, 5),
            Err(ClockSampleError::ClientTimeReversed { t0: 10, t3: 5 })
        );
        assert_eq!(
            sync.add(0, 10, 5, 20),
            Err(ClockSampleError::RelayTimeReversed { t1: 10, t2: 5 })
        );
        assert!(matches!(
            sync.add(0, 0, 100, 10),
            Err(ClockSampleError::NegativeDelay { .. })
        ));
        assert!(matches!(
            sync.add(0, u64::MAX, u64::MAX, 0),
            Err(ClockSampleError::OffsetOverflow { .. })
        ));
        assert_eq!(sync.rejected(), 4);
        assert_eq!(sync.estimate(), None);
        let pong = Pong {
            id: 2,
            t0: 0,
            t1: 5,
            t2: 5,
        };
        assert!(matches!(
            sync.on_pong(1, 0, &pong, 10),
            Err(ClockSampleError::IdMismatch { .. })
        ));
        assert!(matches!(
            sync.on_pong(2, 1, &pong, 10),
            Err(ClockSampleError::T0Mismatch { .. })
        ));
        assert!(sync.on_pong(2, 0, &pong, 10).is_ok());
    }

    #[test]
    fn engineer_age_combines_uncertainties() {
        let driver_offset = RELAY_EPOCH;
        let engineer_offset = RELAY_EPOCH + 7 * MS;
        let capture_driver = 1_000 * MS;
        let capture_relay = (capture_driver + driver_offset) as i64;
        let engineer = ClockEstimate {
            offset_ns: engineer_offset as i64,
            uncertainty_ns: 3 * MS,
            min_delay_ns: 6 * MS,
            samples: 4,
        };
        // Engineer observes 50 ms after capture (in relay time).
        let now_engineer = capture_driver + driver_offset + 50 * MS - engineer_offset;
        let age = sample_age(capture_relay, now_engineer, &engineer, 2 * MS).expect("known");
        assert_eq!(age.age_ns, (50 * MS) as i64);
        assert_eq!(age.uncertainty_ns, 5 * MS);
        assert_eq!(
            sample_age(
                telemetry_model::codec::RELAY_NS_UNKNOWN,
                now_engineer,
                &engineer,
                0
            ),
            None
        );
    }
}

//! Link liveness evaluation (PROTOCOL.md §7, ARCHITECTURE.md §5.1). Pure logic driven by injected
//! instants, so every threshold is unit-testable.
//!
//! Precedence of the verdicts:
//! 1. no inbound **bytes** for `timeout_after` (5 s) → [`LinkHealth::TimedOut`] → RECONNECTING;
//! 2. no inbound **frame** for `stale_after` (3 s) → [`LinkHealth::Stale`];
//! 3. no PONG for `degraded_after` (2.5 s: a missed heartbeat) → DEGRADED;
//! 4. smoothed RTT above `rtt_degraded` → DEGRADED (left again below 80 % of it);
//! 5. sequence loss above `loss_degraded_fraction` over the last ~5 s (at least
//!    [`LOSS_MIN_FRAMES`] frames) → DEGRADED (left again below half of it);
//! 6. otherwise healthy.
//!
//! PONGs are inbound frames, so on a stalled link the states follow each other as
//! DEGRADED (2.5 s) → STALE (3 s) → RECONNECTING (5 s).

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

use crate::state::DegradedReason;

/// Default smoothed RTT above which the link is DEGRADED. The spec gives no number; 750 ms is far
/// above intercontinental RTTs (~300 ms) and well below the heartbeat period (assumption, tunable).
pub const DEFAULT_RTT_DEGRADED: Duration = Duration::from_millis(750);
/// Default loss fraction above which the link is DEGRADED (assumption, tunable).
pub const DEFAULT_LOSS_DEGRADED_FRACTION: f64 = 0.05;
/// Default "no inbound frame" threshold for STALE (ARCHITECTURE.md §5.1: 3 s).
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_millis(3_000);
/// Number of loss samples kept (one per liveness check, i.e. ~5 s at 250 ms).
pub const LOSS_WINDOW_SAMPLES: usize = 20;
/// Minimum frames in the loss window before loss is judged at all.
pub const LOSS_MIN_FRAMES: u64 = 20;
/// SRTT smoothing factor (RFC 6298: α = 1/8).
const SRTT_ALPHA: f64 = 0.125;
/// Hysteresis: the RTT flag clears below this fraction of the threshold.
const RTT_RECOVER_FRACTION: f64 = 0.8;
/// Hysteresis: the loss flag clears below this fraction of the threshold.
const LOSS_RECOVER_FRACTION: f64 = 0.5;

/// Thresholds of the liveness evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LivenessConfig {
    /// No PONG for this long → DEGRADED (missed heartbeat).
    pub degraded_after: Duration,
    /// No inbound frame for this long → STALE.
    pub stale_after: Duration,
    /// No inbound bytes for this long → reconnect.
    pub timeout_after: Duration,
    /// Smoothed RTT above this → DEGRADED.
    pub rtt_degraded: Duration,
    /// Loss fraction above this → DEGRADED.
    pub loss_degraded_fraction: f64,
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            degraded_after: Duration::from_millis(protocol::timing::HEARTBEAT_DEGRADED_MS),
            stale_after: DEFAULT_STALE_AFTER,
            timeout_after: Duration::from_millis(protocol::timing::HEARTBEAT_TIMEOUT_MS),
            rtt_degraded: DEFAULT_RTT_DEGRADED,
            loss_degraded_fraction: DEFAULT_LOSS_DEGRADED_FRACTION,
        }
    }
}

/// Verdict of one evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkHealth {
    Healthy,
    Degraded(DegradedReason),
    /// No inbound frame for longer than the stale threshold; the socket is still open.
    Stale,
    /// No inbound bytes for longer than the heartbeat timeout: the connection is dead.
    TimedOut,
}

/// Liveness tracker of one connection.
#[derive(Debug, Clone)]
pub struct LivenessMonitor {
    cfg: LivenessConfig,
    last_bytes_total: u64,
    last_bytes_at: Instant,
    last_frame_at: Instant,
    last_pong_at: Instant,
    srtt: Option<Duration>,
    rtt_flagged: bool,
    loss_flagged: bool,
    /// Cumulative `(received, lost)` per check, oldest first.
    loss_samples: VecDeque<(u64, u64)>,
}

impl LivenessMonitor {
    /// Monitor of a connection that became usable at `now`.
    pub fn new(cfg: LivenessConfig, now: Instant) -> Self {
        Self {
            cfg,
            last_bytes_total: 0,
            last_bytes_at: now,
            last_frame_at: now,
            last_pong_at: now,
            srtt: None,
            rtt_flagged: false,
            loss_flagged: false,
            loss_samples: VecDeque::with_capacity(LOSS_WINDOW_SAMPLES),
        }
    }

    /// Records the connection's cumulative inbound byte count.
    pub fn observe_bytes(&mut self, total: u64, now: Instant) {
        if total != self.last_bytes_total {
            self.last_bytes_total = total;
            self.last_bytes_at = now;
        }
    }

    /// Records a complete inbound frame.
    pub fn on_frame(&mut self, now: Instant) {
        self.last_frame_at = now;
        self.last_bytes_at = now;
    }

    /// Records a PONG and its round-trip time.
    pub fn on_pong(&mut self, now: Instant, rtt: Duration) {
        self.last_pong_at = now;
        self.srtt = Some(match self.srtt {
            None => rtt,
            Some(srtt) => srtt.mul_f64(1.0 - SRTT_ALPHA) + rtt.mul_f64(SRTT_ALPHA),
        });
    }

    /// Records cumulative sequence counters of the inbound stream (engineer side).
    pub fn observe_sequence(&mut self, received_total: u64, lost_total: u64) {
        if self.loss_samples.len() == LOSS_WINDOW_SAMPLES {
            self.loss_samples.pop_front();
        }
        self.loss_samples.push_back((received_total, lost_total));
    }

    /// Smoothed RTT, `None` before the first PONG.
    pub const fn srtt(&self) -> Option<Duration> {
        self.srtt
    }

    /// Loss fraction over the window, `None` with too few frames.
    pub fn loss_fraction(&self) -> Option<f64> {
        let (&(r0, l0), &(r1, l1)) = (self.loss_samples.front()?, self.loss_samples.back()?);
        let lost = l1.saturating_sub(l0);
        let total = r1.saturating_sub(r0).saturating_add(lost);
        if total < LOSS_MIN_FRAMES {
            return None;
        }
        Some(lost as f64 / total as f64)
    }

    /// Evaluates the link at `now`.
    pub fn evaluate(&mut self, now: Instant) -> LinkHealth {
        if now.saturating_duration_since(self.last_bytes_at) >= self.cfg.timeout_after {
            return LinkHealth::TimedOut;
        }
        if now.saturating_duration_since(self.last_frame_at) >= self.cfg.stale_after {
            return LinkHealth::Stale;
        }
        if now.saturating_duration_since(self.last_pong_at) >= self.cfg.degraded_after {
            return LinkHealth::Degraded(DegradedReason::MissedHeartbeat);
        }
        if let Some(srtt) = self.srtt {
            if srtt > self.cfg.rtt_degraded {
                self.rtt_flagged = true;
            } else if srtt < self.cfg.rtt_degraded.mul_f64(RTT_RECOVER_FRACTION) {
                self.rtt_flagged = false;
            }
        }
        if self.rtt_flagged {
            return LinkHealth::Degraded(DegradedReason::HighRtt);
        }
        if let Some(loss) = self.loss_fraction() {
            if loss > self.cfg.loss_degraded_fraction {
                self.loss_flagged = true;
            } else if loss < self.cfg.loss_degraded_fraction * LOSS_RECOVER_FRACTION {
                self.loss_flagged = false;
            }
        }
        if self.loss_flagged {
            return LinkHealth::Degraded(DegradedReason::PacketLoss);
        }
        LinkHealth::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[tokio::test(start_paused = true)]
    async fn stalled_link_goes_degraded_then_stale_then_timed_out() {
        let t0 = Instant::now();
        let mut m = LivenessMonitor::new(LivenessConfig::default(), t0);
        assert_eq!(m.evaluate(t0 + 2_499 * MS), LinkHealth::Healthy);
        assert_eq!(
            m.evaluate(t0 + 2_500 * MS),
            LinkHealth::Degraded(DegradedReason::MissedHeartbeat)
        );
        assert_eq!(m.evaluate(t0 + 3_000 * MS), LinkHealth::Stale);
        assert_eq!(m.evaluate(t0 + 5_000 * MS), LinkHealth::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn partial_bytes_keep_the_link_alive_but_stale() {
        let t0 = Instant::now();
        let mut m = LivenessMonitor::new(LivenessConfig::default(), t0);
        // A large frame trickles in: bytes arrive, no complete frame.
        m.observe_bytes(1_000, t0 + 4_000 * MS);
        assert_eq!(m.evaluate(t0 + 6_000 * MS), LinkHealth::Stale);
        assert_eq!(m.evaluate(t0 + 9_000 * MS), LinkHealth::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn pongs_keep_it_healthy_and_high_rtt_degrades_with_hysteresis() {
        let t0 = Instant::now();
        let cfg = LivenessConfig::default();
        let mut m = LivenessMonitor::new(cfg, t0);
        let mut t = t0;
        for _ in 0..5 {
            t += 1_000 * MS;
            m.on_frame(t);
            m.on_pong(t, 40 * MS);
        }
        assert_eq!(m.evaluate(t), LinkHealth::Healthy);
        // RTT jumps to 2 s: the smoothed RTT crosses 750 ms after a few samples.
        let mut verdicts = Vec::new();
        for _ in 0..10 {
            t += 1_000 * MS;
            m.on_frame(t);
            m.on_pong(t, 2_000 * MS);
            verdicts.push(m.evaluate(t));
        }
        assert!(verdicts.contains(&LinkHealth::Degraded(DegradedReason::HighRtt)));
        // Back to 40 ms: stays degraded until SRTT < 600 ms, then healthy.
        let mut recovered = false;
        for _ in 0..40 {
            t += 1_000 * MS;
            m.on_frame(t);
            m.on_pong(t, 40 * MS);
            if m.evaluate(t) == LinkHealth::Healthy {
                recovered = true;
                assert!(m.srtt().expect("srtt") < cfg.rtt_degraded.mul_f64(0.8));
                break;
            }
        }
        assert!(recovered);
    }

    #[tokio::test(start_paused = true)]
    async fn loss_above_threshold_degrades() {
        let t0 = Instant::now();
        let mut m = LivenessMonitor::new(LivenessConfig::default(), t0);
        m.on_pong(t0, 20 * MS);
        assert_eq!(m.loss_fraction(), None);
        // 100 frames received, 10 lost: 9 % > 5 %.
        m.observe_sequence(0, 0);
        m.observe_sequence(100, 10);
        assert!((m.loss_fraction().expect("enough frames") - 10.0 / 110.0).abs() < 1e-9);
        assert_eq!(
            m.evaluate(t0),
            LinkHealth::Degraded(DegradedReason::PacketLoss)
        );
        // Window slides to a clean period.
        for i in 0..LOSS_WINDOW_SAMPLES as u64 {
            m.observe_sequence(200 + i * 50, 10);
        }
        assert_eq!(m.evaluate(t0), LinkHealth::Healthy);
        // Too few frames: no verdict.
        let mut few = LivenessMonitor::new(LivenessConfig::default(), t0);
        few.observe_sequence(0, 0);
        few.observe_sequence(5, 5);
        assert_eq!(few.loss_fraction(), None);
    }
}

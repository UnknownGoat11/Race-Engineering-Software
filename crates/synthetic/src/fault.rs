//! Network fault configuration and the pure, seeded [`FaultModel`] that decides the fate of every unit
//! (protocol frame or opaque chunk) crossing a simulated link: delivered at a time, or dropped.
//!
//! The model is deterministic (ChaCha) and clock-free: callers pass the arrival time as a [`Duration`]
//! since their own epoch, so it can drive unit tests of sequence tracking as well as the TCP
//! [`crate::proxy::FaultProxy`]. Delivery order always equals arrival order (an ordered byte stream cannot
//! reorder): jitter delays a unit and everything queued behind it (head-of-line blocking).

use std::time::Duration;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::error::SyntheticError;

/// Largest one-way latency the simulator accepts.
pub const MAX_LATENCY: Duration = Duration::from_secs(1);
/// Latency presets of the test matrix (20 ms .. 1 s).
pub const LATENCY_PRESETS: [Duration; 6] = [
    Duration::from_millis(20),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
];
/// Most blackout windows kept in one configuration (older, finished windows are pruned first).
pub const MAX_BLACKOUTS: usize = 64;
/// Longest blackout window. Bounded so that delivery times stay far from `Instant` overflow.
pub const MAX_BLACKOUT: Duration = Duration::from_secs(3_600);

/// Loss presets of the test matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LossPreset {
    None,
    OnePercent,
    FivePercent,
    TenPercent,
    TwentyPercent,
}

impl LossPreset {
    /// Every preset.
    pub const ALL: [LossPreset; 5] = [
        LossPreset::None,
        LossPreset::OnePercent,
        LossPreset::FivePercent,
        LossPreset::TenPercent,
        LossPreset::TwentyPercent,
    ];

    /// Drop probability of the preset.
    pub const fn probability(self) -> f64 {
        match self {
            LossPreset::None => 0.0,
            LossPreset::OnePercent => 0.01,
            LossPreset::FivePercent => 0.05,
            LossPreset::TenPercent => 0.10,
            LossPreset::TwentyPercent => 0.20,
        }
    }
}

/// Extra random delay added on top of the base latency (uniform in `0..=max_extra`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Jitter {
    None,
    /// Up to 5 ms.
    Low,
    /// Up to 25 ms.
    Medium,
    /// Up to 100 ms.
    High,
}

impl Jitter {
    /// Largest extra delay.
    pub const fn max_extra(self) -> Duration {
        match self {
            Jitter::None => Duration::ZERO,
            Jitter::Low => Duration::from_millis(5),
            Jitter::Medium => Duration::from_millis(25),
            Jitter::High => Duration::from_millis(100),
        }
    }
}

/// What a blackout does to the units that arrive during it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlackoutPolicy {
    /// Held and delivered (in order) when the blackout ends: TCP retransmission after an outage.
    Hold,
    /// Droppable units are discarded; non-droppable ones (control frames) are held.
    Drop,
}

/// A time window, relative to the model's epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    pub start: Duration,
    pub duration: Duration,
}

impl Window {
    /// End of the window.
    pub fn end(&self) -> Duration {
        self.start.saturating_add(self.duration)
    }

    /// Whether `t` lies inside the window.
    pub fn contains(&self, t: Duration) -> bool {
        t >= self.start && t < self.end()
    }
}

/// Faults of one link direction.
#[derive(Debug, Clone, PartialEq)]
pub struct FaultConfig {
    /// Probability (0..=1) that a droppable unit is lost.
    pub loss: f64,
    /// Base one-way latency (≤ [`MAX_LATENCY`]).
    pub latency: Duration,
    pub jitter: Jitter,
    pub blackouts: Vec<Window>,
    pub blackout_policy: BlackoutPolicy,
}

impl Default for FaultConfig {
    /// A perfect link.
    fn default() -> Self {
        Self {
            loss: 0.0,
            latency: Duration::ZERO,
            jitter: Jitter::None,
            blackouts: Vec::new(),
            blackout_policy: BlackoutPolicy::Hold,
        }
    }
}

impl FaultConfig {
    /// A link with the given loss preset, latency and jitter.
    pub fn new(loss: LossPreset, latency: Duration, jitter: Jitter) -> Self {
        Self {
            loss: loss.probability(),
            latency,
            jitter,
            ..Self::default()
        }
    }

    /// Adds a blackout window.
    pub fn with_blackout(mut self, start: Duration, duration: Duration) -> Self {
        self.blackouts.push(Window { start, duration });
        self
    }

    /// Sets the blackout policy.
    pub fn with_blackout_policy(mut self, policy: BlackoutPolicy) -> Self {
        self.blackout_policy = policy;
        self
    }

    /// Checks the parameters.
    pub fn validate(&self) -> Result<(), SyntheticError> {
        if !(0.0..=1.0).contains(&self.loss) {
            return Err(SyntheticError::config("fault.loss", "must be within 0..=1"));
        }
        if self.latency > MAX_LATENCY {
            return Err(SyntheticError::config(
                "fault.latency",
                format!("at most {MAX_LATENCY:?}"),
            ));
        }
        if self.blackouts.len() > MAX_BLACKOUTS {
            return Err(SyntheticError::config(
                "fault.blackouts",
                format!("at most {MAX_BLACKOUTS} windows"),
            ));
        }
        if self.blackouts.iter().any(|w| w.duration > MAX_BLACKOUT) {
            return Err(SyntheticError::config(
                "fault.blackouts",
                format!("each window lasts at most {MAX_BLACKOUT:?}"),
            ));
        }
        Ok(())
    }

    /// Whether this configuration can be applied to an opaque byte stream (e.g. TLS records), where units
    /// can be delayed but never removed.
    pub fn is_lossless(&self) -> bool {
        self.loss == 0.0
            && (self.blackouts.is_empty() || self.blackout_policy == BlackoutPolicy::Hold)
    }

    /// Adds a blackout starting at `start`, pruning finished windows (before `start`) when full. The
    /// duration is capped at [`MAX_BLACKOUT`] so the configuration stays valid.
    pub fn push_blackout(&mut self, start: Duration, duration: Duration) {
        let duration = duration.min(MAX_BLACKOUT);
        if self.blackouts.len() >= MAX_BLACKOUTS {
            self.blackouts.retain(|w| w.end() > start);
        }
        if self.blackouts.len() >= MAX_BLACKOUTS {
            self.blackouts.remove(0);
        }
        self.blackouts.push(Window { start, duration });
    }

    /// End of the blackout covering `t`, if any (the latest end among overlapping windows).
    fn blackout_end(&self, t: Duration) -> Option<Duration> {
        self.blackouts
            .iter()
            .filter(|w| w.contains(t))
            .map(Window::end)
            .max()
    }
}

/// Why a unit was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropReason {
    Loss,
    Blackout,
}

/// Fate of one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    /// Deliver at this time (since the epoch); never earlier than any previous delivery.
    Deliver {
        at: Duration,
    },
    Drop(DropReason),
}

/// Counters of a [`FaultModel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FaultStats {
    pub offered: u64,
    pub delivered: u64,
    pub dropped_loss: u64,
    pub dropped_blackout: u64,
    /// Units delivered after being held by a blackout.
    pub held_by_blackout: u64,
}

impl FaultStats {
    /// Dropped units of any reason.
    pub fn dropped(&self) -> u64 {
        self.dropped_loss + self.dropped_blackout
    }
}

/// Seeded fault decisions for one link direction.
#[derive(Debug, Clone)]
pub struct FaultModel {
    config: FaultConfig,
    rng: ChaCha8Rng,
    last_delivery: Duration,
    stats: FaultStats,
}

impl FaultModel {
    /// A model with `config`; `seed` makes every decision reproducible.
    pub fn new(config: FaultConfig, seed: u64) -> Result<Self, SyntheticError> {
        config.validate()?;
        Ok(Self {
            config,
            rng: ChaCha8Rng::seed_from_u64(seed),
            last_delivery: Duration::ZERO,
            stats: FaultStats::default(),
        })
    }

    /// Current configuration.
    pub fn config(&self) -> &FaultConfig {
        &self.config
    }

    /// Replaces the configuration (live fault changes); ordering guarantees are kept.
    pub fn set_config(&mut self, config: FaultConfig) -> Result<(), SyntheticError> {
        config.validate()?;
        self.config = config;
        Ok(())
    }

    /// Counters so far.
    pub fn stats(&self) -> FaultStats {
        self.stats
    }

    /// Decides the fate of a unit arriving at `now`. Only `droppable` units can be lost (control frames
    /// are not: dropping them breaks the protocol handshake rather than simulating loss).
    pub fn decide(&mut self, now: Duration, droppable: bool) -> Verdict {
        self.stats.offered += 1;
        let blackout_end = self.config.blackout_end(now);
        if blackout_end.is_some()
            && droppable
            && self.config.blackout_policy == BlackoutPolicy::Drop
        {
            self.stats.dropped_blackout += 1;
            return Verdict::Drop(DropReason::Blackout);
        }
        if droppable && self.config.loss > 0.0 && self.rng.random_bool(self.config.loss) {
            self.stats.dropped_loss += 1;
            return Verdict::Drop(DropReason::Loss);
        }
        let max_extra = self.config.jitter.max_extra();
        let extra = if max_extra.is_zero() {
            Duration::ZERO
        } else {
            max_extra.mul_f64(self.rng.random::<f64>())
        };
        let sent = match blackout_end {
            Some(end) => {
                self.stats.held_by_blackout += 1;
                end
            }
            None => now,
        };
        let at = sent
            .saturating_add(self.config.latency)
            .saturating_add(extra)
            .max(self.last_delivery);
        self.last_delivery = at;
        self.stats.delivered += 1;
        Verdict::Deliver { at }
    }

    /// Runs `units` (arrival time, item) through the model and returns the delivered ones with their
    /// delivery times, in delivery order. `droppable` classifies each item.
    pub fn transmit<T>(
        &mut self,
        units: impl IntoIterator<Item = (Duration, T)>,
        droppable: impl Fn(&T) -> bool,
    ) -> Vec<(Duration, T)> {
        units
            .into_iter()
            .filter_map(|(now, item)| match self.decide(now, droppable(&item)) {
                Verdict::Deliver { at } => Some((at, item)),
                Verdict::Drop(_) => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn perfect_link_delivers_everything_after_the_latency() {
        let mut m = FaultModel::new(FaultConfig::new(LossPreset::None, ms(50), Jitter::None), 1)
            .expect("config");
        for i in 0..100 {
            assert_eq!(
                m.decide(ms(i * 10), true),
                Verdict::Deliver {
                    at: ms(i * 10 + 50)
                }
            );
        }
        assert_eq!(m.stats().delivered, 100);
    }

    #[test]
    fn loss_presets_are_within_tolerance() {
        const UNITS: u64 = 20_000;
        for preset in LossPreset::ALL {
            let mut m =
                FaultModel::new(FaultConfig::new(preset, ms(20), Jitter::Low), 99).expect("config");
            for i in 0..UNITS {
                m.decide(ms(i), true);
            }
            let rate = m.stats().dropped_loss as f64 / UNITS as f64;
            let p = preset.probability();
            // 5 σ of a binomial proportion, at least 0.2 %.
            let tolerance = (5.0 * (p * (1.0 - p) / UNITS as f64).sqrt()).max(0.002);
            assert!((rate - p).abs() <= tolerance, "{preset:?}: {rate}");
        }
    }

    #[test]
    fn control_units_are_never_lost() {
        let mut m = FaultModel::new(
            FaultConfig::new(LossPreset::TwentyPercent, ms(20), Jitter::None),
            3,
        )
        .expect("config");
        for i in 0..1_000 {
            assert!(matches!(m.decide(ms(i), false), Verdict::Deliver { .. }));
        }
    }

    #[test]
    fn jitter_never_reorders() {
        let mut m = FaultModel::new(
            FaultConfig::new(LossPreset::FivePercent, ms(100), Jitter::High),
            5,
        )
        .expect("config");
        let delivered = m.transmit((0..5_000).map(|i| (ms(i), i)), |_| true);
        assert!(
            delivered
                .windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 < w[1].1)
        );
        assert!(delivered.iter().all(|(at, i)| *at >= ms(*i + 100)));
        assert!(delivered.iter().any(|(at, i)| *at > ms(*i + 100)));
    }

    #[test]
    fn blackouts_hold_or_drop() {
        let hold = FaultConfig::new(LossPreset::None, ms(20), Jitter::None)
            .with_blackout(ms(1_000), ms(500));
        let mut m = FaultModel::new(hold.clone(), 1).expect("config");
        assert_eq!(
            m.decide(ms(1_100), true),
            Verdict::Deliver { at: ms(1_520) }
        );
        assert_eq!(
            m.decide(ms(1_600), true),
            Verdict::Deliver { at: ms(1_620) }
        );
        assert_eq!(m.stats().held_by_blackout, 1);

        let mut m =
            FaultModel::new(hold.with_blackout_policy(BlackoutPolicy::Drop), 1).expect("config");
        assert_eq!(
            m.decide(ms(1_100), true),
            Verdict::Drop(DropReason::Blackout)
        );
        assert_eq!(
            m.decide(ms(1_100), false),
            Verdict::Deliver { at: ms(1_520) }
        );
        assert_eq!(
            m.decide(ms(1_499), true),
            Verdict::Drop(DropReason::Blackout)
        );
        assert!(matches!(m.decide(ms(1_500), true), Verdict::Deliver { .. }));
    }

    #[test]
    fn same_seed_same_decisions_and_invalid_configs_rejected() {
        let cfg = FaultConfig::new(LossPreset::TenPercent, ms(250), Jitter::Medium);
        let run = |seed| {
            let mut m = FaultModel::new(cfg.clone(), seed).expect("config");
            (0..2_000)
                .map(|i| m.decide(ms(i), true))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        let mut bad = cfg.clone();
        bad.loss = 1.5;
        assert!(FaultModel::new(bad, 1).is_err());
        let mut bad = cfg;
        bad.latency = Duration::from_secs(2);
        assert!(bad.validate().is_err());
        assert!(FaultConfig::default().is_lossless());
        assert!(!FaultConfig::new(LossPreset::OnePercent, ms(20), Jitter::None).is_lossless());
    }

    /// Regression (review pass 2, F7): blackout windows were unbounded.
    #[test]
    fn blackout_windows_are_bounded() {
        let too_long = FaultConfig::default().with_blackout(ms(0), MAX_BLACKOUT + ms(1));
        assert!(too_long.validate().is_err());
        assert!(FaultModel::new(too_long, 1).is_err());
        let mut pushed = FaultConfig::default();
        pushed.push_blackout(Duration::MAX, Duration::MAX);
        assert!(pushed.validate().is_ok());
        assert_eq!(pushed.blackouts[0].duration, MAX_BLACKOUT);
        assert_eq!(pushed.blackouts[0].end(), Duration::MAX);
    }
}

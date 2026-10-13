//! Per-sample plausibility validation driven by the registry (`valid_range`, `max_rate_per_s`).
//!
//! Everything here is pure and deterministic. A rejected sample becomes `Quality::Invalid` (never clamped,
//! never zeroed). Ranges are deliberately conservative (registry module docs): they catch garbage, torn
//! reads and unit mix-ups, not real extremes.

use thiserror::Error;

use crate::channel::{ChannelDef, ChannelId};
use crate::frame::TelemetryFrame;
use crate::registry;
use crate::value::{Quality, Sample, Value};

/// Why a value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum Rejection {
    #[error("value is NaN or infinite")]
    NonFinite,
    #[error("value {value} outside the plausible range {min}..={max}")]
    OutOfRange { value: f64, min: f64, max: f64 },
    #[error("change of {delta} in {dt_s} s exceeds the plausible maximum of {max_delta}")]
    RateExceeded {
        delta: f64,
        dt_s: f64,
        max_delta: f64,
    },
}

/// Checks one numeric value against the channel's finiteness and `valid_range` (inclusive).
pub fn check_value(def: &ChannelDef, value: f64) -> Result<(), Rejection> {
    if !value.is_finite() {
        return Err(Rejection::NonFinite);
    }
    match def.valid_range {
        Some((min, max)) if value < min || value > max => {
            Err(Rejection::OutOfRange { value, min, max })
        }
        _ => Ok(()),
    }
}

/// Checks the change from (`prev_value` at `prev_t_s`) to (`value` at `t_s`) against `max_rate_per_s`.
/// Passes when the channel has no rate limit or when the time step is not positive and finite (a rate
/// cannot be assessed; time regressions are reported separately by [`classify_et`]).
pub fn check_rate(
    def: &ChannelDef,
    prev_value: f64,
    prev_t_s: f64,
    value: f64,
    t_s: f64,
) -> Result<(), Rejection> {
    let Some(max_rate) = def.max_rate_per_s else {
        return Ok(());
    };
    let dt_s = t_s - prev_t_s;
    if !(dt_s.is_finite() && dt_s > 0.0) {
        return Ok(());
    }
    let delta = (value - prev_value).abs();
    let max_delta = max_rate * dt_s;
    if delta > max_delta {
        Err(Rejection::RateExceeded {
            delta,
            dt_s,
            max_delta,
        })
    } else {
        Ok(())
    }
}

/// Per-channel state for rate checks.
///
/// A value passes if its change is plausible relative to EITHER the last accepted value OR the last raw
/// value. Consequences: a single garbage spike (78 → -100 → 78) rejects only the spike (the return to 78 is
/// close to the last accepted value), and a genuine step (garage teleport, tyre change, refuel) rejects
/// only its first sample (the next one is close to the previous raw value).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RateGuard {
    last_accepted: Option<(f64, f64)>,
    last_raw: Option<(f64, f64)>,
}

impl RateGuard {
    /// Checks `value` at `t_s` and records it.
    pub fn check(&mut self, def: &ChannelDef, value: f64, t_s: f64) -> Result<(), Rejection> {
        let vs_accepted = self
            .last_accepted
            .map(|(v, t)| check_rate(def, v, t, value, t_s));
        let vs_raw = self
            .last_raw
            .map(|(v, t)| check_rate(def, v, t, value, t_s));
        self.last_raw = Some((value, t_s));
        let result = match (vs_accepted, vs_raw) {
            (None, None) | (Some(Ok(())), _) | (_, Some(Ok(()))) => Ok(()),
            (Some(Err(e)), _) | (None, Some(Err(e))) => Err(e),
        };
        if result.is_ok() {
            self.last_accepted = Some((value, t_s));
        }
        result
    }

    /// Forgets history (session change, reconnect).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Validates one sample (finiteness, range, rate). Non-valid and non-numeric samples pass through
/// unchanged; a rejected sample becomes `Sample::invalid()`. `t_s` is the sample time used for rate checks
/// (normally the source ET); `None` skips the rate check without touching `guard`.
pub fn validate_sample(
    def: &ChannelDef,
    sample: Sample,
    guard: &mut RateGuard,
    t_s: Option<f64>,
) -> (Sample, Option<Rejection>) {
    if !sample.is_valid() {
        return (sample, None);
    }
    let Some(value) = sample.value.as_f64() else {
        return (sample, None);
    };
    let verdict = check_value(def, value).and_then(|()| match t_s {
        Some(t) if def.max_rate_per_s.is_some() => guard.check(def, value, t),
        _ => Ok(()),
    });
    match verdict {
        Ok(()) => (sample, None),
        Err(rejection) => (Sample::invalid(), Some(rejection)),
    }
}

/// Result of validating a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameValidation {
    /// Samples downgraded to `Invalid`, with the reason (registry order).
    pub rejected: Vec<(ChannelId, Rejection)>,
}

/// Validates whole frames, keeping one [`RateGuard`] per registry channel.
#[derive(Debug, Clone)]
pub struct FrameValidator {
    guards: Vec<RateGuard>,
}

impl Default for FrameValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameValidator {
    /// A validator with empty history.
    pub fn new() -> Self {
        Self {
            guards: vec![RateGuard::default(); registry::len()],
        }
    }

    /// Validates every valid numeric sample of `frame` in place, using the frame's source ET for rate
    /// checks (skipped when unknown). Rejected samples become `Invalid`.
    pub fn validate(&mut self, frame: &mut TelemetryFrame) -> FrameValidation {
        let t_s = frame.source_et_s;
        let mut report = FrameValidation::default();
        let mut downgrade = Vec::new();
        for (index, ((def, sample), guard)) in frame.iter().zip(self.guards.iter_mut()).enumerate()
        {
            if !sample.is_valid() {
                continue;
            }
            let Some(value) = sample.value.as_f64() else {
                continue;
            };
            let verdict = check_value(def, value).and_then(|()| match t_s {
                Some(t) if def.max_rate_per_s.is_some() => guard.check(def, value, t),
                _ => Ok(()),
            });
            if let Err(rejection) = verdict {
                downgrade.push(index);
                report.rejected.push((def.id, rejection));
            }
        }
        for index in downgrade {
            frame.downgrade_at(index, Quality::Invalid);
        }
        report
    }

    /// Forgets all rate history (session change, reconnect, ET regression, teleport).
    pub fn reset(&mut self) {
        self.guards.iter_mut().for_each(RateGuard::reset);
    }

    /// Forgets the rate history of `ids` only, so that their next value is accepted as a new reference
    /// (a known discontinuity such as a refuel or a tyre change). Range checks still apply. Ids that are not
    /// registry channels are ignored; returns how many guards were reset.
    pub fn reset_channels(&mut self, ids: &[ChannelId]) -> usize {
        let mut count = 0;
        for id in ids {
            if let Some(guard) = registry::index_of(*id).and_then(|i| self.guards.get_mut(i)) {
                guard.reset();
                count += 1;
            }
        }
        count
    }
}

/// How a new source ET relates to the previous one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EtStep {
    /// No previous ET.
    First,
    /// Time moved forward by `dt_s`.
    Advanced { dt_s: f64 },
    /// Same ET as before (duplicate read or paused game).
    Unchanged,
    /// Time went backwards by `by_s` (session restart, replay seek, or corruption).
    Regressed { by_s: f64 },
    /// The new ET is NaN or infinite.
    NonFinite,
}

/// Classifies a source ET step (exact comparison: LMU ET is a double that never jitters backwards).
pub fn classify_et(prev_et_s: Option<f64>, et_s: f64) -> EtStep {
    if !et_s.is_finite() {
        return EtStep::NonFinite;
    }
    match prev_et_s.filter(|p| p.is_finite()) {
        None => EtStep::First,
        Some(prev) if et_s > prev => EtStep::Advanced { dt_s: et_s - prev },
        Some(prev) if et_s < prev => EtStep::Regressed { by_s: prev - et_s },
        Some(_) => EtStep::Unchanged,
    }
}

/// Relation between two consecutive frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRepeat {
    /// Different source time.
    Distinct,
    /// Same source ET and identical samples: an exact duplicate (drop it, count it).
    Duplicate,
    /// Same source ET but different samples (suspicious: torn read or out-of-band update).
    SameTimeDifferentData,
    /// At least one frame has no source ET; duplicates cannot be decided by time.
    Unknown,
}

/// Duplicate detection between consecutive frames.
pub fn classify_repeat(prev: &TelemetryFrame, next: &TelemetryFrame) -> FrameRepeat {
    match (prev.source_et_s, next.source_et_s) {
        (Some(a), Some(b)) if a.to_bits() == b.to_bits() => {
            let same = prev
                .iter()
                .zip(next.iter())
                .all(|((_, x), (_, y))| same_sample(x, y));
            if same {
                FrameRepeat::Duplicate
            } else {
                FrameRepeat::SameTimeDifferentData
            }
        }
        (Some(_), Some(_)) => FrameRepeat::Distinct,
        _ => FrameRepeat::Unknown,
    }
}

/// Bitwise sample equality (so that 0.0 and -0.0 differ and comparisons are exact).
fn same_sample(a: &Sample, b: &Sample) -> bool {
    a.quality == b.quality
        && match (&a.value, &b.value) {
            (Value::F32(x), Value::F32(y)) => x.to_bits() == y.to_bits(),
            (Value::F64(x), Value::F64(y)) => x.to_bits() == y.to_bits(),
            (x, y) => x == y,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{by_id, ch};

    fn def(id: crate::channel::ChannelId) -> &'static ChannelDef {
        by_id(id).expect("registered channel")
    }

    #[test]
    fn non_finite_and_out_of_range_are_rejected() {
        let speed = def(ch::SPEED);
        assert_eq!(check_value(speed, f64::NAN), Err(Rejection::NonFinite));
        assert_eq!(check_value(speed, f64::INFINITY), Err(Rejection::NonFinite));
        // -700 km/h is impossible; 400 km/h is fast but real.
        assert!(matches!(
            check_value(speed, -700.0 / 3.6),
            Err(Rejection::OutOfRange { .. })
        ));
        assert_eq!(check_value(speed, 400.0 / 3.6), Ok(()));
        assert_eq!(check_value(speed, 0.0), Ok(()));
    }

    #[test]
    fn impossible_jump_is_rejected() {
        let fwd = def(ch::SPEED_FORWARD);
        // 78 m/s → -194 m/s in 10 ms: out of range anyway.
        assert!(check_value(fwd, -194.0).is_err());
        // 78 m/s → -100 m/s in 10 ms is in range but needs ~1800 g.
        assert!(matches!(
            check_rate(fwd, 78.0, 1.00, -100.0, 1.01),
            Err(Rejection::RateExceeded { .. })
        ));
        // A 5 g braking step is fine.
        assert_eq!(check_rate(fwd, 78.0, 1.00, 77.5, 1.01), Ok(()));
        // No positive time step: rate cannot be assessed.
        assert_eq!(check_rate(fwd, 78.0, 1.00, -100.0, 1.00), Ok(()));
        // Channels without a rate limit always pass.
        assert_eq!(check_rate(def(ch::GEAR), 1.0, 0.0, 6.0, 0.001), Ok(()));
    }

    #[test]
    fn rate_guard_rejects_spike_once_and_step_once() {
        let fwd = def(ch::SPEED_FORWARD);
        let mut g = RateGuard::default();
        assert_eq!(g.check(fwd, 78.0, 0.00), Ok(()));
        assert!(g.check(fwd, -100.0, 0.01).is_err(), "spike rejected");
        assert_eq!(
            g.check(fwd, 78.0, 0.02),
            Ok(()),
            "return to normal accepted"
        );

        let pos = def(ch::POS_X);
        let mut g = RateGuard::default();
        assert_eq!(g.check(pos, 100.0, 0.00), Ok(()));
        assert!(
            g.check(pos, 2_000.0, 0.01).is_err(),
            "teleport rejected once"
        );
        assert_eq!(
            g.check(pos, 2_000.5, 0.02),
            Ok(()),
            "motion after the teleport accepted"
        );
        g.reset();
        assert_eq!(g, RateGuard::default());
    }

    #[test]
    fn validate_sample_passes_non_numeric_and_non_valid() {
        let mut g = RateGuard::default();
        let name = def(ch::VEHICLE_NAME);
        let s = Sample::valid(Value::Str("x".into()));
        assert_eq!(
            validate_sample(name, s.clone(), &mut g, Some(1.0)),
            (s, None)
        );
        let speed = def(ch::SPEED);
        assert_eq!(
            validate_sample(speed, Sample::unavailable(), &mut g, Some(1.0)),
            (Sample::unavailable(), None)
        );
        let (out, why) = validate_sample(speed, Sample::valid(Value::F32(-5.0)), &mut g, Some(1.0));
        assert_eq!(out, Sample::invalid());
        assert!(matches!(why, Some(Rejection::OutOfRange { .. })));
        let gear = def(ch::GEAR);
        let (out, _) = validate_sample(gear, Sample::valid(Value::I32(42)), &mut g, None);
        assert_eq!(
            out,
            Sample::invalid(),
            "integer channels are range-checked too"
        );
    }

    #[test]
    fn frame_validator_downgrades_in_place() {
        let mut v = FrameValidator::new();
        let mut f = TelemetryFrame::new(1, 0, Some(10.00));
        f.set_valid(ch::SPEED_FORWARD, Value::F32(78.0))
            .expect("set");
        f.set_valid(ch::RPM, Value::F32(8_000.0)).expect("set");
        assert!(v.validate(&mut f).rejected.is_empty());

        let mut g = TelemetryFrame::new(2, 0, Some(10.01));
        g.set_valid(ch::SPEED_FORWARD, Value::F32(-100.0))
            .expect("set");
        g.set_valid(ch::RPM, Value::F32(99_999.0)).expect("set");
        let report = v.validate(&mut g);
        assert_eq!(report.rejected.len(), 2);
        assert_eq!(g.get(ch::SPEED_FORWARD), &Sample::invalid());
        assert_eq!(g.get(ch::RPM), &Sample::invalid());
        v.reset();
    }

    #[test]
    fn reset_channels_accepts_a_known_step_and_keeps_other_guards() {
        let mut v = FrameValidator::new();
        let frame = |seq: u64, t: f64, fuel: f32, pos: f64| {
            let mut f = TelemetryFrame::new(seq, 0, Some(t));
            f.set_valid(ch::FUEL, Value::F32(fuel)).expect("set");
            f.set_valid(ch::POS_X, Value::F64(pos)).expect("set");
            f
        };
        assert!(
            v.validate(&mut frame(1, 10.00, 40.0, 100.0))
                .rejected
                .is_empty()
        );
        // Refuel step (+30 L in 10 ms) announced as a discontinuity of the fuel channel only.
        assert_eq!(
            v.reset_channels(&[ch::FUEL, ChannelId(0xFFFF)]),
            1,
            "unknown id ignored"
        );
        let mut refuel = frame(2, 10.01, 70.0, 2_000.0);
        let report = v.validate(&mut refuel);
        assert_eq!(refuel.get(ch::FUEL), &Sample::valid(Value::F32(70.0)));
        // The position guard was not reset: a 1.9 km jump in 10 ms is still rejected.
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].0, ch::POS_X);
        assert_eq!(refuel.get(ch::POS_X), &Sample::invalid());
        // The new fuel level is the reference now: a spike after it is rejected again.
        let mut spike = frame(3, 10.02, 10.0, 100.2);
        let report = v.validate(&mut spike);
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].0, ch::FUEL);
        // Range checks still apply after a reset.
        v.reset_channels(&[ch::FUEL]);
        let mut garbage = frame(4, 10.03, 5_000.0, 100.3);
        assert!(matches!(
            v.validate(&mut garbage).rejected.as_slice(),
            [(id, Rejection::OutOfRange { .. })] if *id == ch::FUEL
        ));
    }

    #[test]
    fn et_classification() {
        assert_eq!(classify_et(None, 1.0), EtStep::First);
        assert_eq!(classify_et(Some(1.0), 1.5), EtStep::Advanced { dt_s: 0.5 });
        assert_eq!(classify_et(Some(1.0), 1.0), EtStep::Unchanged);
        assert_eq!(
            classify_et(Some(100.0), 2.0),
            EtStep::Regressed { by_s: 98.0 }
        );
        assert_eq!(classify_et(Some(1.0), f64::NAN), EtStep::NonFinite);
        assert_eq!(classify_et(Some(f64::NAN), 3.0), EtStep::First);
    }

    #[test]
    fn duplicate_detection() {
        let mut a = TelemetryFrame::new(1, 0, Some(5.0));
        a.set_valid(ch::RPM, Value::F32(7_000.0)).expect("set");
        let mut b = a.clone();
        b.sequence = 2;
        assert_eq!(classify_repeat(&a, &b), FrameRepeat::Duplicate);
        b.set_valid(ch::RPM, Value::F32(7_001.0)).expect("set");
        assert_eq!(classify_repeat(&a, &b), FrameRepeat::SameTimeDifferentData);
        b.source_et_s = Some(5.01);
        assert_eq!(classify_repeat(&a, &b), FrameRepeat::Distinct);
        b.source_et_s = None;
        assert_eq!(classify_repeat(&a, &b), FrameRepeat::Unknown);
    }
}

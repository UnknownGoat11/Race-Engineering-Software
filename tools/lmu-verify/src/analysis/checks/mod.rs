//! Explicit Gate-1 checks. Each check turns streaming evidence into a PASS / FAIL / INCONCLUSIVE verdict
//! with the numbers behind it and the assumption ids (docs/ASSUMPTIONS.md `A-LMU-*`) and channel rows
//! (docs/LMU_CHANNEL_MAP.md) it settles.
//!
//! Conventions the checks rely on (registry, A-LMU-024): `dynamics.g_long` + = accelerating,
//! `dynamics.g_lat` + = towards the car's left, `dynamics.yaw_rate_radps` + = yawing left,
//! `inputs.steering_raw` + = right; wheels are FL, FR, RL, RR (A-LMU-019). Unit checks look at the raw LMU
//! fields (that is what the evidence is about); sign checks look at the published channels (that is what
//! the app shows), so a PASS covers the raw convention and the normalizer mapping together.
//!
//! Every check is deterministic and bounded in memory, and never spans a discontinuity (reconnect, ET
//! regression, session/track/vehicle change): [`super::Step::discontinuity`] resets its differential state.

mod dynamics;
mod pit;
mod timing_checks;
mod track;
mod tyres;
mod units;

use std::collections::BTreeMap;

use lmu_normalizer::Normalizer;
use serde::Serialize;
use telemetry_model::{ChannelId, ch};

use super::Step;
use super::timing::TimingReport;

/// Verdict of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    /// The evidence supports the documented assumption / convention.
    Pass,
    /// The evidence contradicts it.
    Fail,
    /// Not enough (or not clear enough) evidence.
    Inconclusive,
}

impl Verdict {
    /// Upper-case label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Inconclusive => "INCONCLUSIVE",
        }
    }

    /// Combined verdict of the same check over several captures: any FAIL fails, otherwise any PASS passes.
    pub fn combine(verdicts: impl IntoIterator<Item = Self>) -> Self {
        let mut any_pass = false;
        for v in verdicts {
            match v {
                Self::Fail => return Self::Fail,
                Self::Pass => any_pass = true,
                Self::Inconclusive => {}
            }
        }
        if any_pass {
            Self::Pass
        } else {
            Self::Inconclusive
        }
    }
}

/// Result of one Gate-1 check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckResult {
    /// Stable id (`brake_temp_unit`, ...).
    pub id: &'static str,
    /// Title.
    pub title: &'static str,
    /// Verdict.
    pub verdict: Verdict,
    /// One-line finding with the decisive numbers.
    pub summary: String,
    /// Detailed numeric evidence.
    pub evidence: Vec<String>,
    /// Key numbers (machine-readable).
    pub metrics: BTreeMap<&'static str, f64>,
    /// Assumption ids the check settles.
    pub settles: Vec<&'static str>,
    /// Channel-map rows (canonical channel names) the check covers.
    pub docs_rows: Vec<&'static str>,
}

/// Builder used by the individual checks.
#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    result: CheckResult,
}

impl Outcome {
    pub(crate) fn new(
        id: &'static str,
        title: &'static str,
        settles: &[&'static str],
        docs_rows: &[&'static str],
    ) -> Self {
        Self {
            result: CheckResult {
                id,
                title,
                verdict: Verdict::Inconclusive,
                summary: String::new(),
                evidence: Vec::new(),
                metrics: BTreeMap::new(),
                settles: settles.to_vec(),
                docs_rows: docs_rows.to_vec(),
            },
        }
    }

    /// Adds an evidence line.
    pub(crate) fn line(&mut self, text: impl Into<String>) -> &mut Self {
        self.result.evidence.push(text.into());
        self
    }

    /// Records a metric (non-finite values are skipped).
    pub(crate) fn metric(&mut self, key: &'static str, value: f64) -> &mut Self {
        if value.is_finite() {
            self.result.metrics.insert(key, value);
        }
        self
    }

    /// Records an optional metric.
    pub(crate) fn metric_opt(&mut self, key: &'static str, value: Option<f64>) -> &mut Self {
        if let Some(v) = value {
            self.metric(key, v);
        }
        self
    }

    /// Sets the verdict and the one-line summary.
    pub(crate) fn verdict(mut self, verdict: Verdict, summary: impl Into<String>) -> CheckResult {
        self.result.verdict = verdict;
        self.result.summary = summary.into();
        self.result
    }
}

/// Formats an optional number with `decimals` places (`n/a` when missing).
pub(crate) fn fmt_opt(v: Option<f64>, decimals: usize) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{x:.decimals$}"))
}

/// Formats a fraction as a percentage.
pub(crate) fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".to_owned(), |x| format!("{:.1} %", 100.0 * x))
}

/// A published channel compared frame by frame with the value expected from its raw source: a check only
/// lists a published (normalizer-derived) row in [`CheckResult::docs_rows`] when it compares that row.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PublishedVsRaw {
    pairs: u64,
    max_error: Option<f64>,
}

impl PublishedVsRaw {
    /// Compares one published value (`None`: not published in this frame, skipped) with `expected`.
    pub(crate) fn push(&mut self, published: Option<f64>, expected: f64) {
        let Some(p) = published else {
            return;
        };
        let err = (p - expected).abs();
        if err.is_finite() {
            self.pairs += 1;
            self.max_error = Some(self.max_error.map_or(err, |m| m.max(err)));
        }
    }

    /// Pairs compared.
    pub(crate) fn pairs(&self) -> u64 {
        self.pairs
    }

    /// Largest |published − expected|.
    pub(crate) fn max_error(&self) -> Option<f64> {
        self.max_error
    }

    /// At least one pair and every pair within `tolerance`.
    pub(crate) fn agrees(&self, tolerance: f64) -> bool {
        self.max_error.is_some_and(|e| e <= tolerance)
    }

    /// Some pair is off by more than `tolerance`.
    pub(crate) fn disagrees(&self, tolerance: f64) -> bool {
        self.max_error.is_some_and(|e| e > tolerance)
    }

    /// `"max |error| x over n pairs"`.
    pub(crate) fn describe(&self) -> String {
        format!(
            "max |error| {} over {} pairs",
            fmt_opt(self.max_error, 6),
            self.pairs
        )
    }
}

/// The verdict when the raw evidence supports a PASS, gated on the published rows `(label, comparison)`:
/// FAIL when a published row disagrees with its raw source by more than `tolerance`, INCONCLUSIVE when
/// one was never published (nothing to compare), PASS with `pass_summary` otherwise.
pub(crate) fn gate_on_published(
    o: Outcome,
    rows: &[(&str, PublishedVsRaw)],
    tolerance: f64,
    pass_summary: String,
) -> CheckResult {
    let rows: Vec<(&str, PublishedVsRaw, f64)> =
        rows.iter().map(|&(l, c)| (l, c, tolerance)).collect();
    gate_rows(o, &rows, pass_summary)
}

/// Largest |published − raw| accepted for a passthrough channel of physical magnitude: f32 storage rounds
/// values up to 10⁵ (lap distance, fuel, rotation, acceleration) by less than this.
pub(crate) const PASSTHROUGH_TOLERANCE: f64 = 0.01;
/// The same for fractions, small inputs, integers and booleans (f32 rounding of |x| ≤ 10 stays below this;
/// integers and booleans are exact).
pub(crate) const EXACT_PASSTHROUGH_TOLERANCE: f64 = 1e-6;

/// The published-row comparisons as evidence lines, one per row.
pub(crate) fn describe_published(o: &mut Outcome, rows: &[(&str, PublishedVsRaw, f64)]) {
    for (label, cmp, _) in rows {
        o.line(format!(
            "published {label} vs its raw source: {}",
            cmp.describe()
        ));
    }
}

/// [`gate_on_published`] with one tolerance per row `(label, comparison, tolerance)`.
pub(crate) fn gate_rows(
    o: Outcome,
    rows: &[(&str, PublishedVsRaw, f64)],
    pass_summary: String,
) -> CheckResult {
    if let Some((label, cmp, _)) = rows.iter().find(|(_, c, t)| c.disagrees(*t)) {
        return o.verdict(
            Verdict::Fail,
            format!(
                "{pass_summary}, but the published {label} does not match its raw source ({})",
                cmp.describe()
            ),
        );
    }
    if let Some((label, _, _)) = rows.iter().find(|(_, c, t)| !c.agrees(*t)) {
        return o.verdict(
            Verdict::Inconclusive,
            format!("{pass_summary}, but {label} was never published to compare"),
        );
    }
    o.verdict(Verdict::Pass, pass_summary)
}

/// Wheel labels in LMU order (A-LMU-019).
pub(crate) const WHEELS: [&str; 4] = ["FL", "FR", "RL", "RR"];
/// Wheels on the car's left (FL, RL).
pub(crate) const LEFT_WHEELS: [usize; 2] = [0, 2];
/// Wheels on the car's right (FR, RR).
pub(crate) const RIGHT_WHEELS: [usize; 2] = [1, 3];

/// Per-wheel channel ids.
pub(crate) const TYRE_LOAD: [ChannelId; 4] = [
    ch::TYRE_LOAD_FL,
    ch::TYRE_LOAD_FR,
    ch::TYRE_LOAD_RL,
    ch::TYRE_LOAD_RR,
];
pub(crate) const WHEEL_SPEED: [ChannelId; 4] = [
    ch::WHEEL_SPEED_FL,
    ch::WHEEL_SPEED_FR,
    ch::WHEEL_SPEED_RL,
    ch::WHEEL_SPEED_RR,
];
pub(crate) const TYRE_TEMP_LEFT: [ChannelId; 4] = [
    ch::TYRE_TEMP_LEFT_FL,
    ch::TYRE_TEMP_LEFT_FR,
    ch::TYRE_TEMP_LEFT_RL,
    ch::TYRE_TEMP_LEFT_RR,
];
pub(crate) const TYRE_TEMP_MIDDLE: [ChannelId; 4] = [
    ch::TYRE_TEMP_MIDDLE_FL,
    ch::TYRE_TEMP_MIDDLE_FR,
    ch::TYRE_TEMP_MIDDLE_RL,
    ch::TYRE_TEMP_MIDDLE_RR,
];
pub(crate) const TYRE_TEMP_RIGHT: [ChannelId; 4] = [
    ch::TYRE_TEMP_RIGHT_FL,
    ch::TYRE_TEMP_RIGHT_FR,
    ch::TYRE_TEMP_RIGHT_RL,
    ch::TYRE_TEMP_RIGHT_RR,
];
pub(crate) const TYRE_PRESSURE: [ChannelId; 4] = [
    ch::TYRE_PRESSURE_FL,
    ch::TYRE_PRESSURE_FR,
    ch::TYRE_PRESSURE_RL,
    ch::TYRE_PRESSURE_RR,
];
pub(crate) const BRAKE_TEMP: [ChannelId; 4] = [
    ch::BRAKE_TEMP_FL,
    ch::BRAKE_TEMP_FR,
    ch::BRAKE_TEMP_RL,
    ch::BRAKE_TEMP_RR,
];

/// All Gate-1 check accumulators.
#[derive(Debug, Clone, Default)]
pub struct ChecksAcc {
    brake_temp: units::BrakeTempCheck,
    tyre_temp: units::TyreTempCheck,
    tyre_pressure: units::TyrePressureCheck,
    brake_pressure: units::BrakePressureCheck,
    virtual_energy: units::VirtualEnergyCheck,
    soc: units::SocCheck,
    g_long: dynamics::GLongCheck,
    g_lat: dynamics::GLatCheck,
    steering: dynamics::SteeringCheck,
    wheel_speed: dynamics::WheelSpeedCheck,
    lap_dist: track::LapDistCheck,
    sectors: track::SectorLapCheck,
    fuel_pit: pit::FuelPitCheck,
    tyre_edges: tyres::TyreEdgeCheck,
    wear: tyres::WearCheck,
}

impl ChecksAcc {
    /// Feeds one normalizer step to every check.
    pub fn observe(&mut self, step: &Step<'_>) {
        self.brake_temp.observe(step);
        self.tyre_temp.observe(step);
        self.tyre_pressure.observe(step);
        self.brake_pressure.observe(step);
        self.virtual_energy.observe(step);
        self.soc.observe(step);
        self.g_long.observe(step);
        self.g_lat.observe(step);
        self.steering.observe(step);
        self.wheel_speed.observe(step);
        self.lap_dist.observe(step);
        self.sectors.observe(step);
        self.fuel_pit.observe(step);
        self.tyre_edges.observe(step);
        self.wear.observe(step);
    }

    /// The results, in a fixed order.
    pub fn finish(
        self,
        normalizer: &Normalizer,
        timing: Option<&TimingReport>,
    ) -> Vec<CheckResult> {
        let [fuel, pit_flags, pit_states] = self.fuel_pit.finish();
        vec![
            timing_checks::rates(timing),
            timing_checks::elapsed_time(timing),
            self.brake_temp.finish(normalizer),
            self.tyre_temp.finish(),
            self.tyre_pressure.finish(),
            self.brake_pressure.finish(),
            self.virtual_energy.finish(normalizer),
            self.soc.finish(normalizer),
            self.g_long.finish(),
            self.g_lat.finish(),
            self.steering.finish(),
            self.wheel_speed.finish(),
            self.lap_dist.finish(),
            self.sectors.finish(),
            fuel,
            pit_flags,
            pit_states,
            self.tyre_edges.finish(),
            self.wear.finish(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_combine_conservatively() {
        use Verdict::{Fail, Inconclusive, Pass};
        assert_eq!(Verdict::combine([Pass, Inconclusive]), Pass);
        assert_eq!(Verdict::combine([Pass, Fail]), Fail);
        assert_eq!(Verdict::combine([Inconclusive]), Inconclusive);
        assert_eq!(Verdict::combine([]), Inconclusive);
    }

    #[test]
    fn published_rows_gate_a_pass() {
        let outcome = || Outcome::new("x", "x", &[], &["a.b"]);
        let mut good = PublishedVsRaw::default();
        good.push(Some(0.5), 0.5);
        good.push(None, 0.7);
        assert_eq!((good.pairs(), good.max_error()), (1, Some(0.0)));
        let mut off = good;
        off.push(Some(0.9), 0.6);
        let never = PublishedVsRaw::default();
        let pass = || "raw ok".to_owned();
        let verdict = |rows: &[(&str, PublishedVsRaw)]| {
            gate_on_published(outcome(), rows, 1e-4, pass()).verdict
        };
        assert_eq!(verdict(&[("a.b", good)]), Verdict::Pass);
        assert_eq!(verdict(&[("a.b", good), ("c.d", off)]), Verdict::Fail);
        assert_eq!(
            verdict(&[("a.b", good), ("c.d", never)]),
            Verdict::Inconclusive
        );
        let failed = gate_on_published(outcome(), &[("c.d", off)], 1e-4, pass());
        assert!(
            failed.summary.contains("published c.d does not match"),
            "{}",
            failed.summary
        );
    }

    /// Second review F3: each published row is judged with its own tolerance, so a passthrough that is off
    /// (a remapped pit state, an inverted steering_filtered) fails even next to a coarse physical row.
    #[test]
    fn rows_are_gated_with_their_own_tolerance() {
        let outcome = || Outcome::new("x", "x", &[], &["pit.state", "fuel.level_l"]);
        let mut fuel = PublishedVsRaw::default();
        fuel.push(Some(50.004), 50.0);
        let mut state = PublishedVsRaw::default();
        state.push(Some(4.0), 4.0);
        let rows = |state| {
            [
                ("pit.state", state, EXACT_PASSTHROUGH_TOLERANCE),
                ("fuel.level_l", fuel, PASSTHROUGH_TOLERANCE),
            ]
        };
        let mut o = outcome();
        describe_published(&mut o, &rows(state));
        let ok = gate_rows(o, &rows(state), "ok".to_owned());
        assert_eq!(ok.verdict, Verdict::Pass);
        assert!(
            ok.evidence
                .iter()
                .any(|l| l.starts_with("published pit.state vs its raw source"))
        );
        let mut remapped = state;
        remapped.push(Some(3.0), 4.0);
        assert_eq!(
            gate_rows(outcome(), &rows(remapped), "ok".to_owned()).verdict,
            Verdict::Fail
        );
    }
}

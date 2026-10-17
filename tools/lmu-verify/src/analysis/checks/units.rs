//! Unit and scale checks on the raw LMU fields: brake temperature (A-LMU-015), tyre temperatures and
//! pressure (A-LMU-049), brake pressure (A-LMU-017), virtual energy (A-LMU-013) and state of charge
//! (A-LMU-014).

use lmu_normalizer::{
    BrakeTempUnit, BrakeTempUnitDetector, FractionScale, FractionScaleDetector, Normalizer,
    UnitDecisionState, detect::BrakeObservation, mapping::SOC_AGREEMENT_TOLERANCE,
};
use telemetry_model::ch;
use telemetry_model::units::{KELVIN_OFFSET, PA_PER_KPA};

use super::pit::TELEPORT_STEP_M;
use super::{
    BRAKE_TEMP, CheckResult, Outcome, PublishedVsRaw, TYRE_PRESSURE, TYRE_TEMP_LEFT,
    TYRE_TEMP_MIDDLE, TYRE_TEMP_RIGHT, Verdict, WHEELS, describe_published, fmt_opt,
    gate_on_published, gate_rows,
};
use crate::analysis::Step;
use crate::analysis::channels::MOVING_SPEED_MPS;
use crate::stats::{Correlation, Summary};

// ----------------------------------------------------------------------------------------------------------
// Brake temperature unit (A-LMU-015)
// ----------------------------------------------------------------------------------------------------------

/// Cold-brake samples: the car has not moved faster than this since the session (or capture) started (m/s).
const COLD_MAX_SPEED_MPS: f64 = 1.0;
/// Cold-brake samples: the four wheels agree within this (raw units).
const COLD_MAX_SPREAD: f64 = 5.0;
/// Cold offset (coldest raw − ambient) within this of 273.15 means Kelvin, within this of 0 Celsius (K).
const COLD_OFFSET_TOLERANCE: f64 = 10.0;
/// Cold samples needed for a cold-brake verdict.
const MIN_COLD_SAMPLES: u64 = 10;
/// A raw reading above this is impossible in °C (but plausible in K, see the normalizer's detector).
const CELSIUS_IMPOSSIBLE_ABOVE: f64 = 1_500.0;

/// Brake temperature unit from cold garage values vs ambient, hot values, an independent evidence-only
/// [`BrakeTempUnitDetector`] and the normalizer's own decision.
#[derive(Debug, Clone, Default)]
pub struct BrakeTempCheck {
    moved: bool,
    prev_realtime: Option<bool>,
    cold_offset: Summary,
    cold_raw: Summary,
    hot_raw: Summary,
    impossible_celsius: u64,
    detector: BrakeTempUnitDetector,
    published_c: Summary,
    /// Published `brakes.*.temp_c` vs raw − 273.15 (the Kelvin convention under test).
    published_vs_raw: PublishedVsRaw,
    samples: u64,
}

impl BrakeTempCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let info = &step.snapshot.scoring.info;
        // A realtime entry starts a new cold window, like a session start.
        if self.prev_realtime == Some(false) && info.in_realtime {
            self.moved = false;
        }
        self.prev_realtime = Some(info.in_realtime);
        if step.discontinuity {
            self.moved = false;
        }
        self.detector.note_realtime(info.in_realtime);
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        let speed = frame.f64(ch::SPEED);
        if speed.is_some_and(|v| v > COLD_MAX_SPEED_MPS) {
            self.moved = true;
        }
        let raw = [
            t.wheels[0].brake_temp,
            t.wheels[1].brake_temp,
            t.wheels[2].brake_temp,
            t.wheels[3].brake_temp,
        ];
        let ambient = info.ambient_temp.is_finite().then_some(info.ambient_temp);
        self.detector.observe(&BrakeObservation {
            raw,
            et_s: t.elapsed_time,
            speed_mps: speed,
            in_realtime: info.in_realtime,
            ambient_c: ambient,
        });
        for (&id, &k) in BRAKE_TEMP.iter().zip(&raw) {
            let published = frame.f64(id);
            if let Some(c) = published {
                self.published_c.push(c);
            }
            self.published_vs_raw.push(published, k - KELVIN_OFFSET);
        }
        if raw.iter().any(|v| !v.is_finite()) || raw.iter().all(|&v| v == 0.0) {
            return;
        }
        self.samples += 1;
        let coldest = raw.iter().copied().fold(f64::INFINITY, f64::min);
        let hottest = raw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if hottest > CELSIUS_IMPOSSIBLE_ABOVE && hottest <= CELSIUS_IMPOSSIBLE_ABOVE + KELVIN_OFFSET
        {
            self.impossible_celsius += 1;
        }
        if speed.is_some_and(|v| v > MOVING_SPEED_MPS) {
            self.hot_raw.push(hottest);
        }
        let cold = !self.moved
            && speed.is_some_and(|v| v <= COLD_MAX_SPEED_MPS)
            && hottest - coldest <= COLD_MAX_SPREAD;
        if let (true, Some(a)) = (cold, ambient) {
            self.cold_offset.push(coldest - a);
            self.cold_raw.push(coldest);
        }
    }

    pub(super) fn finish(self, normalizer: &Normalizer) -> CheckResult {
        let mut o = Outcome::new(
            "brake_temp_unit",
            "Brake temperature unit (cold garage value vs ambient)",
            &["A-LMU-015"],
            &["brakes.{fl,fr,rl,rr}.temp_c"],
        );
        let offset = self.cold_offset.median();
        let n_cold = self.cold_offset.count();
        let independent = self.detector.decision();
        let normalizer_decision = normalizer.brake_temp_unit();
        o.metric("cold_samples", n_cold as f64)
            .metric_opt("cold_offset_median_k", offset)
            .metric_opt("cold_raw_median", self.cold_raw.median())
            .metric_opt("hot_raw_p99", self.hot_raw.p99())
            .metric(
                "impossible_as_celsius_samples",
                self.impossible_celsius as f64,
            )
            .metric(
                "independent_kelvin_votes",
                f64::from(independent.kelvin_votes),
            )
            .metric(
                "independent_celsius_votes",
                f64::from(independent.celsius_votes),
            );
        o.line(format!(
            "cold garage samples (not yet driven, speed ≤ {COLD_MAX_SPEED_MPS} m/s, wheels within {COLD_MAX_SPREAD}): {n_cold}; coldest raw median {}; coldest − ambient median {} (Kelvin ⇒ ≈ {KELVIN_OFFSET}, Celsius ⇒ ≈ 0)",
            fmt_opt(self.cold_raw.median(), 2),
            fmt_opt(offset, 2)
        ));
        if let Some(s) = self.cold_offset.stats() {
            o.line(format!(
                "coldest − ambient: min {:.2}, p1 {:.2}, max {:.2}",
                s.min, s.p1, s.max
            ));
        }
        o.line(format!(
            "hottest wheel while moving: raw median {}, p99 {}, max {} (as K: p99 {} °C); readings impossible as °C (> {CELSIUS_IMPOSSIBLE_ABOVE}): {}",
            fmt_opt(self.hot_raw.median(), 1),
            fmt_opt(self.hot_raw.p99(), 1),
            fmt_opt(self.hot_raw.max(), 1),
            fmt_opt(self.hot_raw.p99().map(|k| k - KELVIN_OFFSET), 1),
            self.impossible_celsius
        ));
        o.line(format!(
            "independent evidence-only detector: {:?} {:?} (Kelvin votes {}, Celsius votes {})",
            independent.state,
            independent.unit,
            independent.kelvin_votes,
            independent.celsius_votes
        ));
        o.line(format!(
            "lmu-normalizer decision at the end: {:?} {:?} (Kelvin votes {}, Celsius votes {}); published brake temperatures: {} samples, median {} °C",
            normalizer_decision.state,
            normalizer_decision.unit,
            normalizer_decision.kelvin_votes,
            normalizer_decision.celsius_votes,
            self.published_c.count(),
            fmt_opt(self.published_c.median(), 1)
        ));
        o.metric("published_pairs", self.published_vs_raw.pairs() as f64)
            .metric_opt("published_max_error_c", self.published_vs_raw.max_error());
        o.line(format!(
            "published brakes.*.temp_c vs raw − {KELVIN_OFFSET}: {}",
            self.published_vs_raw.describe()
        ));
        let published_ok = self.published_vs_raw.agrees(CONVERSION_TOLERANCE_C);
        let enough_cold = n_cold >= MIN_COLD_SAMPLES;
        let cold_kelvin = enough_cold
            && offset.is_some_and(|d| (d - KELVIN_OFFSET).abs() <= COLD_OFFSET_TOLERANCE);
        let cold_celsius = enough_cold && offset.is_some_and(|d| d.abs() <= COLD_OFFSET_TOLERANCE);
        let kelvin = cold_kelvin
            || self.impossible_celsius > 0
            || (independent.state == UnitDecisionState::Decided
                && independent.unit == BrakeTempUnit::Kelvin);
        let celsius = cold_celsius
            || independent.celsius_votes > 0
            || normalizer_decision.celsius_votes > 0
            || (independent.state == UnitDecisionState::Decided
                && independent.unit == BrakeTempUnit::Celsius);
        let conflict = normalizer_decision.state == UnitDecisionState::Conflicting
            || independent.state == UnitDecisionState::Conflicting;
        if celsius || conflict {
            let what = if kelvin { "conflicting" } else { "Celsius" };
            o.verdict(
                Verdict::Fail,
                format!(
                    "evidence is {what}: coldest − ambient median {} K over {n_cold} cold samples",
                    fmt_opt(offset, 2)
                ),
            )
        } else if kelvin && self.published_vs_raw.disagrees(CONVERSION_TOLERANCE_C) {
            o.verdict(
                Verdict::Fail,
                format!(
                    "the raw values are Kelvin but the published brakes.*.temp_c are not raw − {KELVIN_OFFSET} ({})",
                    self.published_vs_raw.describe()
                ),
            )
        } else if kelvin && !published_ok {
            o.verdict(
                Verdict::Inconclusive,
                "the raw values are Kelvin but brakes.*.temp_c were never published to compare",
            )
        } else if kelvin {
            o.verdict(
                Verdict::Pass,
                format!(
                    "Kelvin: coldest − ambient median {} (≈ {KELVIN_OFFSET}) over {n_cold} cold samples; hot p99 {} °C after K conversion",
                    fmt_opt(offset, 2),
                    fmt_opt(self.hot_raw.p99().map(|k| k - KELVIN_OFFSET), 0)
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                format!(
                    "no decisive evidence ({n_cold} cold samples, coldest − ambient median {})",
                    fmt_opt(offset, 2)
                ),
            )
        }
    }
}

// ----------------------------------------------------------------------------------------------------------
// Tyre temperatures (A-LMU-049, A-LMU-062)
// ----------------------------------------------------------------------------------------------------------

/// Tyre temperature samples only above this speed (m/s): a running tyre.
const RUNNING_SPEED_MPS: f64 = 10.0;
/// Raw tyre temperatures below this are not Kelvin values of a running tyre (K).
const TYRE_KELVIN_MIN: f64 = 200.0;
/// Plausible running tyre temperature after conversion (°C).
const TYRE_C_PLAUSIBLE: (f64, f64) = (10.0, 150.0);
/// Median outside this after conversion contradicts the Kelvin unit (°C).
const TYRE_C_IMPOSSIBLE: (f64, f64) = (-40.0, 250.0);
/// Running samples per wheel needed for a verdict.
const MIN_RUNNING_SAMPLES: u64 = 200;
/// Published °C must equal raw − 273.15 within this (°C).
const CONVERSION_TOLERANCE_C: f64 = 0.01;

#[derive(Debug, Clone, Default)]
struct WheelTemps {
    surface: Summary,
    carcass: Summary,
    layer: Summary,
}

/// Tyre surface / carcass / inner-layer temperatures are Kelvin and plausible in °C after conversion.
#[derive(Debug, Clone, Default)]
pub struct TyreTempCheck {
    wheels: [WheelTemps; 4],
    max_conversion_error_c: Option<f64>,
    conversion_pairs: u64,
    /// Every published surface (L/C/R), carcass and inner-layer (L/C/R) row vs raw − 273.15 (non-zero raw).
    surface_vs_raw: PublishedVsRaw,
    carcass_vs_raw: PublishedVsRaw,
    layer_vs_raw: PublishedVsRaw,
}

/// Published carcass temperature ids in LMU wheel order.
const TYRE_CARCASS: [telemetry_model::ChannelId; 4] = [
    ch::TYRE_CARCASS_TEMP_FL,
    ch::TYRE_CARCASS_TEMP_FR,
    ch::TYRE_CARCASS_TEMP_RL,
    ch::TYRE_CARCASS_TEMP_RR,
];
/// Published inner-layer temperature ids (left, middle, right) per wheel in LMU wheel order.
const TYRE_LAYER: [[telemetry_model::ChannelId; 3]; 4] = [
    [
        ch::TYRE_LAYER_TEMP_LEFT_FL,
        ch::TYRE_LAYER_TEMP_MIDDLE_FL,
        ch::TYRE_LAYER_TEMP_RIGHT_FL,
    ],
    [
        ch::TYRE_LAYER_TEMP_LEFT_FR,
        ch::TYRE_LAYER_TEMP_MIDDLE_FR,
        ch::TYRE_LAYER_TEMP_RIGHT_FR,
    ],
    [
        ch::TYRE_LAYER_TEMP_LEFT_RL,
        ch::TYRE_LAYER_TEMP_MIDDLE_RL,
        ch::TYRE_LAYER_TEMP_RIGHT_RL,
    ],
    [
        ch::TYRE_LAYER_TEMP_LEFT_RR,
        ch::TYRE_LAYER_TEMP_MIDDLE_RR,
        ch::TYRE_LAYER_TEMP_RIGHT_RR,
    ],
];

/// Pushes published °C vs raw K − 273.15; an exact raw 0 is "not initialised" (A-LMU-062) and skipped.
fn push_kelvin(cmp: &mut PublishedVsRaw, published: Option<f64>, raw_k: f64) {
    if raw_k != 0.0 {
        cmp.push(published, raw_k - KELVIN_OFFSET);
    }
}

impl TyreTempCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        if !frame.f64(ch::SPEED).is_some_and(|v| v > RUNNING_SPEED_MPS) {
            return;
        }
        for (i, (acc, w)) in self.wheels.iter_mut().zip(&t.wheels).enumerate() {
            // A detached wheel's values are frozen (A-LMU-061); exact zeros are "not initialised" (A-LMU-062).
            if w.detached {
                continue;
            }
            for &k in &w.temperature {
                if k != 0.0 {
                    acc.surface.push(k);
                }
            }
            if w.tire_carcass_temperature != 0.0 {
                acc.carcass.push(w.tire_carcass_temperature);
            }
            for &k in &w.tire_inner_layer_temperature {
                if k != 0.0 {
                    acc.layer.push(k);
                }
            }
            let surface_ids = [
                TYRE_TEMP_LEFT.get(i),
                TYRE_TEMP_MIDDLE.get(i),
                TYRE_TEMP_RIGHT.get(i),
            ];
            for (id, &k) in surface_ids.into_iter().zip(&w.temperature) {
                push_kelvin(
                    &mut self.surface_vs_raw,
                    id.and_then(|&id| frame.f64(id)),
                    k,
                );
            }
            if let Some(&id) = TYRE_CARCASS.get(i) {
                push_kelvin(
                    &mut self.carcass_vs_raw,
                    frame.f64(id),
                    w.tire_carcass_temperature,
                );
            }
            if let Some(ids) = TYRE_LAYER.get(i) {
                for (&id, &k) in ids.iter().zip(&w.tire_inner_layer_temperature) {
                    push_kelvin(&mut self.layer_vs_raw, frame.f64(id), k);
                }
            }
            let published = TYRE_TEMP_MIDDLE.get(i).and_then(|&id| frame.f64(id));
            if let (Some(c), Some(&k)) = (published, w.temperature.get(1)) {
                let err = (c - (k - KELVIN_OFFSET)).abs();
                if err.is_finite() {
                    self.conversion_pairs += 1;
                    self.max_conversion_error_c =
                        Some(self.max_conversion_error_c.map_or(err, |m| m.max(err)));
                }
            }
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "tyre_temperature_unit",
            "Tyre surface / carcass / inner-layer temperatures: Kelvin, plausible °C",
            &["A-LMU-049", "A-LMU-062"],
            &[
                "tyres.{w}.temp_surface_{left,center,right}_c",
                "tyres.{w}.temp_carcass_c",
                "tyres.{w}.temp_inner_layer_{left,center,right}_c",
            ],
        );
        let mut all_plausible = true;
        let mut any_impossible = false;
        let mut enough = true;
        let mut raw_min = f64::INFINITY;
        for (label, w) in WHEELS.iter().zip(&self.wheels) {
            let s = w.surface.median().map(|k| k - KELVIN_OFFSET);
            let c = w.carcass.median().map(|k| k - KELVIN_OFFSET);
            let l = w.layer.median().map(|k| k - KELVIN_OFFSET);
            o.line(format!(
                "{label}: surface median {} °C (raw p1 {} K, p99 {} K), carcass median {} °C, inner layer median {} °C ({} running samples)",
                fmt_opt(s, 1),
                fmt_opt(w.surface.p01(), 1),
                fmt_opt(w.surface.p99(), 1),
                fmt_opt(c, 1),
                fmt_opt(l, 1),
                w.carcass.count()
            ));
            if w.carcass.count() < MIN_RUNNING_SAMPLES {
                enough = false;
            }
            for m in [s, c, l].into_iter().flatten() {
                all_plausible &= (TYRE_C_PLAUSIBLE.0..=TYRE_C_PLAUSIBLE.1).contains(&m);
                any_impossible |= !(TYRE_C_IMPOSSIBLE.0..=TYRE_C_IMPOSSIBLE.1).contains(&m);
            }
            for p in [w.surface.p01(), w.carcass.p01(), w.layer.p01()]
                .into_iter()
                .flatten()
            {
                raw_min = raw_min.min(p);
            }
        }
        o.metric("raw_p1_min_k", raw_min)
            .metric("conversion_pairs", self.conversion_pairs as f64)
            .metric_opt("max_conversion_error_c", self.max_conversion_error_c);
        o.line(format!(
            "published temp_surface_center_c vs raw − {KELVIN_OFFSET}: max |error| {} °C over {} pairs",
            fmt_opt(self.max_conversion_error_c, 4),
            self.conversion_pairs
        ));
        let published = [
            (
                "tyres.{w}.temp_surface_{left,center,right}_c",
                self.surface_vs_raw,
                CONVERSION_TOLERANCE_C,
            ),
            (
                "tyres.{w}.temp_carcass_c",
                self.carcass_vs_raw,
                CONVERSION_TOLERANCE_C,
            ),
            (
                "tyres.{w}.temp_inner_layer_{left,center,right}_c",
                self.layer_vs_raw,
                CONVERSION_TOLERANCE_C,
            ),
        ];
        describe_published(&mut o, &published);
        let conversion_ok = self
            .max_conversion_error_c
            .is_some_and(|e| e <= CONVERSION_TOLERANCE_C);
        let raw_kelvin = raw_min.is_finite() && raw_min >= TYRE_KELVIN_MIN;
        if !enough {
            return o.verdict(
                Verdict::Inconclusive,
                format!("fewer than {MIN_RUNNING_SAMPLES} running samples on some wheel"),
            );
        }
        if any_impossible || !raw_kelvin || self.max_conversion_error_c.is_some_and(|e| e > 0.5) {
            o.verdict(
                Verdict::Fail,
                format!(
                    "running raw p1 {raw_min:.1} (K expected ≥ {TYRE_KELVIN_MIN}); a median is outside {TYRE_C_IMPOSSIBLE:?} °C or the conversion is off"
                ),
            )
        } else if all_plausible && conversion_ok {
            gate_rows(
                o,
                &published,
                format!(
                    "Kelvin: running raw ≥ {raw_min:.1} K, every wheel median within {TYRE_C_PLAUSIBLE:?} °C, published = raw − {KELVIN_OFFSET}"
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                "medians outside the plausible running band or no published comparison",
            )
        }
    }
}

// ----------------------------------------------------------------------------------------------------------
// Tyre pressure (A-LMU-049)
// ----------------------------------------------------------------------------------------------------------

/// Plausible running tyre pressure (kPa).
const PRESSURE_KPA_PLAUSIBLE: (f64, f64) = (100.0, 350.0);
/// A median outside this is not kPa (bar ≈ 2, psi ≈ 25, Pa ≈ 170,000).
const PRESSURE_KPA_IMPOSSIBLE: (f64, f64) = (50.0, 600.0);
/// Published Pa must equal raw kPa × 1000 within this relative error.
const PRESSURE_RELATIVE_TOLERANCE: f64 = 1e-6;

/// Tyre pressure is kPa (A-LMU-049) and published as Pa.
#[derive(Debug, Clone, Default)]
pub struct TyrePressureCheck {
    wheels: [Summary; 4],
    max_relative_error: Option<f64>,
    pairs: u64,
}

impl TyrePressureCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        if !frame.f64(ch::SPEED).is_some_and(|v| v > RUNNING_SPEED_MPS) {
            return;
        }
        for (i, (acc, w)) in self.wheels.iter_mut().zip(&t.wheels).enumerate() {
            if w.detached || w.flat || w.pressure == 0.0 {
                continue;
            }
            acc.push(w.pressure);
            if let Some(pa) = TYRE_PRESSURE.get(i).and_then(|&id| frame.f64(id))
                && w.pressure != 0.0
            {
                let rel = (pa / (w.pressure * PA_PER_KPA) - 1.0).abs();
                if rel.is_finite() {
                    self.pairs += 1;
                    self.max_relative_error =
                        Some(self.max_relative_error.map_or(rel, |m| m.max(rel)));
                }
            }
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "tyre_pressure_unit",
            "Tyre pressure in kPa (published in Pa)",
            &["A-LMU-049"],
            &["tyres.{w}.pressure_pa"],
        );
        let mut plausible = true;
        let mut impossible = false;
        let mut enough = true;
        for (label, s) in WHEELS.iter().zip(&self.wheels) {
            o.line(format!(
                "{label}: running raw median {} kPa (p1 {}, p99 {}, n = {})",
                fmt_opt(s.median(), 1),
                fmt_opt(s.p01(), 1),
                fmt_opt(s.p99(), 1),
                s.count()
            ));
            enough &= s.count() >= MIN_RUNNING_SAMPLES;
            if let Some(m) = s.median() {
                plausible &= (PRESSURE_KPA_PLAUSIBLE.0..=PRESSURE_KPA_PLAUSIBLE.1).contains(&m);
                impossible |= !(PRESSURE_KPA_IMPOSSIBLE.0..=PRESSURE_KPA_IMPOSSIBLE.1).contains(&m);
            }
        }
        o.metric_opt("max_relative_conversion_error", self.max_relative_error)
            .metric("conversion_pairs", self.pairs as f64);
        o.line(format!(
            "published pressure_pa vs raw × {PA_PER_KPA}: max relative error {} over {} pairs",
            fmt_opt(self.max_relative_error, 9),
            self.pairs
        ));
        if !enough {
            return o.verdict(
                Verdict::Inconclusive,
                format!("fewer than {MIN_RUNNING_SAMPLES} running samples on some wheel"),
            );
        }
        let conversion_ok = self
            .max_relative_error
            .is_some_and(|e| e <= PRESSURE_RELATIVE_TOLERANCE);
        if impossible {
            o.verdict(
                Verdict::Fail,
                format!("a running median is outside {PRESSURE_KPA_IMPOSSIBLE:?}: not kPa"),
            )
        } else if plausible && conversion_ok {
            o.verdict(
                Verdict::Pass,
                format!(
                    "kPa: every running median within {PRESSURE_KPA_PLAUSIBLE:?} kPa, published = raw × 1000 Pa"
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                "medians outside the typical band or no published comparison",
            )
        }
    }
}

// ----------------------------------------------------------------------------------------------------------
// Brake pressure (A-LMU-017)
// ----------------------------------------------------------------------------------------------------------

/// Tolerance around 0..1 (fraction).
const FRACTION_TOLERANCE: f64 = 0.01;
/// Above this the value is no longer the documented fraction (kPa?), as in the normalizer.
const BRAKE_PRESSURE_NOT_FRACTION: f64 = 1.5;
/// The pedal must reach this for a verdict.
const MIN_PEDAL_PEAK: f64 = 0.5;
/// Pedal / pressure correlation needed for a PASS.
const MIN_PEDAL_CORRELATION: f64 = 0.8;
/// Full brake pedal (filtered, fraction).
const FULL_PEDAL: f64 = 0.95;
/// Rear brake bias accepted for the balance comparison (fraction).
const BIAS_RANGE: (f64, f64) = (0.05, 0.95);
/// At full pedal, axle pressure ÷ axle balance within this of 1 means "pedal × balance".
const BALANCE_RATIO_TOLERANCE: f64 = 0.1;
/// Full-pedal samples (while rolling) needed for the balance comparison.
const MIN_FULL_PEDAL_SAMPLES: u64 = 50;

/// `mBrakePressure` is a 0..1 fraction following the brake pedal (pedal × brake balance, A-LMU-017).
#[derive(Debug, Clone, Default)]
pub struct BrakePressureCheck {
    all: Summary,
    above_fraction: u64,
    not_fraction: u64,
    below_zero: u64,
    pedal_peak: f64,
    correlation: Correlation,
    correlation_unfiltered: Correlation,
    full_front_ratio: Summary,
    full_rear_ratio: Summary,
    rear_bias: Summary,
    /// Published `brakes.{w}.pressure_frac` vs `mBrakePressure` (all four wheels, when published).
    published_vs_raw: PublishedVsRaw,
}

/// Published brake pressure ids in LMU wheel order.
const BRAKE_PRESSURE: [telemetry_model::ChannelId; 4] = [
    ch::BRAKE_PRESSURE_FL,
    ch::BRAKE_PRESSURE_FR,
    ch::BRAKE_PRESSURE_RL,
    ch::BRAKE_PRESSURE_RR,
];

impl BrakePressureCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let Some(t) = step.raw else {
            return;
        };
        let front = (t.wheels[0].brake_pressure + t.wheels[1].brake_pressure) / 2.0;
        let rear = (t.wheels[2].brake_pressure + t.wheels[3].brake_pressure) / 2.0;
        let bias = t.rear_brake_bias;
        if bias.is_finite() {
            self.rear_bias.push(bias);
        }
        // Only while rolling: a car held on the brakes at standstill (garage, grid) need not report pressure.
        let moving = step
            .frame
            .and_then(|f| f.f64(ch::SPEED))
            .is_some_and(|v| v > RUNNING_SPEED_MPS);
        if moving && t.filtered_brake >= FULL_PEDAL && (BIAS_RANGE.0..=BIAS_RANGE.1).contains(&bias)
        {
            self.full_front_ratio.push(front / (1.0 - bias));
            self.full_rear_ratio.push(rear / bias);
        }
        self.correlation_unfiltered.push(t.unfiltered_brake, front);
        if let Some(frame) = step.frame {
            for (&id, w) in BRAKE_PRESSURE.iter().zip(&t.wheels) {
                self.published_vs_raw.push(frame.f64(id), w.brake_pressure);
            }
        }
        for w in &t.wheels {
            let p = w.brake_pressure;
            self.all.push(p);
            if p > 1.0 + FRACTION_TOLERANCE {
                self.above_fraction += 1;
            }
            if p > BRAKE_PRESSURE_NOT_FRACTION {
                self.not_fraction += 1;
            }
            if p < -FRACTION_TOLERANCE {
                self.below_zero += 1;
            }
        }
        if t.filtered_brake.is_finite() {
            self.pedal_peak = self.pedal_peak.max(t.filtered_brake);
        }
        self.correlation.push(t.filtered_brake, front);
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "brake_pressure_fraction",
            "Brake pressure is a 0..1 fraction following the pedal",
            &["A-LMU-017"],
            &["brakes.{w}.pressure_frac"],
        );
        let r = self.correlation.r();
        o.metric_opt("min", self.all.min())
            .metric_opt("max", self.all.max())
            .metric("above_1", self.above_fraction as f64)
            .metric("above_1_5", self.not_fraction as f64)
            .metric("below_0", self.below_zero as f64)
            .metric("pedal_peak", self.pedal_peak)
            .metric_opt("pedal_correlation", r);
        o.line(format!(
            "raw mBrakePressure over all wheels: min {}, median {}, p99 {}, max {} (n = {}); > 1.01: {}, > {BRAKE_PRESSURE_NOT_FRACTION}: {}, < −0.01: {}",
            fmt_opt(self.all.min(), 4),
            fmt_opt(self.all.median(), 4),
            fmt_opt(self.all.p99(), 4),
            fmt_opt(self.all.max(), 4),
            self.all.count(),
            self.above_fraction,
            self.not_fraction,
            self.below_zero
        ));
        o.line(format!(
            "correlation of the front mean with the filtered brake pedal: r = {} (n = {}), slope {}; with the unfiltered pedal r = {}; pedal peak {:.3}",
            fmt_opt(r, 3),
            self.correlation.count(),
            fmt_opt(self.correlation.slope(), 3),
            fmt_opt(self.correlation_unfiltered.r(), 3),
            self.pedal_peak
        ));
        // Peak (p99) at full pedal: the documented "pedal × balance" is the ceiling; modulation below it
        // (ABS, brake-by-wire blending with regeneration) lowers the median but not the peak.
        let (front_peak, rear_peak) = (self.full_front_ratio.p99(), self.full_rear_ratio.p99());
        o.metric_opt(
            "full_pedal_front_over_balance",
            self.full_front_ratio.median(),
        )
        .metric_opt(
            "full_pedal_rear_over_balance",
            self.full_rear_ratio.median(),
        )
        .metric_opt("full_pedal_front_over_balance_p99", front_peak)
        .metric_opt("full_pedal_rear_over_balance_p99", rear_peak)
        .metric_opt("rear_bias_median", self.rear_bias.median());
        o.line(format!(
            "at full pedal while rolling (filtered ≥ {FULL_PEDAL}, > {RUNNING_SPEED_MPS} m/s, n = {}): front ÷ (1 − rear bias) median {} (p99 {}), rear ÷ rear bias median {} (p99 {}); rear bias median {} (pedal × balance ⇒ peak ≈ 1)",
            self.full_front_ratio.count(),
            fmt_opt(self.full_front_ratio.median(), 3),
            fmt_opt(front_peak, 3),
            fmt_opt(self.full_rear_ratio.median(), 3),
            fmt_opt(rear_peak, 3),
            fmt_opt(self.rear_bias.median(), 4)
        ));
        let near_one =
            |v: Option<f64>| v.is_some_and(|x| (x - 1.0).abs() <= BALANCE_RATIO_TOLERANCE);
        let pedal_times_balance = self.full_front_ratio.count() >= MIN_FULL_PEDAL_SAMPLES
            && near_one(front_peak)
            && near_one(rear_peak);
        let (front_ratio, rear_ratio) = (front_peak, rear_peak);
        let published = [(
            "brakes.{w}.pressure_frac",
            self.published_vs_raw,
            PUBLISHED_FRACTION_TOLERANCE,
        )];
        describe_published(&mut o, &published);
        if self.not_fraction > 0 || self.below_zero > 0 {
            o.verdict(
                Verdict::Fail,
                format!(
                    "values outside the fraction range: {} above {BRAKE_PRESSURE_NOT_FRACTION}, {} below 0",
                    self.not_fraction, self.below_zero
                ),
            )
        } else if self.pedal_peak < MIN_PEDAL_PEAK || self.all.count() == 0 {
            o.verdict(
                Verdict::Inconclusive,
                "the brake pedal was never pressed hard",
            )
        } else if self.above_fraction == 0
            && (pedal_times_balance || r.is_some_and(|r| r >= MIN_PEDAL_CORRELATION))
        {
            gate_rows(
                o,
                &published,
                format!(
                    "fraction 0..{}: at full pedal the peak front ÷ (1 − bias) = {}, rear ÷ bias = {} (pedal × balance); r = {} with the pedal",
                    fmt_opt(self.all.max(), 3),
                    fmt_opt(front_ratio, 3),
                    fmt_opt(rear_ratio, 3),
                    fmt_opt(r, 3)
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                format!(
                    "in 0..{} but neither pedal × balance (full-pedal peak front {}, rear {} of the balance) nor a strong pedal correlation (r = {}); {} values slightly above 1",
                    fmt_opt(self.all.max(), 3),
                    fmt_opt(front_ratio, 3),
                    fmt_opt(rear_ratio, 3),
                    fmt_opt(r, 3),
                    self.above_fraction
                ),
            )
        }
    }
}

// ----------------------------------------------------------------------------------------------------------
// Virtual energy (A-LMU-013) and state of charge (A-LMU-014)
// ----------------------------------------------------------------------------------------------------------

/// Above this a fraction-like value is percent (as in the normalizer).
const PERCENT_ABOVE: f64 = 1.5;
/// Minimum change for a field to count as "in use".
const MIN_SPAN: f64 = 0.02;
/// VE increases adding up to more than this within a lap mean a refill (pit stop).
const VE_REFILL_STEP: f64 = 0.005;
/// A VE drop larger than this between two consecutive samples is a reset by LMU, not use (Gate 1
/// pitstop1, return to the garage: 0.9392 → 0.2100 in 28 ms; a whole lap uses at most
/// `VE_PER_LAP_PLAUSIBLE.1`, a sample a tiny fraction of that).
const VE_RESET_STEP: f64 = 0.05;
/// Plausible VE use per lap (fraction of the allowance).
const VE_PER_LAP_PLAUSIBLE: (f64, f64) = (0.005, 0.25);
/// VE laps listed.
const LISTED_LAPS: usize = 12;
/// `mSoC / (100 × mBatteryChargeFraction)` (or `/ fraction`) within this of 1 decides the relation.
const SOC_RATIO_TOLERANCE: f64 = 0.02;
/// Battery fraction below this is too small for a ratio.
const MIN_BATTERY_FOR_RATIO: f64 = 0.01;
/// Ratio samples needed for a state-of-charge verdict (5 s of telemetry).
const MIN_SOC_SAMPLES: u64 = 500;
/// A published fraction must equal its raw source (scaled) within this (both sides are f32).
const PUBLISHED_FRACTION_TOLERANCE: f64 = 1e-4;

/// Complete laps (a complete lap has no refill, reset or teleport) needed for a PASS.
const MIN_VE_LAPS: u64 = 2;
/// A lap-counter increment within this capture time of a teleport (e.g. "return to the garage") is not a
/// line crossing: the lap before it ends and the lap after it starts away from the line (s).
const TELEPORT_LAP_GUARD_S: f64 = 2.0;

/// One finished lap of VE use.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VeLap {
    lap: i32,
    used: f64,
    /// VE rose by more than [`VE_REFILL_STEP`] during the lap (pit refill).
    refill: bool,
    /// VE dropped by more than [`VE_RESET_STEP`] in one sample during the lap (reset by LMU).
    reset: bool,
    /// Started and ended at an on-track line crossing with no teleport, teleport ET step-back, VE refill
    /// or VE reset in between, so `used` is the energy the lap took. A partial lap (the first lap of the
    /// capture or after a discontinuity, an out-lap from the garage, a lap ended by a return to the garage,
    /// a lap with a pit refill) is listed but never judged.
    complete: bool,
}

/// The lap in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VeLapStart {
    lap: i32,
    ve: f64,
    /// Began at an observed on-track lap increment (not mid-lap).
    from_line: bool,
    /// A teleport (or a teleport ET step-back) happened since the lap began.
    teleport: bool,
}

/// A finished lap that waits [`TELEPORT_LAP_GUARD_S`] for a teleport right after its lap increment.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingVeLap {
    lap: VeLap,
    ended_s: f64,
}

/// One telemetry sample for the VE lap accounting.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VeSample {
    capture_s: f64,
    lap_number: i32,
    ve: f64,
    pos: (f64, f64, f64),
    /// The player's latest scoring row says "in the garage stall".
    in_garage: bool,
}

/// Per-lap VE use. Only complete laps are counted and judged (see [`VeLap::complete`]).
#[derive(Debug, Clone, Default)]
struct VeLaps {
    current: Option<VeLapStart>,
    pending: Option<PendingVeLap>,
    last_ve: Option<f64>,
    prev_pos: Option<(f64, f64, f64)>,
    last_teleport_s: Option<f64>,
    rise_in_lap: f64,
    refill_in_lap: bool,
    reset_in_lap: bool,
    listed: Vec<VeLap>,
    per_lap: Summary,
    complete: u64,
    partial: u64,
    refill: u64,
    implausible: u64,
}

impl VeLaps {
    /// Continuity broke: the lap in progress can no longer be completed, and a lap that ended within
    /// [`TELEPORT_LAP_GUARD_S`] before it ended at the reset, not at the line (Gate 1 practice: the session
    /// ran out with the car in the pit box; LMU counted a lap, moved the car to the garage, reset its
    /// energy and stepped its ET back by 10 ms).
    fn discontinuity(&mut self, capture_s: f64) {
        if let Some(p) = self.pending.as_mut()
            && capture_s - p.ended_s <= TELEPORT_LAP_GUARD_S
        {
            p.lap.complete = false;
        }
        self.current = None;
        self.last_ve = None;
        self.prev_pos = None;
    }

    /// LMU moved the car at `capture_s`: a garage teleport recognised by the normalizer (Gate 1 pitstop1:
    /// a 19.17 m jump, far below [`TELEPORT_STEP_M`]), a teleport ET step-back it dropped (that sample has no
    /// frame), or a jump longer than [`TELEPORT_STEP_M`] seen here. The lap in progress is no longer a
    /// lap driven from line to line, a lap that ended within [`TELEPORT_LAP_GUARD_S`] before ended at the
    /// teleport, and a lap-counter increment within the guard after it is not a line crossing.
    fn teleport(&mut self, capture_s: f64) {
        self.last_teleport_s = Some(capture_s);
        if let Some(cur) = self.current.as_mut() {
            cur.teleport = true;
        }
        if let Some(p) = self.pending.as_mut()
            && capture_s - p.ended_s <= TELEPORT_LAP_GUARD_S
        {
            // The counter went up just before a teleport: that lap ended away from the line.
            p.lap.complete = false;
        }
    }

    fn observe(&mut self, s: VeSample) {
        let jump = self.prev_pos.is_some_and(|p| {
            let d = ((s.pos.0 - p.0).powi(2) + (s.pos.1 - p.1).powi(2) + (s.pos.2 - p.2).powi(2))
                .sqrt();
            d > TELEPORT_STEP_M
        });
        self.prev_pos = Some(s.pos);
        if jump {
            self.teleport(s.capture_s);
        }
        if let Some(p) = self.pending.take() {
            if s.capture_s - p.ended_s > TELEPORT_LAP_GUARD_S {
                self.settle(p.lap);
            } else {
                self.pending = Some(p);
            }
        }
        if let Some(prev) = self.last_ve {
            // VE only rises when refilled in the pits, at once or gradually: the rises within a lap add up.
            if s.ve > prev {
                self.rise_in_lap += s.ve - prev;
                if self.rise_in_lap > VE_REFILL_STEP {
                    self.refill_in_lap = true;
                }
            }
            // A drop no driving can produce between two samples: LMU reset the allowance.
            if prev - s.ve > VE_RESET_STEP {
                self.reset_in_lap = true;
            }
        }
        self.last_ve = Some(s.ve);
        match self.current {
            // `checked_add`: the lap number is raw capture data and may be garbage (i32::MAX).
            Some(cur) if cur.lap.checked_add(1) == Some(s.lap_number) => {
                let at_line = !s.in_garage
                    && !self
                        .last_teleport_s
                        .is_some_and(|t| s.capture_s - t <= TELEPORT_LAP_GUARD_S);
                let lap = VeLap {
                    lap: cur.lap,
                    used: cur.ve - s.ve,
                    refill: self.refill_in_lap,
                    reset: self.reset_in_lap,
                    complete: cur.from_line
                        && !cur.teleport
                        && at_line
                        && !self.refill_in_lap
                        && !self.reset_in_lap,
                };
                if let Some(p) = self.pending.take() {
                    self.settle(p.lap);
                }
                self.pending = Some(PendingVeLap {
                    lap,
                    ended_s: s.capture_s,
                });
                self.start(s, at_line);
            }
            Some(cur) if cur.lap == s.lap_number => {}
            // First sample, after a discontinuity, or a lap-counter jump: the lap began mid-lap.
            _ => self.start(s, false),
        }
    }

    fn start(&mut self, s: VeSample, from_line: bool) {
        self.current = Some(VeLapStart {
            lap: s.lap_number,
            ve: s.ve,
            from_line,
            teleport: false,
        });
        self.refill_in_lap = false;
        self.reset_in_lap = false;
        self.rise_in_lap = 0.0;
    }

    fn settle(&mut self, lap: VeLap) {
        if lap.refill {
            self.refill += 1;
        }
        if lap.complete {
            self.complete += 1;
            self.per_lap.push(lap.used);
            if !(VE_PER_LAP_PLAUSIBLE.0..=VE_PER_LAP_PLAUSIBLE.1).contains(&lap.used) {
                self.implausible += 1;
            }
        } else {
            self.partial += 1;
        }
        if self.listed.len() < LISTED_LAPS {
            self.listed.push(lap);
        }
    }

    /// Settles the lap still waiting for its teleport guard.
    fn finish(&mut self) {
        if let Some(p) = self.pending.take() {
            self.settle(p.lap);
        }
    }
}

/// Virtual energy is a 0..1 fraction with plausible per-lap use.
#[derive(Debug, Clone, Default)]
pub struct VirtualEnergyCheck {
    values: Summary,
    non_zero: u64,
    detector: FractionScaleDetector,
    laps: VeLaps,
    /// Published `energy.virtual_energy_frac` vs `mVirtualEnergy` (the 0..1 convention under test).
    published_vs_raw: PublishedVsRaw,
}

impl VirtualEnergyCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.laps.discontinuity(step.capture_s);
        }
        // Before the sample (the counter may go up on the teleport sample itself), and also for a dropped
        // teleport step-back, which carries no frame.
        if step.normalizer.moved_by_lmu() {
            self.laps.teleport(step.capture_s);
        }
        let Some(t) = step.raw else {
            return;
        };
        let ve = f64::from(t.virtual_energy);
        if !ve.is_finite() {
            return;
        }
        self.values.push(ve);
        if ve != 0.0 {
            self.non_zero += 1;
        }
        self.detector.observe(ve, t.lap_number);
        if let Some(frame) = step.frame {
            self.published_vs_raw
                .push(frame.f64(ch::VIRTUAL_ENERGY), ve);
        }
        self.laps.observe(VeSample {
            capture_s: step.capture_s,
            lap_number: t.lap_number,
            ve,
            pos: (t.pos.x, t.pos.y, t.pos.z),
            in_garage: step.player_row.is_some_and(|r| r.in_garage_stall),
        });
    }

    pub(super) fn finish(mut self, normalizer: &Normalizer) -> CheckResult {
        let mut o = Outcome::new(
            "virtual_energy",
            "Virtual energy: 0..1 fraction, plausible use per lap",
            &["A-LMU-013"],
            // Not `energy.ve_used_last_lap_frac`: that derived row is not compared here.
            &["energy.virtual_energy_frac"],
        );
        self.laps.finish();
        let laps = &self.laps;
        let (min, max) = (self.values.min(), self.values.max());
        o.metric_opt("min", min)
            .metric_opt("max", max)
            .metric("non_zero_samples", self.non_zero as f64)
            .metric("laps", laps.complete as f64)
            .metric("partial_laps", laps.partial as f64)
            .metric("refill_laps", laps.refill as f64)
            .metric("implausible_laps", laps.implausible as f64)
            .metric_opt("per_lap_median", laps.per_lap.median());
        o.line(format!(
            "raw mVirtualEnergy: min {}, median {}, max {} (n = {}, non-zero {})",
            fmt_opt(min, 4),
            fmt_opt(self.values.median(), 4),
            fmt_opt(max, 4),
            self.values.count(),
            self.non_zero
        ));
        o.line(format!(
            "scale: independent detector {:?}, lmu-normalizer {:?}",
            self.detector.scale(),
            normalizer.virtual_energy_scale()
        ));
        o.line(format!(
            "published energy.virtual_energy_frac vs mVirtualEnergy: {}",
            self.published_vs_raw.describe()
        ));
        o.line(format!(
            "complete laps {}, partial laps not judged {} (capture start, after a discontinuity, out-lap from the garage, a garage teleport or teleport ET step-back, a VE refill ({} lap(s)) or VE reset in the lap); use per complete lap: median {}, min {}, max {}; outside {VE_PER_LAP_PLAUSIBLE:?}: {}",
            laps.complete,
            laps.partial,
            laps.refill,
            fmt_opt(laps.per_lap.median(), 4),
            fmt_opt(laps.per_lap.min(), 4),
            fmt_opt(laps.per_lap.max(), 4),
            laps.implausible
        ));
        let listed: Vec<String> = laps
            .listed
            .iter()
            .map(|l| {
                format!(
                    "lap {}: {:.2} %{}{}{}",
                    l.lap,
                    100.0 * l.used,
                    if l.refill { " (refill)" } else { "" },
                    if l.reset { " (reset)" } else { "" },
                    if l.complete { "" } else { " (partial)" }
                )
            })
            .collect();
        if !listed.is_empty() {
            o.line(format!("per lap: {}", listed.join(", ")));
        }
        let span = min.zip(max).map(|(a, b)| b - a);
        if self.non_zero == 0 {
            return o.verdict(
                Verdict::Inconclusive,
                "mVirtualEnergy is always 0 (no virtual energy on this car, A-LMU-012)",
            );
        }
        let percent = max.is_some_and(|m| m > PERCENT_ABOVE);
        let out_of_range = min.is_some_and(|m| m < -FRACTION_TOLERANCE)
            || max.is_some_and(|m| m > 100.0 + 1.0)
            || normalizer.virtual_energy_scale() == FractionScale::Unrecognized;
        if percent || out_of_range {
            return o.verdict(
                Verdict::Fail,
                format!(
                    "not a 0..1 fraction: min {}, max {}",
                    fmt_opt(min, 3),
                    fmt_opt(max, 3)
                ),
            );
        }
        let fraction = max.is_some_and(|m| m <= 1.0 + FRACTION_TOLERANCE)
            && span.is_some_and(|s| s >= MIN_SPAN);
        let laps_ok = laps.per_lap.count() >= MIN_VE_LAPS && laps.implausible == 0;
        if fraction && laps_ok {
            gate_on_published(
                o,
                &[("energy.virtual_energy_frac", self.published_vs_raw)],
                PUBLISHED_FRACTION_TOLERANCE,
                format!(
                    "fraction {}..{}, {} complete lap(s) using {} of the allowance per lap (median)",
                    fmt_opt(min, 3),
                    fmt_opt(max, 3),
                    laps.per_lap.count(),
                    fmt_opt(laps.per_lap.median().map(|u| 100.0 * u), 2) + " %"
                ),
            )
        } else if fraction && laps.implausible > 0 {
            o.verdict(
                Verdict::Fail,
                format!(
                    "{} complete lap(s) with implausible VE use (outside {VE_PER_LAP_PLAUSIBLE:?})",
                    laps.implausible
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                format!(
                    "in 0..1 but not enough change or fewer than {MIN_VE_LAPS} complete laps ({})",
                    laps.per_lap.count()
                ),
            )
        }
    }
}

/// `mSoC` scale and its relation to `mBatteryChargeFraction`.
#[derive(Debug, Clone, Default)]
pub struct SocCheck {
    soc: Summary,
    battery: Summary,
    ratio_percent: Summary,
    ratio_fraction: Summary,
    motor_active: u64,
    detector: FractionScaleDetector,
    /// Published `energy.soc_frac` vs `mSoC / 100` (percent scale) and vs `mSoC` (fraction scale).
    soc_vs_percent: PublishedVsRaw,
    soc_vs_fraction: PublishedVsRaw,
    /// Published `energy.battery_charge_frac` vs `mBatteryChargeFraction`.
    battery_vs_raw: PublishedVsRaw,
    /// Published samples where the A-LMU-014 mitigation expects the battery fraction instead of the
    /// scaled `mSoC` (percent scale, fraction scale).
    substituted_percent: u64,
    substituted_fraction: u64,
}

/// The `energy.soc_frac` the normalizer is expected to publish for one sample, and whether its A-LMU-014
/// mitigation substitutes the battery fraction: exactly `lmu_normalizer`'s `prefer_battery_fraction` rule
/// (an uninitialised `mSoC` of exactly 0 next to a charged battery, or a gap above
/// [`SOC_AGREEMENT_TOLERANCE`], publishes `mBatteryChargeFraction`), otherwise the scaled `mSoC`.
fn expected_published_soc(scaled_soc: f64, soc_raw: f64, battery: f64) -> (f64, bool) {
    if !soc_raw.is_finite() || !(0.0..=1.0).contains(&battery) {
        return (scaled_soc, false);
    }
    let uninitialised = soc_raw == 0.0 && battery > 0.0;
    if uninitialised || (scaled_soc - battery).abs() > SOC_AGREEMENT_TOLERANCE {
        (battery, true)
    } else {
        (scaled_soc, false)
    }
}

impl SocCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let Some(t) = step.raw else {
            return;
        };
        let soc = f64::from(t.soc);
        let battery = t.battery_charge_fraction;
        if let Some(frame) = step.frame {
            let published_soc = frame.f64(ch::BATTERY_SOC);
            let (exp_percent, sub_percent) = expected_published_soc(soc / 100.0, soc, battery);
            let (exp_fraction, sub_fraction) = expected_published_soc(soc, soc, battery);
            self.soc_vs_percent.push(published_soc, exp_percent);
            self.soc_vs_fraction.push(published_soc, exp_fraction);
            if published_soc.is_some() {
                self.substituted_percent += u64::from(sub_percent);
                self.substituted_fraction += u64::from(sub_fraction);
            }
            self.battery_vs_raw
                .push(frame.f64(ch::BATTERY_CHARGE), battery);
        }
        self.soc.push(soc);
        self.battery.push(battery);
        if t.electric_boost_motor_state != 0 {
            self.motor_active += 1;
        }
        if soc.is_finite() {
            self.detector.observe(soc, t.lap_number);
        }
        if battery.is_finite() && battery > MIN_BATTERY_FOR_RATIO && soc.is_finite() {
            self.ratio_percent.push(soc / (100.0 * battery));
            self.ratio_fraction.push(soc / battery);
        }
    }

    pub(super) fn finish(self, normalizer: &Normalizer) -> CheckResult {
        let mut o = Outcome::new(
            "state_of_charge",
            "State of charge (mSoC) scale and battery charge fraction",
            &["A-LMU-014"],
            &["energy.soc_frac", "energy.battery_charge_frac"],
        );
        let rp = self.ratio_percent.median();
        let rf = self.ratio_fraction.median();
        o.metric_opt("soc_min", self.soc.min())
            .metric_opt("soc_max", self.soc.max())
            .metric_opt("battery_min", self.battery.min())
            .metric_opt("battery_max", self.battery.max())
            .metric_opt("soc_over_100_battery_median", rp)
            .metric_opt("soc_over_battery_median", rf)
            .metric("motor_active_samples", self.motor_active as f64);
        o.line(format!(
            "raw mSoC: min {}, median {}, max {}; mBatteryChargeFraction: min {}, median {}, max {} (n = {})",
            fmt_opt(self.soc.min(), 3),
            fmt_opt(self.soc.median(), 3),
            fmt_opt(self.soc.max(), 3),
            fmt_opt(self.battery.min(), 4),
            fmt_opt(self.battery.median(), 4),
            fmt_opt(self.battery.max(), 4),
            self.soc.count()
        ));
        o.line(format!(
            "mSoC / (100 × battery fraction): median {} (p1 {}, p99 {}); mSoC / battery fraction: median {}; samples with motor state ≠ 0: {}",
            fmt_opt(rp, 4),
            fmt_opt(self.ratio_percent.p01(), 4),
            fmt_opt(self.ratio_percent.p99(), 4),
            fmt_opt(rf, 4),
            self.motor_active
        ));
        o.line(format!(
            "scale: independent detector {:?}, lmu-normalizer {:?}",
            self.detector.scale(),
            normalizer.soc_scale()
        ));
        o.line(format!(
            "published energy.soc_frac vs mSoC / 100: {}; vs mSoC: {}; published energy.battery_charge_frac vs mBatteryChargeFraction: {}",
            self.soc_vs_percent.describe(),
            self.soc_vs_fraction.describe(),
            self.battery_vs_raw.describe()
        ));
        o.metric("soc_substituted_percent", self.substituted_percent as f64)
            .metric("soc_substituted_fraction", self.substituted_fraction as f64);
        o.line(format!(
            "A-LMU-014 mitigation: published samples where mSoC is uninitialised (0) or disagrees with the battery \
             fraction by more than {SOC_AGREEMENT_TOLERANCE}, so the battery fraction is expected instead: \
             {} (percent scale), {} (fraction scale)",
            self.substituted_percent, self.substituted_fraction
        ));
        let battery_bad = self.battery.min().is_some_and(|m| m < -FRACTION_TOLERANCE)
            || self
                .battery
                .max()
                .is_some_and(|m| m > 1.0 + FRACTION_TOLERANCE);
        let soc_bad = self.soc.min().is_some_and(|m| m < -1.0)
            || self.soc.max().is_some_and(|m| m > 101.0)
            || normalizer.soc_scale() == FractionScale::Unrecognized;
        let in_use = self
            .soc
            .min()
            .zip(self.soc.max())
            .is_some_and(|(a, b)| b - a >= MIN_SPAN);
        let near_one = |r: Option<f64>| r.is_some_and(|r| (r - 1.0).abs() <= SOC_RATIO_TOLERANCE);
        if battery_bad || soc_bad {
            o.verdict(
                Verdict::Fail,
                "values outside every known scale (battery fraction outside 0..1 or mSoC outside 0..100)",
            )
        } else if !in_use {
            o.verdict(
                Verdict::Inconclusive,
                "mSoC does not change (not a hybrid, or the battery was not used)",
            )
        } else if self.ratio_percent.count() < MIN_SOC_SAMPLES {
            o.verdict(
                Verdict::Inconclusive,
                format!(
                    "too few samples with a charged battery ({} < {MIN_SOC_SAMPLES})",
                    self.ratio_percent.count()
                ),
            )
        } else if near_one(rp) && self.soc.max().is_some_and(|m| m > PERCENT_ABOVE) {
            gate_on_published(
                o,
                &[
                    (
                        "energy.soc_frac (= mSoC / 100, or the battery fraction under the A-LMU-014 mitigation)",
                        self.soc_vs_percent,
                    ),
                    ("energy.battery_charge_frac", self.battery_vs_raw),
                ],
                PUBLISHED_FRACTION_TOLERANCE,
                format!(
                    "percent: mSoC = 100 × mBatteryChargeFraction (median ratio {})",
                    fmt_opt(rp, 4)
                ),
            )
        } else if near_one(rf) {
            gate_on_published(
                o,
                &[
                    (
                        "energy.soc_frac (= mSoC, or the battery fraction under the A-LMU-014 mitigation)",
                        self.soc_vs_fraction,
                    ),
                    ("energy.battery_charge_frac", self.battery_vs_raw),
                ],
                PUBLISHED_FRACTION_TOLERANCE,
                format!(
                    "fraction: mSoC = mBatteryChargeFraction (median ratio {})",
                    fmt_opt(rf, 4)
                ),
            )
        } else {
            o.verdict(
                Verdict::Inconclusive,
                "mSoC changes but its relation to the battery fraction is not 1:1 or 100:1",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sample at `t` s on lap `lap` with `ve` left, the car at `x` m along a straight line.
    fn s(t: f64, lap: i32, ve: f64, x: f64, garage: bool) -> VeSample {
        VeSample {
            capture_s: t,
            lap_number: lap,
            ve,
            pos: (x, 0.0, 0.0),
            in_garage: garage,
        }
    }

    fn feed(samples: &[VeSample]) -> VeLaps {
        let mut laps = VeLaps::default();
        for &x in samples {
            laps.observe(x);
        }
        laps.finish();
        laps
    }

    fn complete_flags(laps: &VeLaps) -> Vec<(i32, bool)> {
        laps.listed.iter().map(|l| (l.lap, l.complete)).collect()
    }

    #[test]
    fn lap_begun_before_the_capture_is_partial_and_not_judged() {
        // The capture starts 50 m before the line: lap 5 uses only 0.1 % and must not fail the check.
        let laps = feed(&[
            s(0.0, 5, 0.900, 0.0, false),
            s(1.0, 6, 0.899, 40.0, false),
            s(30.0, 6, 0.880, 45.0, false),
            s(60.0, 7, 0.863, 50.0, false),
            s(120.0, 8, 0.826, 55.0, false),
            s(130.0, 8, 0.820, 60.0, false),
        ]);
        assert_eq!(complete_flags(&laps), [(5, false), (6, true), (7, true)]);
        assert_eq!((laps.complete, laps.partial, laps.implausible), (2, 1, 0));
        assert_eq!(laps.per_lap.count(), 2);
    }

    #[test]
    fn lap_after_a_discontinuity_is_partial() {
        let mut laps = VeLaps::default();
        laps.observe(s(0.0, 1, 0.90, 0.0, false));
        laps.observe(s(1.0, 2, 0.89, 10.0, false));
        laps.discontinuity(25.0);
        laps.observe(s(50.0, 2, 0.88, 20.0, false));
        laps.observe(s(51.0, 3, 0.879, 30.0, false));
        laps.finish();
        assert_eq!(complete_flags(&laps), [(1, false), (2, false)]);
        assert_eq!(laps.complete, 0);
    }

    #[test]
    fn lap_counted_at_a_session_end_reset_is_partial() {
        // Gate 1 practice: the session ran out with the car in the pit box; LMU counted a lap, moved the car
        // and reset its energy (1.00 → 0.21), then the ET stepped back (a discontinuity) before the teleport
        // could be seen between two samples.
        let mut laps = VeLaps::default();
        laps.observe(s(0.0, 0, 1.00, 0.0, false));
        laps.observe(s(10.0, 1, 1.00, 10.0, false));
        laps.observe(s(100.0, 1, 0.96, 20.0, false));
        laps.observe(s(105.0, 2, 0.21, 21.0, false));
        laps.discontinuity(105.01);
        laps.observe(s(105.02, 2, 0.21, 900.0, true));
        laps.observe(s(200.0, 2, 0.21, 900.0, true));
        laps.finish();
        assert_eq!(complete_flags(&laps), [(0, false), (1, false)]);
        assert_eq!(laps.implausible, 0);
    }

    #[test]
    fn return_to_the_garage_makes_both_neighbouring_laps_partial() {
        let laps = feed(&[
            s(0.0, 0, 0.95, 0.0, true),
            s(10.0, 1, 0.94, 10.0, false),
            s(70.0, 2, 0.90, 20.0, false),
            // Lap 2 is abandoned: teleport to the garage, then the counter goes up there.
            s(100.0, 2, 0.88, 30.0, false),
            s(100.01, 2, 0.88, 700.0, false),
            s(100.1, 3, 0.88, 700.0, false),
            // Out-lap from the garage, then a normal lap.
            s(160.0, 4, 0.85, 710.0, false),
            s(220.0, 5, 0.81, 720.0, false),
        ]);
        assert_eq!(
            complete_flags(&laps),
            [(0, false), (1, true), (2, false), (3, false), (4, true)]
        );
        assert_eq!((laps.complete, laps.partial), (2, 3));
    }

    #[test]
    fn counter_going_up_just_before_a_teleport_makes_the_ended_lap_partial() {
        let laps = feed(&[
            s(0.0, 0, 0.95, 0.0, false),
            s(10.0, 1, 0.94, 10.0, false),
            s(70.0, 2, 0.90, 20.0, false),
            s(100.0, 3, 0.88, 30.0, false),
            s(100.5, 3, 0.88, 800.0, false),
            s(160.0, 4, 0.85, 810.0, false),
        ]);
        assert_eq!(
            complete_flags(&laps),
            [(0, false), (1, true), (2, false), (3, false)]
        );
    }

    #[test]
    fn counter_going_up_in_the_garage_stall_is_not_a_line_crossing() {
        let laps = feed(&[
            s(0.0, 0, 0.95, 0.0, false),
            s(10.0, 1, 0.94, 10.0, false),
            s(70.0, 2, 0.90, 20.0, true),
            s(130.0, 3, 0.87, 30.0, false),
        ]);
        assert_eq!(complete_flags(&laps), [(0, false), (1, false), (2, false)]);
    }

    /// Gate 1 pitstop1 (real sample pattern): lap 1 began at the counter increment in the pit lane during a
    /// stop (VE refilled to 1.0). At the return to the garage the counter went up on the teleport sample
    /// (19.17 m jump, below `TELEPORT_STEP_M`, recognised by the normalizer; VE reset 0.9392 → 0.2100), and
    /// the next sample stepped ET back (dropped by the normalizer: no frame, only its counter). Lap 1 is
    /// partial; judged, its 79 % "use" failed A-LMU-013 (verify-v8).
    #[test]
    fn real_pitstop1_garage_return_lap_is_partial() {
        let mut laps = VeLaps::default();
        laps.observe(s(80.32, 0, 0.1534, 0.0, false));
        laps.observe(s(150.24, 0, 0.1534, 0.17, false));
        laps.observe(s(150.2487, 1, 1.0, 0.34, false));
        laps.observe(s(255.6549, 1, 0.9392, 500.0, false));
        // The normalizer's `teleports` counter grew on this snapshot, before its sample is observed.
        laps.teleport(255.7120);
        laps.observe(s(255.7120, 2, 0.2100, 519.17, false));
        // The ET step-back: no frame, `teleport_et_step_backs` grew.
        laps.teleport(255.7133);
        laps.observe(s(255.7156, 2, 0.2100, 519.17, false));
        laps.observe(s(255.8490, 2, 0.2100, 519.17, true));
        laps.observe(s(265.4, 2, 0.2100, 519.17, true));
        laps.finish();
        assert_eq!(complete_flags(&laps), [(0, false), (1, false)]);
        assert_eq!(
            (laps.complete, laps.partial, laps.implausible, laps.refill),
            (0, 2, 0, 1)
        );
        assert_eq!(laps.listed.get(1).map(|l| l.reset), Some(true));
    }

    /// Without the normalizer teleport signal the same lap is still partial: the VE reset alone (a drop no
    /// driving produces between two samples) makes it unmeasurable.
    #[test]
    fn ve_reset_alone_makes_the_lap_partial() {
        let laps = feed(&[
            s(0.0, 0, 0.95, 0.0, false),
            s(10.0, 1, 0.94, 10.0, false),
            s(70.0, 2, 0.90, 20.0, false),
            s(100.0, 2, 0.88, 30.0, false),
            s(100.03, 3, 0.21, 31.0, false),
            s(200.0, 4, 0.17, 40.0, false),
        ]);
        assert_eq!(
            complete_flags(&laps),
            [(0, false), (1, true), (2, false), (3, true)]
        );
        assert_eq!(laps.implausible, 0);
    }

    /// A short teleport the normalizer recognised mid-lap makes the lap partial even when VE does not move,
    /// and a counter increment within the guard after it is not a line crossing.
    #[test]
    fn normalizer_teleport_mid_lap_makes_the_lap_partial() {
        let mut laps = VeLaps::default();
        laps.observe(s(0.0, 0, 0.95, 0.0, false));
        laps.observe(s(10.0, 1, 0.94, 10.0, false));
        laps.observe(s(70.0, 2, 0.90, 20.0, false));
        laps.teleport(80.0);
        laps.observe(s(80.0, 2, 0.89, 39.0, false));
        laps.observe(s(81.0, 3, 0.89, 39.0, false));
        laps.observe(s(140.0, 4, 0.86, 50.0, false));
        laps.observe(s(200.0, 5, 0.83, 60.0, false));
        laps.finish();
        assert_eq!(
            complete_flags(&laps),
            [(0, false), (1, true), (2, false), (3, false), (4, true)]
        );
    }

    /// A lap with a pit refill is partial: its start-minus-end is not the energy it took.
    #[test]
    fn refill_lap_is_partial() {
        let laps = feed(&[
            s(0.0, 0, 0.30, 0.0, false),
            s(10.0, 1, 0.29, 10.0, false),
            s(40.0, 1, 0.27, 20.0, false),
            s(41.0, 1, 1.0, 20.0, false),
            s(70.0, 2, 0.97, 30.0, false),
            s(100.0, 2, 0.94, 35.0, false),
            s(130.0, 3, 0.91, 40.0, false),
        ]);
        assert_eq!(complete_flags(&laps), [(0, false), (1, false), (2, true)]);
        assert_eq!((laps.complete, laps.refill), (1, 1));
    }

    #[test]
    fn garbage_lap_number_does_not_overflow() {
        let laps = feed(&[
            s(0.0, i32::MAX, 0.9, 0.0, false),
            s(0.01, i32::MAX, 0.9, 0.1, false),
            s(0.02, i32::MIN, 0.9, 0.2, false),
            s(0.03, i32::MAX, 0.9, 0.3, false),
        ]);
        assert_eq!(laps.complete + laps.partial, 0);
    }
}

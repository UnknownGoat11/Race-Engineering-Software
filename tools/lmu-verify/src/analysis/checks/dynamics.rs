//! Sign checks on the published dynamics channels (A-LMU-024) and the wheel-speed consistency check
//! (A-LMU-039).
//!
//! `g_long` is compared with the finite-difference derivative of speed over a short window (both averaged
//! over the same window, so the comparison is not dominated by the differentiation noise); the lateral
//! checks use only clear cornering samples.

use std::collections::VecDeque;

use telemetry_model::ch;
use telemetry_model::units::{CM_PER_M, STANDARD_GRAVITY};

use super::{
    CheckResult, LEFT_WHEELS, Outcome, RIGHT_WHEELS, TYRE_LOAD, Verdict, WHEEL_SPEED, WHEELS,
    fmt_opt, pct,
};
use crate::analysis::Step;
use crate::stats::{Agreement, Correlation, Mean, Summary};

/// Only samples above this speed are used for the derivative and the lateral checks (m/s).
const DYNAMIC_MIN_SPEED_MPS: f64 = 10.0;
/// Cornering / throttle samples need this speed (m/s).
const CORNER_MIN_SPEED_MPS: f64 = 15.0;
/// Speed-derivative window (s).
const DERIVATIVE_SPAN_S: f64 = 0.2;
/// Bound of the derivative window (samples).
const DERIVATIVE_MAX_SAMPLES: usize = 64;
/// A window shorter than this share of the span is not used.
const DERIVATIVE_MIN_SPAN_SHARE: f64 = 0.9;
/// Full throttle (fraction).
const FULL_THROTTLE: f64 = 0.9;
/// Brake pedal released (fraction).
const BRAKE_RELEASED: f64 = 0.05;
/// Heavy braking (fraction).
const HEAVY_BRAKE: f64 = 0.5;
/// Correlation needed for a sign verdict.
const MIN_SIGN_CORRELATION: f64 = 0.7;
/// Mean g while braking hard must be below this (g).
const BRAKING_G_MAX: f64 = -0.3;
/// Pairs needed for a verdict.
const MIN_PAIRS: u64 = 200;
/// Clear cornering: yaw rate magnitude (rad/s).
const CORNER_MIN_YAW_RADPS: f64 = 0.15;
/// Clear cornering: lateral acceleration magnitude (g).
const CORNER_MIN_G_LAT: f64 = 0.3;
/// Steering input counted as steering (fraction).
const MIN_STEERING: f64 = 0.02;
/// Sign agreement for a PASS (fraction of samples).
const SIGN_PASS_SHARE: f64 = 0.85;
/// Sign agreement below this is inverted (FAIL).
const SIGN_FAIL_SHARE: f64 = 0.15;
/// Wheel-speed samples: straight running, |g_lat| below this (g).
const STRAIGHT_MAX_G_LAT: f64 = 0.3;
/// Wheel-speed samples: no hard acceleration or braking, |g_long| below this (g).
const STRAIGHT_MAX_G_LONG: f64 = 0.3;
/// Wheel / body speed ratio accepted (median): the estimate uses the static undeflected radius, so tyre
/// deflection and drive slip put it a few percent above 1 (A-LMU-039); a unit error would be a factor.
const WHEEL_RATIO_PASS: (f64, f64) = (0.9, 1.1);
/// Wheel / body speed ratio outside this is inconsistent (median).
const WHEEL_RATIO_FAIL: (f64, f64) = (0.5, 2.0);
/// Straight-running samples per wheel needed for the speed ratio.
const MIN_WHEEL_RATIO_SAMPLES: u64 = 100;
/// Share of samples with one rotation sign needed to state the sign.
const ROTATION_SIGN_SHARE: f64 = 0.95;

fn sign_verdict(share: Option<f64>, n: u64) -> Verdict {
    match share {
        Some(s) if n >= MIN_PAIRS && s >= SIGN_PASS_SHARE => Verdict::Pass,
        Some(s) if n >= MIN_PAIRS && s <= SIGN_FAIL_SHARE => Verdict::Fail,
        _ => Verdict::Inconclusive,
    }
}

/// `g_long` positive while accelerating: correlated with d(speed)/dt and with throttle vs brake.
#[derive(Debug, Clone, Default)]
pub struct GLongCheck {
    window: VecDeque<(f64, f64, f64)>,
    correlation: Correlation,
    full_throttle: Mean,
    braking: Mean,
}

impl GLongCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.window.clear();
        }
        let Some(frame) = step.frame else {
            return;
        };
        let (Some(et), Some(v), Some(g)) = (
            frame.source_et_s,
            frame.f64(ch::SPEED),
            frame.f64(ch::G_LONG),
        ) else {
            self.window.clear();
            return;
        };
        if v > CORNER_MIN_SPEED_MPS {
            let throttle = frame.f64(ch::THROTTLE);
            let brake = frame.f64(ch::BRAKE);
            if let (Some(th), Some(br)) = (throttle, brake) {
                if th >= FULL_THROTTLE && br <= BRAKE_RELEASED {
                    self.full_throttle.push(g);
                }
                if br >= HEAVY_BRAKE {
                    self.braking.push(g);
                }
            }
        }
        if v <= DYNAMIC_MIN_SPEED_MPS {
            self.window.clear();
            return;
        }
        if self.window.back().is_some_and(|&(t, _, _)| et <= t) {
            self.window.clear();
        }
        self.window.push_back((et, v, g));
        while self.window.len() > DERIVATIVE_MAX_SAMPLES
            || self
                .window
                .front()
                .is_some_and(|&(t, _, _)| et - t > DERIVATIVE_SPAN_S)
        {
            self.window.pop_front();
        }
        let (Some(&(t0, v0, _)), Some(&(t1, v1, _))) = (self.window.front(), self.window.back())
        else {
            return;
        };
        let span = t1 - t0;
        if span < DERIVATIVE_SPAN_S * DERIVATIVE_MIN_SPAN_SHARE {
            return;
        }
        let accel_g = (v1 - v0) / span / STANDARD_GRAVITY;
        let mean_g = self.window.iter().map(|&(_, _, g)| g).sum::<f64>() / self.window.len() as f64;
        self.correlation.push(accel_g, mean_g);
        // Non-overlapping windows: each pair uses fresh samples.
        self.window.clear();
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "g_long_sign",
            "Longitudinal G sign (+ = accelerating)",
            &["A-LMU-024"],
            &["dynamics.g_long", "vehicle.local_accel_z_mps2"],
        );
        let r = self.correlation.r();
        let slope = self.correlation.slope();
        let (thr, brk) = (self.full_throttle.mean(), self.braking.mean());
        o.metric_opt("r_vs_dspeed_dt", r)
            .metric_opt("slope", slope)
            .metric("pairs", self.correlation.count() as f64)
            .metric_opt("mean_g_full_throttle", thr)
            .metric_opt("mean_g_heavy_braking", brk);
        o.line(format!(
            "g_long vs d(speed)/dt over {DERIVATIVE_SPAN_S} s windows above {DYNAMIC_MIN_SPEED_MPS} m/s: r = {}, slope {} (n = {})",
            fmt_opt(r, 3),
            fmt_opt(slope, 3),
            self.correlation.count()
        ));
        o.line(format!(
            "mean g_long at full throttle {} g (n = {}), under heavy braking {} g (n = {})",
            fmt_opt(thr, 3),
            self.full_throttle.count(),
            fmt_opt(brk, 3),
            self.braking.count()
        ));
        let summary = format!(
            "r = {} with d(speed)/dt; throttle {} g vs braking {} g",
            fmt_opt(r, 2),
            fmt_opt(thr, 2),
            fmt_opt(brk, 2)
        );
        let enough = self.correlation.count() >= MIN_PAIRS;
        let inverted = r.is_some_and(|r| r <= -MIN_SIGN_CORRELATION)
            || thr
                .zip(brk)
                .is_some_and(|(t, b)| b > -BRAKING_G_MAX && t < b);
        if enough && inverted {
            o.verdict(Verdict::Fail, summary)
        } else if enough
            && r.is_some_and(|r| r >= MIN_SIGN_CORRELATION)
            && brk.is_some_and(|b| b <= BRAKING_G_MAX)
            && thr.zip(brk).is_some_and(|(t, b)| t > b)
        {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

/// `g_lat` + = left: agrees with the yaw rate (+ = left) and with the load transfer to the outside wheels.
#[derive(Debug, Clone, Default)]
pub struct GLatCheck {
    yaw_agree: Agreement,
    load_agree: Agreement,
    /// Cornering samples whose four tyre loads were all exactly 0 N (not evidence).
    zero_load_samples: u64,
    centripetal: Correlation,
    centripetal_ratio: Summary,
}

/// Clear cornering sample: (speed, g_lat, yaw rate).
fn cornering(frame: &telemetry_model::TelemetryFrame) -> Option<(f64, f64, f64)> {
    let v = frame.f64(ch::SPEED)?;
    let g = frame.f64(ch::G_LAT)?;
    let yaw = frame.f64(ch::YAW_RATE)?;
    (v > CORNER_MIN_SPEED_MPS && yaw.abs() > CORNER_MIN_YAW_RADPS && g.abs() > CORNER_MIN_G_LAT)
        .then_some((v, g, yaw))
}

impl GLatCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let Some(frame) = step.frame else {
            return;
        };
        let Some((v, g, yaw)) = cornering(frame) else {
            return;
        };
        self.yaw_agree.push(g.signum() == yaw.signum());
        let expected_g = v * yaw / STANDARD_GRAVITY;
        self.centripetal.push(expected_g, g);
        // Steady cornering: lateral acceleration = speed × yaw rate (sideslip rate neglected).
        self.centripetal_ratio.push(g / expected_g);
        let loads: Option<Vec<f64>> = TYRE_LOAD.iter().map(|&id| frame.f64(id)).collect();
        // Four loads of exactly 0 N on a cornering car are not a measurement (LMU does not fill them on
        // LMGT3, A-LMU-069): no load-transfer evidence either way.
        let loads = match loads {
            Some(l) if l.iter().all(|&x| x == 0.0) => {
                self.zero_load_samples += 1;
                None
            }
            other => other,
        };
        if let Some(l) = loads {
            let side = |idx: [usize; 2]| idx.iter().filter_map(|&i| l.get(i)).sum::<f64>();
            let right_minus_left = side(RIGHT_WHEELS) - side(LEFT_WHEELS);
            // Accelerating to the left (turning left) loads the right-hand (outside) wheels.
            self.load_agree
                .push(right_minus_left != 0.0 && right_minus_left.signum() == g.signum());
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "g_lat_sign",
            "Lateral G sign (+ = left) vs yaw rate and load transfer",
            &["A-LMU-024"],
            &[
                "dynamics.g_lat",
                "dynamics.yaw_rate_radps",
                "vehicle.local_accel_x_mps2",
                "tyres.{w}.load_n",
            ],
        );
        let yaw = self.yaw_agree.fraction();
        let load = self.load_agree.fraction();
        o.metric_opt("yaw_sign_agreement", yaw)
            .metric("yaw_samples", self.yaw_agree.total as f64)
            .metric_opt("load_transfer_agreement", load)
            .metric("load_samples", self.load_agree.total as f64)
            .metric_opt("centripetal_r", self.centripetal.r())
            .metric_opt("centripetal_slope", self.centripetal.slope());
        o.line(format!(
            "clear cornering samples (speed > {CORNER_MIN_SPEED_MPS} m/s, |yaw| > {CORNER_MIN_YAW_RADPS} rad/s, |g_lat| > {CORNER_MIN_G_LAT} g): sign(g_lat) = sign(yaw rate) in {} of {}",
            pct(yaw),
            self.yaw_agree.total
        ));
        o.metric_opt("centripetal_ratio_median", self.centripetal_ratio.median());
        o.line(format!(
            "g_lat ÷ (v·yaw/g0): median {} (p1 {}, p99 {}; steady cornering ⇒ ≈ 1, also checks the rad/s unit); least squares r = {}, slope {}",
            fmt_opt(self.centripetal_ratio.median(), 3),
            fmt_opt(self.centripetal_ratio.p01(), 3),
            fmt_opt(self.centripetal_ratio.p99(), 3),
            fmt_opt(self.centripetal.r(), 3),
            fmt_opt(self.centripetal.slope(), 3)
        ));
        o.line(format!(
            "load transfer: sign((FR + RR) − (FL + RL)) = sign(g_lat) in {} of {} (independent of the yaw convention)",
            pct(load),
            self.load_agree.total
        ));
        let yaw_v = sign_verdict(yaw, self.yaw_agree.total);
        let load_v = sign_verdict(load, self.load_agree.total);
        o.metric("zero_load_samples", self.zero_load_samples as f64);
        if self.load_agree.total == 0 {
            o.line(format!(
                "load transfer: n/a (tyre loads unavailable, or exactly 0 N on all four wheels in {} cornering samples); the verdict rests on the yaw evidence",
                self.zero_load_samples
            ));
        }
        let load_text = if self.load_agree.total == 0 {
            "n/a (no tyre loads)".to_owned()
        } else {
            pct(load)
        };
        let summary = format!(
            "agrees with yaw rate in {}, with load transfer in {load_text}",
            pct(yaw)
        );
        let verdict = match (yaw_v, load_v) {
            (Verdict::Fail, _) | (_, Verdict::Fail) => Verdict::Fail,
            (Verdict::Pass, Verdict::Pass) => Verdict::Pass,
            // Without tyre loads the yaw agreement alone decides (the load cross-check is then missing).
            (Verdict::Pass, Verdict::Inconclusive) if self.load_agree.total == 0 => Verdict::Pass,
            _ => Verdict::Inconclusive,
        };
        o.verdict(verdict, summary)
    }
}

/// Steering + = right: a right steer gives a negative (rightward) yaw rate.
#[derive(Debug, Clone, Default)]
pub struct SteeringCheck {
    agree: Agreement,
}

impl SteeringCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let Some(frame) = step.frame else {
            return;
        };
        let (Some((_, _, yaw)), Some(steer)) = (cornering(frame), frame.f64(ch::STEERING)) else {
            return;
        };
        if steer.abs() > MIN_STEERING {
            self.agree.push(steer.signum() == -yaw.signum());
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "steering_sign",
            "Steering sign (+ = right)",
            &["A-LMU-024"],
            &["inputs.steering_raw", "inputs.steering_filtered"],
        );
        let share = self.agree.fraction();
        o.metric_opt("agreement", share)
            .metric("samples", self.agree.total as f64);
        o.line(format!(
            "clear cornering samples with |steering| > {MIN_STEERING}: sign(steering) = −sign(yaw rate, + = left) in {} of {}",
            pct(share),
            self.agree.total
        ));
        o.verdict(
            sign_verdict(share, self.agree.total),
            format!("steering right ⇔ yawing right in {}", pct(share)),
        )
    }
}

/// Wheel surface speed matches the body speed; `mRotation` is negative rolling forward (A-LMU-039).
#[derive(Debug, Clone, Default)]
pub struct WheelSpeedCheck {
    published: [Summary; 4],
    raw: [Summary; 4],
    negative: [Agreement; 4],
}

impl WheelSpeedCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        let Some(v) = frame.f64(ch::SPEED).filter(|v| *v > DYNAMIC_MIN_SPEED_MPS) else {
            return;
        };
        let straight = frame
            .f64(ch::G_LAT)
            .is_some_and(|g| g.abs() < STRAIGHT_MAX_G_LAT)
            && frame
                .f64(ch::G_LONG)
                .is_some_and(|g| g.abs() < STRAIGHT_MAX_G_LONG);
        for (i, w) in t.wheels.iter().enumerate() {
            if w.detached || !w.rotation.is_finite() || w.rotation == 0.0 {
                continue;
            }
            if let Some(a) = self.negative.get_mut(i) {
                a.push(w.rotation < 0.0);
            }
            if !straight {
                continue;
            }
            if let Some(s) = self.published.get_mut(i)
                && let Some(ws) = WHEEL_SPEED.get(i).and_then(|&id| frame.f64(id))
            {
                s.push(ws / v);
            }
            if w.static_undeflected_radius != 0
                && let Some(s) = self.raw.get_mut(i)
            {
                let surface = w.rotation.abs() * f64::from(w.static_undeflected_radius) / CM_PER_M;
                s.push(surface / v);
            }
        }
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "wheel_speed",
            "Wheel surface speed vs body speed; mRotation sign",
            &["A-LMU-039", "A-LMU-060"],
            &["wheels.{w}.speed_mps", "wheels.{w}.rotation_radps"],
        );
        let mut all_pass = true;
        let mut any_fail = false;
        let mut any_ratio = false;
        let mut negative_all = true;
        let mut positive_any = false;
        let mut ratio_range: Option<(f64, f64)> = None;
        for (i, label) in WHEELS.iter().enumerate() {
            let p = self.published.get(i).and_then(Summary::median);
            let r = self.raw.get(i).and_then(Summary::median);
            let n_ratio = self
                .published
                .get(i)
                .map_or(0, Summary::count)
                .max(self.raw.get(i).map_or(0, Summary::count));
            let n_sign = self.negative.get(i).map_or(0, |a| a.total);
            let neg = self
                .negative
                .get(i)
                .and_then(Agreement::fraction)
                .filter(|_| n_sign >= MIN_PAIRS);
            o.line(format!(
                "{label}: published wheel speed / speed median {} (n = {}); raw |rotation| × radius / speed median {}; rotation < 0 while moving in {}",
                fmt_opt(p, 4),
                self.published.get(i).map_or(0, Summary::count),
                fmt_opt(r, 4),
                pct(neg)
            ));
            match p.or(r).filter(|_| n_ratio >= MIN_WHEEL_RATIO_SAMPLES) {
                Some(m) => {
                    any_ratio = true;
                    ratio_range = Some(ratio_range.map_or((m, m), |(a, b)| (a.min(m), b.max(m))));
                    all_pass &= (WHEEL_RATIO_PASS.0..=WHEEL_RATIO_PASS.1).contains(&m);
                    any_fail |= !(WHEEL_RATIO_FAIL.0..=WHEEL_RATIO_FAIL.1).contains(&m);
                }
                None => all_pass = false,
            }
            negative_all &= neg.is_some_and(|n| n >= ROTATION_SIGN_SHARE);
            positive_any |= neg.is_some_and(|n| n <= 1.0 - ROTATION_SIGN_SHARE);
        }
        let ratios = ratio_range.map_or_else(
            || format!("too few straight samples for the speed ratio (< {MIN_WHEEL_RATIO_SAMPLES} per wheel)"),
            |(a, b)| format!("wheel / body speed median {a:.3}..{b:.3} over the four wheels"),
        );
        let summary = if negative_all {
            format!("rotation negative rolling forward; {ratios}")
        } else {
            format!("rotation sign not consistently negative rolling forward; {ratios}")
        };
        if any_fail || positive_any {
            o.verdict(Verdict::Fail, summary)
        } else if any_ratio && all_pass && negative_all {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

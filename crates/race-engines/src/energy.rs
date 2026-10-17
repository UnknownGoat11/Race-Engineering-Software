//! Energy engine (spec §30, §145 "energy state", §314): virtual energy, battery state of charge,
//! hybrid deployment / harvest and regeneration per lap, their rates, and the estimated virtual energy at
//! the end of the lap.
//!
//! # Availability (spec §314)
//! Every output is computed only from the channel that measures it. Virtual energy (`mVirtualEnergy`) and
//! the battery state of charge (`mSoC`) are **different quantities**: when virtual energy is unavailable
//! (a non-hybrid or a car without the virtual energy rule) every virtual-energy output is `None`; the
//! battery fraction is never substituted. Likewise motor deployment needs both motor torque and speed.
//!
//! # Per-lap accounting
//! The caller marks lap boundaries with [`EnergyEngine::lap_completed`]. Within a lap:
//! - VE used and SoC deployed are the **sum of decreases**, SoC harvested the sum of increases, so a pit
//!   refill (a VE step up) never makes consumption negative; a VE drop of more than
//!   [`VE_RESET_STEP_FRAC`] or a SoC change of more than [`SOC_JUMP_STEP_FRAC`] in one sample is a reset by
//!   LMU (return to the garage), not use, and makes the lap incomplete for that quantity;
//! - motor deployment / harvest energy is the trapezoidal integral of the mechanical motor power
//!   `P = τ · ω` (ω = rpm · 2π / 60), split by sign (positive drives, negative regenerates);
//! - regeneration energy is the trapezoidal integral of `|mRegen|` (W). The sign convention of `mRegen`
//!   is unverified (LMU_CHANNEL_MAP, A-LMU), so the magnitude is integrated: whichever sign LMU uses for
//!   regeneration, the energy is counted and never reported as a confident zero.
//!
//! A lap is *complete* for a quantity only when that quantity was available at the lap's first sample and
//! on every sample of the lap with no time gap above [`MAX_INTEGRATION_GAP_S`], and at least one interval
//! was integrated; only complete, included laps feed the per-lap window average. The first lap the engine
//! sees is complete only when its first sample's lap progress is known and within
//! [`LAP_START_PROGRESS_TOLERANCE`] of the line (a lap joined part-way is never reported as a whole lap).
//! The live this-lap values are None while the lap is incomplete for that quantity (an under-counted
//! value is never shown).
//!
//! # Rates
//! The VE and SoC rate windows are cleared on a change that is not driving: a VE reset (above), a VE rise
//! of more than [`VE_REFILL_STEP_FRAC`] above the lowest level since the last clear (pit refill, in one step or gradual;
//! no VE rate is reported until consumption resumes) or a SoC change above [`SOC_JUMP_STEP_FRAC`] in one
//! sample, so a refill never appears as a huge (or negative) consumption or harvest rate.
//!
//! # Publication
//! [`publish_energy`] writes the VE outputs that have registry channels (`energy.ve_*`), so rules such as
//! the default `energy_low` alert can read them from the frame.

use std::collections::VecDeque;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};
use telemetry_model::registry;
use telemetry_model::{ChannelId, Sample, TelemetryFrame, Value, ValueType, ch};
use thiserror::Error;
use tracing::{debug, warn};

use crate::common::{Metric, finite, frame_f64};
use crate::publish::{PublishError, PublishReport, check_contract};
use crate::tyre::{TimeWindow, valid_window};

/// Tracing component name.
const COMPONENT: &str = "race_engines::energy";

/// Formula ids of the energy outputs. Where a registry channel exists the id equals its `formula:` origin.
pub mod formula {
    /// VE used during the last completed lap (sum of decreases).
    pub const VE_USED_LAST_LAP: &str = "ve_used_last_lap";
    /// Window average of VE per lap.
    pub const VE_PER_LAP: &str = "ve_per_lap_window_average";
    /// VE used since the lap start (sum of decreases).
    pub const VE_USED_THIS_LAP: &str = "ve_used_since_lap_start";
    /// Current VE divided by VE per lap.
    pub const VE_LAPS_REMAINING: &str = "ve_laps_remaining";
    /// Current VE minus VE per lap times the remaining lap fraction.
    pub const VE_AT_LAP_END: &str = "ve_estimated_at_lap_end";
    /// Negated least-squares slope of VE over the rate window (consumption per second).
    pub const VE_RATE: &str = "ve_use_rate_least_squares";
    /// SoC decreases summed over the lap.
    pub const SOC_DEPLOYED: &str = "soc_sum_of_decreases";
    /// SoC increases summed over the lap.
    pub const SOC_HARVESTED: &str = "soc_sum_of_increases";
    /// Least-squares slope of SoC over the rate window.
    pub const SOC_RATE: &str = "soc_rate_least_squares";
    /// Mechanical motor power τ·ω.
    pub const MOTOR_POWER: &str = "motor_power_torque_times_omega";
    /// Trapezoidal integral of positive motor power.
    pub const DEPLOYED_ENERGY: &str = "motor_deployed_energy_trapezoid";
    /// Trapezoidal integral of negative motor power (reported positive).
    pub const HARVESTED_ENERGY: &str = "motor_harvested_energy_trapezoid";
    /// Trapezoidal integral of `mRegen`.
    pub const REGEN_ENERGY: &str = "regen_energy_trapezoid";
}

/// Largest sample gap (s) that is still integrated / treated as continuous.
pub const MAX_INTEGRATION_GAP_S: f64 = 0.5;
/// Laps kept for the per-lap window average and history.
pub const MAX_LAP_HISTORY: usize = 64;
/// Default window of laps for the VE per-lap average.
pub const DEFAULT_VE_WINDOW_LAPS: usize = 5;
/// Default rate window (s).
pub const DEFAULT_RATE_WINDOW_S: f64 = 5.0;
/// Minimum samples for a rate.
pub const RATE_MIN_SAMPLES: usize = 3;
/// rpm → rad/s.
const RPM_TO_RAD_PER_S: f64 = 2.0 * PI / 60.0;
/// Largest lap progress (0..1) at the first observed sample for which the first lap counts as observed
/// from its start.
pub const LAP_START_PROGRESS_TOLERANCE: f64 = 0.02;
/// VE rise above the lowest level since the last refill treated as a refill (VE never rises while
/// driving), whether it arrives in one sample or gradually.
pub const VE_REFILL_STEP_FRAC: f64 = 0.005;
/// VE drop in one sample treated as a reset by LMU rather than use (Gate 1 pitstop1, return to the
/// garage: 0.9392 -> 0.2100 in 28 ms). A whole lap uses at most about a quarter of the allowance, so even
/// across the longest integrated step ([`MAX_INTEGRATION_GAP_S`]) real use stays far below this.
pub const VE_RESET_STEP_FRAC: f64 = 0.05;
/// SoC change in one sample treated as a reset / refill rather than deployment or harvest.
pub const SOC_JUMP_STEP_FRAC: f64 = 0.05;

/// One energy sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EnergySample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Virtual energy remaining (fraction).
    pub virtual_energy: Option<f64>,
    /// Battery state of charge (fraction).
    pub soc: Option<f64>,
    /// Regeneration power (W).
    pub regen_power_w: Option<f64>,
    /// Hybrid motor torque (N·m; negative regenerates).
    pub motor_torque_nm: Option<f64>,
    /// Hybrid motor speed (rpm).
    pub motor_rpm: Option<f64>,
    /// Lap progress 0..1 (from the timing / track engines), when known.
    pub lap_progress: Option<f64>,
}

impl EnergySample {
    /// From a canonical frame; `lap_progress` comes from the timing engine. `None` when the sample time is
    /// unknown. Unavailable channels stay `None`.
    pub fn from_frame(frame: &TelemetryFrame, lap_progress: Option<f64>) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        Some(Self {
            et_s,
            virtual_energy: frame_f64(frame, ch::VIRTUAL_ENERGY),
            soc: frame_f64(frame, ch::BATTERY_SOC),
            regen_power_w: frame_f64(frame, ch::REGEN_POWER),
            motor_torque_nm: frame_f64(frame, ch::HYBRID_MOTOR_TORQUE),
            motor_rpm: frame_f64(frame, ch::HYBRID_MOTOR_RPM),
            lap_progress,
        })
    }

    fn motor_power_w(&self) -> Option<f64> {
        finite(Some(
            finite(self.motor_torque_nm)? * finite(self.motor_rpm)? * RPM_TO_RAD_PER_S,
        ))
    }
}

/// Energy engine configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnergyConfig {
    /// Laps in the VE per-lap window average (1..=[`MAX_LAP_HISTORY`]).
    pub ve_window_laps: usize,
    /// Rate window (s).
    pub rate_window_s: f64,
}

impl Default for EnergyConfig {
    fn default() -> Self {
        Self {
            ve_window_laps: DEFAULT_VE_WINDOW_LAPS,
            rate_window_s: DEFAULT_RATE_WINDOW_S,
        }
    }
}

/// Invalid configuration.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum EnergyConfigError {
    /// Window of laps out of range.
    #[error("ve_window_laps must be in 1..={max}, got {got}")]
    InvalidLapWindow {
        /// Rejected value.
        got: usize,
        /// Upper bound.
        max: usize,
    },
    /// Rate window not a positive finite number.
    #[error("rate_window_s must be a positive finite number of seconds, got {0}")]
    InvalidRateWindow(f64),
}

impl EnergyConfig {
    /// Validates the configuration.
    pub fn validate(&self) -> Result<(), EnergyConfigError> {
        if self.ve_window_laps == 0 || self.ve_window_laps > MAX_LAP_HISTORY {
            return Err(EnergyConfigError::InvalidLapWindow {
                got: self.ve_window_laps,
                max: MAX_LAP_HISTORY,
            });
        }
        if !valid_window(self.rate_window_s) {
            return Err(EnergyConfigError::InvalidRateWindow(self.rate_window_s));
        }
        Ok(())
    }
}

/// A per-lap accumulator of one quantity: sums and completeness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
struct Accum {
    /// Sum of decreases (or positive integral).
    down: f64,
    /// Sum of increases (or negative integral, reported positive).
    up: f64,
    /// Seen on the lap's first sample and never missing since.
    complete: bool,
    /// Seen at least once.
    seen: bool,
    /// At least one interval was integrated (a lap re-opened at the boundary and closed again without a
    /// new sample has none and is not complete).
    integrated: bool,
}

impl Accum {
    fn starting(available: bool) -> Self {
        Self {
            complete: available,
            seen: available,
            ..Self::default()
        }
    }

    fn missing(&mut self) {
        self.complete = false;
    }

    fn add(&mut self, delta: f64) {
        self.seen = true;
        self.integrated = true;
        if delta < 0.0 {
            self.down -= delta;
        } else {
            self.up += delta;
        }
    }

    fn is_complete(&self) -> bool {
        self.seen && self.complete && self.integrated
    }

    fn down_complete(&self) -> Option<f64> {
        self.is_complete().then_some(self.down)
    }

    fn up_complete(&self) -> Option<f64> {
        self.is_complete().then_some(self.up)
    }

    /// Both directions summed (a magnitude integral).
    fn total_complete(&self) -> Option<f64> {
        self.is_complete().then_some(self.down + self.up)
    }

    /// Splits the trapezoid of a power over `dt` by sign so the two directions do not cancel: the
    /// positive part goes to `down`, the negative part (as a magnitude) to `up`.
    fn add_power_interval(&mut self, a: f64, b: f64, dt: f64) {
        let pos = (a.max(0.0) + b.max(0.0)) / 2.0 * dt;
        let neg = (a.min(0.0) + b.min(0.0)) / 2.0 * dt;
        self.add(-pos);
        self.add(-neg);
    }
}

/// Totals of one completed lap. A value is `None` when the quantity was unavailable or the lap was not
/// fully observed for it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EnergyLapRecord {
    /// Lap ordinal within this engine (0-based).
    pub index: u64,
    /// VE used (fraction).
    pub ve_used: Option<f64>,
    /// SoC deployed (fraction).
    pub soc_deployed: Option<f64>,
    /// SoC harvested (fraction).
    pub soc_harvested: Option<f64>,
    /// Motor energy deployed (J).
    pub deployed_j: Option<f64>,
    /// Motor energy harvested (J).
    pub harvested_j: Option<f64>,
    /// Integral of `|mRegen|` (J); the sign convention of `mRegen` is unverified, so the magnitude is used.
    pub regen_j: Option<f64>,
    /// Whether the caller included the lap in averages (no pit, no caution, observed start).
    pub included: bool,
}

/// Energy engine snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EnergySnapshot {
    /// Current VE (fraction).
    pub virtual_energy: Metric<f64>,
    /// VE used since the lap start.
    pub ve_used_this_lap: Metric<f64>,
    /// VE used last lap (complete laps only). Like the fuel engine's last-lap value it ignores the lap's
    /// `included` flag (a pit lap still used energy); only the per-lap average honours it.
    pub ve_used_last_lap: Metric<f64>,
    /// VE per lap, window average of complete included laps.
    pub ve_per_lap: Metric<f64>,
    /// Laps of VE remaining.
    pub ve_laps_remaining: Metric<f64>,
    /// Estimated VE at the end of the current lap.
    pub ve_at_lap_end: Metric<f64>,
    /// VE consumption rate (fraction/s, positive when consuming).
    pub ve_rate_per_s: Metric<f64>,
    /// Current battery SoC (fraction).
    pub soc: Metric<f64>,
    /// SoC deployed this lap.
    pub soc_deployed_this_lap: Metric<f64>,
    /// SoC harvested this lap.
    pub soc_harvested_this_lap: Metric<f64>,
    /// SoC rate (fraction/s, negative when depleting).
    pub soc_rate_per_s: Metric<f64>,
    /// Current mechanical motor power (W; negative regenerating).
    pub motor_power_w: Metric<f64>,
    /// Motor energy deployed this lap (J).
    pub deployed_this_lap_j: Metric<f64>,
    /// Motor energy harvested this lap (J).
    pub harvested_this_lap_j: Metric<f64>,
    /// Regeneration energy this lap (J): integral of `|mRegen|` (sign convention unverified).
    pub regen_this_lap_j: Metric<f64>,
    /// The last completed lap.
    pub last_lap: Option<EnergyLapRecord>,
}

#[derive(Debug, Clone, Copy, Default)]
struct LapAccum {
    ve: Accum,
    soc: Accum,
    motor: Accum,
    regen: Accum,
    started: bool,
}

impl LapAccum {
    /// A lap whose first sample is `s`.
    fn opening(s: &EnergySample) -> Self {
        Self {
            ve: Accum::starting(finite(s.virtual_energy).is_some()),
            soc: Accum::starting(finite(s.soc).is_some()),
            motor: Accum::starting(s.motor_power_w().is_some()),
            regen: Accum::starting(finite(s.regen_power_w).is_some()),
            started: true,
        }
    }

    fn all_missing(&mut self) {
        for a in [
            &mut self.ve,
            &mut self.soc,
            &mut self.motor,
            &mut self.regen,
        ] {
            a.missing();
        }
    }
}

/// Deterministic energy engine.
#[derive(Debug, Clone)]
pub struct EnergyEngine {
    config: EnergyConfig,
    lap: LapAccum,
    prev: Option<EnergySample>,
    last: EnergySample,
    ve_rate: TimeWindow,
    /// Lowest VE since the rate window was last cleared by a refill.
    ve_window_min: Option<f64>,
    /// Highest VE of a refill in progress; `Some` until consumption resumes.
    ve_refill_top: Option<f64>,
    soc_rate: TimeWindow,
    history: VecDeque<EnergyLapRecord>,
    laps: u64,
}

impl EnergyEngine {
    /// New engine; rejects an invalid configuration.
    pub fn new(config: EnergyConfig) -> Result<Self, EnergyConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            lap: LapAccum::default(),
            prev: None,
            last: EnergySample::default(),
            ve_rate: TimeWindow::default(),
            ve_window_min: None,
            ve_refill_top: None,
            soc_rate: TimeWindow::default(),
            history: VecDeque::new(),
            laps: 0,
        })
    }

    /// Consumes one sample. A non-finite time is ignored; a time regression or gap restarts the
    /// integration (the lap becomes incomplete for every quantity).
    pub fn update(&mut self, s: &EnergySample) {
        if !s.et_s.is_finite() {
            return;
        }
        if !self.lap.started {
            self.lap = LapAccum::opening(s);
            // Only boundaries the caller marks are known lap starts; the first lap begins wherever the
            // stream began, so it counts only when that was at the line.
            let at_line = finite(s.lap_progress)
                .is_some_and(|p| (0.0..=LAP_START_PROGRESS_TOLERANCE).contains(&p));
            if self.laps == 0 && !at_line {
                debug!(
                    component = COMPONENT,
                    et = s.et_s,
                    "first lap joined part-way: incomplete"
                );
                self.lap.all_missing();
            }
        }
        let continuous = self
            .prev
            .is_some_and(|p| s.et_s >= p.et_s && s.et_s - p.et_s <= MAX_INTEGRATION_GAP_S);
        if let Some(p) = self.prev.filter(|_| continuous) {
            let dt = s.et_s - p.et_s;
            step(
                &mut self.lap.ve,
                finite(p.virtual_energy),
                finite(s.virtual_energy),
            );
            step(&mut self.lap.soc, finite(p.soc), finite(s.soc));
            match (p.motor_power_w(), s.motor_power_w()) {
                (Some(a), Some(b)) => self.lap.motor.add_power_interval(a, b, dt),
                _ => self.lap.motor.missing(),
            }
            match (finite(p.regen_power_w), finite(s.regen_power_w)) {
                (Some(a), Some(b)) => self.lap.regen.add_power_interval(a, b, dt),
                _ => self.lap.regen.missing(),
            }
        } else if self.prev.is_some() {
            debug!(
                component = COMPONENT,
                et = s.et_s,
                "energy sample gap or regression"
            );
            self.lap.all_missing();
        }
        let ve_reset = self.prev.is_some_and(|p| {
            finite(p.virtual_energy)
                .zip(finite(s.virtual_energy))
                .is_some_and(|(a, b)| a - b > VE_RESET_STEP_FRAC)
        });
        if let Some(v) = finite(s.virtual_energy) {
            if ve_reset {
                // Not use: the lap's VE total and the rate window would read the reset as consumption.
                debug!(
                    component = COMPONENT,
                    et = s.et_s,
                    "VE reset: lap incomplete for VE, rate window cleared"
                );
                self.lap.ve.missing();
                self.ve_rate.clear();
                self.ve_window_min = Some(v);
                self.ve_refill_top = None;
            } else {
                self.track_ve_refill(s.et_s, v);
            }
        }
        if let Some(p) = self.prev
            && let (Some(a), Some(b)) = (finite(p.soc), finite(s.soc))
            && (b - a).abs() > SOC_JUMP_STEP_FRAC
        {
            debug!(
                component = COMPONENT,
                et = s.et_s,
                "SoC jump: lap incomplete for SoC, rate window cleared"
            );
            // A reset / refill is neither deployment nor harvest.
            self.lap.soc.missing();
            self.soc_rate.clear();
        }
        let w = self.config.rate_window_s;
        match finite(s.virtual_energy) {
            Some(v) => self.ve_rate.push(s.et_s, v, w),
            None => self.ve_rate.expire(s.et_s, w),
        }
        match finite(s.soc) {
            Some(v) => self.soc_rate.push(s.et_s, v, w),
            None => self.soc_rate.expire(s.et_s, w),
        }
        self.prev = Some(*s);
        self.last = *s;
    }

    /// Keeps a VE refill out of the use-rate window. Virtual energy only falls while driving, so a rise of
    /// more than [`VE_REFILL_STEP_FRAC`] above the lowest level since the window was last cleared is a
    /// refill, whether LMU applies it in one step or gradually over the stop. From then on the window is
    /// cleared on every sample (no rate is reported) while the level keeps rising or holds at its top;
    /// the first sample below the top (consumption resumed) starts a fresh window.
    fn track_ve_refill(&mut self, et_s: f64, v: f64) {
        let clear = if let Some(top) = self.ve_refill_top {
            if v < top {
                self.ve_refill_top = None;
                false
            } else {
                self.ve_refill_top = Some(v);
                true
            }
        } else if self
            .ve_window_min
            .is_some_and(|m| v > m + VE_REFILL_STEP_FRAC)
        {
            debug!(
                component = COMPONENT,
                et = et_s,
                "VE refill: rate window cleared until consumption resumes"
            );
            self.ve_refill_top = Some(v);
            true
        } else {
            false
        };
        if clear {
            self.ve_rate.clear();
            self.ve_window_min = Some(v);
        } else {
            self.ve_window_min = Some(self.ve_window_min.map_or(v, |m| m.min(v)));
        }
    }

    /// Closes the current lap. `included` is false for laps that must not feed averages (pit in/out,
    /// caution, formation, unobserved start). The next sample starts the new lap.
    pub fn lap_completed(&mut self, included: bool) -> EnergyLapRecord {
        let l = self.lap;
        // `motor` stores deployment as `down` and harvest as `up` (see `update`).
        let rec = EnergyLapRecord {
            index: self.laps,
            ve_used: l.ve.down_complete(),
            soc_deployed: l.soc.down_complete(),
            soc_harvested: l.soc.up_complete(),
            deployed_j: l.motor.down_complete(),
            harvested_j: l.motor.up_complete(),
            regen_j: l.regen.total_complete(),
            included,
        };
        self.laps += 1;
        self.history.push_back(rec);
        while self.history.len() > MAX_LAP_HISTORY {
            self.history.pop_front();
        }
        // The boundary sample belongs to both laps: re-open with it so no interval is lost.
        self.lap = self
            .prev
            .as_ref()
            .map_or_else(LapAccum::default, LapAccum::opening);
        rec
    }

    /// Completed laps, oldest first (bounded by [`MAX_LAP_HISTORY`]).
    pub fn laps(&self) -> impl Iterator<Item = &EnergyLapRecord> {
        self.history.iter()
    }

    fn ve_per_lap(&self) -> Option<f64> {
        let vals: Vec<f64> = self
            .history
            .iter()
            .rev()
            .filter(|r| r.included)
            .filter_map(|r| r.ve_used)
            .take(self.config.ve_window_laps)
            .collect();
        if vals.is_empty() {
            None
        } else {
            finite(Some(vals.iter().sum::<f64>() / vals.len() as f64))
        }
    }

    /// Current outputs.
    pub fn snapshot(&self) -> EnergySnapshot {
        let ve = finite(self.last.virtual_energy);
        let per_lap = ve.and(self.ve_per_lap());
        let laps_remaining = match (ve, per_lap) {
            (Some(v), Some(p)) if p > 0.0 => finite(Some(v / p)),
            _ => None,
        };
        let progress = finite(self.last.lap_progress).filter(|p| (0.0..=1.0).contains(p));
        let at_end = match (ve, per_lap, progress) {
            (Some(v), Some(p), Some(x)) => finite(Some((v - p * (1.0 - x)).max(0.0))),
            _ => None,
        };
        let ve_lap = |f: fn(&Accum) -> Option<f64>| ve.and(f(&self.lap.ve));
        let soc = finite(self.last.soc);
        let last_lap = self.history.back().copied();
        EnergySnapshot {
            virtual_energy: Metric::measured("mVirtualEnergy", ve),
            ve_used_this_lap: Metric::derived(
                formula::VE_USED_THIS_LAP,
                ve_lap(Accum::down_complete),
            ),
            ve_used_last_lap: Metric::derived(
                formula::VE_USED_LAST_LAP,
                last_lap.and_then(|r| r.ve_used),
            ),
            ve_per_lap: Metric::estimated(formula::VE_PER_LAP, per_lap),
            ve_laps_remaining: Metric::estimated(formula::VE_LAPS_REMAINING, laps_remaining),
            ve_at_lap_end: Metric::estimated(formula::VE_AT_LAP_END, at_end),
            ve_rate_per_s: Metric::estimated(
                formula::VE_RATE,
                ve.and(self.ve_rate.slope(RATE_MIN_SAMPLES)).map(|s| -s),
            ),
            soc: Metric::measured("mSoC", soc),
            soc_deployed_this_lap: Metric::derived(
                formula::SOC_DEPLOYED,
                soc.and(self.lap.soc.down_complete()),
            ),
            soc_harvested_this_lap: Metric::derived(
                formula::SOC_HARVESTED,
                soc.and(self.lap.soc.up_complete()),
            ),
            soc_rate_per_s: Metric::estimated(
                formula::SOC_RATE,
                soc.and(self.soc_rate.slope(RATE_MIN_SAMPLES)),
            ),
            motor_power_w: Metric::derived(formula::MOTOR_POWER, self.last.motor_power_w()),
            deployed_this_lap_j: Metric::derived(
                formula::DEPLOYED_ENERGY,
                self.last
                    .motor_power_w()
                    .and(self.lap.motor.down_complete()),
            ),
            harvested_this_lap_j: Metric::derived(
                formula::HARVESTED_ENERGY,
                self.last.motor_power_w().and(self.lap.motor.up_complete()),
            ),
            regen_this_lap_j: Metric::derived(
                formula::REGEN_ENERGY,
                finite(self.last.regen_power_w).and(self.lap.regen.total_complete()),
            ),
            last_lap,
        }
    }
}

/// Registry channels written by [`publish_energy`] (the VE outputs that have a registry channel).
pub const PUBLISHED_CHANNELS: &[ChannelId] = &[
    ch::VE_USED_LAST_LAP,
    ch::VE_PER_LAP,
    ch::VE_USED_THIS_LAP,
    ch::VE_LAPS_REMAINING,
];

/// Writes the VE outputs of `snapshot` into their registry channels ([`PUBLISHED_CHANNELS`]) with the
/// same rules as [`crate::publish::publish`]: every channel is written on every call; a missing value (or
/// `snapshot == None`) is written `Unavailable`, never 0; a value outside the registry range is written
/// `Invalid` (never clamped); a formula / provenance that differs from the registry is written `Invalid`
/// and reported.
pub fn publish_energy(
    frame: &mut TelemetryFrame,
    snapshot: Option<&EnergySnapshot>,
) -> PublishReport {
    let none = Metric::<f64>::derived("", None);
    let metrics = snapshot.map_or([none; 4], |s| {
        [
            s.ve_used_last_lap,
            s.ve_per_lap,
            s.ve_used_this_lap,
            s.ve_laps_remaining,
        ]
    });
    let mut r = PublishReport::default();
    for (id, m) in PUBLISHED_CHANNELS.iter().copied().zip(metrics) {
        write_metric(frame, id, m, &mut r);
    }
    debug!(
        component = COMPONENT,
        valid = r.valid,
        unavailable = r.unavailable,
        invalid = r.out_of_range.len(),
        errors = r.errors.len(),
        "energy outputs published"
    );
    r
}

fn write_metric(frame: &mut TelemetryFrame, id: ChannelId, m: Metric<f64>, r: &mut PublishReport) {
    let Some(def) = registry::by_id(id) else {
        r.errors.push(PublishError::UnknownChannel(id.0));
        return;
    };
    let sample = match finite(m.value) {
        None => {
            r.unavailable += 1;
            Sample::unavailable()
        }
        Some(x) => {
            let contract_ok = check_contract(def, &m, r);
            let in_range = def.valid_range.is_none_or(|(lo, hi)| x >= lo && x <= hi);
            let value = match def.value_type {
                // The registry checks that its f32 bounds are representable; a range-checked value fits.
                ValueType::F32 => Some(Value::F32(x as f32)),
                ValueType::F64 => Some(Value::F64(x)),
                // Never round a calculated float into an integer channel.
                _ => None,
            };
            match value {
                _ if !contract_ok => Sample::invalid(),
                _ if !in_range => {
                    debug!(
                        component = COMPONENT,
                        channel = def.name,
                        value = x,
                        "energy value outside the registry range: published INVALID"
                    );
                    r.out_of_range.push(id);
                    Sample::invalid()
                }
                Some(v) => {
                    r.valid += 1;
                    Sample::valid(v)
                }
                None => {
                    r.errors.push(PublishError::NotRepresentable {
                        channel: id.0,
                        value_type: def.value_type,
                    });
                    Sample::invalid()
                }
            }
        }
    };
    if let Err(source) = frame.set(id, sample) {
        warn!(component = COMPONENT, channel = def.name, error = %source, "sample rejected");
        r.errors.push(PublishError::Frame {
            channel: id.0,
            source,
        });
    }
}

/// Adds the change between two level samples, or marks the quantity missing.
fn step(acc: &mut Accum, prev: Option<f64>, cur: Option<f64>) {
    match (prev, cur) {
        (Some(a), Some(b)) => acc.add(b - a),
        _ => acc.missing(),
    }
}

#[cfg(test)]
mod tests;

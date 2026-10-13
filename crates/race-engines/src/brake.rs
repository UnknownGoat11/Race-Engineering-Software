//! Brake engine (spec §28, §145 "brake state"): per-wheel brake temperature, its trend and peak, the
//! front/rear temperature distribution and the brake bias where LMU provides it.
//!
//! # Inputs
//! Brake temperatures are `mWheel[].mBrakeTemp` in °C (the unit is detected by the normalizer, A-LMU-015).
//! The bias is `mRearBrakeBias` (rear share of braking); its front share is `1 − rear`. When the bias
//! channel is unavailable the bias outputs are `None` — never a guessed 50/50.
//!
//! # Thresholds
//! Brake operating windows depend on the car and pad, so thresholds are optional and engineer-configured;
//! without them the band is [`Band::Unknown`].

use serde::{Deserialize, Serialize};
use telemetry_model::{ChannelId, TelemetryFrame, ch};
use thiserror::Error;
use tracing::debug;

use crate::common::{Metric, finite, frame_f64};
use crate::tyre::{Band, TimeWindow, WHEELS, Wheel, check_pair, valid_window};

/// Tracing component name.
const COMPONENT: &str = "race_engines::brake";

/// Formula ids of the brake outputs.
pub mod formula {
    /// Least-squares slope of the brake temperature over the trend window.
    pub const TREND: &str = "brake_temp_least_squares_slope";
    /// Highest brake temperature since the stint start.
    pub const PEAK: &str = "brake_temp_peak_since_stint_start";
    /// Front-axle mean minus rear-axle mean brake temperature. (There is deliberately no front/total
    /// temperature *ratio*: a ratio of Celsius values depends on the unit's offset zero and would read like
    /// a brake balance, which it is not.)
    pub const FRONT_REAR_DELTA: &str = "brake_front_minus_rear_axle_mean";
    /// Front bias as `1 − mRearBrakeBias`.
    pub const BIAS_FRONT: &str = "brake_bias_front_one_minus_rear";
    /// Value compared against the engineer's configured thresholds.
    pub const BAND: &str = "brake_threshold_band";
}

/// LMU origin of the rear bias.
const BIAS_ORIGIN: &str = "mRearBrakeBias";
/// LMU origin of the brake temperatures.
const TEMP_ORIGIN: &str = "mWheel[].mBrakeTemp";
/// Default trend window (s).
pub const DEFAULT_TREND_WINDOW_S: f64 = 5.0;
/// Minimum samples for a trend.
pub const TREND_MIN_SAMPLES: usize = 3;

const BRAKE_TEMP_CHANNELS: [ChannelId; WHEELS] = [
    ch::BRAKE_TEMP_FL,
    ch::BRAKE_TEMP_FR,
    ch::BRAKE_TEMP_RL,
    ch::BRAKE_TEMP_RR,
];

/// One brake sample.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct BrakeSample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Brake temperature per wheel (°C), indexed by [`Wheel::index`].
    pub temps_c: [Option<f64>; WHEELS],
    /// Rear brake bias (fraction of braking on the rear axle), when available.
    pub bias_rear: Option<f64>,
}

impl BrakeSample {
    /// From a canonical frame; `None` when the sample time is unknown.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        Some(Self {
            et_s,
            temps_c: BRAKE_TEMP_CHANNELS.map(|id| frame_f64(frame, id)),
            bias_rear: frame_f64(frame, ch::BRAKE_BIAS_REAR),
        })
    }
}

/// Engineer-configured brake thresholds (optional).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrakeThresholds {
    /// Below this the brake is `Low` (cold) (°C).
    pub temp_low_c: Option<f64>,
    /// Above this the brake is `High` (overheating) (°C).
    pub temp_high_c: Option<f64>,
}

/// Brake engine configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrakeConfig {
    /// Trend window (s).
    pub trend_window_s: f64,
    /// Optional thresholds.
    pub thresholds: BrakeThresholds,
}

impl Default for BrakeConfig {
    fn default() -> Self {
        Self {
            trend_window_s: DEFAULT_TREND_WINDOW_S,
            thresholds: BrakeThresholds::default(),
        }
    }
}

/// Invalid configuration.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BrakeConfigError {
    /// The trend window is not a positive finite number.
    #[error("trend_window_s must be a positive finite number of seconds, got {0}")]
    InvalidWindow(f64),
    /// A threshold is not finite or low ≥ high.
    #[error("invalid threshold {field}: {reason}")]
    InvalidThreshold {
        /// Field name.
        field: &'static str,
        /// Why.
        reason: &'static str,
    },
}

impl BrakeConfig {
    /// Validates the window and thresholds.
    pub fn validate(&self) -> Result<(), BrakeConfigError> {
        if !valid_window(self.trend_window_s) {
            return Err(BrakeConfigError::InvalidWindow(self.trend_window_s));
        }
        check_pair(
            self.thresholds.temp_low_c,
            self.thresholds.temp_high_c,
            "temp_low_c",
            "temp_high_c",
            false,
        )
        .map_err(|(field, reason)| BrakeConfigError::InvalidThreshold { field, reason })
    }
}

/// Per-wheel brake outputs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WheelBrakeState {
    /// Which wheel.
    pub wheel: Wheel,
    /// Current temperature (°C).
    pub temp_c: Metric<f64>,
    /// Temperature trend (°C/s).
    pub trend_c_per_s: Metric<f64>,
    /// Peak since the stint start (°C).
    pub peak_c: Metric<f64>,
    /// Temperature against the thresholds.
    pub band: Band,
}

/// Brake engine snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BrakeSnapshot {
    /// Per wheel, indexed by [`Wheel::index`].
    pub wheels: [WheelBrakeState; WHEELS],
    /// Front-axle minus rear-axle mean temperature (°C); needs all four wheels.
    pub front_rear_delta_c: Metric<f64>,
    /// Rear brake bias (LMU pass-through), when available.
    pub bias_rear: Metric<f64>,
    /// Front brake bias (`1 − rear`), when available.
    pub bias_front: Metric<f64>,
}

#[derive(Debug, Clone, Default)]
struct WheelState {
    last: Option<f64>,
    peak: Option<f64>,
    trend: TimeWindow,
}

/// Deterministic brake engine.
#[derive(Debug, Clone)]
pub struct BrakeEngine {
    config: BrakeConfig,
    wheels: [WheelState; WHEELS],
    bias_rear: Option<f64>,
}

impl BrakeEngine {
    /// New engine; rejects an invalid configuration.
    pub fn new(config: BrakeConfig) -> Result<Self, BrakeConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            wheels: Default::default(),
            bias_rear: None,
        })
    }

    /// Resets peaks and trends at a stint start.
    pub fn start_stint(&mut self) {
        debug!(component = COMPONENT, "brake stint start");
        for w in &mut self.wheels {
            w.peak = None;
            w.trend.clear();
        }
    }

    /// Consumes one sample. A non-finite time is ignored.
    pub fn update(&mut self, sample: &BrakeSample) {
        let et = sample.et_s;
        if !et.is_finite() {
            return;
        }
        let window = self.config.trend_window_s;
        for (st, t) in self.wheels.iter_mut().zip(sample.temps_c) {
            let t = finite(t);
            st.last = t;
            match t {
                Some(v) => {
                    st.peak = Some(st.peak.map_or(v, |p| p.max(v)));
                    st.trend.push(et, v, window);
                }
                None => st.trend.expire(et, window),
            }
        }
        // Bias is a setting; it is only meaningful inside (0, 1).
        self.bias_rear = finite(sample.bias_rear).filter(|b| *b > 0.0 && *b < 1.0);
    }

    /// Current outputs.
    pub fn snapshot(&self) -> BrakeSnapshot {
        let th = self.config.thresholds;
        let wheels = Wheel::ALL.map(|wheel| {
            let st = &self.wheels[wheel.index()];
            WheelBrakeState {
                wheel,
                temp_c: Metric::measured(TEMP_ORIGIN, st.last),
                trend_c_per_s: Metric::estimated(formula::TREND, st.trend.slope(TREND_MIN_SAMPLES)),
                peak_c: Metric::derived(formula::PEAK, st.peak),
                band: Band::classify(st.last, th.temp_low_c, th.temp_high_c),
            }
        });
        let all: Option<[f64; WHEELS]> = {
            let mut out = [0.0; WHEELS];
            let mut ok = true;
            for (o, st) in out.iter_mut().zip(&self.wheels) {
                match st.last {
                    Some(v) => *o = v,
                    None => ok = false,
                }
            }
            ok.then_some(out)
        };
        let fl = Wheel::FrontLeft.index();
        let fr = Wheel::FrontRight.index();
        let rl = Wheel::RearLeft.index();
        let rr = Wheel::RearRight.index();
        let delta = all.and_then(|t| finite(Some((t[fl] + t[fr]) / 2.0 - (t[rl] + t[rr]) / 2.0)));
        BrakeSnapshot {
            wheels,
            front_rear_delta_c: Metric::derived(formula::FRONT_REAR_DELTA, delta),
            bias_rear: Metric::measured(BIAS_ORIGIN, self.bias_rear),
            bias_front: Metric::derived(formula::BIAS_FRONT, self.bias_rear.map(|r| 1.0 - r)),
        }
    }
}

#[cfg(test)]
mod tests;

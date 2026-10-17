//! Tyre engine (spec §27, §145 "tyre state"): per-wheel pressures and temperatures, deltas against the
//! stint start, peak, rolling average, edge deltas, axle balances and the temperature rise rate.
//!
//! # Wheel mapping (ASSUMPTION A-LMU-016)
//! LMU orders `mTemperature[3]` / `mTireInnerLayerTemperature[3]` **left / centre / right in car
//! coordinates**, not inner / centre / outer. The inner edge is the one facing the car's centre line, so
//! (LMU_CHANNEL_MAP §11):
//!
//! | wheel | inner      | outer      |
//! |-------|------------|------------|
//! | FL/RL | right `[2]`| left `[0]` |
//! | FR/RR | left `[0]` | right `[2]`|
//!
//! [`Wheel::inner_outer`] is the only place this mapping lives.
//!
//! # Thresholds
//! There is no universal "correct" tyre temperature or pressure: every compound and car differs, so all
//! thresholds in [`TyreThresholds`] are optional and configured by the engineer. Without a threshold the
//! band is [`Band::Unknown`], never a guessed "OK".
//!
//! # Time base
//! Rolling statistics use a time window over LMU elapsed time. An elapsed-time regression (session restart
//! or rewind) starts a new stint ([`TyreEngine::start_stint`]): windows, peaks and stint-start values are
//! reset so no delta compares against the previous session; memory is bounded by [`MAX_WINDOW_SAMPLES`]
//! per wheel.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use telemetry_model::{ChannelId, TelemetryFrame, ch};
use thiserror::Error;
use tracing::debug;

use crate::common::{Metric, finite, frame_f64, least_squares_slope};

/// Tracing component name.
const COMPONENT: &str = "race_engines::tyre";

/// Formula ids of the tyre outputs.
pub mod formula {
    /// Inner/outer edge selected from the car-frame left/right edge by wheel side (A-LMU-016).
    pub const EDGE_MAPPING: &str = "tyre_edge_inner_outer_by_wheel_side";
    /// Mean of the three surface temperatures (inner, middle, outer).
    pub const SURFACE_AVG: &str = "tyre_surface_temp_mean_of_three";
    /// Mean of the three inner-layer temperatures.
    pub const LAYER_AVG: &str = "tyre_layer_temp_mean_of_three";
    /// Current value minus the value captured at the stint start.
    pub const DELTA_VS_STINT_START: &str = "tyre_delta_vs_stint_start";
    /// Highest surface average since the stint start.
    pub const PEAK: &str = "tyre_surface_avg_peak_since_stint_start";
    /// Time-window mean of the surface average.
    pub const ROLLING_AVG: &str = "tyre_surface_avg_time_window_mean";
    /// Inner minus middle surface temperature.
    pub const INNER_MIDDLE: &str = "tyre_inner_minus_middle";
    /// Middle minus outer surface temperature.
    pub const MIDDLE_OUTER: &str = "tyre_middle_minus_outer";
    /// Least-squares slope of the surface average over the rise-rate window.
    pub const RISE_RATE: &str = "tyre_surface_avg_least_squares_slope";
    /// Front axle mean minus rear axle mean.
    pub const FRONT_REAR: &str = "tyre_front_minus_rear_axle_mean";
    /// Left side mean minus right side mean.
    pub const LEFT_RIGHT: &str = "tyre_left_minus_right_side_mean";
    /// Value compared against the engineer's configured thresholds.
    pub const BAND: &str = "tyre_threshold_band";
}

/// Number of wheels.
pub const WHEELS: usize = 4;
/// Upper bound on samples kept per wheel window (bounded memory at any sample rate).
pub const MAX_WINDOW_SAMPLES: usize = 4_096;
/// Default rolling-average window (s).
pub const DEFAULT_ROLLING_WINDOW_S: f64 = 10.0;
/// Default rise-rate window (s).
pub const DEFAULT_RISE_RATE_WINDOW_S: f64 = 3.0;
/// Minimum samples for a rise rate.
pub const RISE_RATE_MIN_SAMPLES: usize = 3;

/// A wheel, in LMU `mWheel[]` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Wheel {
    /// Front left (`mWheel[0]`).
    FrontLeft = 0,
    /// Front right (`mWheel[1]`).
    FrontRight = 1,
    /// Rear left (`mWheel[2]`).
    RearLeft = 2,
    /// Rear right (`mWheel[3]`).
    RearRight = 3,
}

impl Wheel {
    /// All wheels in `mWheel[]` order.
    pub const ALL: [Wheel; WHEELS] = [
        Wheel::FrontLeft,
        Wheel::FrontRight,
        Wheel::RearLeft,
        Wheel::RearRight,
    ];

    /// Index into `mWheel[]` and the per-wheel arrays of this crate.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// True for FL and FR.
    pub const fn is_front(self) -> bool {
        matches!(self, Wheel::FrontLeft | Wheel::FrontRight)
    }

    /// True for FL and RL.
    pub const fn is_left(self) -> bool {
        matches!(self, Wheel::FrontLeft | Wheel::RearLeft)
    }

    /// Short label (`FL`, `FR`, `RL`, `RR`).
    pub const fn label(self) -> &'static str {
        match self {
            Wheel::FrontLeft => "FL",
            Wheel::FrontRight => "FR",
            Wheel::RearLeft => "RL",
            Wheel::RearRight => "RR",
        }
    }

    /// Maps car-frame `(left, right)` edge values to `(inner, outer)` for this wheel (A-LMU-016): on a
    /// left-side wheel the right edge faces the car, on a right-side wheel the left edge does.
    pub fn inner_outer<T>(self, left: T, right: T) -> (T, T) {
        if self.is_left() {
            (right, left)
        } else {
            (left, right)
        }
    }
}

/// Raw per-wheel tyre inputs (°C, Pa), car-frame edge order as LMU provides it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TyreWheelSample {
    /// Pressure (Pa).
    pub pressure_pa: Option<f64>,
    /// Surface temperature, car-frame left edge (°C).
    pub surface_left_c: Option<f64>,
    /// Surface temperature, centre (°C).
    pub surface_center_c: Option<f64>,
    /// Surface temperature, car-frame right edge (°C).
    pub surface_right_c: Option<f64>,
    /// Inner-layer temperature, car-frame left (°C).
    pub layer_left_c: Option<f64>,
    /// Inner-layer temperature, centre (°C).
    pub layer_center_c: Option<f64>,
    /// Inner-layer temperature, car-frame right (°C).
    pub layer_right_c: Option<f64>,
    /// Carcass temperature (°C).
    pub carcass_c: Option<f64>,
}

/// One tyre sample for all four wheels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TyreSample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Per wheel, indexed by [`Wheel::index`].
    pub wheels: [TyreWheelSample; WHEELS],
}

/// Channel ids of one wheel, in `TyreWheelSample` field order.
struct WheelChannels {
    pressure: ChannelId,
    surface: [ChannelId; 3],
    layer: [ChannelId; 3],
    carcass: ChannelId,
}

const WHEEL_CHANNELS: [WheelChannels; WHEELS] = [
    WheelChannels {
        pressure: ch::TYRE_PRESSURE_FL,
        surface: [
            ch::TYRE_TEMP_LEFT_FL,
            ch::TYRE_TEMP_MIDDLE_FL,
            ch::TYRE_TEMP_RIGHT_FL,
        ],
        layer: [
            ch::TYRE_LAYER_TEMP_LEFT_FL,
            ch::TYRE_LAYER_TEMP_MIDDLE_FL,
            ch::TYRE_LAYER_TEMP_RIGHT_FL,
        ],
        carcass: ch::TYRE_CARCASS_TEMP_FL,
    },
    WheelChannels {
        pressure: ch::TYRE_PRESSURE_FR,
        surface: [
            ch::TYRE_TEMP_LEFT_FR,
            ch::TYRE_TEMP_MIDDLE_FR,
            ch::TYRE_TEMP_RIGHT_FR,
        ],
        layer: [
            ch::TYRE_LAYER_TEMP_LEFT_FR,
            ch::TYRE_LAYER_TEMP_MIDDLE_FR,
            ch::TYRE_LAYER_TEMP_RIGHT_FR,
        ],
        carcass: ch::TYRE_CARCASS_TEMP_FR,
    },
    WheelChannels {
        pressure: ch::TYRE_PRESSURE_RL,
        surface: [
            ch::TYRE_TEMP_LEFT_RL,
            ch::TYRE_TEMP_MIDDLE_RL,
            ch::TYRE_TEMP_RIGHT_RL,
        ],
        layer: [
            ch::TYRE_LAYER_TEMP_LEFT_RL,
            ch::TYRE_LAYER_TEMP_MIDDLE_RL,
            ch::TYRE_LAYER_TEMP_RIGHT_RL,
        ],
        carcass: ch::TYRE_CARCASS_TEMP_RL,
    },
    WheelChannels {
        pressure: ch::TYRE_PRESSURE_RR,
        surface: [
            ch::TYRE_TEMP_LEFT_RR,
            ch::TYRE_TEMP_MIDDLE_RR,
            ch::TYRE_TEMP_RIGHT_RR,
        ],
        layer: [
            ch::TYRE_LAYER_TEMP_LEFT_RR,
            ch::TYRE_LAYER_TEMP_MIDDLE_RR,
            ch::TYRE_LAYER_TEMP_RIGHT_RR,
        ],
        carcass: ch::TYRE_CARCASS_TEMP_RR,
    },
];

impl TyreSample {
    /// From a canonical frame (values already in °C / Pa); `None` when the sample time is unknown.
    /// Invalid, stale or unavailable channels become `None`.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        let mut wheels = [TyreWheelSample::default(); WHEELS];
        for (w, c) in wheels.iter_mut().zip(WHEEL_CHANNELS.iter()) {
            *w = TyreWheelSample {
                pressure_pa: frame_f64(frame, c.pressure),
                surface_left_c: frame_f64(frame, c.surface[0]),
                surface_center_c: frame_f64(frame, c.surface[1]),
                surface_right_c: frame_f64(frame, c.surface[2]),
                layer_left_c: frame_f64(frame, c.layer[0]),
                layer_center_c: frame_f64(frame, c.layer[1]),
                layer_right_c: frame_f64(frame, c.layer[2]),
                carcass_c: frame_f64(frame, c.carcass),
            };
        }
        Some(Self { et_s, wheels })
    }
}

/// Position of a value relative to optional low/high thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Band {
    /// No value, or no threshold configured on the side that would decide.
    Unknown,
    /// Below the low threshold.
    Low,
    /// Within the configured thresholds.
    InRange,
    /// Above the high threshold.
    High,
}

impl Band {
    /// Classifies `value` against optional `low` / `high` bounds. With no bounds at all the band is
    /// `Unknown` (there is no universal correct value).
    pub fn classify(value: Option<f64>, low: Option<f64>, high: Option<f64>) -> Band {
        let Some(v) = finite(value) else {
            return Band::Unknown;
        };
        if low.is_none() && high.is_none() {
            return Band::Unknown;
        }
        if high.is_some_and(|h| v > h) {
            Band::High
        } else if low.is_some_and(|l| v < l) {
            Band::Low
        } else {
            Band::InRange
        }
    }
}

/// Engineer-configured tyre thresholds. All optional; there is no universal default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TyreThresholds {
    /// Surface average below this is `Low` (°C).
    pub surface_low_c: Option<f64>,
    /// Surface average above this is `High` (°C).
    pub surface_high_c: Option<f64>,
    /// Pressure below this is `Low` (Pa).
    pub pressure_low_pa: Option<f64>,
    /// Pressure above this is `High` (Pa).
    pub pressure_high_pa: Option<f64>,
    /// |inner − outer| above this is `High` (°C).
    pub edge_spread_max_c: Option<f64>,
}

/// Tyre engine configuration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TyreConfig {
    /// Rolling-average window (s).
    pub rolling_window_s: f64,
    /// Rise-rate window (s).
    pub rise_rate_window_s: f64,
    /// Optional thresholds.
    pub thresholds: TyreThresholds,
}

impl Default for TyreConfig {
    fn default() -> Self {
        Self {
            rolling_window_s: DEFAULT_ROLLING_WINDOW_S,
            rise_rate_window_s: DEFAULT_RISE_RATE_WINDOW_S,
            thresholds: TyreThresholds::default(),
        }
    }
}

/// Invalid configuration.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TyreConfigError {
    /// A window length is not a positive finite number.
    #[error("{field} must be a positive finite number of seconds, got {value}")]
    InvalidWindow {
        /// Field name.
        field: &'static str,
        /// Rejected value.
        value: f64,
    },
    /// A threshold is not finite, negative where that is impossible, or low ≥ high.
    #[error("invalid threshold {field}: {reason}")]
    InvalidThreshold {
        /// Field name.
        field: &'static str,
        /// Why.
        reason: &'static str,
    },
}

/// Checks an optional low/high pair: both finite, low < high when both are set, and non-negative when
/// `non_negative` (pressures, spreads).
pub(crate) fn check_pair(
    low: Option<f64>,
    high: Option<f64>,
    low_field: &'static str,
    high_field: &'static str,
    non_negative: bool,
) -> Result<(), (&'static str, &'static str)> {
    for (v, f) in [(low, low_field), (high, high_field)] {
        if let Some(x) = v {
            if !x.is_finite() {
                return Err((f, "not finite"));
            }
            if non_negative && x < 0.0 {
                return Err((f, "negative"));
            }
        }
    }
    if let (Some(l), Some(h)) = (low, high)
        && l >= h
    {
        return Err((low_field, "low must be below high"));
    }
    Ok(())
}

/// True for a positive finite window length.
pub(crate) fn valid_window(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

impl TyreConfig {
    /// Validates windows and thresholds.
    pub fn validate(&self) -> Result<(), TyreConfigError> {
        for (field, value) in [
            ("rolling_window_s", self.rolling_window_s),
            ("rise_rate_window_s", self.rise_rate_window_s),
        ] {
            if !valid_window(value) {
                return Err(TyreConfigError::InvalidWindow { field, value });
            }
        }
        let t = &self.thresholds;
        let to_err = |(field, reason)| TyreConfigError::InvalidThreshold { field, reason };
        check_pair(
            t.surface_low_c,
            t.surface_high_c,
            "surface_low_c",
            "surface_high_c",
            false,
        )
        .map_err(to_err)?;
        check_pair(
            t.pressure_low_pa,
            t.pressure_high_pa,
            "pressure_low_pa",
            "pressure_high_pa",
            true,
        )
        .map_err(to_err)?;
        check_pair(
            None,
            t.edge_spread_max_c,
            "edge_spread_max_c",
            "edge_spread_max_c",
            true,
        )
        .map_err(to_err)?;
        Ok(())
    }
}

/// Bounded time window of `(et, value)` points with time-regression reset.
#[derive(Debug, Clone, Default)]
pub(crate) struct TimeWindow {
    points: VecDeque<(f64, f64)>,
}

impl TimeWindow {
    /// Adds a point and drops points older than `window_s` before `et`. A time regression clears it.
    pub(crate) fn push(&mut self, et: f64, value: f64, window_s: f64) {
        if self.points.back().is_some_and(|(t, _)| et < *t) {
            self.points.clear();
        }
        self.points.push_back((et, value));
        while self.points.front().is_some_and(|(t, _)| et - *t > window_s)
            || self.points.len() > MAX_WINDOW_SAMPLES
        {
            self.points.pop_front();
        }
    }

    /// Drops points older than `window_s` before `et` without adding one (keeps windows honest while the
    /// input is missing).
    pub(crate) fn expire(&mut self, et: f64, window_s: f64) {
        if self.points.back().is_some_and(|(t, _)| et < *t) {
            self.points.clear();
        }
        while self.points.front().is_some_and(|(t, _)| et - *t > window_s) {
            self.points.pop_front();
        }
    }

    pub(crate) fn mean(&self) -> Option<f64> {
        if self.points.is_empty() {
            return None;
        }
        finite(Some(
            self.points.iter().map(|p| p.1).sum::<f64>() / self.points.len() as f64,
        ))
    }

    pub(crate) fn slope(&self, min_samples: usize) -> Option<f64> {
        if self.points.len() < min_samples {
            return None;
        }
        let pts: Vec<(f64, f64)> = self.points.iter().copied().collect();
        least_squares_slope(&pts)
    }

    pub(crate) fn clear(&mut self) {
        self.points.clear();
    }
}

/// Mean of the values when all are present.
fn mean_all(values: &[Option<f64>]) -> Option<f64> {
    let mut sum = 0.0;
    for v in values {
        sum += finite(*v)?;
    }
    if values.is_empty() {
        None
    } else {
        finite(Some(sum / values.len() as f64))
    }
}

fn sub(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    finite(Some(finite(a)? - finite(b)?))
}

/// Values captured at the stint start (first sample where each is available after [`TyreEngine::start_stint`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
struct StartValues {
    pressure_pa: Option<f64>,
    surface_avg_c: Option<f64>,
    carcass_c: Option<f64>,
}

#[derive(Debug, Clone, Default)]
struct WheelState {
    start: StartValues,
    peak_surface_c: Option<f64>,
    rolling: TimeWindow,
    rise: TimeWindow,
    last: TyreWheelSample,
}

/// Per-wheel tyre outputs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WheelTyreState {
    /// Which wheel.
    pub wheel: Wheel,
    /// Current pressure (Pa).
    pub pressure_pa: Metric<f64>,
    /// Pressure at the stint start (Pa).
    pub start_pressure_pa: Metric<f64>,
    /// Current minus start pressure (Pa).
    pub pressure_delta_pa: Metric<f64>,
    /// Inner-edge surface temperature (°C).
    pub inner_c: Metric<f64>,
    /// Middle surface temperature (°C).
    pub middle_c: Metric<f64>,
    /// Outer-edge surface temperature (°C).
    pub outer_c: Metric<f64>,
    /// Mean of inner, middle, outer (°C).
    pub surface_avg_c: Metric<f64>,
    /// Mean of the three inner-layer temperatures (°C).
    pub layer_avg_c: Metric<f64>,
    /// Carcass temperature (°C).
    pub carcass_c: Metric<f64>,
    /// Surface average minus its stint-start value (°C).
    pub surface_delta_vs_start_c: Metric<f64>,
    /// Carcass minus its stint-start value (°C).
    pub carcass_delta_vs_start_c: Metric<f64>,
    /// Peak surface average since the stint start (°C).
    pub peak_surface_c: Metric<f64>,
    /// Rolling time-window mean of the surface average (°C).
    pub rolling_avg_surface_c: Metric<f64>,
    /// Inner minus middle (°C).
    pub inner_middle_delta_c: Metric<f64>,
    /// Middle minus outer (°C).
    pub middle_outer_delta_c: Metric<f64>,
    /// Rise rate of the surface average (°C/s).
    pub rise_rate_c_per_s: Metric<f64>,
    /// Surface average against the thresholds.
    pub surface_band: Band,
    /// Pressure against the thresholds.
    pub pressure_band: Band,
    /// |inner − outer| against the spread threshold (`High` or `InRange`).
    pub edge_spread_band: Band,
}

/// Tyre engine snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TyreSnapshot {
    /// Per wheel, indexed by [`Wheel::index`].
    pub wheels: [WheelTyreState; WHEELS],
    /// Front-axle minus rear-axle mean surface temperature (°C); needs all four wheels.
    pub front_rear_temp_balance_c: Metric<f64>,
    /// Left-side minus right-side mean surface temperature (°C); needs all four wheels.
    pub left_right_temp_balance_c: Metric<f64>,
    /// Front-axle minus rear-axle mean pressure (Pa); needs all four wheels.
    pub front_rear_pressure_balance_pa: Metric<f64>,
    /// Left-side minus right-side mean pressure (Pa); needs all four wheels.
    pub left_right_pressure_balance_pa: Metric<f64>,
}

/// Deterministic tyre engine. Feed [`TyreSample`]s with [`TyreEngine::update`]; call
/// [`TyreEngine::start_stint`] at each stint start (the stint engine's boundaries).
#[derive(Debug, Clone)]
pub struct TyreEngine {
    config: TyreConfig,
    wheels: [WheelState; WHEELS],
    last_et: Option<f64>,
}

impl TyreEngine {
    /// New engine; rejects an invalid configuration.
    pub fn new(config: TyreConfig) -> Result<Self, TyreConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            wheels: Default::default(),
            last_et: None,
        })
    }

    /// Current configuration.
    pub fn config(&self) -> &TyreConfig {
        &self.config
    }

    /// Starts a new stint: start values are re-captured from the next samples, peaks and windows reset.
    pub fn start_stint(&mut self) {
        debug!(component = COMPONENT, "tyre stint start");
        for w in &mut self.wheels {
            w.start = StartValues::default();
            w.peak_surface_c = None;
            w.rolling.clear();
            w.rise.clear();
        }
    }

    /// Consumes one sample. A non-finite time is ignored; an elapsed-time regression restarts the stint.
    pub fn update(&mut self, sample: &TyreSample) {
        let et = sample.et_s;
        if !et.is_finite() {
            return;
        }
        if self.last_et.is_some_and(|t| et < t) {
            // A new session (or a rewind): the previous stint-start values no longer describe these tyres.
            debug!(
                component = COMPONENT,
                et, "elapsed time regression, stint restarted"
            );
            self.start_stint();
        }
        self.last_et = Some(et);
        let cfg = self.config;
        for wheel in Wheel::ALL {
            let raw = sample.wheels[wheel.index()];
            let st = &mut self.wheels[wheel.index()];
            st.last = raw;
            let avg = surface_avg(&raw);
            if st.start.pressure_pa.is_none() {
                st.start.pressure_pa = finite(raw.pressure_pa);
            }
            if st.start.surface_avg_c.is_none() {
                st.start.surface_avg_c = avg;
            }
            if st.start.carcass_c.is_none() {
                st.start.carcass_c = finite(raw.carcass_c);
            }
            match avg {
                Some(a) => {
                    st.peak_surface_c = Some(st.peak_surface_c.map_or(a, |p| p.max(a)));
                    st.rolling.push(et, a, cfg.rolling_window_s);
                    st.rise.push(et, a, cfg.rise_rate_window_s);
                }
                None => {
                    st.rolling.expire(et, cfg.rolling_window_s);
                    st.rise.expire(et, cfg.rise_rate_window_s);
                }
            }
        }
    }

    /// Current outputs.
    pub fn snapshot(&self) -> TyreSnapshot {
        let t = self.config.thresholds;
        let wheels = Wheel::ALL.map(|wheel| {
            let st = &self.wheels[wheel.index()];
            let raw = st.last;
            let (inner, outer) = wheel.inner_outer(raw.surface_left_c, raw.surface_right_c);
            let inner = finite(inner);
            let outer = finite(outer);
            let middle = finite(raw.surface_center_c);
            let avg = surface_avg(&raw);
            let pressure = finite(raw.pressure_pa);
            let spread = sub(inner, outer).map(f64::abs);
            WheelTyreState {
                wheel,
                pressure_pa: Metric::measured("mWheel[].mPressure", pressure),
                start_pressure_pa: Metric::measured("mWheel[].mPressure", st.start.pressure_pa),
                pressure_delta_pa: Metric::derived(
                    formula::DELTA_VS_STINT_START,
                    sub(pressure, st.start.pressure_pa),
                ),
                inner_c: Metric::derived(formula::EDGE_MAPPING, inner),
                middle_c: Metric::measured("mWheel[].mTemperature[1]", middle),
                outer_c: Metric::derived(formula::EDGE_MAPPING, outer),
                surface_avg_c: Metric::derived(formula::SURFACE_AVG, avg),
                layer_avg_c: Metric::derived(
                    formula::LAYER_AVG,
                    mean_all(&[raw.layer_left_c, raw.layer_center_c, raw.layer_right_c]),
                ),
                carcass_c: Metric::measured(
                    "mWheel[].mTireCarcassTemperature",
                    finite(raw.carcass_c),
                ),
                surface_delta_vs_start_c: Metric::derived(
                    formula::DELTA_VS_STINT_START,
                    sub(avg, st.start.surface_avg_c),
                ),
                carcass_delta_vs_start_c: Metric::derived(
                    formula::DELTA_VS_STINT_START,
                    sub(raw.carcass_c, st.start.carcass_c),
                ),
                peak_surface_c: Metric::derived(formula::PEAK, st.peak_surface_c),
                rolling_avg_surface_c: Metric::estimated(formula::ROLLING_AVG, st.rolling.mean()),
                inner_middle_delta_c: Metric::derived(formula::INNER_MIDDLE, sub(inner, middle)),
                middle_outer_delta_c: Metric::derived(formula::MIDDLE_OUTER, sub(middle, outer)),
                rise_rate_c_per_s: Metric::estimated(
                    formula::RISE_RATE,
                    st.rise.slope(RISE_RATE_MIN_SAMPLES),
                ),
                surface_band: Band::classify(avg, t.surface_low_c, t.surface_high_c),
                pressure_band: Band::classify(pressure, t.pressure_low_pa, t.pressure_high_pa),
                edge_spread_band: Band::classify(spread, None, t.edge_spread_max_c),
            }
        });
        let avg = |w: &WheelTyreState| w.surface_avg_c.value;
        let pr = |w: &WheelTyreState| w.pressure_pa.value;
        TyreSnapshot {
            front_rear_temp_balance_c: Metric::derived(
                formula::FRONT_REAR,
                front_rear(&wheels, avg),
            ),
            left_right_temp_balance_c: Metric::derived(
                formula::LEFT_RIGHT,
                left_right(&wheels, avg),
            ),
            front_rear_pressure_balance_pa: Metric::derived(
                formula::FRONT_REAR,
                front_rear(&wheels, pr),
            ),
            left_right_pressure_balance_pa: Metric::derived(
                formula::LEFT_RIGHT,
                left_right(&wheels, pr),
            ),
            wheels,
        }
    }
}

/// Surface average (inner, middle, outer); all three required.
fn surface_avg(raw: &TyreWheelSample) -> Option<f64> {
    mean_all(&[
        raw.surface_left_c,
        raw.surface_center_c,
        raw.surface_right_c,
    ])
}

fn axle_diff<F: Fn(&WheelTyreState) -> Option<f64>>(
    wheels: &[WheelTyreState; WHEELS],
    pick: F,
    first: fn(Wheel) -> bool,
) -> Option<f64> {
    let a: Vec<Option<f64>> = wheels
        .iter()
        .filter(|w| first(w.wheel))
        .map(&pick)
        .collect();
    let b: Vec<Option<f64>> = wheels
        .iter()
        .filter(|w| !first(w.wheel))
        .map(&pick)
        .collect();
    sub(mean_all(&a), mean_all(&b))
}

fn front_rear<F: Fn(&WheelTyreState) -> Option<f64>>(
    wheels: &[WheelTyreState; WHEELS],
    pick: F,
) -> Option<f64> {
    axle_diff(wheels, pick, Wheel::is_front)
}

fn left_right<F: Fn(&WheelTyreState) -> Option<f64>>(
    wheels: &[WheelTyreState; WHEELS],
    pick: F,
) -> Option<f64> {
    axle_diff(wheels, pick, Wheel::is_left)
}

#[cfg(test)]
mod tests;

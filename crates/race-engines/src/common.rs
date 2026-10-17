//! Shared building blocks: the provenance-carrying [`Metric`], the reference [`CompatibilityKey`], formula
//! ids and small deterministic statistics helpers.

use serde::{Deserialize, Serialize};
use telemetry_model::{ChannelId, Provenance, TelemetryFrame, Value};

use crate::CALCULATION_ENGINE_VERSION;

/// Formula ids of the engine outputs. Where a registry channel exists, the id equals its `formula:<id>`
/// origin, so a value can be traced from the UI to the code that produced it.
pub mod formula {
    /// Distance-aligned live delta: current elapsed minus reference elapsed at the same lap distance.
    pub const LIVE_DELTA: &str = "delta_distance_time_trace";
    /// Predicted lap: reference lap time plus the live delta to it.
    pub const PREDICTED_LAP: &str = "predicted_lap_best_plus_delta";
    /// Current lap elapsed time: sample time minus lap start time.
    pub const CURRENT_LAP_TIME: &str = "current_lap_time_engine";
    /// Completed lap time from consecutive LMU lap start times (or the line crossing interpolation).
    pub const LAP_TIME: &str = "lap_time_from_lap_start_et";
    /// Sector split from sector transitions.
    pub const SECTOR_TIME: &str = "sector_split_from_transitions";
    /// Sum of the best sector of each sector over compatible eligible laps.
    pub const THEORETICAL_BEST: &str = "theoretical_best_sum_of_best_sectors";
    /// Best lap over compatible eligible laps.
    pub const BEST_LAP: &str = "best_lap_compatible_eligible";
    /// Fuel used during one lap: fuel at lap start minus fuel at lap end.
    pub const FUEL_USED_LAST_LAP: &str = "fuel_used_last_lap";
    /// Rolling window average of fuel per lap.
    pub const FUEL_PER_LAP: &str = "fuel_per_lap_window_average";
    /// Fuel used since the current lap started.
    pub const FUEL_USED_THIS_LAP: &str = "fuel_used_since_lap_start";
    /// Current fuel divided by fuel per lap.
    pub const FUEL_LAPS_REMAINING: &str = "fuel_laps_remaining";
    /// Laps of fuel remaining multiplied by the window average lap time.
    pub const FUEL_TIME_REMAINING: &str = "fuel_time_remaining";
    /// Laps to finish multiplied by fuel per lap.
    pub const FUEL_TO_FINISH: &str = "fuel_to_finish";
    /// Fuel to finish plus reserve minus current fuel.
    pub const FUEL_TO_ADD: &str = "fuel_to_add_next_stop";
    /// Projected finish fuel divided by fuel per lap.
    pub const FUEL_MARGIN_LAPS: &str = "fuel_margin_laps";
    /// Current fuel minus fuel to finish.
    pub const FUEL_PROJECTED_FINISH: &str = "fuel_projected_finish";
    /// Current fuel minus fuel per lap times the laps until the planned stop.
    pub const FUEL_AT_PIT_ENTRY: &str = "fuel_at_planned_pit_entry";
    /// Fuel to finish plus the reserve.
    pub const FUEL_TARGET: &str = "fuel_target_to_finish_plus_reserve";
    /// Current fuel minus fuel per lap times the laps until the planned stint end.
    pub const FUEL_AT_STINT_END: &str = "fuel_at_planned_stint_end";
    /// Fuel consumed: per fuel stint, the highest level after the refuel minus the current level, summed.
    pub const FUEL_CONSUMED: &str = "fuel_consumed_sum_of_decreases";
    /// Stint counter, incremented at each pit exit after a stop.
    pub const STINT_NUMBER: &str = "stint_counter_pit_exits";
    /// Completed laps since the stint started.
    pub const STINT_LAPS: &str = "laps_since_pit_exit";
    /// Time since the stint started.
    pub const STINT_TIME: &str = "time_since_pit_exit";
    /// Laps on the current tyre set.
    pub const TYRE_SET_LAPS: &str = "laps_since_tyre_change";
    /// Tyre change: a wear step of any size while the car is stopped in the box or the garage.
    pub const TYRE_CHANGE: &str = "tyre_change_wear_step_while_stopped";
    /// Mean absolute wear change per stint lap of one tyre (direction-agnostic, A-LMU wear direction
    /// unverified).
    pub const TYRE_WEAR_PER_LAP: &str = "tyre_wear_per_lap_abs_change_stint_average";
    /// Fuel used in the stint: per refuel segment, the highest level minus the lowest, summed.
    pub const STINT_FUEL_USED: &str = "stint_fuel_used_sum_of_decreases";
    /// Mean fuel per lap of the stint's included laps.
    pub const STINT_FUEL_PER_LAP: &str = "stint_fuel_per_lap_average";
    /// Mean virtual-energy fraction per lap of the stint.
    pub const STINT_ENERGY_PER_LAP: &str = "stint_ve_per_lap_average";
    /// Stint length limit: the smaller of a full tank over fuel per lap and full energy over energy per lap.
    pub const STINT_MAX_LAPS: &str = "stint_max_laps_min_fuel_ve";
    /// Least-squares slope of lap time over lap index.
    pub const LAP_TIME_TREND: &str = "lap_time_trend_least_squares";
    /// Mean lap time of eligible laps.
    pub const AVERAGE_PACE: &str = "average_lap_time_eligible";
    /// Race laps remaining (lap race: total minus completed; time race: time remaining over pace).
    pub const RACE_LAPS_REMAINING: &str = "race_laps_remaining";
    /// Class position ranked by overall place within the class.
    pub const CLASS_POSITION: &str = "class_place_rank_within_class";
    /// Gap between two cars from their time behind the leader.
    pub const GAP_FROM_LEADER_TIMES: &str = "gap_from_time_behind_leader";
    /// Gap between two cars of a best-lap classification (practice, qualifying, warm-up): the difference
    /// of their LMU best lap times (`back.best − front.best`).
    pub const GAP_BEST_LAP: &str = "gap_best_lap_difference";
    /// Pit stop inferred from speed because no LMU pit signal exists ("CALCULATED", spec §66).
    pub const PIT_SPEED_INFERENCE: &str = "pit_stop_inferred_from_speed";
    /// Pit state from LMU pit flags.
    pub const PIT_STATE_LMU: &str = "pit_state_from_lmu_flags";
    /// Most important flag by priority: red, chequered, full-course yellow, blue, local yellow, green
    /// (`race::FlagState::summary`).
    pub const RACE_FLAG_SUMMARY: &str = "race_flag_summary_priority";
    /// Formation lap: game phase formation or the player's individual phase formation.
    pub const FORMATION: &str = "formation_from_game_or_individual_phase";
    /// Projection of a world position onto the circuit polyline.
    pub const TRACK_PROJECTION: &str = "track_projection_nearest_segment";
}

/// One engine output: the value (or `None` when it cannot be computed — never a fabricated zero), its
/// provenance, the formula that produced it and the engine version (D-010).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Metric<T> {
    /// The value; `None` when inputs are missing or the result would not be meaningful.
    pub value: Option<T>,
    /// Measured (LMU pass-through), Derived (deterministic) or Estimated (statistical assumptions).
    pub provenance: Provenance,
    /// Formula id (see [`formula`]) or LMU origin for pass-through values.
    pub formula: &'static str,
    /// [`CALCULATION_ENGINE_VERSION`] of the engine that produced the value.
    pub engine_version: u32,
}

impl<T> Metric<T> {
    /// A metric with an explicit provenance.
    pub const fn new(provenance: Provenance, formula: &'static str, value: Option<T>) -> Self {
        Self {
            value,
            provenance,
            formula,
            engine_version: CALCULATION_ENGINE_VERSION,
        }
    }

    /// A deterministic calculation from measured inputs.
    pub const fn derived(formula: &'static str, value: Option<T>) -> Self {
        Self::new(Provenance::Derived, formula, value)
    }

    /// A calculation that relies on statistical assumptions (windows, extrapolation, inference).
    pub const fn estimated(formula: &'static str, value: Option<T>) -> Self {
        Self::new(Provenance::Estimated, formula, value)
    }

    /// An LMU value passed through unchanged (`origin` names the LMU field).
    pub const fn measured(origin: &'static str, value: Option<T>) -> Self {
        Self::new(Provenance::Measured, origin, value)
    }

    /// True when a value is present.
    pub const fn is_available(&self) -> bool {
        self.value.is_some()
    }

    /// Transforms the value, keeping provenance and formula.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Metric<U> {
        Metric {
            value: self.value.map(f),
            provenance: self.provenance,
            formula: self.formula,
            engine_version: self.engine_version,
        }
    }
}

/// What must match for two laps to be compared (spec §18: never mix tracks, car classes, vehicle
/// configurations or sessions unless explicitly requested).
///
/// The caller builds it from the session identity: `track_id` should be a layout-unique id such as
/// [`crate::track::TrackId`], `session` a caller-assigned session instance counter (bumped on every session
/// change). Unknown parts are `None`; a key without a track or vehicle is never compatible with anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CompatibilityKey {
    /// Layout-unique track id.
    pub track_id: Option<String>,
    /// Vehicle (configuration) name.
    pub vehicle: Option<String>,
    /// Class name.
    pub class: Option<String>,
    /// Session instance (caller-assigned, unique per session).
    pub session: u64,
}

impl CompatibilityKey {
    /// True when track and vehicle are known (the minimum for any comparison).
    pub fn is_identified(&self) -> bool {
        self.track_id.is_some() && self.vehicle.is_some()
    }

    /// Whether laps recorded under `other` may be compared with laps under `self`. Track, vehicle and class
    /// must be equal and identified; the session must also be equal unless `allow_cross_session` is set
    /// (an explicit engineer request).
    pub fn is_compatible(&self, other: &Self, allow_cross_session: bool) -> bool {
        self.is_identified()
            && self.track_id == other.track_id
            && self.vehicle == other.vehicle
            && self.class == other.class
            && (allow_cross_session || self.session == other.session)
    }
}

/// Descriptive statistics of a sample of values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SampleStats {
    /// Number of values.
    pub n: usize,
    /// Arithmetic mean.
    pub mean: f64,
    /// Sample variance (n − 1 denominator); 0 for a single value.
    pub variance: f64,
    /// Sample standard deviation.
    pub std_dev: f64,
    /// Smallest value.
    pub min: f64,
    /// Largest value.
    pub max: f64,
}

impl SampleStats {
    /// Statistics of `values`; `None` when empty or when any value is not finite.
    pub fn from_values(values: &[f64]) -> Option<Self> {
        if values.is_empty() || values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let n = values.len();
        let n_f = n as f64;
        let mean = values.iter().sum::<f64>() / n_f;
        let variance = if n > 1 {
            values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n_f - 1.0)
        } else {
            0.0
        };
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        Some(Self {
            n,
            mean,
            variance,
            std_dev: variance.sqrt(),
            min,
            max,
        })
    }

    /// Coefficient of variation (std dev / |mean|); `None` when the mean is zero.
    pub fn coefficient_of_variation(&self) -> Option<f64> {
        let cv = self.std_dev / self.mean.abs();
        (self.mean != 0.0 && cv.is_finite()).then_some(cv)
    }
}

/// Least-squares slope dy/dx of `points`; `None` with fewer than two distinct x values or non-finite input.
pub fn least_squares_slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 2 || points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    let n = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    let sxy: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let slope = sxy / sxx;
    (sxx > 0.0 && slope.is_finite()).then_some(slope)
}

/// Linear interpolation of y at `x` between `(x0, y0)` and `(x1, y1)`; returns `y0` when `x0 == x1`.
pub fn lerp(x0: f64, y0: f64, x1: f64, y1: f64, x: f64) -> f64 {
    let dx = x1 - x0;
    if dx == 0.0 {
        y0
    } else {
        y0 + (y1 - y0) * (x - x0) / dx
    }
}

/// Keeps only finite values.
pub fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// Boolean value of `id` when its sample is `Valid` and carries a bool.
pub fn frame_bool(frame: &TelemetryFrame, id: ChannelId) -> Option<bool> {
    let s = frame.get(id);
    match (s.is_valid(), &s.value) {
        (true, Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Integer value of `id` when its sample is `Valid` and carries an integer type.
pub fn frame_int(frame: &TelemetryFrame, id: ChannelId) -> Option<i64> {
    let s = frame.get(id);
    if !s.is_valid() {
        return None;
    }
    match s.value {
        Value::I32(v) => Some(i64::from(v)),
        Value::U32(v) => Some(i64::from(v)),
        Value::U8(v) => Some(i64::from(v)),
        Value::I8(v) => Some(i64::from(v)),
        Value::U64(v) => i64::try_from(v).ok(),
        Value::None | Value::F32(_) | Value::F64(_) | Value::Bool(_) | Value::Str(_) => None,
    }
}

/// Finite numeric value of `id` when its sample is `Valid`.
pub fn frame_f64(frame: &TelemetryFrame, id: ChannelId) -> Option<f64> {
    finite(frame.f64(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_known_sample() {
        let s = SampleStats::from_values(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).expect("value");
        assert_eq!(s.n, 8);
        assert!((s.mean - 5.0).abs() < 1e-12);
        // Sample variance = 32 / 7.
        assert!((s.variance - 32.0 / 7.0).abs() < 1e-12);
        assert_eq!(s.min, 2.0);
        assert_eq!(s.max, 9.0);
    }

    #[test]
    fn stats_reject_empty_and_non_finite() {
        assert!(SampleStats::from_values(&[]).is_none());
        assert!(SampleStats::from_values(&[1.0, f64::NAN]).is_none());
        let one = SampleStats::from_values(&[3.0]).expect("value");
        assert_eq!(one.variance, 0.0);
    }

    #[test]
    fn slope_of_line() {
        let pts: Vec<(f64, f64)> = (0..5).map(|i| (i as f64, 100.0 + 0.3 * i as f64)).collect();
        assert!((least_squares_slope(&pts).expect("value") - 0.3).abs() < 1e-12);
        assert!(least_squares_slope(&[(1.0, 2.0)]).is_none());
        assert!(least_squares_slope(&[(1.0, 2.0), (1.0, 3.0)]).is_none());
    }

    #[test]
    fn compatibility_rules() {
        let a = CompatibilityKey {
            track_id: Some("SPA_7004".into()),
            vehicle: Some("Ferrari 499P".into()),
            class: Some("Hypercar".into()),
            session: 1,
        };
        let mut b = a.clone();
        assert!(a.is_compatible(&b, false));
        b.session = 2;
        assert!(!a.is_compatible(&b, false));
        assert!(a.is_compatible(&b, true));
        b.vehicle = Some("Porsche 963".into());
        assert!(!a.is_compatible(&b, true));
        let unknown = CompatibilityKey::default();
        assert!(!unknown.is_compatible(&unknown, true));
    }

    #[test]
    fn metric_carries_version_and_provenance() {
        let m = Metric::estimated(formula::FUEL_PER_LAP, Some(3.2));
        assert_eq!(m.engine_version, CALCULATION_ENGINE_VERSION);
        assert_eq!(m.provenance, Provenance::Estimated);
        let none: Metric<f64> = Metric::derived(formula::LIVE_DELTA, None);
        assert!(!none.is_available());
    }
}

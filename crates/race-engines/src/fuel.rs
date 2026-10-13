//! Fuel engine (spec §25–28, §67, §289): measured fuel, rolling fuel per lap over selectable windows with
//! explicit exclusion rules, confidence, reserve, fuel to finish, refuel amount and projections, plus a
//! [`CalculationTrace`] for the debug panel.
//!
//! # Lap consumption
//! Fuel used in a lap is `fuel at lap start − fuel at lap end`, taken from the fuel samples at the two lap
//! boundaries (the caller feeds samples with [`FuelEngine::update`] and reports each completed lap with
//! [`FuelEngine::lap_completed`] right after the sample on which the timing engine completed it).
//!
//! # Exclusion rules (spec §26)
//! Always excluded (the consumption value itself is not meaningful): a refuel during the lap, missing fuel
//! data at either boundary, zero or negative consumption, consumption above
//! [`MAX_PLAUSIBLE_FUEL_PER_LAP_L`]. Excluded by default and configurable in [`ExclusionRules`]: in-laps,
//! out-laps, incomplete laps (start not observed or untrusted timing), formation laps, caution laps
//! (full-course yellow / safety car at any time during the lap) and laps invalidated by LMU.
//!
//! # Confidence (spec §27)
//! With `n` included laps and coefficient of variation `cv = std dev / mean`:
//! - LOW: `n < 3` or `cv > 0.10`;
//! - MEDIUM: `n < 5` or `cv > 0.05`;
//! - HIGH: otherwise (at least five consistent laps).
//!
//! # Fuel stints
//! For the "stint" window a fuel stint runs from one refuel to the next (or from the session start). A
//! refuel is a rise of more than [`REFUEL_MIN_INCREASE_L`] above the lowest level since the last refuel;
//! while the level keeps rising after it, it is the same refuel (a gradual refuel spread over many samples
//! is detected once) and fuel-level noise below the threshold never is. Fuel consumed in a fuel stint is
//! its highest level (after the refuel) minus the current level, so noise never accumulates.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use telemetry_model::{TelemetryFrame, ch};
use tracing::debug;

use crate::common::{Metric, SampleStats, finite, formula, frame_f64};
use crate::timing::{LapId, LapRecord};

/// Tracing component name.
const COMPONENT: &str = "race_engines::fuel";

/// A rise above the lowest level since the last refuel larger than this (L) is a refuel.
pub const REFUEL_MIN_INCREASE_L: f64 = 0.5;
/// Largest plausible consumption in one lap (L), matching the registry range of `fuel.used_per_lap_l`.
pub const MAX_PLAUSIBLE_FUEL_PER_LAP_L: f64 = 50.0;
/// Laps kept for windows and the stint table (a 24-hour race is about 400 laps).
pub const MAX_FUEL_LAP_HISTORY: usize = 2_000;
/// Minimum included laps for MEDIUM confidence.
pub const CONFIDENCE_MEDIUM_MIN_LAPS: usize = 3;
/// Minimum included laps for HIGH confidence.
pub const CONFIDENCE_HIGH_MIN_LAPS: usize = 5;
/// Largest coefficient of variation for MEDIUM confidence.
pub const CONFIDENCE_MEDIUM_MAX_CV: f64 = 0.10;
/// Largest coefficient of variation for HIGH confidence.
pub const CONFIDENCE_HIGH_MAX_CV: f64 = 0.05;
/// Upper bound of excluded laps listed in a calculation trace.
pub const MAX_TRACE_EXCLUDED: usize = 20;
/// Percent conversion.
const PERCENT: f64 = 100.0;

/// Laps averaged for fuel per lap (spec §26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FuelWindow {
    /// Last included lap.
    Laps1,
    /// Last two included laps.
    Laps2,
    /// Last three included laps.
    Laps3,
    /// Last five included laps (default: robust and recent).
    #[default]
    Laps5,
    /// Last ten included laps.
    Laps10,
    /// Every included lap of the current fuel stint.
    Stint,
}

impl FuelWindow {
    /// Every window, for selectors.
    pub const ALL: [FuelWindow; 6] = [
        FuelWindow::Laps1,
        FuelWindow::Laps2,
        FuelWindow::Laps3,
        FuelWindow::Laps5,
        FuelWindow::Laps10,
        FuelWindow::Stint,
    ];

    /// Number of laps of a lap-count window; `None` for the stint window.
    pub const fn lap_count(self) -> Option<usize> {
        match self {
            FuelWindow::Laps1 => Some(1),
            FuelWindow::Laps2 => Some(2),
            FuelWindow::Laps3 => Some(3),
            FuelWindow::Laps5 => Some(5),
            FuelWindow::Laps10 => Some(10),
            FuelWindow::Stint => None,
        }
    }

    /// Short label.
    pub const fn label(self) -> &'static str {
        match self {
            FuelWindow::Laps1 => "1 lap",
            FuelWindow::Laps2 => "2 laps",
            FuelWindow::Laps3 => "3 laps",
            FuelWindow::Laps5 => "5 laps",
            FuelWindow::Laps10 => "10 laps",
            FuelWindow::Stint => "stint",
        }
    }
}

/// Configurable exclusions (`true` = excluded from the baseline). Defaults exclude everything that is not a
/// representative green-flag racing lap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExclusionRules {
    /// Laps that entered the pit lane.
    pub pit_in: bool,
    /// Laps that started in the pit lane.
    pub pit_out: bool,
    /// Laps whose start was not observed or whose timing is untrusted.
    pub incomplete: bool,
    /// Formation laps.
    pub formation: bool,
    /// Laps with a full-course yellow / safety car at any time.
    pub caution: bool,
    /// Laps invalidated by LMU (track limits).
    pub invalidated: bool,
}

impl Default for ExclusionRules {
    fn default() -> Self {
        Self {
            pit_in: true,
            pit_out: true,
            incomplete: true,
            formation: true,
            caution: true,
            invalidated: true,
        }
    }
}

/// Why a lap is not in the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExclusionReason {
    /// Fuel was added during the lap (always excluded).
    Refuelled,
    /// No fuel value at one of the lap boundaries (always excluded).
    NoFuelData,
    /// Zero or negative consumption (always excluded).
    NonPositiveConsumption,
    /// Consumption above [`MAX_PLAUSIBLE_FUEL_PER_LAP_L`] (always excluded).
    ImplausibleConsumption,
    /// Start not observed or timing untrusted.
    Incomplete,
    /// Formation lap.
    Formation,
    /// Full-course yellow / safety car during the lap.
    Caution,
    /// In-lap.
    PitIn,
    /// Out-lap.
    PitOut,
    /// Invalidated by LMU.
    Invalidated,
}

/// Confidence of the fuel-per-lap estimate (thresholds in the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    /// Few or inconsistent laps.
    Low,
    /// Some consistent laps.
    Medium,
    /// At least five consistent laps.
    High,
}

impl Confidence {
    /// Confidence for `stats` per the documented thresholds.
    pub fn from_stats(stats: &SampleStats) -> Self {
        let cv = stats.coefficient_of_variation().unwrap_or(f64::INFINITY);
        if stats.n < CONFIDENCE_MEDIUM_MIN_LAPS || cv > CONFIDENCE_MEDIUM_MAX_CV {
            Confidence::Low
        } else if stats.n < CONFIDENCE_HIGH_MIN_LAPS || cv > CONFIDENCE_HIGH_MAX_CV {
            Confidence::Medium
        } else {
            Confidence::High
        }
    }
}

/// Configured reserve (spec §28). The breakdown in every estimate shows all three equivalents.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ReserveMode {
    /// Fixed litres.
    Litres(f64),
    /// Laps of fuel at the current fuel per lap.
    Laps(f64),
    /// Percentage of the fuel needed to finish.
    PercentOfRequired(f64),
}

impl Default for ReserveMode {
    /// No reserve; shown explicitly as 0 L, never hidden.
    fn default() -> Self {
        ReserveMode::Litres(0.0)
    }
}

/// The reserve in litres, laps and percent of the fuel to finish.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ReserveBreakdown {
    /// What was configured.
    pub mode: ReserveMode,
    /// Reserve (L).
    pub litres: Option<f64>,
    /// Reserve in laps at the current fuel per lap.
    pub laps: Option<f64>,
    /// Reserve as a percentage of the fuel to finish.
    pub percent_of_required: Option<f64>,
}

/// How the race ends, for fuel to finish.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RaceFinish {
    /// Lap race: fractional laps the car still has to drive.
    Laps {
        /// Laps remaining from the current position (≥ 0).
        laps_remaining: f64,
    },
    /// Timed race.
    Time {
        /// Session time remaining (s).
        time_remaining_s: f64,
    },
    /// Unknown (no fuel to finish).
    Unknown,
}

impl RaceFinish {
    /// Lap race from the scheduled laps, the completed laps and the progress through the current lap.
    pub fn lap_race(total_laps: u32, completed_laps: u32, lap_progress: f64) -> Self {
        let remaining = f64::from(total_laps) - f64::from(completed_laps) - lap_progress;
        if remaining.is_finite() {
            RaceFinish::Laps {
                laps_remaining: remaining.max(0.0),
            }
        } else {
            RaceFinish::Unknown
        }
    }

    /// Laps the car still drives, given its progress through the current lap and its pace.
    ///
    /// Timed race (ESTIMATED): when the clock runs out the car finishes the lap it is on, so it crosses the
    /// line `floor(progress + time / pace) + 1` laps after the start of its current lap. Conservative for
    /// fuel: it assumes the car itself takes the flag (a car behind the leader may finish one lap earlier)
    /// and an exact multiple of the pace counts one more lap.
    pub fn laps_to_finish(&self, lap_progress: Option<f64>, pace_s: Option<f64>) -> Option<f64> {
        match *self {
            RaceFinish::Laps { laps_remaining } => finite(Some(laps_remaining)).map(|l| l.max(0.0)),
            RaceFinish::Time { time_remaining_s } => {
                let p = finite(lap_progress)?.clamp(0.0, 1.0);
                let pace = finite(pace_s).filter(|v| *v > 0.0)?;
                let t = finite(Some(time_remaining_s))?.max(0.0);
                let laps = (p + t / pace).floor() + 1.0 - p;
                finite(Some(laps))
            }
            RaceFinish::Unknown => None,
        }
    }
}

/// One fuel sample.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FuelSample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Fuel in the tank (L).
    pub fuel_l: Option<f64>,
    /// Tank capacity (L).
    pub capacity_l: Option<f64>,
}

impl FuelSample {
    /// From a canonical frame; `None` when the sample time is unknown.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        Some(Self {
            et_s,
            fuel_l: frame_f64(frame, ch::FUEL),
            capacity_l: frame_f64(frame, ch::FUEL_CAPACITY).filter(|c| *c > 0.0),
        })
    }
}

/// What the fuel engine needs to know about a completed lap.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FuelLapInfo {
    /// Timing lap id.
    pub lap_id: LapId,
    /// LMU lap number.
    pub lap_number: Option<i32>,
    /// Lap time (s), when known.
    pub lap_time_s: Option<f64>,
    /// Start observed and timing trusted.
    pub complete: bool,
    /// In-lap.
    pub pit_in: bool,
    /// Out-lap.
    pub pit_out: bool,
    /// Formation lap.
    pub formation: bool,
    /// Full-course yellow / safety car at any time during the lap.
    pub caution: bool,
    /// Invalidated by LMU.
    pub invalidated: bool,
}

impl FuelLapInfo {
    /// From a timing lap record plus the race conditions seen during the lap.
    pub fn from_record(record: &LapRecord, formation: bool, caution: bool) -> Self {
        Self {
            lap_id: record.id,
            lap_number: record.lap_number,
            lap_time_s: record.time_s,
            complete: record.time_trusted() && !record.flags.joined_mid_lap,
            pit_in: record.flags.pit_in,
            pit_out: record.flags.pit_out,
            formation,
            caution,
            invalidated: record.flags.invalidated,
        }
    }
}

/// One completed lap as seen by the fuel engine.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FuelLap {
    /// Lap facts.
    pub info: FuelLapInfo,
    /// Fuel used (L), when computable.
    pub used_l: Option<f64>,
    /// Why the lap is not in the baseline; `None` = included.
    pub exclusion: Option<ExclusionReason>,
    /// Fuel stint (refuel-to-refuel) index.
    pub fuel_stint: u32,
}

/// One lap line of the calculation trace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TraceLap {
    /// Timing lap id.
    pub lap_id: LapId,
    /// LMU lap number.
    pub lap_number: Option<i32>,
    /// Fuel used (L).
    pub used_l: Option<f64>,
    /// Exclusion reason; `None` = included.
    pub exclusion: Option<ExclusionReason>,
}

/// How fuel per lap was computed (debug panel).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalculationTrace {
    /// Formula id.
    pub formula: &'static str,
    /// Window used.
    pub window: FuelWindow,
    /// Laps averaged, newest first.
    pub included: Vec<TraceLap>,
    /// Laps skipped while filling the window, newest first (bounded by [`MAX_TRACE_EXCLUDED`]).
    pub excluded: Vec<TraceLap>,
    /// Mean of the included values (L/lap).
    pub mean: Option<f64>,
    /// Sample standard deviation (L/lap).
    pub std_dev: Option<f64>,
    /// Sample variance.
    pub variance: Option<f64>,
    /// Result (L/lap) — equal to the mean.
    pub result_l_per_lap: Option<f64>,
    /// Confidence.
    pub confidence: Option<Confidence>,
}

/// Engineer-side planning inputs for one estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FuelContext {
    /// How the race ends.
    pub race_finish: RaceFinish,
    /// Progress through the current lap, 0..1.
    pub lap_progress: Option<f64>,
    /// Laps until the planned pit entry; `None` = at the end of the current lap.
    pub laps_until_pit: Option<f64>,
    /// Laps until the planned end of the stint; `None` = no plan.
    pub laps_until_stint_end: Option<f64>,
}

impl Default for FuelContext {
    fn default() -> Self {
        Self {
            race_finish: RaceFinish::Unknown,
            lap_progress: None,
            laps_until_pit: None,
            laps_until_stint_end: None,
        }
    }
}

/// Fuel engine configuration.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct FuelConfig {
    /// Window for fuel per lap.
    pub window: FuelWindow,
    /// Exclusion rules.
    pub exclusions: ExclusionRules,
    /// Reserve.
    pub reserve: ReserveMode,
}

/// All fuel outputs (spec §25).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FuelEstimate {
    /// Fuel in the tank (L).
    pub current_fuel: Metric<f64>,
    /// Tank capacity (L).
    pub capacity: Metric<f64>,
    /// Fuel consumed since the engine started (refuels excluded).
    pub fuel_used_since_start: Metric<f64>,
    /// Fuel consumed since the last refuel.
    pub fuel_used_this_stint: Metric<f64>,
    /// Fuel used since the current lap started.
    pub fuel_used_this_lap: Metric<f64>,
    /// Fuel used in the last completed lap (even when excluded from the baseline).
    pub fuel_used_last_lap: Metric<f64>,
    /// Window average fuel per lap.
    pub fuel_per_lap: Metric<f64>,
    /// Window used.
    pub window: FuelWindow,
    /// Laps averaged.
    pub sample_count: usize,
    /// Sample variance of the averaged laps.
    pub variance: Option<f64>,
    /// Sample standard deviation of the averaged laps.
    pub std_dev: Option<f64>,
    /// Confidence.
    pub confidence: Option<Confidence>,
    /// Laps of fuel remaining.
    pub laps_remaining: Metric<f64>,
    /// Time of fuel remaining (s).
    pub time_remaining: Metric<f64>,
    /// Mean lap time of the averaged laps (s).
    pub average_lap_time: Metric<f64>,
    /// Laps to the finish.
    pub laps_to_finish: Metric<f64>,
    /// Fuel needed to finish (L).
    pub fuel_to_finish: Metric<f64>,
    /// Reserve in all units (never hidden).
    pub reserve: ReserveBreakdown,
    /// Fuel to finish plus reserve (L).
    pub target_fuel: Metric<f64>,
    /// Fuel to add at the next stop to reach the target (L, ≥ 0).
    pub fuel_to_add: Metric<f64>,
    /// The refuel does not fit in the tank at the planned stop (another stop is needed).
    pub refuel_exceeds_tank: Option<bool>,
    /// Fuel left at the finish without refuelling (L, negative = short).
    pub projected_finish_fuel: Metric<f64>,
    /// Projected finish fuel in laps (+ spare, − short).
    pub margin_laps: Metric<f64>,
    /// Fuel at the planned pit entry (L).
    pub fuel_at_pit_entry: Metric<f64>,
    /// Fuel at the planned stint end (L).
    pub fuel_at_stint_end: Metric<f64>,
    /// How fuel per lap was computed.
    pub trace: CalculationTrace,
}

/// Rolling fuel calculations. Deterministic and clock-free.
#[derive(Debug, Clone)]
pub struct FuelEngine {
    config: FuelConfig,
    current_fuel: Option<f64>,
    capacity: Option<f64>,
    lap_start_fuel: Option<f64>,
    refuelled_this_lap: bool,
    stint_start_fuel: Option<f64>,
    stint_min_fuel: Option<f64>,
    filling: bool,
    consumed_previous_stints: f64,
    fuel_stint: u32,
    laps: VecDeque<FuelLap>,
}

impl FuelEngine {
    /// A new engine.
    pub fn new(config: FuelConfig) -> Self {
        Self {
            config,
            current_fuel: None,
            capacity: None,
            lap_start_fuel: None,
            refuelled_this_lap: false,
            stint_start_fuel: None,
            stint_min_fuel: None,
            filling: false,
            consumed_previous_stints: 0.0,
            fuel_stint: 0,
            laps: VecDeque::new(),
        }
    }

    /// Configuration.
    pub fn config(&self) -> &FuelConfig {
        &self.config
    }

    /// Replaces the configuration (window, exclusions, reserve); history is kept and re-evaluated.
    pub fn set_config(&mut self, config: FuelConfig) {
        self.config = config;
        let rules = config.exclusions;
        for lap in &mut self.laps {
            lap.exclusion = classify(&lap.info, lap.used_l, lap.exclusion, &rules);
        }
    }

    /// Completed laps, oldest first.
    pub fn laps(&self) -> impl Iterator<Item = &FuelLap> {
        self.laps.iter()
    }

    /// Current fuel stint index (incremented at each detected refuel).
    pub fn fuel_stint(&self) -> u32 {
        self.fuel_stint
    }

    /// Feeds one sample. Missing or non-finite fuel values are ignored (never read as 0).
    pub fn update(&mut self, s: &FuelSample) {
        if let Some(c) = finite(s.capacity_l).filter(|c| *c > 0.0) {
            self.capacity = Some(c);
        }
        let Some(fuel) = finite(s.fuel_l).filter(|f| *f >= 0.0) else {
            return;
        };
        match (self.stint_start_fuel, self.stint_min_fuel) {
            (Some(start), Some(min)) => {
                if fuel > min + REFUEL_MIN_INCREASE_L && !(self.filling && fuel >= start) {
                    debug!(
                        component = COMPONENT,
                        et = s.et_s,
                        from = min,
                        to = fuel,
                        "refuel detected"
                    );
                    self.consumed_previous_stints += (start - min).max(0.0);
                    self.fuel_stint = self.fuel_stint.saturating_add(1);
                    self.refuelled_this_lap = true;
                    self.filling = true;
                    self.stint_start_fuel = Some(fuel);
                    self.stint_min_fuel = Some(fuel);
                } else if fuel >= start {
                    // Still filling (or level noise at the stint start): the stint starts at the highest level.
                    self.stint_start_fuel = Some(fuel);
                    self.stint_min_fuel = Some(fuel);
                } else if fuel < min {
                    self.stint_min_fuel = Some(fuel);
                    if fuel < start - REFUEL_MIN_INCREASE_L {
                        // Consumption has resumed: the next rise is a new refuel.
                        self.filling = false;
                    }
                }
            }
            _ => {
                self.stint_start_fuel = Some(fuel);
                self.stint_min_fuel = Some(fuel);
            }
        }
        // The first value ever does not set `lap_start_fuel`: the lap in progress started before fuel was
        // seen, so its consumption stays unknown until the next lap boundary.
        self.current_fuel = Some(fuel);
    }

    /// Reports a completed lap (call right after the sample on which it completed).
    pub fn lap_completed(&mut self, info: FuelLapInfo) -> FuelLap {
        let used = if self.refuelled_this_lap {
            None
        } else {
            self.lap_start_fuel
                .zip(self.current_fuel)
                .map(|(a, b)| a - b)
        };
        let base = if self.refuelled_this_lap {
            Some(ExclusionReason::Refuelled)
        } else {
            None
        };
        let exclusion = classify(&info, used, base, &self.config.exclusions);
        let lap = FuelLap {
            info,
            used_l: used,
            exclusion,
            fuel_stint: self.fuel_stint,
        };
        self.laps.push_back(lap);
        while self.laps.len() > MAX_FUEL_LAP_HISTORY {
            self.laps.pop_front();
        }
        self.lap_start_fuel = self.current_fuel;
        self.refuelled_this_lap = false;
        lap
    }

    /// The timing engine abandoned its lap: the next lap starts now with unknown history.
    pub fn lap_abandoned(&mut self) {
        self.lap_start_fuel = self.current_fuel;
        self.refuelled_this_lap = false;
    }

    /// Fuel per lap over `window` with its trace.
    pub fn window_trace(&self, window: FuelWindow) -> CalculationTrace {
        let mut included = Vec::new();
        let mut excluded = Vec::new();
        let want = window.lap_count();
        for lap in self.laps.iter().rev() {
            if want.is_some_and(|n| included.len() >= n) {
                break;
            }
            if window == FuelWindow::Stint && lap.fuel_stint != self.fuel_stint {
                break;
            }
            let line = TraceLap {
                lap_id: lap.info.lap_id,
                lap_number: lap.info.lap_number,
                used_l: lap.used_l,
                exclusion: lap.exclusion,
            };
            if lap.exclusion.is_none() {
                included.push(line);
            } else if excluded.len() < MAX_TRACE_EXCLUDED {
                excluded.push(line);
            }
        }
        let values: Vec<f64> = included.iter().filter_map(|l| l.used_l).collect();
        let stats = SampleStats::from_values(&values);
        CalculationTrace {
            formula: formula::FUEL_PER_LAP,
            window,
            included,
            excluded,
            mean: stats.map(|s| s.mean),
            std_dev: stats.map(|s| s.std_dev),
            variance: stats.map(|s| s.variance),
            result_l_per_lap: stats.map(|s| s.mean),
            confidence: stats.as_ref().map(Confidence::from_stats),
        }
    }

    /// All fuel outputs for the configured window.
    pub fn estimate(&self, ctx: &FuelContext) -> FuelEstimate {
        let trace = self.window_trace(self.config.window);
        let fpl = trace.result_l_per_lap.filter(|v| *v > 0.0);
        let current = self.current_fuel;
        let lap_times: Vec<f64> = trace
            .included
            .iter()
            .filter_map(|l| {
                self.laps
                    .iter()
                    .rev()
                    .find(|x| x.info.lap_id == l.lap_id)
                    .and_then(|x| x.info.lap_time_s)
            })
            .collect();
        let pace = SampleStats::from_values(&lap_times).map(|s| s.mean);

        let laps_remaining = current.zip(fpl).map(|(c, f)| c / f);
        let time_remaining = laps_remaining.zip(pace).map(|(l, p)| l * p);
        let laps_to_finish = ctx.race_finish.laps_to_finish(ctx.lap_progress, pace);
        let fuel_to_finish = laps_to_finish.zip(fpl).map(|(l, f)| l * f);

        let reserve_l = match self.config.reserve {
            ReserveMode::Litres(l) => finite(Some(l)).map(|l| l.max(0.0)),
            ReserveMode::Laps(n) => finite(Some(n)).zip(fpl).map(|(n, f)| n.max(0.0) * f),
            ReserveMode::PercentOfRequired(p) => finite(Some(p))
                .zip(fuel_to_finish)
                .map(|(p, r)| p.max(0.0) / PERCENT * r),
        };
        let reserve = ReserveBreakdown {
            mode: self.config.reserve,
            litres: reserve_l,
            laps: reserve_l.zip(fpl).map(|(r, f)| r / f),
            percent_of_required: reserve_l
                .zip(fuel_to_finish.filter(|r| *r > 0.0))
                .map(|(r, req)| r / req * PERCENT),
        };
        let target = fuel_to_finish.zip(reserve_l).map(|(f, r)| f + r);
        let to_add = target.zip(current).map(|(t, c)| (t - c).max(0.0));
        let laps_until_pit = ctx
            .laps_until_pit
            .or_else(|| finite(ctx.lap_progress).map(|p| 1.0 - p.clamp(0.0, 1.0)));
        let at_pit = current
            .zip(fpl)
            .zip(finite(laps_until_pit))
            .map(|((c, f), n)| c - f * n.max(0.0));
        let refuel_exceeds_tank = to_add
            .zip(self.capacity)
            .zip(at_pit)
            .map(|((add, cap), at)| add > cap - at.max(0.0));
        let projected = current.zip(fuel_to_finish).map(|(c, f)| c - f);
        let margin = projected.zip(fpl).map(|(p, f)| p / f);
        let at_stint_end = current
            .zip(fpl)
            .zip(finite(ctx.laps_until_stint_end))
            .map(|((c, f), n)| c - f * n.max(0.0));

        let last = self.laps.back();
        let used_this_lap = if self.refuelled_this_lap {
            None
        } else {
            self.lap_start_fuel.zip(current).map(|(a, b)| a - b)
        };
        let stint_used = self
            .stint_start_fuel
            .zip(current)
            .map(|(s, c)| (s - c).max(0.0));

        FuelEstimate {
            current_fuel: Metric::measured("telemetry.mFuel", current),
            capacity: Metric::measured("telemetry.mFuelCapacity", self.capacity),
            fuel_used_since_start: Metric::derived(
                formula::FUEL_CONSUMED,
                stint_used.map(|s| self.consumed_previous_stints + s),
            ),
            fuel_used_this_stint: Metric::derived(formula::FUEL_CONSUMED, stint_used),
            fuel_used_this_lap: Metric::derived(formula::FUEL_USED_THIS_LAP, used_this_lap),
            fuel_used_last_lap: Metric::derived(
                formula::FUEL_USED_LAST_LAP,
                last.and_then(|l| l.used_l),
            ),
            fuel_per_lap: Metric::estimated(formula::FUEL_PER_LAP, fpl),
            window: self.config.window,
            sample_count: trace.included.len(),
            variance: trace.variance,
            std_dev: trace.std_dev,
            confidence: trace.confidence,
            laps_remaining: Metric::estimated(formula::FUEL_LAPS_REMAINING, laps_remaining),
            time_remaining: Metric::estimated(formula::FUEL_TIME_REMAINING, time_remaining),
            average_lap_time: Metric::estimated(formula::AVERAGE_PACE, pace),
            laps_to_finish: Metric::estimated(formula::RACE_LAPS_REMAINING, laps_to_finish),
            fuel_to_finish: Metric::estimated(formula::FUEL_TO_FINISH, fuel_to_finish),
            reserve,
            target_fuel: Metric::estimated(formula::FUEL_TARGET, target),
            fuel_to_add: Metric::estimated(formula::FUEL_TO_ADD, to_add),
            refuel_exceeds_tank,
            projected_finish_fuel: Metric::estimated(formula::FUEL_PROJECTED_FINISH, projected),
            margin_laps: Metric::estimated(formula::FUEL_MARGIN_LAPS, margin),
            fuel_at_pit_entry: Metric::estimated(formula::FUEL_AT_PIT_ENTRY, at_pit),
            fuel_at_stint_end: Metric::estimated(formula::FUEL_AT_STINT_END, at_stint_end),
            trace,
        }
    }
}

/// Applies the exclusion rules in a fixed order: data-invalid reasons first, then the configurable ones.
/// `base` carries a reason decided when the lap completed (a refuel), which no configuration can lift.
fn classify(
    info: &FuelLapInfo,
    used: Option<f64>,
    base: Option<ExclusionReason>,
    rules: &ExclusionRules,
) -> Option<ExclusionReason> {
    if base == Some(ExclusionReason::Refuelled) {
        return base;
    }
    let Some(u) = used.filter(|u| u.is_finite()) else {
        return Some(ExclusionReason::NoFuelData);
    };
    if u <= 0.0 {
        return Some(ExclusionReason::NonPositiveConsumption);
    }
    if u > MAX_PLAUSIBLE_FUEL_PER_LAP_L {
        return Some(ExclusionReason::ImplausibleConsumption);
    }
    let checks = [
        (
            rules.incomplete && !info.complete,
            ExclusionReason::Incomplete,
        ),
        (
            rules.formation && info.formation,
            ExclusionReason::Formation,
        ),
        (rules.caution && info.caution, ExclusionReason::Caution),
        (rules.pit_in && info.pit_in, ExclusionReason::PitIn),
        (rules.pit_out && info.pit_out, ExclusionReason::PitOut),
        (
            rules.invalidated && info.invalidated,
            ExclusionReason::Invalidated,
        ),
    ];
    checks
        .iter()
        .find(|(hit, _)| *hit)
        .map(|(_, reason)| *reason)
}

#[cfg(test)]
mod tests;

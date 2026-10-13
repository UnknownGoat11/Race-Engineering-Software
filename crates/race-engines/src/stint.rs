//! Stint engine (spec §29, §68, §85, §145 "stint state"): stint boundaries, stint time and laps, tyre age
//! and tyre changes, fuel and energy per stint, average pace and the lap-time trend, and the stint table.
//!
//! # Stint boundaries
//! A stint starts when the car reaches the circuit for the first time (session start, or the exit from the
//! garage or pit lane) and at every pit exit after a **stop** in the box or a visit to the garage
//! ([`formula::STINT_NUMBER`]). A drive-through is not a new stint. A stint ends at the pit-lane entry of
//! the visit that ended it, so its time runs from pit exit to pit entry ([`formula::STINT_TIME`]). The pit
//! phases come from the race engine's [`crate::race::PitStateMachine`].
//!
//! # Tyres
//! LMU's tyre wear direction (1 = new or 1 = worn) is UNVERIFIED (LMU_CHANNEL_MAP `tyres.{w}.wear_frac`), so
//! a tyre change is detected direction-agnostically: wear cannot change while the car is stationary, so a
//! wear step larger than [`TYRE_CHANGE_MIN_WEAR_STEP`] between entering and leaving the box (or garage) on
//! a wheel means that wheel was changed ([`formula::TYRE_CHANGE`]). Changing to a set with exactly the same
//! wear cannot be detected. A tyre's age is known only after an observed change; before that its age counts
//! laps since the session start and is flagged as a lower bound (never published as the set age).
//!
//! # Pace
//! Average pace is the mean of the stint's pace-eligible laps (trusted time, not invalidated, not an in- or
//! out-lap, no formation or caution), the trend is the least-squares slope of the last
//! [`TREND_WINDOW_LAPS`] of them (`+0.31 s/lap over the last 5 laps`; no cause is attributed, spec §68).
//!
//! # Time base
//! Stint time accumulates per segment of continuous elapsed time; an elapsed-time regression (LMU rewinds
//! ET after the session end) starts a new segment, so the stint time never goes backwards or negative.
//! Memory is bounded: at most [`MAX_STINTS`] stints and [`TREND_WINDOW_LAPS`] lap times per stint are kept.

use std::collections::VecDeque;

use serde::Serialize;
use tracing::{debug, warn};

use crate::common::{Metric, finite, formula, least_squares_slope};
use crate::fuel::{FuelLap, REFUEL_MIN_INCREASE_L};
use crate::race::{LapConditions, PitPhase};
use crate::timing::LapRecord;

/// Tracing component name.
const COMPONENT: &str = "race_engines::stint";

/// Number of wheels (FL, FR, RL, RR).
pub const WHEELS: usize = 4;
/// Stints kept (oldest dropped first; numbering continues).
pub const MAX_STINTS: usize = 1_000;
/// Pace-eligible laps used for the lap-time trend.
pub const TREND_WINDOW_LAPS: usize = 5;
/// Minimum pace-eligible laps for a trend.
pub const TREND_MIN_LAPS: usize = 3;
/// Smallest wear step (fraction) between entering and leaving the box that counts as a tyre change. Wear
/// does not change while stationary; a one-lap-old tyre differs from a new one by about 0.01.
pub const TYRE_CHANGE_MIN_WEAR_STEP: f64 = 0.002;

/// Why a stint started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StintStart {
    /// First time on the circuit in this session.
    SessionStart,
    /// Pit exit after a stop in the box.
    PitStop,
    /// Exit after a visit to the garage.
    GarageExit,
}

/// Stint boundary event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum StintEvent {
    /// A stint started.
    Started {
        /// Stint number (1-based).
        number: u32,
        /// Why.
        reason: StintStart,
        /// Start time (s).
        et_s: f64,
        /// Wheels whose tyres were changed during the stop that started the stint.
        tyres_changed: [bool; WHEELS],
    },
    /// A stint ended.
    Ended {
        /// Stint number.
        number: u32,
        /// End time (s, the pit-lane entry).
        et_s: f64,
    },
}

/// Sample-rate inputs of the stint engine.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StintSample<'a> {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Pit phase from the race engine.
    pub pit_phase: PitPhase,
    /// Fuel in the tank (L).
    pub fuel_l: Option<f64>,
    /// Tyre wear fraction per wheel (FL, FR, RL, RR); `None` = unavailable.
    pub tyre_wear: [Option<f64>; WHEELS],
    /// Front compound name.
    pub front_compound: Option<&'a str>,
    /// Rear compound name.
    pub rear_compound: Option<&'a str>,
}

/// What the stint engine needs to know about a completed lap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct StintLap {
    /// Lap time (s).
    pub lap_time_s: Option<f64>,
    /// Lap counts towards pace (see the module docs).
    pub pace_eligible: bool,
    /// Fuel used in the lap when the fuel engine included it in its averages.
    pub fuel_used_l: Option<f64>,
    /// Virtual energy fraction used in the lap (energy engine), when available.
    pub energy_used_frac: Option<f64>,
}

impl StintLap {
    /// From the timing record, the fuel engine's lap and the race conditions during the lap.
    pub fn from_laps(
        record: &LapRecord,
        fuel: Option<&FuelLap>,
        conditions: LapConditions,
        energy_used_frac: Option<f64>,
    ) -> Self {
        Self {
            lap_time_s: record.time_s,
            pace_eligible: record.eligible_for_best()
                && !conditions.formation
                && !conditions.caution,
            fuel_used_l: fuel
                .filter(|f| f.exclusion.is_none())
                .and_then(|f| finite(f.used_l)),
            energy_used_frac: finite(energy_used_frac),
        }
    }
}

/// Running mean.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
struct Mean {
    sum: f64,
    n: u32,
}

impl Mean {
    fn add(&mut self, v: f64) {
        self.sum += v;
        self.n = self.n.saturating_add(1);
    }

    fn value(&self) -> Option<f64> {
        (self.n > 0).then(|| self.sum / f64::from(self.n))
    }
}

/// Fuel consumed with refuels removed: per segment between refuels, highest minus lowest level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
struct FuelUse {
    high: Option<f64>,
    low: Option<f64>,
    previous_segments: f64,
}

impl FuelUse {
    fn add(&mut self, fuel: f64) {
        match (self.high, self.low) {
            (Some(high), Some(low)) => {
                if fuel > low + REFUEL_MIN_INCREASE_L {
                    self.previous_segments += (high - low).max(0.0);
                    self.high = Some(fuel);
                    self.low = Some(fuel);
                } else if fuel < low {
                    self.low = Some(fuel);
                }
            }
            _ => {
                self.high = Some(fuel);
                self.low = Some(fuel);
            }
        }
    }

    fn used(&self) -> Option<f64> {
        Some(self.previous_segments + (self.high? - self.low?).max(0.0))
    }
}

/// One stint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StintRecord {
    /// Stint number (1-based).
    pub number: u32,
    /// Why it started.
    pub start_reason: StintStart,
    /// Start time (s, pit exit).
    pub start_et_s: f64,
    /// End time (s, pit entry); `None` while running.
    pub end_et_s: Option<f64>,
    /// Completed laps in the stint.
    pub laps: u32,
    /// Front compound at the start.
    pub front_compound: Option<String>,
    /// Rear compound at the start.
    pub rear_compound: Option<String>,
    /// Tyre age (laps) per wheel at the start; `None` when not known (no change observed yet).
    pub tyre_age_at_start: [Option<u32>; WHEELS],
    /// Wheels changed at the stop that started the stint.
    pub tyres_changed_at_start: [bool; WHEELS],
    /// Fuel at the start (L).
    pub fuel_at_start_l: Option<f64>,
    elapsed_closed_s: f64,
    segment_start_s: f64,
    segment_end_s: f64,
    fuel: FuelUse,
    pace: Mean,
    recent: VecDeque<(f64, f64)>,
    fuel_per_lap: Mean,
    energy_per_lap: Mean,
    wear_per_lap: [Mean; WHEELS],
}

impl StintRecord {
    fn new(number: u32, reason: StintStart, et_s: f64) -> Self {
        Self {
            number,
            start_reason: reason,
            start_et_s: et_s,
            end_et_s: None,
            laps: 0,
            front_compound: None,
            rear_compound: None,
            tyre_age_at_start: [None; WHEELS],
            tyres_changed_at_start: [false; WHEELS],
            fuel_at_start_l: None,
            elapsed_closed_s: 0.0,
            segment_start_s: et_s,
            segment_end_s: et_s,
            fuel: FuelUse::default(),
            pace: Mean::default(),
            recent: VecDeque::with_capacity(TREND_WINDOW_LAPS),
            fuel_per_lap: Mean::default(),
            energy_per_lap: Mean::default(),
            wear_per_lap: [Mean::default(); WHEELS],
        }
    }

    /// Stint time (s): sum of the continuous elapsed-time segments, never negative.
    pub fn elapsed_s(&self) -> f64 {
        self.elapsed_closed_s + (self.segment_end_s - self.segment_start_s).max(0.0)
    }

    /// Stint time as a metric.
    pub fn time(&self) -> Metric<f64> {
        Metric::derived(formula::STINT_TIME, Some(self.elapsed_s()))
    }

    /// Fuel used in the stint (L).
    pub fn fuel_used(&self) -> Metric<f64> {
        Metric::derived(formula::STINT_FUEL_USED, self.fuel.used())
    }

    /// Mean fuel per lap over the stint's included laps (L/lap).
    pub fn fuel_per_lap(&self) -> Metric<f64> {
        Metric::estimated(formula::STINT_FUEL_PER_LAP, self.fuel_per_lap.value())
    }

    /// Mean virtual energy fraction per lap.
    pub fn energy_per_lap(&self) -> Metric<f64> {
        Metric::estimated(formula::STINT_ENERGY_PER_LAP, self.energy_per_lap.value())
    }

    /// Mean lap time of the pace-eligible laps (s).
    pub fn average_pace(&self) -> Metric<f64> {
        Metric::derived(formula::AVERAGE_PACE, self.pace.value())
    }

    /// Least-squares slope of the last [`TREND_WINDOW_LAPS`] pace-eligible lap times (s/lap); `None` with
    /// fewer than [`TREND_MIN_LAPS`].
    pub fn lap_time_trend(&self) -> Metric<f64> {
        let points: Vec<(f64, f64)> = self.recent.iter().copied().collect();
        let slope = (points.len() >= TREND_MIN_LAPS)
            .then(|| least_squares_slope(&points))
            .flatten();
        Metric::estimated(formula::LAP_TIME_TREND, slope)
    }

    /// Laps used for the trend.
    pub fn trend_laps(&self) -> usize {
        self.recent.len()
    }

    /// Mean absolute wear change per lap per wheel (fraction/lap).
    pub fn wear_per_lap(&self) -> [Metric<f64>; WHEELS] {
        self.wear_per_lap
            .map(|m| Metric::estimated(formula::TYRE_WEAR_PER_LAP, m.value()))
    }

    /// Tyre label for the table: the compound, or `front/rear` when they differ.
    pub fn tyre_label(&self) -> Option<String> {
        match (&self.front_compound, &self.rear_compound) {
            (Some(f), Some(r)) if f != r => Some(format!("{f}/{r}")),
            (Some(f), _) => Some(f.clone()),
            (None, Some(r)) => Some(r.clone()),
            (None, None) => None,
        }
    }

    fn advance(&mut self, et_s: f64) {
        if et_s < self.segment_end_s {
            // Time regression: close the segment, start a new one.
            self.elapsed_closed_s += (self.segment_end_s - self.segment_start_s).max(0.0);
            self.segment_start_s = et_s;
        }
        self.segment_end_s = et_s;
    }
}

/// One row of the stint table (spec §85).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StintRow {
    /// Stint number.
    pub number: u32,
    /// Completed laps.
    pub laps: u32,
    /// Stint time (s).
    pub time_s: f64,
    /// Tyre compound label.
    pub tyre: Option<String>,
    /// Average pace (s).
    pub avg_lap_s: Option<f64>,
    /// Fuel per lap (L/lap).
    pub fuel_per_lap_l: Option<f64>,
    /// Fuel used (L).
    pub fuel_used_l: Option<f64>,
    /// The stint is still running.
    pub running: bool,
}

/// Current stint state for display and publication.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StintSnapshot {
    /// Stint number.
    pub number: Metric<u32>,
    /// Laps in the stint.
    pub laps: Metric<u32>,
    /// Time in the stint (s).
    pub time: Metric<f64>,
    /// Stint start (s).
    pub start_et_s: Option<f64>,
    /// Laps on the current tyre set: the oldest tyre's age, only when every tyre's age is known.
    pub tyre_set_laps: Metric<u32>,
    /// Age (laps) per wheel since the session start or the last observed change.
    pub tyre_age_laps: [u32; WHEELS],
    /// Per wheel: the age is a lower bound (no change observed this session).
    pub tyre_age_lower_bound: [bool; WHEELS],
    /// Fuel used in the stint (L).
    pub fuel_used: Metric<f64>,
    /// Fuel per lap in the stint (L/lap).
    pub fuel_per_lap: Metric<f64>,
    /// Energy per lap in the stint (fraction/lap).
    pub energy_per_lap: Metric<f64>,
    /// Average pace (s).
    pub average_pace: Metric<f64>,
    /// Lap-time trend (s/lap).
    pub lap_time_trend: Metric<f64>,
    /// Laps behind the trend.
    pub trend_laps: usize,
    /// Mean absolute wear change per lap per wheel.
    pub wear_per_lap: [Metric<f64>; WHEELS],
    /// Tyre label.
    pub tyre: Option<String>,
}

/// Stint tracking. Deterministic and clock-free.
#[derive(Debug, Clone, Default)]
pub struct StintEngine {
    stints: VecDeque<StintRecord>,
    open: bool,
    next_number: u32,
    phase: Option<PitPhase>,
    last_et_s: Option<f64>,
    lane_entry_et_s: Option<f64>,
    visit_stopped: bool,
    visit_garage: bool,
    wear: [Option<f64>; WHEELS],
    wear_before_stop: [Option<f64>; WHEELS],
    wear_at_lap_start: [Option<f64>; WHEELS],
    changed_this_visit: [bool; WHEELS],
    tyre_age: [u32; WHEELS],
    tyre_age_known: [bool; WHEELS],
    front_compound: Option<String>,
    rear_compound: Option<String>,
    fuel_l: Option<f64>,
}

impl StintEngine {
    /// A new engine (no stint until the car is on the circuit).
    pub fn new() -> Self {
        Self::default()
    }

    /// All kept stints, oldest first.
    pub fn stints(&self) -> impl Iterator<Item = &StintRecord> {
        self.stints.iter()
    }

    /// The running stint.
    pub fn current(&self) -> Option<&StintRecord> {
        self.stints.back().filter(|_| self.open)
    }

    /// Feeds one sample; returns the stint boundaries it caused, in order.
    pub fn update(&mut self, s: &StintSample<'_>) -> Vec<StintEvent> {
        let mut events = Vec::new();
        let et = s.et_s;
        if !et.is_finite() {
            return events;
        }
        if self.last_et_s.is_some_and(|p| et < p) {
            warn!(
                component = COMPONENT,
                et,
                prev_et = ?self.last_et_s,
                "elapsed time regression: stint time continues in a new segment"
            );
        }
        self.last_et_s = Some(et);
        for (w, v) in s.tyre_wear.iter().enumerate() {
            if let Some(v) = finite(*v) {
                self.wear[w] = Some(v);
                if self.wear_at_lap_start[w].is_none() {
                    self.wear_at_lap_start[w] = Some(v);
                }
            }
        }
        // Only allocate when the name changes (samples arrive at ~100 Hz).
        if let Some(c) = s
            .front_compound
            .filter(|c| self.front_compound.as_deref() != Some(*c))
        {
            self.front_compound = Some(c.to_owned());
        }
        if let Some(c) = s
            .rear_compound
            .filter(|c| self.rear_compound.as_deref() != Some(*c))
        {
            self.rear_compound = Some(c.to_owned());
        }
        let fuel = finite(s.fuel_l).filter(|f| *f >= 0.0);
        if fuel.is_some() {
            self.fuel_l = fuel;
        }
        let open = self.open;
        if let Some(cur) = self.stints.back_mut().filter(|_| open) {
            cur.advance(et);
        }

        let prev = self.phase;
        let phase = s.pit_phase;
        self.phase = Some(phase);
        let stopped = |p: PitPhase| matches!(p, PitPhase::Stopped | PitPhase::Garage);
        if prev == Some(PitPhase::OnTrack) && phase != PitPhase::OnTrack {
            self.lane_entry_et_s = Some(et);
            self.visit_stopped = false;
            self.visit_garage = false;
            self.changed_this_visit = [false; WHEELS];
        }
        if stopped(phase) && !prev.is_some_and(stopped) {
            self.wear_before_stop = self.wear;
            self.visit_stopped |= phase == PitPhase::Stopped;
            self.visit_garage |= phase == PitPhase::Garage;
        }
        if phase == PitPhase::Garage {
            self.visit_garage = true;
        }
        if prev.is_some_and(stopped) && !stopped(phase) {
            self.detect_tyre_change(et);
        }
        if phase == PitPhase::OnTrack && prev != Some(PitPhase::OnTrack) {
            let reason = if self.visit_garage {
                StintStart::GarageExit
            } else if self.visit_stopped {
                StintStart::PitStop
            } else {
                StintStart::SessionStart
            };
            let new_stint = !self.open || self.visit_garage || self.visit_stopped;
            if new_stint {
                if self.open {
                    let end = self.lane_entry_et_s.unwrap_or(et);
                    self.close(end, &mut events);
                }
                self.start(reason, et, &mut events);
            }
            self.visit_stopped = false;
            self.visit_garage = false;
            self.lane_entry_et_s = None;
        }
        // After the boundary: the sample at the pit exit belongs to the new stint.
        let open = self.open;
        if let (Some(cur), Some(f)) = (self.stints.back_mut().filter(|_| open), fuel) {
            cur.fuel.add(f);
        }
        events
    }

    /// Reports a completed lap (after the timing, fuel and energy engines processed it).
    pub fn lap_completed(&mut self, lap: &StintLap) {
        for w in 0..WHEELS {
            self.tyre_age[w] = self.tyre_age[w].saturating_add(1);
        }
        let wear_delta: [Option<f64>; WHEELS] = std::array::from_fn(|w| {
            self.wear[w]
                .zip(self.wear_at_lap_start[w])
                .map(|(now, start)| (now - start).abs())
        });
        self.wear_at_lap_start = self.wear;
        let open = self.open;
        let Some(cur) = self.stints.back_mut().filter(|_| open) else {
            return;
        };
        cur.laps = cur.laps.saturating_add(1);
        if let Some(t) = finite(lap.lap_time_s).filter(|t| *t > 0.0 && lap.pace_eligible) {
            cur.pace.add(t);
            if cur.recent.len() == TREND_WINDOW_LAPS {
                cur.recent.pop_front();
            }
            cur.recent.push_back((f64::from(cur.laps), t));
        }
        if let Some(f) = finite(lap.fuel_used_l).filter(|f| *f > 0.0) {
            cur.fuel_per_lap.add(f);
        }
        if let Some(e) = finite(lap.energy_used_frac).filter(|e| *e > 0.0) {
            cur.energy_per_lap.add(e);
        }
        for (w, d) in wear_delta.iter().enumerate() {
            if let Some(d) = d {
                cur.wear_per_lap[w].add(*d);
            }
        }
    }

    /// The timing engine abandoned its lap: wear deltas restart from now.
    pub fn lap_abandoned(&mut self) {
        self.wear_at_lap_start = self.wear;
    }

    /// Current stint state.
    pub fn snapshot(&self) -> StintSnapshot {
        let cur = self.current();
        let all_known = self.tyre_age_known.iter().all(|k| *k);
        let tyre_set_laps = all_known
            .then(|| self.tyre_age.iter().copied().max())
            .flatten();
        StintSnapshot {
            number: Metric::derived(formula::STINT_NUMBER, cur.map(|c| c.number)),
            laps: Metric::derived(formula::STINT_LAPS, cur.map(|c| c.laps)),
            time: cur.map_or_else(
                || Metric::derived(formula::STINT_TIME, None),
                StintRecord::time,
            ),
            start_et_s: cur.map(|c| c.start_et_s),
            tyre_set_laps: Metric::derived(formula::TYRE_SET_LAPS, tyre_set_laps),
            tyre_age_laps: self.tyre_age,
            tyre_age_lower_bound: self.tyre_age_known.map(|k| !k),
            fuel_used: cur.map_or_else(
                || Metric::derived(formula::STINT_FUEL_USED, None),
                StintRecord::fuel_used,
            ),
            fuel_per_lap: cur.map_or_else(
                || Metric::estimated(formula::STINT_FUEL_PER_LAP, None),
                StintRecord::fuel_per_lap,
            ),
            energy_per_lap: cur.map_or_else(
                || Metric::estimated(formula::STINT_ENERGY_PER_LAP, None),
                StintRecord::energy_per_lap,
            ),
            average_pace: cur.map_or_else(
                || Metric::derived(formula::AVERAGE_PACE, None),
                StintRecord::average_pace,
            ),
            lap_time_trend: cur.map_or_else(
                || Metric::estimated(formula::LAP_TIME_TREND, None),
                StintRecord::lap_time_trend,
            ),
            trend_laps: cur.map_or(0, StintRecord::trend_laps),
            wear_per_lap: cur.map_or_else(
                || [Metric::estimated(formula::TYRE_WEAR_PER_LAP, None); WHEELS],
                StintRecord::wear_per_lap,
            ),
            tyre: cur.and_then(StintRecord::tyre_label),
        }
    }

    /// The stint table (spec §85), oldest first.
    pub fn table(&self) -> Vec<StintRow> {
        let last = self.stints.len().saturating_sub(1);
        self.stints
            .iter()
            .enumerate()
            .map(|(i, s)| StintRow {
                number: s.number,
                laps: s.laps,
                time_s: s.elapsed_s(),
                tyre: s.tyre_label(),
                avg_lap_s: s.average_pace().value,
                fuel_per_lap_l: s.fuel_per_lap().value,
                fuel_used_l: s.fuel_used().value,
                running: self.open && i == last,
            })
            .collect()
    }

    fn detect_tyre_change(&mut self, et: f64) {
        for w in 0..WHEELS {
            let changed = self.wear[w]
                .zip(self.wear_before_stop[w])
                .is_some_and(|(after, before)| (after - before).abs() > TYRE_CHANGE_MIN_WEAR_STEP);
            if changed {
                self.tyre_age[w] = 0;
                self.tyre_age_known[w] = true;
                self.changed_this_visit[w] = true;
                self.wear_at_lap_start[w] = self.wear[w];
            }
        }
        if self.changed_this_visit.iter().any(|c| *c) {
            debug!(
                component = COMPONENT,
                et,
                wheels = ?self.changed_this_visit,
                "tyre change detected"
            );
        }
    }

    fn close(&mut self, end_et_s: f64, events: &mut Vec<StintEvent>) {
        if let Some(cur) = self.stints.back_mut() {
            if end_et_s >= cur.segment_start_s {
                cur.segment_end_s = end_et_s;
            }
            cur.end_et_s = Some(end_et_s);
            events.push(StintEvent::Ended {
                number: cur.number,
                et_s: end_et_s,
            });
            debug!(component = COMPONENT, number = cur.number, "stint ended");
        }
        self.open = false;
    }

    fn start(&mut self, reason: StintStart, et: f64, events: &mut Vec<StintEvent>) {
        self.next_number = self.next_number.saturating_add(1);
        let mut rec = StintRecord::new(self.next_number, reason, et);
        rec.front_compound = self.front_compound.clone();
        rec.rear_compound = self.rear_compound.clone();
        rec.tyre_age_at_start =
            std::array::from_fn(|w| self.tyre_age_known[w].then_some(self.tyre_age[w]));
        rec.tyres_changed_at_start = self.changed_this_visit;
        rec.fuel_at_start_l = self.fuel_l;
        if let Some(f) = self.fuel_l {
            rec.fuel.add(f);
        }
        events.push(StintEvent::Started {
            number: rec.number,
            reason,
            et_s: et,
            tyres_changed: self.changed_this_visit,
        });
        debug!(
            component = COMPONENT,
            number = rec.number,
            ?reason,
            "stint started"
        );
        self.changed_this_visit = [false; WHEELS];
        self.stints.push_back(rec);
        while self.stints.len() > MAX_STINTS {
            self.stints.pop_front();
        }
        self.open = true;
    }
}

/// Stint length limit (laps): the smaller of `capacity / fuel per lap` and `1 / energy per lap` over the
/// inputs that are known; `None` when neither is (spec §67 "stint duration").
pub fn stint_max_laps(
    capacity_l: Option<f64>,
    fuel_per_lap_l: Option<f64>,
    energy_per_lap_frac: Option<f64>,
) -> Metric<f64> {
    let positive = |v: Option<f64>| finite(v).filter(|x| *x > 0.0);
    let fuel = positive(capacity_l)
        .zip(positive(fuel_per_lap_l))
        .map(|(c, f)| c / f);
    let energy = positive(energy_per_lap_frac).map(|e| 1.0 / e);
    let value = match (fuel, energy) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    Metric::estimated(formula::STINT_MAX_LAPS, value)
}

#[cfg(test)]
mod tests;

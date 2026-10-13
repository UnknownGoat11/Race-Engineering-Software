//! Fuel and pit checks: fuel decreases while driving and rises only in the pit area (A-LMU-018); the
//! pit-lane bit (`mCurrentSector` bit 31, A-LMU-026), `mInPits` and `mPitState` agree around pit-lane
//! passages and stops (A-LMU-029); `mPitState` uses only the values LMU 1.4.2.0 was seen to use, each in its
//! place. Observed at real pit-box stops (Gate 1): 0 → 1 request (on track) → 2 entering (pit lane) → 4
//! stopped in the box (service) → 5 leaving the box → 0 at the pit exit; 5 also in and out of the garage
//! stall; the header's 3 ("stopped") never appeared and its 4 means "stopped", not "exiting".
//!
//! Stationary episodes are classified as a garage stop (`mInGarageStall` seen), a pit-box stop (pit-lane
//! bit set) or a stop on track; a position jump while stationary marks a teleport (e.g. "return to
//! garage"), so a refill after a teleport is not mistaken for a pit-lane service stop.

use std::collections::{BTreeMap, BTreeSet};

use lmu_reader::split_current_sector;
use telemetry_model::ch;

use super::{CheckResult, Outcome, Verdict, fmt_opt, pct};
use crate::analysis::Step;
use crate::stats::{Agreement, Summary};

/// Driving: above this speed and outside the pit lane (m/s).
const DRIVING_SPEED_MPS: f64 = 10.0;
/// Stationary below this speed (m/s).
const STATIONARY_SPEED_MPS: f64 = 0.5;
/// A stop ends once the car is faster than this (m/s; hysteresis).
const STOP_END_SPEED_MPS: f64 = 2.0;
/// Shortest stop analysed (s of ET).
const MIN_STOP_S: f64 = 2.0;
/// Fuel added during a stop that counts as a refill (L).
const REFUEL_MIN_L: f64 = 1.0;
/// Fuel changes below this are noise (L).
const FUEL_NOISE_L: f64 = 1e-4;
/// Fuel above capacity + this is implausible (L, A-LMU-018 validation).
const FUEL_OVER_CAPACITY_L: f64 = 1.0;
/// Fuel increase while driving accepted as noise (share of the use).
const DRIVING_INCREASE_PASS_SHARE: f64 = 0.01;
/// Fuel increase while driving that fails (share of the use).
const DRIVING_INCREASE_FAIL_SHARE: f64 = 0.05;
/// Fuel use needed to judge "decreasing while driving" (L).
const MIN_DRIVING_USE_L: f64 = 0.5;
/// A wear change during a stop above this counts as a tyre change.
const TYRE_CHANGE_WEAR_STEP: f64 = 0.005;
/// A position step between consecutive samples longer than this is a teleport (m).
pub(super) const TELEPORT_STEP_M: f64 = 50.0;
/// A stop with the pit-lane bit set for at least this share of its samples is a pit-box stop.
const PIT_BOX_SHARE: f64 = 0.95;
/// Scoring updates within this capture time of a flag change are not compared (s): scoring lags
/// telemetry by up to one scoring period.
const FLAG_GUARD_S: f64 = 0.5;
/// Flag agreement for a PASS.
const FLAG_PASS_SHARE: f64 = 0.99;
/// Flag agreement below this fails.
const FLAG_FAIL_SHARE: f64 = 0.9;
/// In-pit scoring updates needed for a flag verdict.
const MIN_PIT_UPDATES: u64 = 10;
/// Highest `mPitState` LMU 1.4.2.0 uses (A-LMU-029, A-LMU-063); the header documents only 0..4.
const PIT_STATE_OBSERVED_MAX: u8 = 5;
/// `mPitState` "entering" (header and observed).
const PIT_STATE_ENTERING: u8 = 2;
/// `mPitState` "stopped in the box" as observed (the header's "exiting").
const PIT_STATE_STOPPED: u8 = 4;
/// Highest pit-lane state (5: leaving the box or the garage, observed).
const PIT_STATE_LANE_MAX: u8 = 5;
/// Pit-lane states (2..=5) seen outside the pit area, beyond the flag guard, above this share fail.
const MISPLACED_FAIL_SHARE: f64 = 0.05;
/// ... and at most this share passes.
const MISPLACED_PASS_SHARE: f64 = 0.01;
/// Misplaced updates needed before a share can fail.
const MIN_MISPLACED_FOR_FAIL: u64 = 3;
/// Stops listed.
const MAX_STOPS: usize = 20;
/// Pit-state transitions and teleports listed.
const MAX_LISTED: usize = 40;

/// Where a stationary episode happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopKind {
    Garage,
    PitBox,
    Track,
}

impl StopKind {
    fn label(self) -> &'static str {
        match self {
            Self::Garage => "garage stall",
            Self::PitBox => "pit box",
            Self::Track => "on track",
        }
    }
}

/// One stationary episode.
#[derive(Debug, Clone, Default, PartialEq)]
struct Stop {
    start_capture_s: f64,
    start_et: f64,
    end_et: f64,
    fuel_start: f64,
    fuel_end: f64,
    samples: u64,
    pit_lane: u64,
    scoring_updates: u64,
    in_pits: u64,
    garage_seen: bool,
    teleport: bool,
    pit_states: BTreeSet<u8>,
    wear_start: [f64; 4],
    wear_end: [f64; 4],
}

impl Stop {
    fn duration_s(&self) -> f64 {
        self.end_et - self.start_et
    }
    fn added_l(&self) -> f64 {
        self.fuel_end - self.fuel_start
    }
    fn share(part: u64, total: u64) -> Option<f64> {
        (total > 0).then(|| part as f64 / total as f64)
    }
    fn pit_lane_share(&self) -> Option<f64> {
        Self::share(self.pit_lane, self.samples)
    }
    fn in_pits_share(&self) -> Option<f64> {
        Self::share(self.in_pits, self.scoring_updates)
    }
    fn kind(&self) -> StopKind {
        if self.garage_seen {
            StopKind::Garage
        } else if self.pit_lane_share().is_some_and(|s| s >= PIT_BOX_SHARE) {
            StopKind::PitBox
        } else {
            StopKind::Track
        }
    }
    fn tyre_change(&self) -> bool {
        self.wear_start
            .iter()
            .zip(&self.wear_end)
            .any(|(a, b)| (b - a).abs() > TYRE_CHANGE_WEAR_STEP)
    }
    fn describe(&self) -> String {
        format!(
            "stop at capture {:.1} s ({}{}): {:.1} s stationary, fuel {:+.2} L, tyre change {}, pit-lane bit {}, mInPits {}, pit states {:?}",
            self.start_capture_s,
            self.kind().label(),
            if self.teleport {
                ", after a teleport (return to the garage)"
            } else {
                ""
            },
            self.duration_s(),
            self.added_l(),
            if self.tyre_change() { "yes" } else { "no" },
            pct(self.pit_lane_share()),
            pct(self.in_pits_share()),
            self.pit_states
        )
    }
}

/// Fuel behaviour and pit flags.
#[derive(Debug, Clone, Default)]
pub struct FuelPitCheck {
    prev_fuel: Option<f64>,
    prev_pos: Option<(f64, f64, f64)>,
    driving_decrease: f64,
    driving_increase: f64,
    driving_increase_steps: u64,
    stationary_outside_increase: f64,
    over_capacity: u64,
    capacity: Summary,
    in_pits: Option<bool>,
    pit_state: Option<u8>,
    garage: bool,
    pit_lane: Option<bool>,
    last_flag_change_s: Option<f64>,
    lane_vs_in_pits: Agreement,
    in_pits_state_non_zero: Agreement,
    pit_state_counts: BTreeMap<(u8, &'static str), u64>,
    /// Pit-lane states (2..=5) outside the pit area, beyond the flag guard.
    lane_states_on_track: u64,
    /// Pit-lane states (2..=5) in total.
    lane_states_total: u64,
    current: Option<Stop>,
    stops: Vec<Stop>,
    stops_not_listed: u64,
    transitions: Vec<String>,
    teleports: Vec<String>,
    teleport_count: u64,
    pit_lane_entries: u64,
    pit_lane_exits: u64,
    last_flags: Option<(bool, Option<bool>, Option<u8>, bool)>,
}

impl FuelPitCheck {
    fn close_stop(&mut self) {
        if let Some(stop) = self.current.take()
            && stop.duration_s() >= MIN_STOP_S
        {
            tracing::debug!(component = crate::COMPONENT, stop = %stop.describe(), "stationary episode");
            if self.stops.len() < MAX_STOPS {
                self.stops.push(stop);
            } else {
                self.stops_not_listed += 1;
            }
        }
    }

    fn scoring(&mut self, step: &Step<'_>) {
        if !step.snapshot.scoring_updated {
            return;
        }
        let Some(row) = step.player_row else {
            return;
        };
        if self.pit_state != Some(row.pit_state) {
            if let Some(prev) = self.pit_state
                && self.transitions.len() < MAX_LISTED
            {
                self.transitions.push(format!(
                    "{prev}→{} at capture {:.1} s (mInPits {}, garage {})",
                    row.pit_state, step.capture_s, row.in_pits, row.in_garage_stall
                ));
            }
            self.pit_state = Some(row.pit_state);
        }
        if self.in_pits != Some(row.in_pits) {
            self.last_flag_change_s = Some(step.capture_s);
            self.in_pits = Some(row.in_pits);
        }
        self.garage = row.in_garage_stall;
        let context = if row.in_garage_stall {
            "garage"
        } else if self.pit_lane == Some(true) || row.in_pits {
            "pit lane"
        } else {
            "track"
        };
        *self
            .pit_state_counts
            .entry((row.pit_state, context))
            .or_insert(0) += 1;
        let guarded = self
            .last_flag_change_s
            .is_some_and(|c| step.capture_s - c < FLAG_GUARD_S);
        if let (Some(lane), false) = (self.pit_lane, guarded)
            && (lane || row.in_pits)
        {
            self.lane_vs_in_pits.push(lane == row.in_pits);
        }
        // In the garage stall LMU reports 0 at a session start and 5 on a return (Gate 1 race): only the pit
        // lane and the box are judged against the pit-lane states.
        if row.in_pits && !guarded && !row.in_garage_stall {
            self.in_pits_state_non_zero.push(row.pit_state != 0);
        }
        if (PIT_STATE_ENTERING..=PIT_STATE_LANE_MAX).contains(&row.pit_state) {
            self.lane_states_total += 1;
            if context == "track" && !guarded {
                self.lane_states_on_track += 1;
            }
        }
        if let Some(stop) = &mut self.current {
            stop.scoring_updates += 1;
            stop.in_pits += u64::from(row.in_pits);
            stop.garage_seen |= row.in_garage_stall;
            stop.pit_states.insert(row.pit_state);
        }
    }

    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.close_stop();
            self.prev_fuel = None;
            self.prev_pos = None;
        }
        self.scoring(step);
        let (Some(frame), Some(t)) = (step.frame, step.raw) else {
            return;
        };
        let Some(speed) = frame.f64(ch::SPEED) else {
            return;
        };
        let (_, pit_lane) = split_current_sector(t.current_sector);
        match (self.pit_lane, pit_lane) {
            (Some(false), true) => self.pit_lane_entries += 1,
            (Some(true), false) => self.pit_lane_exits += 1,
            _ => {}
        }
        if self.pit_lane != Some(pit_lane) {
            self.last_flag_change_s = Some(step.capture_s);
            self.pit_lane = Some(pit_lane);
        }
        let flags = (pit_lane, self.in_pits, self.pit_state, self.garage);
        if self.last_flags != Some(flags) {
            tracing::debug!(
                component = crate::COMPONENT,
                capture_s = step.capture_s,
                et = t.elapsed_time,
                speed,
                pit_lane,
                in_pits = ?self.in_pits,
                pit_state = ?self.pit_state,
                garage = self.garage,
                fuel = t.fuel,
                "pit flags changed"
            );
            self.last_flags = Some(flags);
        }
        let pos = (t.pos.x, t.pos.y, t.pos.z);
        let jump = self.prev_pos.map(|p| {
            ((pos.0 - p.0).powi(2) + (pos.1 - p.1).powi(2) + (pos.2 - p.2).powi(2)).sqrt()
        });
        self.prev_pos = Some(pos);
        let teleport = jump.is_some_and(|d| d > TELEPORT_STEP_M);
        if teleport {
            self.teleport_count += 1;
            if self.teleports.len() < MAX_LISTED {
                self.teleports.push(format!(
                    "{:.0} m at capture {:.1} s (pit-lane bit {pit_lane})",
                    jump.unwrap_or(0.0),
                    step.capture_s
                ));
            }
        }
        let fuel = t.fuel;
        if !fuel.is_finite() {
            return;
        }
        if t.fuel_capacity > 0.0 {
            self.capacity.push(t.fuel_capacity);
            if fuel > t.fuel_capacity + FUEL_OVER_CAPACITY_L {
                self.over_capacity += 1;
            }
        }
        let delta = self.prev_fuel.map(|p| fuel - p);
        self.prev_fuel = Some(fuel);
        let in_pit_area = pit_lane || self.in_pits == Some(true) || self.garage;
        if let Some(d) = delta.filter(|d| d.abs() > FUEL_NOISE_L) {
            if speed > DRIVING_SPEED_MPS && !pit_lane {
                if d < 0.0 {
                    self.driving_decrease -= d;
                } else {
                    self.driving_increase += d;
                    self.driving_increase_steps += 1;
                }
            } else if speed < STATIONARY_SPEED_MPS && d > 0.0 && !in_pit_area {
                self.stationary_outside_increase += d;
            }
        }
        let et = t.elapsed_time;
        let wear: [f64; 4] = [
            t.wheels[0].wear,
            t.wheels[1].wear,
            t.wheels[2].wear,
            t.wheels[3].wear,
        ];
        if speed < STATIONARY_SPEED_MPS {
            let stop = self.current.get_or_insert_with(|| Stop {
                start_capture_s: step.capture_s,
                start_et: et,
                end_et: et,
                fuel_start: fuel,
                fuel_end: fuel,
                wear_start: wear,
                wear_end: wear,
                ..Stop::default()
            });
            stop.end_et = et;
            stop.fuel_end = fuel;
            stop.wear_end = wear;
            stop.samples += 1;
            stop.pit_lane += u64::from(pit_lane);
            stop.garage_seen |= self.garage;
            stop.teleport |= teleport;
        } else if speed > STOP_END_SPEED_MPS {
            self.close_stop();
        }
    }

    /// Fuel, pit flags and pit-state values results.
    pub(super) fn finish(mut self) -> [CheckResult; 3] {
        self.close_stop();
        [
            self.fuel_result(),
            self.pit_flags_result(),
            self.pit_state_result(),
        ]
    }

    fn fuel_result(&self) -> CheckResult {
        let mut o = Outcome::new(
            "fuel",
            "Fuel decreases while driving and rises only in the pit area",
            &["A-LMU-018"],
            &["fuel.level_l", "fuel.capacity_l"],
        );
        let refills: Vec<&Stop> = self
            .stops
            .iter()
            .filter(|s| s.added_l() > REFUEL_MIN_L)
            .collect();
        let outside = refills
            .iter()
            .filter(|s| s.kind() == StopKind::Track)
            .count();
        let increase_share =
            (self.driving_decrease > 0.0).then(|| self.driving_increase / self.driving_decrease);
        o.metric("driving_use_l", self.driving_decrease)
            .metric("driving_increase_l", self.driving_increase)
            .metric("refills", refills.len() as f64)
            .metric("refills_outside_pit_area", outside as f64)
            .metric(
                "stationary_increase_outside_pits_l",
                self.stationary_outside_increase,
            )
            .metric("over_capacity_samples", self.over_capacity as f64)
            .metric_opt("capacity_l", self.capacity.median());
        o.line(format!(
            "while driving (> {DRIVING_SPEED_MPS} m/s, outside the pit lane): used {:.2} L, increases {:.3} L in {} steps ({} of the use)",
            self.driving_decrease,
            self.driving_increase,
            self.driving_increase_steps,
            pct(increase_share)
        ));
        o.line(format!(
            "capacity {} L; samples above capacity + {FUEL_OVER_CAPACITY_L} L: {}; stationary increase outside the pit area: {:.2} L",
            fmt_opt(self.capacity.median(), 1),
            self.over_capacity,
            self.stationary_outside_increase
        ));
        for s in &refills {
            o.line(format!("refill: {}", s.describe()));
        }
        let pit_box_refills = refills
            .iter()
            .filter(|s| s.kind() == StopKind::PitBox)
            .count();
        let decreasing = self.driving_decrease >= MIN_DRIVING_USE_L
            && increase_share.is_some_and(|s| s <= DRIVING_INCREASE_PASS_SHARE);
        let summary = format!(
            "used {:.1} L driving ({} back up); {} refill(s): {} in the pit box, {} in the garage stall, {} on track",
            self.driving_decrease,
            pct(increase_share),
            refills.len(),
            pit_box_refills,
            refills.len() - pit_box_refills - outside,
            outside
        );
        let fail = increase_share.is_some_and(|s| s > DRIVING_INCREASE_FAIL_SHARE)
            || outside > 0
            || self.stationary_outside_increase > REFUEL_MIN_L
            || self.over_capacity > 0;
        if fail {
            o.verdict(Verdict::Fail, summary)
        } else if decreasing && !refills.is_empty() {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }

    fn pit_flags_result(&self) -> CheckResult {
        let mut o = Outcome::new(
            "pit_flags",
            "Pit-lane bit, mInPits and mPitState agree in the pit lane and at stops",
            &["A-LMU-029", "A-LMU-026"],
            &[
                "pit.in_pit_lane",
                "pit.in_pits",
                "pit.state",
                "pit.in_garage_stall",
            ],
        );
        let lane = self.lane_vs_in_pits.fraction();
        let state = self.in_pits_state_non_zero.fraction();
        let pit_box: Vec<&Stop> = self
            .stops
            .iter()
            .filter(|s| s.kind() == StopKind::PitBox)
            .collect();
        let box_with_stopped_state = pit_box
            .iter()
            .filter(|s| s.pit_states.contains(&PIT_STATE_STOPPED))
            .count();
        let serviced_box_without_stopped_state = pit_box
            .iter()
            .filter(|s| {
                (s.added_l() > REFUEL_MIN_L || s.tyre_change())
                    && !s.pit_states.contains(&PIT_STATE_STOPPED)
            })
            .count();
        o.metric("pit_lane_entries", self.pit_lane_entries as f64)
            .metric("pit_lane_exits", self.pit_lane_exits as f64)
            .metric_opt("lane_bit_vs_in_pits_agreement", lane)
            .metric(
                "lane_bit_vs_in_pits_updates",
                self.lane_vs_in_pits.total as f64,
            )
            .metric_opt("in_pits_pit_state_non_zero", state)
            .metric("pit_box_stops", pit_box.len() as f64)
            .metric("pit_box_stops_with_state_4", box_with_stopped_state as f64)
            .metric("teleports", self.teleport_count as f64);
        o.line(format!(
            "pit-lane bit (mCurrentSector bit 31) entries {}, exits {}; at scoring updates with either flag set (skipping {FLAG_GUARD_S} s after a change), bit = mInPits in {} of {}",
            self.pit_lane_entries,
            self.pit_lane_exits,
            pct(lane),
            self.lane_vs_in_pits.total
        ));
        o.line(format!(
            "mPitState ≠ 0 while mInPits outside the garage stall in {} of {} scoring updates",
            pct(state),
            self.in_pits_state_non_zero.total
        ));
        o.line(format!(
            "stationary episodes ≥ {MIN_STOP_S} s: {} ({} beyond the list); pit-box stops (pit-lane bit ≥ {:.0} %, not in the garage stall): {}",
            self.stops.len(),
            self.stops_not_listed,
            PIT_BOX_SHARE * 100.0,
            pit_box.len()
        ));
        for s in &self.stops {
            o.line(s.describe());
        }
        if !self.teleports.is_empty() {
            o.line(format!(
                "position jumps > {TELEPORT_STEP_M} m between samples (teleports): {}",
                self.teleports.join("; ")
            ));
        }
        if !self.transitions.is_empty() {
            o.line(format!(
                "mPitState transitions: {}",
                self.transitions.join("; ")
            ));
        }
        let passages = self.pit_lane_entries + self.pit_lane_exits;
        let service = if pit_box.is_empty() {
            "no pit-box service stop in this capture"
        } else {
            "pit-box stop(s) seen"
        };
        // A-LMU-029's pit rule relies on the entering / stopped / exiting states: without them in the pit
        // lane the rule is not exercised, whatever the flags say.
        let lane_states: BTreeSet<u8> = self
            .pit_state_counts
            .keys()
            .filter(|&&(v, context)| {
                context == "pit lane" && (PIT_STATE_ENTERING..=PIT_STATE_LANE_MAX).contains(&v)
            })
            .map(|&(v, _)| v)
            .collect();
        o.metric("pit_lane_service_states_seen", lane_states.len() as f64);
        o.line(format!(
            "pit-lane states seen in the pit lane (observed: 2 entering, 4 stopped in the box, 5 leaving; header 3 stopped): {lane_states:?}; pit-box stops showing 4 (stopped): {box_with_stopped_state} of {}",
            pit_box.len()
        ));
        let summary = format!(
            "bit = mInPits in {}, pit state ≠ 0 while in the pits in {}, {passages} pit-lane transitions; {service}; pit-lane states {lane_states:?}",
            pct(lane),
            pct(state)
        );
        // Each comparison is judged on its own evidence: mInPits never set in the pit lane leaves no
        // in-pit updates for the pit-state comparison but is itself a clear disagreement.
        // The pit-state rule (A-LMU-029) is only exercised by a pit-box service stop: without one the
        // states are not judged (a garage start or return is not a pit stop).
        let lane_enough = passages > 0 && self.lane_vs_in_pits.total >= MIN_PIT_UPDATES;
        let state_enough = !pit_box.is_empty()
            && passages > 0
            && self.in_pits_state_non_zero.total >= MIN_PIT_UPDATES;
        let lane_fail = lane_enough && lane.is_some_and(|l| l < FLAG_FAIL_SHARE);
        let state_fail = state_enough
            && (state.is_some_and(|s| s < FLAG_FAIL_SHARE)
                || serviced_box_without_stopped_state > 0);
        if lane_fail || state_fail {
            o.verdict(Verdict::Fail, summary)
        } else if lane_enough
            && state_enough
            && lane.is_some_and(|l| l >= FLAG_PASS_SHARE)
            && state.is_some_and(|s| s >= FLAG_PASS_SHARE)
            && box_with_stopped_state == pit_box.len()
            && lane_states.contains(&PIT_STATE_ENTERING)
        {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }

    fn pit_state_result(&self) -> CheckResult {
        let mut o = Outcome::new(
            "pit_state_values",
            "mPitState uses only the observed values 0..5, the pit-lane states only in the pit area",
            &["A-LMU-029", "A-LMU-063"],
            &["pit.state"],
        );
        let mut unknown = 0u64;
        let mut non_zero = 0u64;
        let mut header_stopped = 0u64;
        let lines: Vec<String> = self
            .pit_state_counts
            .iter()
            .map(|(&(value, context), &n)| {
                if value > PIT_STATE_OBSERVED_MAX {
                    unknown += n;
                }
                if value == PIT_STATE_ENTERING + 1 {
                    header_stopped += n;
                }
                if value != 0 {
                    non_zero += n;
                }
                format!("{value} in {context}: {n}")
            })
            .collect();
        let misplaced_share = (self.lane_states_total > 0)
            .then(|| self.lane_states_on_track as f64 / self.lane_states_total as f64);
        o.metric("unknown_updates", unknown as f64)
            .metric("non_zero_updates", non_zero as f64)
            .metric("header_stopped_updates", header_stopped as f64)
            .metric("lane_states_on_track", self.lane_states_on_track as f64)
            .metric("lane_states_total", self.lane_states_total as f64);
        o.line(format!(
            "scoring updates per mPitState value and place: {}",
            if lines.is_empty() {
                "none".to_owned()
            } else {
                lines.join(", ")
            }
        ));
        o.line(
            "observed on LMU 1.4.2.0: 0 none, 1 request, 2 entering, 4 stopped in the box, 5 leaving the box / garage (header: 3 stopped, 4 exiting); the registry range of pit.state is 0..5",
        );
        o.line(format!(
            "pit-lane states (2..5) on track (outside the pit lane, mInPits and the garage, {FLAG_GUARD_S} s after a flag change skipped): {} of {} ({})",
            self.lane_states_on_track,
            self.lane_states_total,
            pct(misplaced_share)
        ));
        let summary = format!(
            "values {:?}; {} unknown; pit-lane states on track {} of {}",
            self.pit_state_counts
                .keys()
                .map(|&(v, _)| v)
                .collect::<BTreeSet<u8>>(),
            unknown,
            self.lane_states_on_track,
            self.lane_states_total
        );
        let misplaced_fail = self.lane_states_on_track >= MIN_MISPLACED_FOR_FAIL
            && misplaced_share.is_some_and(|s| s > MISPLACED_FAIL_SHARE);
        if unknown > 0 || misplaced_fail {
            o.verdict(Verdict::Fail, summary)
        } else if non_zero > 0 && misplaced_share.is_none_or(|s| s <= MISPLACED_PASS_SHARE) {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

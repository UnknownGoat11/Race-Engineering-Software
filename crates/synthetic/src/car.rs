//! Car kinematics along the lap (shared by the player and every competitor): speed tracking of a target
//! profile within the vehicle's acceleration limits, pit-lane phases, line/sector crossings with exact
//! interpolated crossing times, and lap/sector timing.

use crate::profile::SpeedProfile;
use crate::track::{PitLane, Track};
use crate::vehicle::{ROLLING_DECEL_MPS2, VehicleProfile};

/// Deceleration planned for slowing down to the pit-lane limit before the entry line (m/s²).
const PIT_ENTRY_DECEL_MPS2: f64 = 12.0;
/// Deceleration planned for stopping in the pit box (m/s²).
const PIT_BOX_DECEL_MPS2: f64 = 6.0;
/// The box approach aims this far short of the box mark; the last centimetres are braked away (m).
const BOX_STOP_MARGIN_M: f64 = 0.05;
/// Near the box the target speed is the remaining distance over this time constant (s).
const BOX_APPROACH_TAU_S: f64 = 0.5;
/// Speed added to the exponential approach so the car reaches the stop point in finite time (m/s).
const BOX_CREEP_SPEED_MPS: f64 = 0.05;
/// A car that came to rest within this distance of the box mark is stopped in its box (m).
const BOX_STOP_TOLERANCE_M: f64 = 0.5;

/// Where a car is in its pit sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PitPhase {
    /// Racing.
    None,
    /// A stop is due: the car turns into the pit lane at the next entry.
    Planned,
    /// In the pit lane, before the box.
    Entering,
    /// Stationary in the box; service time left (s).
    Stopped { remaining_s: f64 },
    /// From the box to the pit exit.
    Exiting,
}

impl PitPhase {
    /// True while the car is in the pit lane.
    pub fn in_pit_lane(self) -> bool {
        matches!(
            self,
            PitPhase::Entering | PitPhase::Stopped { .. } | PitPhase::Exiting
        )
    }
}

/// What happened to a car during one step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StepOutcome {
    /// Distance driven (m).
    pub distance_m: f64,
    /// The car crossed the start/finish line: crossing time (ET, s).
    pub line_crossed_at: Option<f64>,
    /// A completed lap (`laps_completed` after the crossing), if the crossing completed one.
    pub completed_lap: Option<CompletedLap>,
    /// The car entered the pit lane.
    pub entered_pit_lane: bool,
    /// The car stopped in its box.
    pub stopped_in_box: bool,
    /// The car left the pit lane.
    pub exited_pit_lane: bool,
}

/// Timing of a completed lap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompletedLap {
    /// 1-based number of the completed lap.
    pub lap: u32,
    pub lap_time_s: f64,
    /// Sector 1, 2, 3 times (s); `None` if a sector crossing was not observed (never estimated).
    pub sectors_s: Option<[f64; 3]>,
    pub valid: bool,
}

/// Lap and sector timing of one car, LMU conventions: sector 2 is stored cumulatively (S1 + S2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LapTiming {
    /// ET at which the current lap started; `None` before the car first crossed the line.
    pub lap_start_et_s: Option<f64>,
    pub cur_sector1_s: Option<f64>,
    pub cur_sector12_s: Option<f64>,
    pub last_lap_s: Option<f64>,
    pub last_sector1_s: Option<f64>,
    pub last_sector12_s: Option<f64>,
    pub best_lap_s: Option<f64>,
    /// Best sector 1 / best S1+S2 over all valid laps (may come from different laps).
    pub best_sector1_s: Option<f64>,
    pub best_sector12_s: Option<f64>,
    /// Sectors of the best lap.
    pub best_lap_sector1_s: Option<f64>,
    pub best_lap_sector12_s: Option<f64>,
    /// The current lap still counts (no track-limits violation).
    pub lap_valid: bool,
}

/// One car on track.
#[derive(Debug, Clone, PartialEq)]
pub struct Car {
    pub slot_id: i32,
    pub is_player: bool,
    /// Lap distance, 0..track length (m).
    pub s_m: f64,
    pub v_mps: f64,
    /// Longitudinal acceleration of the last step (m/s²).
    pub a_long_mps2: f64,
    pub laps_completed: u32,
    /// Lap-time multiplier of the current lap (1.0 = the profile).
    pub pace: f64,
    pub pit: PitPhase,
    pub pit_stops: u32,
    pub timing: LapTiming,
    /// Service time of the next stop (s).
    pub next_service_s: f64,
}

/// Speed-limiting conditions applied on top of the racing profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeedContext<'a> {
    pub dry: &'a SpeedProfile,
    pub wet: &'a SpeedProfile,
    /// 0 = dry profile, 1 = wet profile.
    pub wet_blend: f64,
    /// Caution speed cap (m/s).
    pub caution_cap_mps: Option<f64>,
    /// Grip multiplier for the acceleration limits.
    pub grip: f64,
}

impl Car {
    /// A car at lap distance `s_m` with speed `v_mps`; it starts timing when it first crosses the line
    /// (immediately if it starts on it).
    pub fn new(slot_id: i32, is_player: bool, s_m: f64, v_mps: f64, pace: f64) -> Self {
        Self {
            slot_id,
            is_player,
            s_m,
            v_mps,
            a_long_mps2: 0.0,
            laps_completed: 0,
            pace,
            pit: PitPhase::None,
            pit_stops: 0,
            timing: LapTiming {
                lap_valid: true,
                ..LapTiming::default()
            },
            next_service_s: 0.0,
        }
    }

    /// Starts timing at `et` (the car is on the line at the start of the session).
    pub fn start_timing(&mut self, et_s: f64) {
        self.timing.lap_start_et_s = Some(et_s);
    }

    /// Whether the car has started its first timed lap.
    pub fn started(&self) -> bool {
        self.timing.lap_start_et_s.is_some()
    }

    /// Lap number in progress (1-based; 0 before the first crossing).
    pub fn current_lap(&self) -> u32 {
        if self.started() {
            self.laps_completed.saturating_add(1)
        } else {
            0
        }
    }

    /// Race progress: completed laps × length + lap distance (negative before the first crossing).
    pub fn progress_m(&self, track_length_m: f64) -> f64 {
        if self.started() {
            f64::from(self.laps_completed) * track_length_m + self.s_m
        } else {
            self.s_m - track_length_m
        }
    }

    /// Target speed at the current position (m/s).
    pub fn target_speed(&self, ctx: &SpeedContext<'_>, pit: &PitLane) -> f64 {
        let dry = ctx.dry.speed_at(self.s_m);
        let wet = ctx.wet.speed_at(self.s_m);
        let mut target = (dry + (wet - dry) * ctx.wet_blend) / self.pace.max(f64::EPSILON);
        if let Some(cap) = ctx.caution_cap_mps {
            target = target.min(cap);
        }
        let limit = pit.speed_limit_mps;
        match self.pit {
            PitPhase::None => target,
            PitPhase::Planned => {
                let d = pit.distance_to_entry_m(self.s_m);
                target.min((limit * limit + 2.0 * PIT_ENTRY_DECEL_MPS2 * d).sqrt())
            }
            PitPhase::Entering => {
                // Outside the lane (impossible on a validated track) the stop is aborted in `step`.
                let Some(progress_m) = pit.progress_m(self.s_m) else {
                    return target;
                };
                let to_box = pit.box_progress_m() - progress_m;
                let d = (to_box - BOX_STOP_MARGIN_M).max(0.0);
                // Braking curve far out, then a near-exponential approach; at the stop point the target is 0 and
                // the car brakes the last few cm/s away within its deceleration limit.
                let approach = (2.0 * PIT_BOX_DECEL_MPS2 * d)
                    .sqrt()
                    .min(d / BOX_APPROACH_TAU_S + BOX_CREEP_SPEED_MPS);
                limit.min(approach).min(target)
            }
            PitPhase::Stopped { .. } => 0.0,
            PitPhase::Exiting => limit,
        }
    }

    /// Advances the car by `dt` from session time `et_s`.
    pub fn step(
        &mut self,
        track: &Track,
        vehicle: &VehicleProfile,
        ctx: &SpeedContext<'_>,
        et_s: f64,
        dt_s: f64,
    ) -> StepOutcome {
        let mut out = StepOutcome::default();
        if let PitPhase::Stopped { remaining_s } = self.pit {
            self.v_mps = 0.0;
            self.a_long_mps2 = 0.0;
            let left = remaining_s - dt_s;
            self.pit = if left <= 0.0 {
                PitPhase::Exiting
            } else {
                PitPhase::Stopped { remaining_s: left }
            };
            return out;
        }
        let pit = track.pit();
        let target = self.target_speed(ctx, pit);
        let v0 = self.v_mps;
        let v1 = if target >= v0 {
            let accel =
                vehicle.max_drive_accel(v0, ctx.grip) - vehicle.drag_decel(v0) - ROLLING_DECEL_MPS2;
            (v0 + accel * dt_s).min(target).max(0.0)
        } else {
            let decel =
                vehicle.max_brake_decel(v0, ctx.grip) + vehicle.drag_decel(v0) + ROLLING_DECEL_MPS2;
            (v0 - decel * dt_s).max(target).max(0.0)
        };
        let ds = 0.5 * (v0 + v1) * dt_s;
        self.a_long_mps2 = (v1 - v0) / dt_s;
        self.v_mps = v1;
        out.distance_m = ds;

        let s0 = self.s_m;
        let length = track.length_m();
        let was_in_lane = pit.progress_m(s0).is_some();
        let raw = s0 + ds;
        // Sector crossings inside this step (sector boundaries 1 and 2).
        for (k, &boundary) in track.sector_starts_m().iter().enumerate().skip(1) {
            if s0 < boundary && raw >= boundary {
                let t = crossing_time(et_s, dt_s, s0, raw, boundary);
                self.on_sector_boundary(k, t);
            }
        }
        if raw >= length {
            let t = crossing_time(et_s, dt_s, s0, raw, length);
            out.line_crossed_at = Some(t);
            out.completed_lap = self.on_line(t);
            // Sector boundaries passed after the line within the same step belong to the new lap.
            let past_line_m = raw - length;
            for (k, &boundary) in track.sector_starts_m().iter().enumerate().skip(1) {
                if boundary <= past_line_m {
                    let t = crossing_time(et_s, dt_s, s0, raw, length + boundary);
                    self.on_sector_boundary(k, t);
                }
            }
        }
        self.s_m = track.wrap(raw);

        let in_lane = pit.progress_m(self.s_m).is_some();
        match self.pit {
            PitPhase::Planned if in_lane && !was_in_lane => {
                self.pit = PitPhase::Entering;
                out.entered_pit_lane = true;
            }
            PitPhase::Entering => {
                let Some(progress_m) = pit.progress_m(self.s_m) else {
                    tracing::warn!(
                        component = "synthetic",
                        slot_id = self.slot_id,
                        s_m = self.s_m,
                        "car left the pit lane before its box; stop aborted"
                    );
                    self.pit = PitPhase::None;
                    return out;
                };
                let to_box = pit.box_progress_m() - progress_m;
                if self.v_mps == 0.0 && to_box <= BOX_STOP_TOLERANCE_M {
                    self.pit = PitPhase::Stopped {
                        remaining_s: self.next_service_s,
                    };
                    self.pit_stops = self.pit_stops.saturating_add(1);
                    out.stopped_in_box = true;
                }
            }
            PitPhase::Exiting if !in_lane => {
                self.pit = PitPhase::None;
                out.exited_pit_lane = true;
            }
            _ => {}
        }
        out
    }

    fn on_sector_boundary(&mut self, boundary: usize, t_s: f64) {
        let Some(start) = self.timing.lap_start_et_s else {
            return;
        };
        let elapsed = t_s - start;
        if boundary == 1 {
            self.timing.cur_sector1_s = Some(elapsed);
        } else {
            self.timing.cur_sector12_s = Some(elapsed);
        }
    }

    /// Line crossing at `t_s`: completes the lap in progress (if any) and starts the next.
    fn on_line(&mut self, t_s: f64) -> Option<CompletedLap> {
        let completed = match self.timing.lap_start_et_s {
            None => None,
            Some(start) => {
                let lap_time = t_s - start;
                let tm = &mut self.timing;
                // Missing sector times stay missing (published as unavailable), never estimated.
                let (s1, s12) = (tm.cur_sector1_s, tm.cur_sector12_s);
                let sectors_s = match (s1, s12) {
                    (Some(s1), Some(s12)) => Some([s1, s12 - s1, lap_time - s12]),
                    _ => {
                        tracing::warn!(
                            component = "synthetic",
                            slot_id = self.slot_id,
                            "lap completed without both sector crossings; sector times unavailable"
                        );
                        None
                    }
                };
                tm.last_lap_s = Some(lap_time);
                tm.last_sector1_s = s1;
                tm.last_sector12_s = s12;
                if tm.lap_valid {
                    if tm.best_lap_s.is_none_or(|b| lap_time < b) {
                        tm.best_lap_s = Some(lap_time);
                        tm.best_lap_sector1_s = s1;
                        tm.best_lap_sector12_s = s12;
                    }
                    if let Some(s1) = s1
                        && tm.best_sector1_s.is_none_or(|b| s1 < b)
                    {
                        tm.best_sector1_s = Some(s1);
                    }
                    if let Some(s12) = s12
                        && tm.best_sector12_s.is_none_or(|b| s12 < b)
                    {
                        tm.best_sector12_s = Some(s12);
                    }
                }
                self.laps_completed = self.laps_completed.saturating_add(1);
                Some(CompletedLap {
                    lap: self.laps_completed,
                    lap_time_s: lap_time,
                    sectors_s,
                    valid: tm.lap_valid,
                })
            }
        };
        self.timing.lap_start_et_s = Some(t_s);
        self.timing.cur_sector1_s = None;
        self.timing.cur_sector12_s = None;
        self.timing.lap_valid = true;
        completed
    }
}

/// Interpolated time at which a car moving from `s0` to `s1` during [`et`, `et + dt`] passed `mark`.
fn crossing_time(et_s: f64, dt_s: f64, s0: f64, s1: f64, mark: f64) -> f64 {
    let span = s1 - s0;
    if span <= 0.0 {
        return et_s + dt_s;
    }
    et_s + dt_s * ((mark - s0) / span).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackSpec;
    use crate::vehicle::VehicleKind;

    struct Fixture {
        track: Track,
        vehicle: VehicleProfile,
        dry: SpeedProfile,
    }

    fn fixture() -> Fixture {
        let track = Track::build(&TrackSpec::synthetic_circuit()).expect("track");
        let vehicle = VehicleProfile::new(VehicleKind::HybridHypercar);
        let dry = SpeedProfile::compute(&track, &vehicle, 1.0);
        Fixture {
            track,
            vehicle,
            dry,
        }
    }

    fn run(f: &Fixture, car: &mut Car, seconds: f64, dt: f64) -> Vec<StepOutcome> {
        let ctx = SpeedContext {
            dry: &f.dry,
            wet: &f.dry,
            wet_blend: 0.0,
            caution_cap_mps: None,
            grip: 1.0,
        };
        let steps = (seconds / dt).round() as usize;
        (0..steps)
            .map(|i| car.step(&f.track, &f.vehicle, &ctx, i as f64 * dt, dt))
            .collect()
    }

    #[test]
    fn flying_laps_match_the_profile() {
        let f = fixture();
        let mut car = Car::new(1, true, 0.0, f.dry.speed_at(0.0), 1.0);
        car.start_timing(0.0);
        let outcomes = run(&f, &mut car, 3.0 * f.dry.lap_time_s(), 0.01);
        let laps: Vec<CompletedLap> = outcomes.iter().filter_map(|o| o.completed_lap).collect();
        assert!(laps.len() >= 2);
        for lap in &laps {
            assert!((lap.lap_time_s - f.dry.lap_time_s()).abs() < 0.5, "{lap:?}");
            let sectors = lap.sectors_s.expect("sectors");
            let sum: f64 = sectors.iter().sum();
            assert!((sum - lap.lap_time_s).abs() < 1e-9);
            assert!(sectors.iter().all(|s| *s > 10.0));
        }
        assert_eq!(
            car.timing.best_lap_s,
            laps.iter().map(|l| l.lap_time_s).reduce(f64::min)
        );
    }

    #[test]
    fn pit_stop_sequence() {
        let f = fixture();
        let mut car = Car::new(1, true, 0.0, f.dry.speed_at(0.0), 1.0);
        car.start_timing(0.0);
        car.pit = PitPhase::Planned;
        car.next_service_s = 20.0;
        let outcomes = run(&f, &mut car, 2.0 * f.dry.lap_time_s() + 40.0, 0.01);
        let entered = outcomes
            .iter()
            .position(|o| o.entered_pit_lane)
            .expect("entered");
        let stopped = outcomes
            .iter()
            .position(|o| o.stopped_in_box)
            .expect("stopped");
        let exited = outcomes
            .iter()
            .position(|o| o.exited_pit_lane)
            .expect("exited");
        assert!(entered < stopped && stopped < exited);
        // Service time plus two pit-lane transits at the limit.
        let lane_time = (exited - entered) as f64 * 0.01;
        assert!(
            lane_time > 20.0 + 30.0 && lane_time < 20.0 + 60.0,
            "{lane_time}"
        );
        assert_eq!(car.pit_stops, 1);
        assert_eq!(car.pit, PitPhase::None);
    }

    /// Regression (review pass 2, F3): a sector boundary passed after the line within the same step was
    /// never detected, and the lap got fabricated S1 = lap / 3 times.
    #[test]
    fn sector_boundary_just_after_the_line_is_timed_at_low_rates() {
        let mut spec = TrackSpec::synthetic_circuit();
        spec.sector_fractions = [0.001, 0.66];
        let track = Track::build(&spec).expect("track");
        let vehicle = VehicleProfile::new(VehicleKind::HybridHypercar);
        let dry = SpeedProfile::compute(&track, &vehicle, 1.0);
        let f = Fixture {
            track,
            vehicle,
            dry,
        };
        let s1_m = f.track.sector_starts_m()[1];
        let mut car = Car::new(1, true, 0.0, f.dry.speed_at(0.0), 1.0);
        car.start_timing(0.0);
        // 2 Hz: every step covers tens of metres, so the line and S2 start are crossed in one step.
        let outcomes = run(&f, &mut car, 3.0 * f.dry.lap_time_s(), 0.5);
        let laps: Vec<CompletedLap> = outcomes.iter().filter_map(|o| o.completed_lap).collect();
        assert!(laps.len() >= 2);
        let v_line = f.dry.speed_at(0.0);
        for lap in &laps {
            let [s1, s2, s3] = lap.sectors_s.expect("both sector crossings observed");
            // Sector 1 is a few metres long: driven in a fraction of a second, never lap / 3.
            assert!(s1 > 0.0 && s1 < 2.0 * s1_m / v_line, "{lap:?}");
            assert!(s2 > 0.0 && s3 > 0.0);
            assert!((s1 + s2 + s3 - lap.lap_time_s).abs() < 1e-9);
        }
    }

    #[test]
    fn missing_sector_crossings_are_not_estimated() {
        let f = fixture();
        let mut car = Car::new(1, true, 0.0, f.dry.speed_at(0.0), 1.0);
        car.start_timing(0.0);
        let lap = car.on_line(90.0).expect("lap");
        assert_eq!(lap.sectors_s, None);
        assert_eq!(car.timing.last_sector1_s, None);
        assert_eq!(car.timing.best_lap_s, Some(90.0));
        assert_eq!(car.timing.best_lap_sector1_s, None);
        assert_eq!(car.timing.best_sector1_s, None);
    }

    /// Regression (review pass 2, F4): a car entering outside the lane got progress 0 and never reached its box.
    #[test]
    fn entering_outside_the_pit_lane_aborts_the_stop() {
        let f = fixture();
        let s_m = f.track.length_m() / 2.0;
        assert!(f.track.pit().progress_m(s_m).is_none());
        let mut car = Car::new(1, true, s_m, 30.0, 1.0);
        car.start_timing(0.0);
        car.pit = PitPhase::Entering;
        run(&f, &mut car, 0.1, 0.01);
        assert_eq!(car.pit, PitPhase::None);
        assert!(car.v_mps > 30.0, "drives on at racing speed: {}", car.v_mps);
    }

    #[test]
    fn crossing_time_interpolates() {
        assert!((crossing_time(10.0, 0.1, 99.0, 101.0, 100.0) - 10.05).abs() < 1e-12);
    }
}

//! Session-level state: caution flags (full-course yellow / safety car state machine, local yellows) and
//! the [`ScoringState`] snapshot built at the scoring rate from every car.

use telemetry_model::enums::{
    CarFlag, ControlKind, CountLapFlag, FinishStatus, GameMode, GamePhase, IndividualPhase,
    PitState, YellowFlagState,
};
use telemetry_model::scoring::{
    CompetitorEntry, ScoringState, ScoringWeather, SessionScoring, lmu_session_time_remaining,
};

use crate::car::{Car, PitPhase};
use crate::scenario::{CautionKind, CautionSpec, SessionSpec};
use crate::track::Track;
use crate::weather::WeatherState;

/// Raw sector flag values (A-LMU-030, CLAIMED: 1 = yellow, 11 = clear). The synthetic source maps flag
/// index k to sector k (LMU's index order is unverified).
pub const SECTOR_FLAG_CLEAR: i8 = 11;
pub const SECTOR_FLAG_YELLOW: i8 = 1;
/// Speed caps under caution (m/s).
pub const FCY_SPEED_MPS: f64 = 80.0 / 3.6;
pub const SAFETY_CAR_SPEED_MPS: f64 = 120.0 / 3.6;
/// The caution is "pending" (no cap yet) for this long, then the cap ramps in (s).
const CAUTION_PENDING_S: f64 = 10.0;
const CAUTION_RAMP_S: f64 = 10.0;
/// Cap at the start of the ramp: effectively unlimited (m/s).
const CAP_RAMP_START_MPS: f64 = 100.0;
/// Fractions of the caution at which the yellow-flag state advances.
const PITS_CLOSED_UNTIL: f64 = 0.4;
const PIT_LEAD_LAP_UNTIL: f64 = 0.6;
const PITS_OPEN_UNTIL: f64 = 0.9;
/// "Resume" is shown this long after a full-course caution ends (s).
const RESUME_S: f64 = 15.0;
/// A faster car this close behind on track that is more than half a lap ahead in the race shows blue (m).
const BLUE_FLAG_DISTANCE_M: f64 = 150.0;
/// Number of red lights of the (unused) start procedure.
const NUM_RED_LIGHTS: u8 = 5;
/// Seconds per day.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// Flag state at one session time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlagState {
    pub game_phase: GamePhase,
    pub yellow_flag_state: YellowFlagState,
    pub sector_flags_raw: [i8; 3],
    /// A full-course caution (incl. its resume phase) is in progress.
    pub full_course: bool,
    pub caution_cap_mps: Option<f64>,
    pub local_yellow_sector: Option<u8>,
}

/// Flags at session time `et`.
pub fn flags_at(cautions: &[CautionSpec], et_s: f64, session_over: bool) -> FlagState {
    let mut state = FlagState {
        game_phase: GamePhase::GreenFlag,
        yellow_flag_state: YellowFlagState::NoFlag,
        sector_flags_raw: [SECTOR_FLAG_CLEAR; 3],
        full_course: false,
        caution_cap_mps: None,
        local_yellow_sector: None,
    };
    for c in cautions {
        let t = et_s - c.start_et_s;
        match c.kind {
            CautionKind::LocalYellow { sector } => {
                if (0.0..c.duration_s).contains(&t) {
                    state.local_yellow_sector = Some(sector);
                    if let Some(flag) = state.sector_flags_raw.get_mut(usize::from(sector)) {
                        *flag = SECTOR_FLAG_YELLOW;
                    }
                }
            }
            CautionKind::FullCourseYellow | CautionKind::SafetyCar => {
                let cap = if c.kind == CautionKind::SafetyCar {
                    SAFETY_CAR_SPEED_MPS
                } else {
                    FCY_SPEED_MPS
                };
                if (0.0..c.duration_s).contains(&t) {
                    let f = t / c.duration_s;
                    state.yellow_flag_state = if t < CAUTION_PENDING_S {
                        YellowFlagState::Pending
                    } else if f < PITS_CLOSED_UNTIL {
                        YellowFlagState::PitsClosed
                    } else if f < PIT_LEAD_LAP_UNTIL {
                        YellowFlagState::PitLeadLap
                    } else if f < PITS_OPEN_UNTIL {
                        YellowFlagState::PitsOpen
                    } else {
                        YellowFlagState::LastLap
                    };
                    state.full_course = true;
                    state.sector_flags_raw = [SECTOR_FLAG_YELLOW; 3];
                    if t >= CAUTION_PENDING_S {
                        let ramp = ((t - CAUTION_PENDING_S) / CAUTION_RAMP_S).min(1.0);
                        state.caution_cap_mps =
                            Some(cap + (CAP_RAMP_START_MPS - cap) * (1.0 - ramp));
                    }
                } else if (c.duration_s..c.duration_s + RESUME_S).contains(&t) {
                    state.yellow_flag_state = YellowFlagState::Resume;
                    state.full_course = true;
                }
            }
        }
    }
    state.game_phase = if session_over {
        GamePhase::SessionOver
    } else if state.full_course {
        GamePhase::FullCourseYellow
    } else {
        GamePhase::GreenFlag
    };
    state
}

/// Static identity of one car in the scoring table.
#[derive(Debug, Clone, PartialEq)]
pub struct CarIdentity {
    pub driver: String,
    pub vehicle_name: String,
    pub vehicle_filename: String,
    pub class_name: String,
    pub pit_group: String,
    pub qualification: i32,
}

/// Gaps of the player to the neighbouring cars (s).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PlayerGaps {
    pub place_ahead_s: Option<f64>,
    pub place_behind_s: Option<f64>,
    pub car_ahead_s: Option<f64>,
    pub car_behind_s: Option<f64>,
}

/// One scoring update.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoringSnapshot {
    pub state: ScoringState,
    /// Index of the player's row in `state.vehicles`.
    pub player_index: usize,
    pub gaps: PlayerGaps,
    /// Session time of the update (s).
    pub et_s: f64,
}

impl ScoringSnapshot {
    /// The player's row.
    pub fn player(&self) -> Option<&CompetitorEntry> {
        self.state.vehicles.get(self.player_index)
    }
}

/// Inputs of a scoring update.
#[derive(Debug, Clone, Copy)]
pub struct ScoringInputs<'a> {
    pub et_s: f64,
    pub track: &'a Track,
    pub session: &'a SessionSpec,
    pub flags: &'a FlagState,
    pub weather: &'a WeatherState,
    /// Cars; the player is the one with `is_player`.
    pub cars: &'a [Car],
    /// Identities, parallel to `cars`.
    pub identities: &'a [CarIdentity],
    /// Energy (virtual energy or fuel) fractions, parallel to `cars`.
    pub energy_fracs: &'a [Option<f64>],
    /// World positions (x, z), parallel to `cars`.
    pub positions: &'a [(f64, f64)],
    /// Reference lap time used to turn distance gaps into time gaps (s).
    pub reference_lap_s: f64,
}

/// Builds the scoring snapshot.
pub fn build_scoring(inp: &ScoringInputs<'_>) -> ScoringSnapshot {
    let length = inp.track.length_m();
    let ref_speed = length / inp.reference_lap_s.max(f64::EPSILON);
    let progress: Vec<f64> = inp.cars.iter().map(|c| c.progress_m(length)).collect();
    let mut order: Vec<usize> = (0..inp.cars.len()).collect();
    order.sort_by(|&a, &b| {
        progress[b]
            .total_cmp(&progress[a])
            .then(inp.cars[a].slot_id.cmp(&inp.cars[b].slot_id))
    });
    let mut place_of = vec![0_usize; inp.cars.len()];
    for (rank, &i) in order.iter().enumerate() {
        place_of[i] = rank;
    }
    let leader_progress = order.first().map_or(0.0, |&i| progress[i]);
    let session_over = inp.flags.game_phase == GamePhase::SessionOver;

    let mut vehicles = Vec::with_capacity(inp.cars.len());
    for (i, car) in inp.cars.iter().enumerate() {
        let rank = place_of[i];
        let ahead = rank.checked_sub(1).and_then(|r| order.get(r)).copied();
        let behind_leader = leader_progress - progress[i];
        let behind_next = ahead.map_or(0.0, |a| progress[a] - progress[i]);
        let blue = inp.cars.iter().enumerate().any(|(j, other)| {
            j != i
                && progress[j] - progress[i] > length / 2.0
                && (car.s_m - other.s_m).rem_euclid(length) <= BLUE_FLAG_DISTANCE_M
        });
        let sector_index = inp.track.sector_index(car.s_m);
        let under_local = inp.flags.local_yellow_sector == Some(sector_index);
        let under_yellow = inp.flags.full_course || under_local;
        let id = inp.identities.get(i);
        let tm = &car.timing;
        let (x, z) = inp.positions.get(i).copied().unwrap_or((0.0, 0.0));
        vehicles.push(CompetitorEntry {
            slot_id: car.slot_id,
            driver: id.map(|d| d.driver.clone()),
            vehicle_name: id.map(|d| d.vehicle_name.clone()),
            vehicle_filename: id.map(|d| d.vehicle_filename.clone()),
            class_name: id.map(|d| d.class_name.clone()),
            pit_group: id.map(|d| d.pit_group.clone()),
            place: u8::try_from(rank + 1).ok(),
            class_place: None,
            laps: i32::try_from(car.laps_completed).ok(),
            // Negative before the first line crossing, like LMU's mLapDist (A-LMU-070).
            lap_dist_m: Some(if car.started() {
                car.s_m
            } else {
                car.s_m - length
            }),
            sector: Some(sector_index + 1),
            best_lap_s: tm.best_lap_s,
            last_lap_s: tm.last_lap_s,
            best_sector1_s: tm.best_sector1_s,
            best_sector12_s: tm.best_sector12_s,
            last_sector1_s: tm.last_sector1_s,
            last_sector12_s: tm.last_sector12_s,
            cur_sector1_s: tm.cur_sector1_s,
            cur_sector12_s: tm.cur_sector12_s,
            time_behind_next_s: Some(behind_next.max(0.0) / ref_speed),
            laps_behind_next: Some(whole_laps(behind_next, length)),
            time_behind_leader_s: Some(behind_leader.max(0.0) / ref_speed),
            laps_behind_leader: Some(whole_laps(behind_leader, length)),
            time_into_lap_s: tm.lap_start_et_s.map(|s| (inp.et_s - s).max(0.0)),
            estimated_lap_s: Some(tm.best_lap_s.unwrap_or(inp.reference_lap_s * car.pace)),
            lap_start_et_s: tm.lap_start_et_s,
            in_pits: Some(car.pit.in_pit_lane()),
            in_garage_stall: Some(false),
            pit_state: Some(pit_state(car.pit)),
            num_pitstops: i32::try_from(car.pit_stops).ok(),
            num_penalties: Some(0),
            finish_status: Some(FinishStatus::NotFinished),
            flag: Some(if blue { CarFlag::Blue } else { CarFlag::Green }),
            under_yellow: Some(under_yellow),
            individual_phase: Some(if session_over {
                IndividualPhase::SessionOver
            } else if inp.flags.full_course {
                IndividualPhase::FullCourseYellow
            } else if under_local {
                IndividualPhase::UnderYellow
            } else if blue {
                IndividualPhase::UnderBlue
            } else {
                IndividualPhase::GreenFlag
            }),
            count_lap_flag: Some(if tm.lap_valid {
                CountLapFlag::LapAndTime
            } else {
                CountLapFlag::LapOnly
            }),
            control: Some(if car.is_player {
                ControlKind::LocalPlayer
            } else {
                ControlKind::LocalAi
            }),
            is_player: car.is_player,
            qualification: id.map(|d| d.qualification),
            energy_or_fuel_frac: inp.energy_fracs.get(i).copied().flatten(),
            pos_x_m: Some(x),
            pos_z_m: Some(z),
        });
    }

    let player_index = inp.cars.iter().position(|c| c.is_player).unwrap_or(0);
    let gaps = player_gaps(inp, &order, &place_of, &progress, player_index, ref_speed);
    let w = inp.weather;
    let (wet_min, wet_max) = w.wetness_min_max();
    let mut state = ScoringState {
        session: SessionScoring {
            track_name: Some(inp.track.name().to_owned()),
            session_type: Some(inp.session.session_type),
            game_phase: Some(inp.flags.game_phase),
            yellow_flag_state: Some(inp.flags.yellow_flag_state),
            sector_flags_raw: Some(inp.flags.sector_flags_raw),
            start_light: Some(0),
            num_red_lights: Some(NUM_RED_LIGHTS),
            num_vehicles: u32::try_from(inp.cars.len()).ok(),
            current_et_s: Some(inp.et_s),
            end_et_s: Some(inp.session.duration_s),
            time_remaining_s: lmu_session_time_remaining(inp.session.duration_s, inp.et_s),
            max_laps: None,
            track_length_m: Some(length),
            in_realtime: Some(true),
            game_mode: Some(GameMode::Offline),
            max_players: i32::try_from(inp.cars.len()).ok(),
            time_of_day_s: Some(
                (inp.session.start_time_of_day_s + inp.et_s).rem_euclid(SECONDS_PER_DAY),
            ),
            fixed_setup: Some(false),
            weather: ScoringWeather {
                ambient_temp_c: Some(w.air_temp_c),
                track_temp_c: Some(w.track_temp_c),
                raining_frac: Some(w.rain),
                dark_cloud_frac: Some(w.dark_cloud),
                cloud_coverage_raw: Some(w.cloud_coverage_raw()),
                path_wetness_min_frac: Some(wet_min),
                path_wetness_max_frac: Some(wet_max),
                path_wetness_avg_frac: Some(w.wetness),
                wind_mps: Some([w.wind_x_mps, 0.0, w.wind_z_mps]),
                track_grip_level_raw: None,
            },
        },
        vehicles,
    };
    state.assign_class_places();
    ScoringSnapshot {
        state,
        player_index,
        gaps,
        et_s: inp.et_s,
    }
}

fn player_gaps(
    inp: &ScoringInputs<'_>,
    order: &[usize],
    place_of: &[usize],
    progress: &[f64],
    player: usize,
    ref_speed: f64,
) -> PlayerGaps {
    let length = inp.track.length_m();
    let Some(me) = inp.cars.get(player) else {
        return PlayerGaps::default();
    };
    let rank = place_of.get(player).copied().unwrap_or(0);
    let place_ahead_s = rank
        .checked_sub(1)
        .and_then(|r| order.get(r))
        .map(|&a| (progress[a] - progress[player]).max(0.0) / ref_speed);
    let place_behind_s = order
        .get(rank + 1)
        .map(|&b| (progress[player] - progress[b]).max(0.0) / ref_speed);
    let others = inp.cars.iter().enumerate().filter(|(j, _)| *j != player);
    let car_ahead = others
        .clone()
        .map(|(_, o)| (o.s_m - me.s_m).rem_euclid(length))
        .reduce(f64::min);
    let car_behind = others
        .map(|(_, o)| (me.s_m - o.s_m).rem_euclid(length))
        .reduce(f64::min);
    PlayerGaps {
        place_ahead_s,
        place_behind_s,
        car_ahead_s: car_ahead.map(|d| d / ref_speed),
        car_behind_s: car_behind.map(|d| d / ref_speed),
    }
}

/// Whole laps contained in a distance gap.
fn whole_laps(gap_m: f64, length_m: f64) -> i32 {
    // Gaps are bounded by the session distance (far below i32::MAX laps).
    (gap_m.max(0.0) / length_m).floor() as i32
}

/// LMU pit state of a pit phase.
pub fn pit_state(phase: PitPhase) -> PitState {
    match phase {
        PitPhase::None => PitState::Idle,
        PitPhase::Planned => PitState::Requested,
        PitPhase::Entering => PitState::Entering,
        PitPhase::Stopped { .. } => PitState::Stopped,
        PitPhase::Exiting => PitState::Exiting,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_course_caution_runs_its_state_machine() {
        let cautions = [CautionSpec {
            kind: CautionKind::FullCourseYellow,
            start_et_s: 100.0,
            duration_s: 100.0,
        }];
        let at = |t: f64| flags_at(&cautions, t, false);
        assert_eq!(at(50.0).game_phase, GamePhase::GreenFlag);
        assert_eq!(at(50.0).caution_cap_mps, None);
        assert_eq!(at(105.0).yellow_flag_state, YellowFlagState::Pending);
        assert_eq!(at(105.0).caution_cap_mps, None);
        assert_eq!(at(130.0).yellow_flag_state, YellowFlagState::PitsClosed);
        assert_eq!(at(150.0).yellow_flag_state, YellowFlagState::PitLeadLap);
        assert_eq!(at(170.0).yellow_flag_state, YellowFlagState::PitsOpen);
        assert_eq!(at(195.0).yellow_flag_state, YellowFlagState::LastLap);
        assert_eq!(at(195.0).caution_cap_mps, Some(FCY_SPEED_MPS));
        assert_eq!(at(195.0).game_phase, GamePhase::FullCourseYellow);
        assert_eq!(at(195.0).sector_flags_raw, [SECTOR_FLAG_YELLOW; 3]);
        assert_eq!(at(205.0).yellow_flag_state, YellowFlagState::Resume);
        assert_eq!(at(205.0).caution_cap_mps, None);
        assert_eq!(at(230.0).game_phase, GamePhase::GreenFlag);
        assert_eq!(
            flags_at(&cautions, 230.0, true).game_phase,
            GamePhase::SessionOver
        );
        let cap = at(115.0).caution_cap_mps.expect("ramping");
        assert!(cap > FCY_SPEED_MPS && cap < CAP_RAMP_START_MPS);
    }

    #[test]
    fn local_yellow_marks_one_sector() {
        let cautions = [CautionSpec {
            kind: CautionKind::LocalYellow { sector: 1 },
            start_et_s: 0.0,
            duration_s: 10.0,
        }];
        let f = flags_at(&cautions, 5.0, false);
        assert_eq!(
            f.sector_flags_raw,
            [SECTOR_FLAG_CLEAR, SECTOR_FLAG_YELLOW, SECTOR_FLAG_CLEAR]
        );
        assert_eq!(f.local_yellow_sector, Some(1));
        assert!(!f.full_course);
        assert_eq!(f.game_phase, GamePhase::GreenFlag);
    }
}

//! Scoring block → typed [`ScoringState`] (session + standings) and the player's scoring row.
//!
//! Conversions follow docs/LMU_CHANNEL_MAP.md §1, §3, §14–§17: times `≤ 0` are "no time" (`None`), the
//! `mMaxLaps` INT32_MAX sentinel is `None`, `mLapStartET < 0` and `mQualification < 1` are `None`, scoring
//! `mSector` (0 = S3) is decoded to a 1-based sector, non-finite floats are `None`. Strings are bounded
//! lossy UTF-8 from the reader; an empty string is `None`.

use lmu_reader::{RawScoringInfo, RawString, RawVehicleScoring};
use telemetry_model::enums::{
    CarFlag, ControlKind, CountLapFlag, FinishStatus, GameMode, GamePhase, IndividualPhase,
    PitState, SessionType, YellowFlagState,
};
use telemetry_model::scoring::{
    CompetitorEntry, ScoringState, ScoringWeather, SessionScoring, lmu_max_laps, lmu_sector,
    lmu_time,
};
use telemetry_model::units::convert;

use crate::detect::TimeOfDayDetector;

/// Number of rows LMU_Data can hold (`vehScoringInfo[104]`).
pub const MAX_SCORING_ROWS: usize = 104;

/// `Some(v)` if finite.
pub(crate) fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// `Some(v)` if finite and ≥ 0.
pub(crate) fn non_negative(v: f64) -> Option<f64> {
    (v.is_finite() && v >= 0.0).then_some(v)
}

/// Non-empty decoded text.
pub(crate) fn text(s: &RawString) -> Option<String> {
    (!s.is_empty()).then(|| s.text.clone())
}

/// Lap-start ET: `< 0` (−1 before the first lap) is the "none" sentinel (A-LMU-043).
pub(crate) fn lap_start_et(raw: f64) -> Option<f64> {
    non_negative(raw)
}

/// Qualification position: 1-based; −1 (and anything below 1) means none.
pub(crate) fn qualification(raw: i32) -> Option<i32> {
    (raw >= 1).then_some(raw)
}

/// Session-wide scoring (`scoringInfo`). `time_of_day` decides the `mTimeOfDay` scale (A-LMU-036).
pub fn session_scoring(
    info: &RawScoringInfo,
    time_of_day: &mut TimeOfDayDetector,
) -> SessionScoring {
    let wind = [info.wind.x, info.wind.y, info.wind.z];
    SessionScoring {
        track_name: text(&info.track_name),
        session_type: Some(SessionType::from_lmu(info.session)),
        game_phase: Some(GamePhase::from_raw(info.game_phase)),
        yellow_flag_state: Some(YellowFlagState::from_raw(info.yellow_flag_state)),
        sector_flags_raw: Some(info.sector_flag),
        start_light: Some(info.start_light),
        num_red_lights: Some(info.num_red_lights),
        num_vehicles: u32::try_from(info.num_vehicles).ok(),
        current_et_s: non_negative(info.current_et),
        end_et_s: non_negative(info.end_et),
        time_remaining_s: non_negative(f64::from(info.session_time_remaining)),
        max_laps: lmu_max_laps(info.max_laps),
        track_length_m: non_negative(info.lap_dist).filter(|l| *l > 0.0),
        in_realtime: Some(info.in_realtime),
        game_mode: Some(GameMode::from_raw(info.game_mode)),
        max_players: (info.max_players >= 0).then_some(info.max_players),
        time_of_day_s: time_of_day.read(f64::from(info.time_of_day)),
        fixed_setup: Some(info.is_fixed_setup),
        weather: ScoringWeather {
            ambient_temp_c: finite(info.ambient_temp),
            track_temp_c: finite(info.track_temp),
            raining_frac: finite(info.raining),
            dark_cloud_frac: finite(info.dark_cloud),
            cloud_coverage_raw: Some(info.cloud_coverage),
            path_wetness_min_frac: finite(info.min_path_wetness),
            path_wetness_max_frac: finite(info.max_path_wetness),
            path_wetness_avg_frac: finite(info.avg_path_wetness),
            wind_mps: wind.iter().all(|v| v.is_finite()).then_some(wind),
            track_grip_level_raw: Some(info.track_grip_level),
        },
    }
}

/// One competitor row (`vehScoringInfo[i]`).
pub fn competitor(row: &RawVehicleScoring) -> CompetitorEntry {
    let pos_ok = row.pos.x.is_finite() && row.pos.z.is_finite();
    CompetitorEntry {
        slot_id: row.id,
        driver: text(&row.driver_name),
        vehicle_name: text(&row.vehicle_name),
        vehicle_filename: text(&row.veh_filename),
        class_name: text(&row.vehicle_class),
        pit_group: text(&row.pit_group),
        place: (row.place >= 1).then_some(row.place),
        class_place: None,
        laps: (row.total_laps >= 0).then_some(i32::from(row.total_laps)),
        lap_dist_m: finite(row.lap_dist),
        sector: lmu_sector(row.sector),
        best_lap_s: lmu_time(row.best_lap_time),
        last_lap_s: lmu_time(row.last_lap_time),
        best_sector1_s: lmu_time(row.best_sector1),
        best_sector12_s: lmu_time(row.best_sector2),
        last_sector1_s: lmu_time(row.last_sector1),
        last_sector12_s: lmu_time(row.last_sector2),
        cur_sector1_s: lmu_time(row.cur_sector1),
        cur_sector12_s: lmu_time(row.cur_sector2),
        time_behind_next_s: non_negative(row.time_behind_next),
        laps_behind_next: (row.laps_behind_next >= 0).then_some(row.laps_behind_next),
        time_behind_leader_s: non_negative(row.time_behind_leader),
        laps_behind_leader: (row.laps_behind_leader >= 0).then_some(row.laps_behind_leader),
        time_into_lap_s: non_negative(row.time_into_lap),
        estimated_lap_s: lmu_time(row.estimated_lap_time),
        lap_start_et_s: lap_start_et(row.lap_start_et),
        in_pits: Some(row.in_pits),
        in_garage_stall: Some(row.in_garage_stall),
        pit_state: Some(PitState::from_raw(row.pit_state)),
        num_pitstops: (row.num_pitstops >= 0).then_some(i32::from(row.num_pitstops)),
        num_penalties: (row.num_penalties >= 0).then_some(i32::from(row.num_penalties)),
        finish_status: Some(FinishStatus::from_raw(row.finish_status)),
        flag: Some(CarFlag::from_raw(row.flag)),
        under_yellow: Some(row.under_yellow),
        individual_phase: Some(IndividualPhase::from_raw(row.individual_phase)),
        count_lap_flag: Some(CountLapFlag::from_raw(row.count_lap_flag)),
        control: Some(ControlKind::from_raw(row.control)),
        is_player: row.is_player,
        qualification: qualification(row.qualification),
        energy_or_fuel_frac: Some(convert::u8_to_fraction(row.fuel_fraction)),
        pos_x_m: pos_ok.then_some(row.pos.x),
        pos_z_m: pos_ok.then_some(row.pos.z),
    }
}

/// Full scoring state with class places derived (CALCULATED: rank by `mPlace` within the same class
/// string). Rows beyond [`MAX_SCORING_ROWS`] are ignored (the reader already clamps).
pub fn scoring_state(
    info: &RawScoringInfo,
    rows: &[RawVehicleScoring],
    time_of_day: &mut TimeOfDayDetector,
) -> ScoringState {
    let mut state = ScoringState {
        session: session_scoring(info, time_of_day),
        vehicles: rows.iter().take(MAX_SCORING_ROWS).map(competitor).collect(),
    };
    state.assign_class_places();
    state
}

/// Why the player's scoring row could not be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerRowIssue {
    /// No row is flagged `mIsPlayer`.
    NoPlayerRow,
    /// Several rows are flagged `mIsPlayer`.
    SeveralPlayerRows,
    /// The single `mIsPlayer` row's `mID` differs from the player telemetry `mID`.
    IdMismatch,
}

/// The player's scoring row: the single `mIsPlayer` row whose `mID` equals the player telemetry `mID`
/// (docs/LMU_CHANNEL_MAP.md §0.3 index `k`). Anything else makes every player scoring channel UNAVAILABLE.
pub fn player_row(
    rows: &[RawVehicleScoring],
    telemetry_id: i32,
) -> Result<&RawVehicleScoring, PlayerRowIssue> {
    let mut players = rows.iter().take(MAX_SCORING_ROWS).filter(|r| r.is_player);
    let first = players.next().ok_or(PlayerRowIssue::NoPlayerRow)?;
    if players.next().is_some() {
        return Err(PlayerRowIssue::SeveralPlayerRows);
    }
    if first.id == telemetry_id {
        Ok(first)
    } else {
        Err(PlayerRowIssue::IdMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{race_scoring_info, row};

    #[test]
    fn player_row_is_joined_by_id() {
        let rows = vec![row(3, 1, "Hyper", false), row(7, 2, "GT3", true)];
        assert_eq!(player_row(&rows, 7).map(|r| r.id), Ok(7));
        assert_eq!(player_row(&rows, 3).err(), Some(PlayerRowIssue::IdMismatch));
        let none = vec![row(3, 1, "Hyper", false)];
        assert_eq!(
            player_row(&none, 3).err(),
            Some(PlayerRowIssue::NoPlayerRow)
        );
        let two = vec![row(3, 1, "Hyper", true), row(7, 2, "GT3", true)];
        assert_eq!(
            player_row(&two, 7).err(),
            Some(PlayerRowIssue::SeveralPlayerRows)
        );
    }

    #[test]
    fn class_positions_are_derived_from_class_strings() {
        let rows = vec![
            row(1, 1, "Hyper", false),
            row(2, 2, "GT3", true),
            row(3, 3, "Hyper", false),
            row(4, 4, "GT3", false),
            row(5, 5, "", false),
        ];
        let mut tod = TimeOfDayDetector::default();
        let s = scoring_state(&race_scoring_info(10.0), &rows, &mut tod);
        let class_place = |id| s.by_slot(id).and_then(|r| r.class_place);
        assert_eq!(
            (
                class_place(1),
                class_place(2),
                class_place(3),
                class_place(4)
            ),
            (Some(1), Some(1), Some(2), Some(2))
        );
        assert_eq!(class_place(5), None, "no class string, no class place");
        assert_eq!(s.player().map(|p| p.slot_id), Some(2));
    }

    #[test]
    fn sentinels_become_none() {
        let mut r = row(1, 1, "GT3", true);
        r.best_lap_time = -1.0;
        r.last_lap_time = 0.0;
        r.lap_start_et = -1.0;
        r.qualification = -1;
        r.sector = 0;
        r.lap_dist = f64::NAN;
        let c = competitor(&r);
        assert_eq!(
            (c.best_lap_s, c.last_lap_s, c.lap_start_et_s),
            (None, None, None)
        );
        assert_eq!(c.qualification, None);
        assert_eq!(c.sector, Some(3), "mSector 0 = sector 3");
        assert_eq!(c.lap_dist_m, None, "NaN is never published");
        let mut tod = TimeOfDayDetector::default();
        let s = session_scoring(&race_scoring_info(5.0), &mut tod);
        assert_eq!(s.max_laps, None, "INT32_MAX = timed race");
        assert_eq!(s.track_length_m, Some(13_600.0));
        assert_eq!(s.time_of_day_s, None, "0 cannot decide hours vs seconds");
    }
}

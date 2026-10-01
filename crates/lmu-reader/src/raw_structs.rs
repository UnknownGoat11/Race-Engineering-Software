//! Typed raw LMU structs and their bounds-checked decoders.
//!
//! **Generated** by `crates/lmu-reader/codegen/gen_layout.py` from `docs/research/lmu_abi_layout.json`;
//! do not edit by hand. Every field keeps raw C semantics: no unit conversion, no clamping, NaN and
//! infinities pass through unchanged (the normalizer validates and converts). Field docs name the C
//! field; unit hints paraphrase docs/LMU_CHANNEL_MAP.md and docs/ASSUMPTIONS.md.

use serde::Serialize;

use crate::bytes::{DecodeError, FieldReader, RawString, ReservedBytes, Vec3};
use crate::offsets::{
    WHEEL_COUNT, app_state, generic, paths, scoring_info, telem_info, telem_wheel, veh_scoring,
};

/// Decoded `SharedMemoryGeneric` (`data.generic`, absolute offset 0). Raw C semantics, no unit conversion.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawGeneric {
    /// `SharedMemoryGeneric.events`: Queue of `SharedMemoryEvent` codes, not flags (A-LMU-009).
    pub events: [u32; 16],
    /// `SharedMemoryGeneric.gameVersion`: Game version number (recorded, A-LMU-010).
    pub game_version: i32,
    /// `SharedMemoryGeneric.FFBTorque`: Force-feedback output; scale unverified.
    pub ffb_torque: f32,
    /// `SharedMemoryGeneric.appInfo`.
    pub app_info: RawAppState,
}

/// Decodes one `SharedMemoryGeneric` from exactly `generic::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_generic(buf: &[u8]) -> Result<RawGeneric, DecodeError> {
    let r = FieldReader::new(buf, generic::SIZE, "SharedMemoryGeneric")?;
    Ok(RawGeneric {
        events: r.u32_array::<16>(generic::EVENTS)?,
        game_version: r.i32(generic::GAME_VERSION)?,
        ffb_torque: r.f32(generic::FFB_TORQUE)?,
        app_info: decode_app_state(r.sub(generic::APP_INFO)?)?,
    })
}

/// Decoded `ApplicationStateV01` (`data.generic.appInfo`). The window handle is never decoded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawAppState {
    /// `ApplicationStateV01.mWidth`.
    pub width: u32,
    /// `ApplicationStateV01.mHeight`.
    pub height: u32,
    /// `ApplicationStateV01.mRefreshRate`.
    pub refresh_rate: u32,
    /// `ApplicationStateV01.mWindowed`.
    pub windowed: u32,
    /// `ApplicationStateV01.mOptionsLocation`: 0 main UI, 1 track loading, 2 monitor, 3 on track.
    pub options_location: u8,
    /// `ApplicationStateV01.mOptionsPage`.
    pub options_page: RawString,
    /// `ApplicationStateV01.mExpansion` (204 reserved bytes, diagnostics only).
    pub expansion: ReservedBytes,
}

/// Decodes one `ApplicationStateV01` from exactly `app_state::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_app_state(buf: &[u8]) -> Result<RawAppState, DecodeError> {
    let r = FieldReader::new(buf, app_state::SIZE, "ApplicationStateV01")?;
    Ok(RawAppState {
        width: r.u32(app_state::WIDTH)?,
        height: r.u32(app_state::HEIGHT)?,
        refresh_rate: r.u32(app_state::REFRESH_RATE)?,
        windowed: r.u32(app_state::WINDOWED)?,
        options_location: r.u8(app_state::OPTIONS_LOCATION)?,
        options_page: r.string(app_state::OPTIONS_PAGE)?,
        expansion: r.reserved(app_state::EXPANSION)?,
    })
}

/// Decoded `SharedMemoryPathData` (`data.paths`). **PII**: contains Windows user paths; never transmit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawPaths {
    /// `SharedMemoryPathData.userData`.
    pub user_data: RawString,
    /// `SharedMemoryPathData.customVariables`.
    pub custom_variables: RawString,
    /// `SharedMemoryPathData.stewardResults`.
    pub steward_results: RawString,
    /// `SharedMemoryPathData.playerProfile`.
    pub player_profile: RawString,
    /// `SharedMemoryPathData.pluginsFolder`.
    pub plugins_folder: RawString,
}

/// Decodes one `SharedMemoryPathData` from exactly `paths::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_paths(buf: &[u8]) -> Result<RawPaths, DecodeError> {
    let r = FieldReader::new(buf, paths::SIZE, "SharedMemoryPathData")?;
    Ok(RawPaths {
        user_data: r.string(paths::USER_DATA)?,
        custom_variables: r.string(paths::CUSTOM_VARIABLES)?,
        steward_results: r.string(paths::STEWARD_RESULTS)?,
        player_profile: r.string(paths::PLAYER_PROFILE)?,
        plugins_folder: r.string(paths::PLUGINS_FOLDER)?,
    })
}

/// Decoded `ScoringInfoV01` (`data.scoring.scoringInfo`). Pointer fields are never decoded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawScoringInfo {
    /// `ScoringInfoV01.mTrackName`.
    pub track_name: RawString,
    /// `ScoringInfoV01.mSession`: 0 test day, 1-4 practice, 5-8 qualifying, 9 warm-up, 10-13 race.
    pub session: i32,
    /// `ScoringInfoV01.mCurrentET`: Current session time, s. Scoring freshness key (A-LMU-031).
    pub current_et: f64,
    /// `ScoringInfoV01.mEndET`: Session end time, s.
    pub end_et: f64,
    /// `ScoringInfoV01.mMaxLaps`: Maximum laps; INT32_MAX in timed races (A-LMU-043).
    pub max_laps: i32,
    /// `ScoringInfoV01.mLapDist`: Track length, m.
    pub lap_dist: f64,
    /// `ScoringInfoV01.mNumVehicles`: Number of valid `vehScoringInfo` rows (clamped to 104 when copied).
    pub num_vehicles: i32,
    /// `ScoringInfoV01.mGamePhase`: 0 before session .. 5 green, 6 FCY/safety car, 7 stopped, 8 over, 9 paused.
    pub game_phase: u8,
    /// `ScoringInfoV01.mYellowFlagState`: Full-course yellow state machine value.
    pub yellow_flag_state: i8,
    /// `ScoringInfoV01.mSectorFlag`: Per-sector flags; 1 = yellow, 11 = clear (CLAIMED, A-LMU-030); index order unknown.
    pub sector_flag: [i8; 3],
    /// `ScoringInfoV01.mStartLight`.
    pub start_light: u8,
    /// `ScoringInfoV01.mNumRedLights`.
    pub num_red_lights: u8,
    /// `ScoringInfoV01.mInRealtime`.
    pub in_realtime: bool,
    /// `ScoringInfoV01.mPlayerName`: Player name (PII, never transmitted).
    pub player_name: RawString,
    /// `ScoringInfoV01.mPlrFileName`: Player file name (PII, never transmitted).
    pub plr_file_name: RawString,
    /// `ScoringInfoV01.mDarkCloud`: Cloud darkness, 0..1.
    pub dark_cloud: f64,
    /// `ScoringInfoV01.mRaining`: Rain severity, 0..1.
    pub raining: f64,
    /// `ScoringInfoV01.mAmbientTemp`: Ambient temperature, degrees Celsius.
    pub ambient_temp: f64,
    /// `ScoringInfoV01.mTrackTemp`: Track temperature, degrees Celsius.
    pub track_temp: f64,
    /// `ScoringInfoV01.mWind`: Wind vector; frame and unit unverified (A-LMU-042).
    pub wind: Vec3,
    /// `ScoringInfoV01.mMinPathWetness`: Minimum wetness on the racing line, 0..1.
    pub min_path_wetness: f64,
    /// `ScoringInfoV01.mMaxPathWetness`: Maximum wetness on the racing line, 0..1.
    pub max_path_wetness: f64,
    /// `ScoringInfoV01.mGameMode`.
    pub game_mode: u8,
    /// `ScoringInfoV01.mIsPasswordProtected`.
    pub is_password_protected: bool,
    /// `ScoringInfoV01.mServerPort`: Server port (privacy: not transmitted).
    pub server_port: u16,
    /// `ScoringInfoV01.mServerPublicIP`: Server public IP (privacy: not transmitted).
    pub server_public_ip: u32,
    /// `ScoringInfoV01.mMaxPlayers`.
    pub max_players: i32,
    /// `ScoringInfoV01.mServerName`: Server name (privacy: not transmitted by default).
    pub server_name: RawString,
    /// `ScoringInfoV01.mStartET`: Start time of day, seconds since midnight.
    pub start_et: f32,
    /// `ScoringInfoV01.mAvgPathWetness`: Average wetness on the racing line, 0..1.
    pub avg_path_wetness: f64,
    /// `ScoringInfoV01.mSessionTimeRemaining`: Session time remaining; unit not stated (assumed s, A-LMU-035).
    pub session_time_remaining: f32,
    /// `ScoringInfoV01.mTimeOfDay`: Time of day; scale unverified (A-LMU-036).
    pub time_of_day: f32,
    /// `ScoringInfoV01.mIsFixedSetup`.
    pub is_fixed_setup: bool,
    /// `ScoringInfoV01.mTrackGripLevel`.
    pub track_grip_level: u8,
    /// `ScoringInfoV01.mCloudCoverage`.
    pub cloud_coverage: u8,
    /// `ScoringInfoV01.mTrackLimitsStepsPerPenalty`.
    pub track_limits_steps_per_penalty: u8,
    /// `ScoringInfoV01.mTrackLimitsStepsPerPoint`.
    pub track_limits_steps_per_point: u8,
    /// `ScoringInfoV01.mExpansion` (187 reserved bytes, diagnostics only).
    pub expansion: ReservedBytes,
}

/// Decodes one `ScoringInfoV01` from exactly `scoring_info::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_scoring_info(buf: &[u8]) -> Result<RawScoringInfo, DecodeError> {
    let r = FieldReader::new(buf, scoring_info::SIZE, "ScoringInfoV01")?;
    Ok(RawScoringInfo {
        track_name: r.string(scoring_info::TRACK_NAME)?,
        session: r.i32(scoring_info::SESSION)?,
        current_et: r.f64(scoring_info::CURRENT_ET)?,
        end_et: r.f64(scoring_info::END_ET)?,
        max_laps: r.i32(scoring_info::MAX_LAPS)?,
        lap_dist: r.f64(scoring_info::LAP_DIST)?,
        num_vehicles: r.i32(scoring_info::NUM_VEHICLES)?,
        game_phase: r.u8(scoring_info::GAME_PHASE)?,
        yellow_flag_state: r.i8(scoring_info::YELLOW_FLAG_STATE)?,
        sector_flag: r.i8_array::<3>(scoring_info::SECTOR_FLAG)?,
        start_light: r.u8(scoring_info::START_LIGHT)?,
        num_red_lights: r.u8(scoring_info::NUM_RED_LIGHTS)?,
        in_realtime: r.bool(scoring_info::IN_REALTIME)?,
        player_name: r.string(scoring_info::PLAYER_NAME)?,
        plr_file_name: r.string(scoring_info::PLR_FILE_NAME)?,
        dark_cloud: r.f64(scoring_info::DARK_CLOUD)?,
        raining: r.f64(scoring_info::RAINING)?,
        ambient_temp: r.f64(scoring_info::AMBIENT_TEMP)?,
        track_temp: r.f64(scoring_info::TRACK_TEMP)?,
        wind: r.vec3(scoring_info::WIND)?,
        min_path_wetness: r.f64(scoring_info::MIN_PATH_WETNESS)?,
        max_path_wetness: r.f64(scoring_info::MAX_PATH_WETNESS)?,
        game_mode: r.u8(scoring_info::GAME_MODE)?,
        is_password_protected: r.bool(scoring_info::IS_PASSWORD_PROTECTED)?,
        server_port: r.u16(scoring_info::SERVER_PORT)?,
        server_public_ip: r.u32(scoring_info::SERVER_PUBLIC_IP)?,
        max_players: r.i32(scoring_info::MAX_PLAYERS)?,
        server_name: r.string(scoring_info::SERVER_NAME)?,
        start_et: r.f32(scoring_info::START_ET)?,
        avg_path_wetness: r.f64(scoring_info::AVG_PATH_WETNESS)?,
        session_time_remaining: r.f32(scoring_info::SESSION_TIME_REMAINING)?,
        time_of_day: r.f32(scoring_info::TIME_OF_DAY)?,
        is_fixed_setup: r.bool(scoring_info::IS_FIXED_SETUP)?,
        track_grip_level: r.u8(scoring_info::TRACK_GRIP_LEVEL)?,
        cloud_coverage: r.u8(scoring_info::CLOUD_COVERAGE)?,
        track_limits_steps_per_penalty: r.u8(scoring_info::TRACK_LIMITS_STEPS_PER_PENALTY)?,
        track_limits_steps_per_point: r.u8(scoring_info::TRACK_LIMITS_STEPS_PER_POINT)?,
        expansion: r.reserved(scoring_info::EXPANSION)?,
    })
}

/// Decoded `VehicleScoringInfoV01`: one row of `data.scoring.vehScoringInfo[]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawVehicleScoring {
    /// `VehicleScoringInfoV01.mID`: Slot id; join key with telemetry, reused in multiplayer (A-LMU-023).
    pub id: i32,
    /// `VehicleScoringInfoV01.mDriverName`: Driver name (PII).
    pub driver_name: RawString,
    /// `VehicleScoringInfoV01.mVehicleName`.
    pub vehicle_name: RawString,
    /// `VehicleScoringInfoV01.mTotalLaps`.
    pub total_laps: i16,
    /// `VehicleScoringInfoV01.mSector`: Sector: 0 = sector 3, 1 = sector 1, 2 = sector 2 (A-LMU-026).
    pub sector: i8,
    /// `VehicleScoringInfoV01.mFinishStatus`.
    pub finish_status: i8,
    /// `VehicleScoringInfoV01.mLapDist`: Distance around the track, m.
    pub lap_dist: f64,
    /// `VehicleScoringInfoV01.mPathLateral`.
    pub path_lateral: f64,
    /// `VehicleScoringInfoV01.mTrackEdge`.
    pub track_edge: f64,
    /// `VehicleScoringInfoV01.mBestSector1`: Best sector 1, s (<= 0 means none).
    pub best_sector1: f64,
    /// `VehicleScoringInfoV01.mBestSector2`: Best sector 1 + 2 (cumulative), s (<= 0 means none).
    pub best_sector2: f64,
    /// `VehicleScoringInfoV01.mBestLapTime`: Best lap time, s (<= 0 means none).
    pub best_lap_time: f64,
    /// `VehicleScoringInfoV01.mLastSector1`: Last sector 1, s (<= 0 means none).
    pub last_sector1: f64,
    /// `VehicleScoringInfoV01.mLastSector2`: Last sector 1 + 2 (cumulative), s (<= 0 means none).
    pub last_sector2: f64,
    /// `VehicleScoringInfoV01.mLastLapTime`: Last lap time, s (<= 0 means none).
    pub last_lap_time: f64,
    /// `VehicleScoringInfoV01.mCurSector1`: Current lap sector 1, s (<= 0 means none).
    pub cur_sector1: f64,
    /// `VehicleScoringInfoV01.mCurSector2`: Current lap sector 1 + 2 (cumulative), s (<= 0 means none).
    pub cur_sector2: f64,
    /// `VehicleScoringInfoV01.mNumPitstops`.
    pub num_pitstops: i16,
    /// `VehicleScoringInfoV01.mNumPenalties`.
    pub num_penalties: i16,
    /// `VehicleScoringInfoV01.mIsPlayer`: Row belongs to the local player.
    pub is_player: bool,
    /// `VehicleScoringInfoV01.mControl`.
    pub control: i8,
    /// `VehicleScoringInfoV01.mInPits`: In the pit lane; unreliable for remote cars (A-LMU-029).
    pub in_pits: bool,
    /// `VehicleScoringInfoV01.mPlace`: Position in the classification, 1-based.
    pub place: u8,
    /// `VehicleScoringInfoV01.mVehicleClass`: Class name text (not the telemetry enum, A-LMU-055).
    pub vehicle_class: RawString,
    /// `VehicleScoringInfoV01.mTimeBehindNext`.
    pub time_behind_next: f64,
    /// `VehicleScoringInfoV01.mLapsBehindNext`.
    pub laps_behind_next: i32,
    /// `VehicleScoringInfoV01.mTimeBehindLeader`.
    pub time_behind_leader: f64,
    /// `VehicleScoringInfoV01.mLapsBehindLeader`.
    pub laps_behind_leader: i32,
    /// `VehicleScoringInfoV01.mLapStartET`.
    pub lap_start_et: f64,
    /// `VehicleScoringInfoV01.mPos`.
    pub pos: Vec3,
    /// `VehicleScoringInfoV01.mLocalVel`.
    pub local_vel: Vec3,
    /// `VehicleScoringInfoV01.mLocalAccel`.
    pub local_accel: Vec3,
    /// `VehicleScoringInfoV01.mOri`.
    pub ori: [Vec3; 3],
    /// `VehicleScoringInfoV01.mLocalRot`.
    pub local_rot: Vec3,
    /// `VehicleScoringInfoV01.mLocalRotAccel`.
    pub local_rot_accel: Vec3,
    /// `VehicleScoringInfoV01.mHeadlights`.
    pub headlights: u8,
    /// `VehicleScoringInfoV01.mPitState`: 0 none, 1 request, 2 entering, 3 stopped, 4 exiting.
    pub pit_state: u8,
    /// `VehicleScoringInfoV01.mServerScored`.
    pub server_scored: u8,
    /// `VehicleScoringInfoV01.mIndividualPhase`.
    pub individual_phase: u8,
    /// `VehicleScoringInfoV01.mQualification`.
    pub qualification: i32,
    /// `VehicleScoringInfoV01.mTimeIntoLap`.
    pub time_into_lap: f64,
    /// `VehicleScoringInfoV01.mEstimatedLapTime`.
    pub estimated_lap_time: f64,
    /// `VehicleScoringInfoV01.mPitGroup`.
    pub pit_group: RawString,
    /// `VehicleScoringInfoV01.mFlag`: Flag shown to the car (0 green, 6 blue).
    pub flag: u8,
    /// `VehicleScoringInfoV01.mUnderYellow`.
    pub under_yellow: bool,
    /// `VehicleScoringInfoV01.mCountLapFlag`.
    pub count_lap_flag: u8,
    /// `VehicleScoringInfoV01.mInGarageStall`.
    pub in_garage_stall: bool,
    /// `VehicleScoringInfoV01.mUpgradePack`.
    pub upgrade_pack: [u8; 16],
    /// `VehicleScoringInfoV01.mPitLapDist`.
    pub pit_lap_dist: f32,
    /// `VehicleScoringInfoV01.mBestLapSector1`.
    pub best_lap_sector1: f32,
    /// `VehicleScoringInfoV01.mBestLapSector2`.
    pub best_lap_sector2: f32,
    /// `VehicleScoringInfoV01.mSteamID`: Steam id (PII, never transmitted).
    pub steam_id: u64,
    /// `VehicleScoringInfoV01.mVehFilename`.
    pub veh_filename: RawString,
    /// `VehicleScoringInfoV01.mAttackMode`.
    pub attack_mode: i16,
    /// `VehicleScoringInfoV01.mFuelFraction`: Fuel or battery fraction, 0..255 (A-LMU-044).
    pub fuel_fraction: u8,
    /// `VehicleScoringInfoV01.mDRSState`.
    pub drs_state: bool,
    /// `VehicleScoringInfoV01.mExpansion` (4 reserved bytes, diagnostics only).
    pub expansion: ReservedBytes,
}

/// Decodes one `VehicleScoringInfoV01` from exactly `veh_scoring::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_vehicle_scoring(buf: &[u8]) -> Result<RawVehicleScoring, DecodeError> {
    let r = FieldReader::new(buf, veh_scoring::SIZE, "VehicleScoringInfoV01")?;
    Ok(RawVehicleScoring {
        id: r.i32(veh_scoring::ID)?,
        driver_name: r.string(veh_scoring::DRIVER_NAME)?,
        vehicle_name: r.string(veh_scoring::VEHICLE_NAME)?,
        total_laps: r.i16(veh_scoring::TOTAL_LAPS)?,
        sector: r.i8(veh_scoring::SECTOR)?,
        finish_status: r.i8(veh_scoring::FINISH_STATUS)?,
        lap_dist: r.f64(veh_scoring::LAP_DIST)?,
        path_lateral: r.f64(veh_scoring::PATH_LATERAL)?,
        track_edge: r.f64(veh_scoring::TRACK_EDGE)?,
        best_sector1: r.f64(veh_scoring::BEST_SECTOR1)?,
        best_sector2: r.f64(veh_scoring::BEST_SECTOR2)?,
        best_lap_time: r.f64(veh_scoring::BEST_LAP_TIME)?,
        last_sector1: r.f64(veh_scoring::LAST_SECTOR1)?,
        last_sector2: r.f64(veh_scoring::LAST_SECTOR2)?,
        last_lap_time: r.f64(veh_scoring::LAST_LAP_TIME)?,
        cur_sector1: r.f64(veh_scoring::CUR_SECTOR1)?,
        cur_sector2: r.f64(veh_scoring::CUR_SECTOR2)?,
        num_pitstops: r.i16(veh_scoring::NUM_PITSTOPS)?,
        num_penalties: r.i16(veh_scoring::NUM_PENALTIES)?,
        is_player: r.bool(veh_scoring::IS_PLAYER)?,
        control: r.i8(veh_scoring::CONTROL)?,
        in_pits: r.bool(veh_scoring::IN_PITS)?,
        place: r.u8(veh_scoring::PLACE)?,
        vehicle_class: r.string(veh_scoring::VEHICLE_CLASS)?,
        time_behind_next: r.f64(veh_scoring::TIME_BEHIND_NEXT)?,
        laps_behind_next: r.i32(veh_scoring::LAPS_BEHIND_NEXT)?,
        time_behind_leader: r.f64(veh_scoring::TIME_BEHIND_LEADER)?,
        laps_behind_leader: r.i32(veh_scoring::LAPS_BEHIND_LEADER)?,
        lap_start_et: r.f64(veh_scoring::LAP_START_ET)?,
        pos: r.vec3(veh_scoring::POS)?,
        local_vel: r.vec3(veh_scoring::LOCAL_VEL)?,
        local_accel: r.vec3(veh_scoring::LOCAL_ACCEL)?,
        ori: r.vec3_array::<3>(veh_scoring::ORI)?,
        local_rot: r.vec3(veh_scoring::LOCAL_ROT)?,
        local_rot_accel: r.vec3(veh_scoring::LOCAL_ROT_ACCEL)?,
        headlights: r.u8(veh_scoring::HEADLIGHTS)?,
        pit_state: r.u8(veh_scoring::PIT_STATE)?,
        server_scored: r.u8(veh_scoring::SERVER_SCORED)?,
        individual_phase: r.u8(veh_scoring::INDIVIDUAL_PHASE)?,
        qualification: r.i32(veh_scoring::QUALIFICATION)?,
        time_into_lap: r.f64(veh_scoring::TIME_INTO_LAP)?,
        estimated_lap_time: r.f64(veh_scoring::ESTIMATED_LAP_TIME)?,
        pit_group: r.string(veh_scoring::PIT_GROUP)?,
        flag: r.u8(veh_scoring::FLAG)?,
        under_yellow: r.bool(veh_scoring::UNDER_YELLOW)?,
        count_lap_flag: r.u8(veh_scoring::COUNT_LAP_FLAG)?,
        in_garage_stall: r.bool(veh_scoring::IN_GARAGE_STALL)?,
        upgrade_pack: r.u8_array::<16>(veh_scoring::UPGRADE_PACK)?,
        pit_lap_dist: r.f32(veh_scoring::PIT_LAP_DIST)?,
        best_lap_sector1: r.f32(veh_scoring::BEST_LAP_SECTOR1)?,
        best_lap_sector2: r.f32(veh_scoring::BEST_LAP_SECTOR2)?,
        steam_id: r.u64(veh_scoring::STEAM_ID)?,
        veh_filename: r.string(veh_scoring::VEH_FILENAME)?,
        attack_mode: r.i16(veh_scoring::ATTACK_MODE)?,
        fuel_fraction: r.u8(veh_scoring::FUEL_FRACTION)?,
        drs_state: r.bool(veh_scoring::DRS_STATE)?,
        expansion: r.reserved(veh_scoring::EXPANSION)?,
    })
}

/// Decoded `TelemInfoV01`: one slot of `data.telemetry.telemInfo[]` (normally the player's).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawTelemetry {
    /// `TelemInfoV01.mID`: Slot id; unique within a snapshot, reused in multiplayer (A-LMU-023).
    pub id: i32,
    /// `TelemInfoV01.mDeltaTime`: Time since the previous update, s.
    pub delta_time: f64,
    /// `TelemInfoV01.mElapsedTime`: Game session time, s. Telemetry freshness key (A-LMU-031).
    pub elapsed_time: f64,
    /// `TelemInfoV01.mLapNumber`: Current lap number.
    pub lap_number: i32,
    /// `TelemInfoV01.mLapStartET`: Session time at which the current lap started, s.
    pub lap_start_et: f64,
    /// `TelemInfoV01.mVehicleName`.
    pub vehicle_name: RawString,
    /// `TelemInfoV01.mTrackName`.
    pub track_name: RawString,
    /// `TelemInfoV01.mPos`: World position, m (left-handed world, +y up).
    pub pos: Vec3,
    /// `TelemInfoV01.mLocalVel`: Velocity in the local vehicle frame, m/s (+x left, +y up, +z rear).
    pub local_vel: Vec3,
    /// `TelemInfoV01.mLocalAccel`: Acceleration in the local vehicle frame, m/s^2 (gravity handling: A-LMU-032).
    pub local_accel: Vec3,
    /// `TelemInfoV01.mOri`: Orientation matrix rows (world frame).
    pub ori: [Vec3; 3],
    /// `TelemInfoV01.mLocalRot`: Rotation rate in the local frame, rad/s (+y yaws right).
    pub local_rot: Vec3,
    /// `TelemInfoV01.mLocalRotAccel`: Rotational acceleration in the local frame, rad/s^2.
    pub local_rot_accel: Vec3,
    /// `TelemInfoV01.mGear`: -1 reverse, 0 neutral, 1.. forward.
    pub gear: i32,
    /// `TelemInfoV01.mEngineRPM`: Engine speed, rpm.
    pub engine_rpm: f64,
    /// `TelemInfoV01.mEngineWaterTemp`: Engine water temperature, degrees Celsius per header.
    pub engine_water_temp: f64,
    /// `TelemInfoV01.mEngineOilTemp`: Engine oil temperature, degrees Celsius per header.
    pub engine_oil_temp: f64,
    /// `TelemInfoV01.mClutchRPM`: Clutch speed, rpm.
    pub clutch_rpm: f64,
    /// `TelemInfoV01.mUnfilteredThrottle`: Raw driver input, 0..1.
    pub unfiltered_throttle: f64,
    /// `TelemInfoV01.mUnfilteredBrake`: Raw driver input, 0..1.
    pub unfiltered_brake: f64,
    /// `TelemInfoV01.mUnfilteredSteering`: Raw driver input, -1..1.
    pub unfiltered_steering: f64,
    /// `TelemInfoV01.mUnfilteredClutch`: Raw driver input, 0..1.
    pub unfiltered_clutch: f64,
    /// `TelemInfoV01.mFilteredThrottle`: Input after aids, 0..1.
    pub filtered_throttle: f64,
    /// `TelemInfoV01.mFilteredBrake`: Input after aids, 0..1.
    pub filtered_brake: f64,
    /// `TelemInfoV01.mFilteredSteering`: Input after aids, -1..1.
    pub filtered_steering: f64,
    /// `TelemInfoV01.mFilteredClutch`: Input after aids, 0..1.
    pub filtered_clutch: f64,
    /// `TelemInfoV01.mSteeringShaftTorque`: Steering shaft torque; unit not stated (assumed N*m, A-LMU-053).
    pub steering_shaft_torque: f64,
    /// `TelemInfoV01.mFront3rdDeflection`.
    pub front3rd_deflection: f64,
    /// `TelemInfoV01.mRear3rdDeflection`.
    pub rear3rd_deflection: f64,
    /// `TelemInfoV01.mFrontWingHeight`.
    pub front_wing_height: f64,
    /// `TelemInfoV01.mFrontRideHeight`.
    pub front_ride_height: f64,
    /// `TelemInfoV01.mRearRideHeight`.
    pub rear_ride_height: f64,
    /// `TelemInfoV01.mDrag`.
    pub drag: f64,
    /// `TelemInfoV01.mFrontDownforce`.
    pub front_downforce: f64,
    /// `TelemInfoV01.mRearDownforce`.
    pub rear_downforce: f64,
    /// `TelemInfoV01.mFuel`: Fuel in the tank, litres (A-LMU-018).
    pub fuel: f64,
    /// `TelemInfoV01.mEngineMaxRPM`: Rev limit, rpm.
    pub engine_max_rpm: f64,
    /// `TelemInfoV01.mScheduledStops`.
    pub scheduled_stops: u8,
    /// `TelemInfoV01.mOverheating`.
    pub overheating: bool,
    /// `TelemInfoV01.mDetached`.
    pub detached: bool,
    /// `TelemInfoV01.mHeadlights`.
    pub headlights: bool,
    /// `TelemInfoV01.mDentSeverity`: Dent severity per body location, 0/1/2.
    pub dent_severity: [u8; 8],
    /// `TelemInfoV01.mLastImpactET`.
    pub last_impact_et: f64,
    /// `TelemInfoV01.mLastImpactMagnitude`.
    pub last_impact_magnitude: f64,
    /// `TelemInfoV01.mLastImpactPos`.
    pub last_impact_pos: Vec3,
    /// `TelemInfoV01.mEngineTorque`: Engine torque; unit not stated (assumed N*m, A-LMU-053).
    pub engine_torque: f64,
    /// `TelemInfoV01.mCurrentSector`: Zero-based sector in the low bits; bit 31 set while in the pit lane (A-LMU-026).
    pub current_sector: i32,
    /// `TelemInfoV01.mSpeedLimiter`.
    pub speed_limiter: u8,
    /// `TelemInfoV01.mMaxGears`.
    pub max_gears: u8,
    /// `TelemInfoV01.mFrontTireCompoundIndex`.
    pub front_tire_compound_index: u8,
    /// `TelemInfoV01.mRearTireCompoundIndex`.
    pub rear_tire_compound_index: u8,
    /// `TelemInfoV01.mFuelCapacity`: Tank capacity, litres (A-LMU-018).
    pub fuel_capacity: f64,
    /// `TelemInfoV01.mFrontFlapActivated`.
    pub front_flap_activated: u8,
    /// `TelemInfoV01.mRearFlapActivated`.
    pub rear_flap_activated: u8,
    /// `TelemInfoV01.mRearFlapLegalStatus`.
    pub rear_flap_legal_status: u8,
    /// `TelemInfoV01.mIgnitionStarter`.
    pub ignition_starter: u8,
    /// `TelemInfoV01.mFrontTireCompoundName`.
    pub front_tire_compound_name: RawString,
    /// `TelemInfoV01.mRearTireCompoundName`.
    pub rear_tire_compound_name: RawString,
    /// `TelemInfoV01.mSpeedLimiterAvailable`.
    pub speed_limiter_available: u8,
    /// `TelemInfoV01.mAntiStallActivated`.
    pub anti_stall_activated: u8,
    /// `TelemInfoV01.mUnused` (2 reserved bytes, diagnostics only).
    pub unused: ReservedBytes,
    /// `TelemInfoV01.mVisualSteeringWheelRange`.
    pub visual_steering_wheel_range: f32,
    /// `TelemInfoV01.mRearBrakeBias`: Fraction of brake force on the rear axle.
    pub rear_brake_bias: f64,
    /// `TelemInfoV01.mTurboBoostPressure`: Boost pressure; unit not stated (A-LMU-027).
    pub turbo_boost_pressure: f64,
    /// `TelemInfoV01.mPhysicsToGraphicsOffset`.
    pub physics_to_graphics_offset: [f32; 3],
    /// `TelemInfoV01.mPhysicalSteeringWheelRange`: Steering wheel range; unit not stated (assumed degrees, A-LMU-037).
    pub physical_steering_wheel_range: f32,
    /// `TelemInfoV01.mDeltaBest`: Delta to best lap; unit not stated (assumed s, A-LMU-033).
    pub delta_best: f64,
    /// `TelemInfoV01.mBatteryChargeFraction`: Battery charge, 0..1.
    pub battery_charge_fraction: f64,
    /// `TelemInfoV01.mElectricBoostMotorTorque`.
    pub electric_boost_motor_torque: f64,
    /// `TelemInfoV01.mElectricBoostMotorRPM`.
    pub electric_boost_motor_rpm: f64,
    /// `TelemInfoV01.mElectricBoostMotorTemperature`.
    pub electric_boost_motor_temperature: f64,
    /// `TelemInfoV01.mElectricBoostWaterTemperature`.
    pub electric_boost_water_temperature: f64,
    /// `TelemInfoV01.mElectricBoostMotorState`: 0 unavailable, 1 inactive, 2 propulsion, 3 regeneration.
    pub electric_boost_motor_state: u8,
    /// `TelemInfoV01.mLapInvalidated`.
    pub lap_invalidated: bool,
    /// `TelemInfoV01.mABSActive`: ABS intervening right now.
    pub abs_active: bool,
    /// `TelemInfoV01.mTCActive`: TC intervening right now.
    pub tc_active: bool,
    /// `TelemInfoV01.mSpeedLimiterActive`.
    pub speed_limiter_active: bool,
    /// `TelemInfoV01.mWiperState`.
    pub wiper_state: u8,
    /// `TelemInfoV01.mTC`: TC setting level.
    pub tc: u8,
    /// `TelemInfoV01.mTCMax`.
    pub tc_max: u8,
    /// `TelemInfoV01.mTCSlip`.
    pub tc_slip: u8,
    /// `TelemInfoV01.mTCSlipMax`.
    pub tc_slip_max: u8,
    /// `TelemInfoV01.mTCCut`.
    pub tc_cut: u8,
    /// `TelemInfoV01.mTCCutMax`.
    pub tc_cut_max: u8,
    /// `TelemInfoV01.mABS`: ABS setting level.
    pub abs: u8,
    /// `TelemInfoV01.mABSMax`.
    pub abs_max: u8,
    /// `TelemInfoV01.mMotorMap`.
    pub motor_map: u8,
    /// `TelemInfoV01.mMotorMapMax`.
    pub motor_map_max: u8,
    /// `TelemInfoV01.mMigration`.
    pub migration: u8,
    /// `TelemInfoV01.mMigrationMax`.
    pub migration_max: u8,
    /// `TelemInfoV01.mFrontAntiSway`.
    pub front_anti_sway: u8,
    /// `TelemInfoV01.mFrontAntiSwayMax`.
    pub front_anti_sway_max: u8,
    /// `TelemInfoV01.mRearAntiSway`.
    pub rear_anti_sway: u8,
    /// `TelemInfoV01.mRearAntiSwayMax`.
    pub rear_anti_sway_max: u8,
    /// `TelemInfoV01.mLiftAndCoastProgress`.
    pub lift_and_coast_progress: u8,
    /// `TelemInfoV01.mTrackLimitsSteps`.
    pub track_limits_steps: u8,
    /// `TelemInfoV01.mRegen`: Regeneration power, kW per header.
    pub regen: f32,
    /// `TelemInfoV01.mSoC`: State of charge; unit not stated (A-LMU-014).
    pub soc: f32,
    /// `TelemInfoV01.mVirtualEnergy`: Virtual energy; unit not stated (A-LMU-013).
    pub virtual_energy: f32,
    /// `TelemInfoV01.mTimeGapCarAhead`: Gap to the car physically ahead; unit not stated (A-LMU-034).
    pub time_gap_car_ahead: f32,
    /// `TelemInfoV01.mTimeGapCarBehind`: Gap to the car physically behind; unit not stated (A-LMU-034).
    pub time_gap_car_behind: f32,
    /// `TelemInfoV01.mTimeGapPlaceAhead`: Gap to the car ahead in the classification; unit not stated (A-LMU-034).
    pub time_gap_place_ahead: f32,
    /// `TelemInfoV01.mTimeGapPlaceBehind`: Gap to the car behind in the classification; unit not stated (A-LMU-034).
    pub time_gap_place_behind: f32,
    /// `TelemInfoV01.mVehicleModel`.
    pub vehicle_model: RawString,
    /// `TelemInfoV01.mVehicleClass`: Raw `IP_VehicleClass` value, see [`crate::VehicleClass`].
    pub vehicle_class: u8,
    /// `TelemInfoV01.mVehicleChampionship`: Raw `IP_VehicleChampionship` value, see [`crate::VehicleChampionship`].
    pub vehicle_championship: u8,
    /// `TelemInfoV01.mExpansion` (20 reserved bytes, diagnostics only).
    pub expansion: ReservedBytes,
    /// `TelemInfoV01.mWheel[4]`: FL, FR, RL, RR (A-LMU-019).
    pub wheels: [RawWheel; WHEEL_COUNT],
}

/// Decodes one `TelemInfoV01` from exactly `telem_info::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_telemetry(buf: &[u8]) -> Result<RawTelemetry, DecodeError> {
    let r = FieldReader::new(buf, telem_info::SIZE, "TelemInfoV01")?;
    Ok(RawTelemetry {
        id: r.i32(telem_info::ID)?,
        delta_time: r.f64(telem_info::DELTA_TIME)?,
        elapsed_time: r.f64(telem_info::ELAPSED_TIME)?,
        lap_number: r.i32(telem_info::LAP_NUMBER)?,
        lap_start_et: r.f64(telem_info::LAP_START_ET)?,
        vehicle_name: r.string(telem_info::VEHICLE_NAME)?,
        track_name: r.string(telem_info::TRACK_NAME)?,
        pos: r.vec3(telem_info::POS)?,
        local_vel: r.vec3(telem_info::LOCAL_VEL)?,
        local_accel: r.vec3(telem_info::LOCAL_ACCEL)?,
        ori: r.vec3_array::<3>(telem_info::ORI)?,
        local_rot: r.vec3(telem_info::LOCAL_ROT)?,
        local_rot_accel: r.vec3(telem_info::LOCAL_ROT_ACCEL)?,
        gear: r.i32(telem_info::GEAR)?,
        engine_rpm: r.f64(telem_info::ENGINE_RPM)?,
        engine_water_temp: r.f64(telem_info::ENGINE_WATER_TEMP)?,
        engine_oil_temp: r.f64(telem_info::ENGINE_OIL_TEMP)?,
        clutch_rpm: r.f64(telem_info::CLUTCH_RPM)?,
        unfiltered_throttle: r.f64(telem_info::UNFILTERED_THROTTLE)?,
        unfiltered_brake: r.f64(telem_info::UNFILTERED_BRAKE)?,
        unfiltered_steering: r.f64(telem_info::UNFILTERED_STEERING)?,
        unfiltered_clutch: r.f64(telem_info::UNFILTERED_CLUTCH)?,
        filtered_throttle: r.f64(telem_info::FILTERED_THROTTLE)?,
        filtered_brake: r.f64(telem_info::FILTERED_BRAKE)?,
        filtered_steering: r.f64(telem_info::FILTERED_STEERING)?,
        filtered_clutch: r.f64(telem_info::FILTERED_CLUTCH)?,
        steering_shaft_torque: r.f64(telem_info::STEERING_SHAFT_TORQUE)?,
        front3rd_deflection: r.f64(telem_info::FRONT3RD_DEFLECTION)?,
        rear3rd_deflection: r.f64(telem_info::REAR3RD_DEFLECTION)?,
        front_wing_height: r.f64(telem_info::FRONT_WING_HEIGHT)?,
        front_ride_height: r.f64(telem_info::FRONT_RIDE_HEIGHT)?,
        rear_ride_height: r.f64(telem_info::REAR_RIDE_HEIGHT)?,
        drag: r.f64(telem_info::DRAG)?,
        front_downforce: r.f64(telem_info::FRONT_DOWNFORCE)?,
        rear_downforce: r.f64(telem_info::REAR_DOWNFORCE)?,
        fuel: r.f64(telem_info::FUEL)?,
        engine_max_rpm: r.f64(telem_info::ENGINE_MAX_RPM)?,
        scheduled_stops: r.u8(telem_info::SCHEDULED_STOPS)?,
        overheating: r.bool(telem_info::OVERHEATING)?,
        detached: r.bool(telem_info::DETACHED)?,
        headlights: r.bool(telem_info::HEADLIGHTS)?,
        dent_severity: r.u8_array::<8>(telem_info::DENT_SEVERITY)?,
        last_impact_et: r.f64(telem_info::LAST_IMPACT_ET)?,
        last_impact_magnitude: r.f64(telem_info::LAST_IMPACT_MAGNITUDE)?,
        last_impact_pos: r.vec3(telem_info::LAST_IMPACT_POS)?,
        engine_torque: r.f64(telem_info::ENGINE_TORQUE)?,
        current_sector: r.i32(telem_info::CURRENT_SECTOR)?,
        speed_limiter: r.u8(telem_info::SPEED_LIMITER)?,
        max_gears: r.u8(telem_info::MAX_GEARS)?,
        front_tire_compound_index: r.u8(telem_info::FRONT_TIRE_COMPOUND_INDEX)?,
        rear_tire_compound_index: r.u8(telem_info::REAR_TIRE_COMPOUND_INDEX)?,
        fuel_capacity: r.f64(telem_info::FUEL_CAPACITY)?,
        front_flap_activated: r.u8(telem_info::FRONT_FLAP_ACTIVATED)?,
        rear_flap_activated: r.u8(telem_info::REAR_FLAP_ACTIVATED)?,
        rear_flap_legal_status: r.u8(telem_info::REAR_FLAP_LEGAL_STATUS)?,
        ignition_starter: r.u8(telem_info::IGNITION_STARTER)?,
        front_tire_compound_name: r.string(telem_info::FRONT_TIRE_COMPOUND_NAME)?,
        rear_tire_compound_name: r.string(telem_info::REAR_TIRE_COMPOUND_NAME)?,
        speed_limiter_available: r.u8(telem_info::SPEED_LIMITER_AVAILABLE)?,
        anti_stall_activated: r.u8(telem_info::ANTI_STALL_ACTIVATED)?,
        unused: r.reserved(telem_info::UNUSED)?,
        visual_steering_wheel_range: r.f32(telem_info::VISUAL_STEERING_WHEEL_RANGE)?,
        rear_brake_bias: r.f64(telem_info::REAR_BRAKE_BIAS)?,
        turbo_boost_pressure: r.f64(telem_info::TURBO_BOOST_PRESSURE)?,
        physics_to_graphics_offset: r.f32_array::<3>(telem_info::PHYSICS_TO_GRAPHICS_OFFSET)?,
        physical_steering_wheel_range: r.f32(telem_info::PHYSICAL_STEERING_WHEEL_RANGE)?,
        delta_best: r.f64(telem_info::DELTA_BEST)?,
        battery_charge_fraction: r.f64(telem_info::BATTERY_CHARGE_FRACTION)?,
        electric_boost_motor_torque: r.f64(telem_info::ELECTRIC_BOOST_MOTOR_TORQUE)?,
        electric_boost_motor_rpm: r.f64(telem_info::ELECTRIC_BOOST_MOTOR_RPM)?,
        electric_boost_motor_temperature: r.f64(telem_info::ELECTRIC_BOOST_MOTOR_TEMPERATURE)?,
        electric_boost_water_temperature: r.f64(telem_info::ELECTRIC_BOOST_WATER_TEMPERATURE)?,
        electric_boost_motor_state: r.u8(telem_info::ELECTRIC_BOOST_MOTOR_STATE)?,
        lap_invalidated: r.bool(telem_info::LAP_INVALIDATED)?,
        abs_active: r.bool(telem_info::ABS_ACTIVE)?,
        tc_active: r.bool(telem_info::TC_ACTIVE)?,
        speed_limiter_active: r.bool(telem_info::SPEED_LIMITER_ACTIVE)?,
        wiper_state: r.u8(telem_info::WIPER_STATE)?,
        tc: r.u8(telem_info::TC)?,
        tc_max: r.u8(telem_info::TC_MAX)?,
        tc_slip: r.u8(telem_info::TC_SLIP)?,
        tc_slip_max: r.u8(telem_info::TC_SLIP_MAX)?,
        tc_cut: r.u8(telem_info::TC_CUT)?,
        tc_cut_max: r.u8(telem_info::TC_CUT_MAX)?,
        abs: r.u8(telem_info::ABS)?,
        abs_max: r.u8(telem_info::ABS_MAX)?,
        motor_map: r.u8(telem_info::MOTOR_MAP)?,
        motor_map_max: r.u8(telem_info::MOTOR_MAP_MAX)?,
        migration: r.u8(telem_info::MIGRATION)?,
        migration_max: r.u8(telem_info::MIGRATION_MAX)?,
        front_anti_sway: r.u8(telem_info::FRONT_ANTI_SWAY)?,
        front_anti_sway_max: r.u8(telem_info::FRONT_ANTI_SWAY_MAX)?,
        rear_anti_sway: r.u8(telem_info::REAR_ANTI_SWAY)?,
        rear_anti_sway_max: r.u8(telem_info::REAR_ANTI_SWAY_MAX)?,
        lift_and_coast_progress: r.u8(telem_info::LIFT_AND_COAST_PROGRESS)?,
        track_limits_steps: r.u8(telem_info::TRACK_LIMITS_STEPS)?,
        regen: r.f32(telem_info::REGEN)?,
        soc: r.f32(telem_info::SOC)?,
        virtual_energy: r.f32(telem_info::VIRTUAL_ENERGY)?,
        time_gap_car_ahead: r.f32(telem_info::TIME_GAP_CAR_AHEAD)?,
        time_gap_car_behind: r.f32(telem_info::TIME_GAP_CAR_BEHIND)?,
        time_gap_place_ahead: r.f32(telem_info::TIME_GAP_PLACE_AHEAD)?,
        time_gap_place_behind: r.f32(telem_info::TIME_GAP_PLACE_BEHIND)?,
        vehicle_model: r.string(telem_info::VEHICLE_MODEL)?,
        vehicle_class: r.u8(telem_info::VEHICLE_CLASS)?,
        vehicle_championship: r.u8(telem_info::VEHICLE_CHAMPIONSHIP)?,
        expansion: r.reserved(telem_info::EXPANSION)?,
        wheels: decode_wheels(&r)?,
    })
}

/// Decoded `TelemWheelV01`: one wheel; index 0 FL, 1 FR, 2 RL, 3 RR (A-LMU-019).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawWheel {
    /// `TelemWheelV01.mSuspensionDeflection`.
    pub suspension_deflection: f64,
    /// `TelemWheelV01.mRideHeight`.
    pub ride_height: f64,
    /// `TelemWheelV01.mSuspForce`.
    pub susp_force: f64,
    /// `TelemWheelV01.mBrakeTemp`: Brake temperature; header says Celsius, community says Kelvin (A-LMU-015).
    pub brake_temp: f64,
    /// `TelemWheelV01.mBrakePressure`: Normalised 0..1 (pedal x bias), not a line pressure (A-LMU-017).
    pub brake_pressure: f64,
    /// `TelemWheelV01.mRotation`: Wheel rotation, rad/s (sign: A-LMU-039).
    pub rotation: f64,
    /// `TelemWheelV01.mLateralPatchVel`.
    pub lateral_patch_vel: f64,
    /// `TelemWheelV01.mLongitudinalPatchVel`.
    pub longitudinal_patch_vel: f64,
    /// `TelemWheelV01.mLateralGroundVel`.
    pub lateral_ground_vel: f64,
    /// `TelemWheelV01.mLongitudinalGroundVel`.
    pub longitudinal_ground_vel: f64,
    /// `TelemWheelV01.mCamber`.
    pub camber: f64,
    /// `TelemWheelV01.mLateralForce`.
    pub lateral_force: f64,
    /// `TelemWheelV01.mLongitudinalForce`.
    pub longitudinal_force: f64,
    /// `TelemWheelV01.mTireLoad`.
    pub tire_load: f64,
    /// `TelemWheelV01.mGripFract`.
    pub grip_fract: f64,
    /// `TelemWheelV01.mPressure`: Tyre pressure, kPa.
    pub pressure: f64,
    /// `TelemWheelV01.mTemperature`: Tread temperature left/centre/right in the car frame, Kelvin (A-LMU-016).
    pub temperature: [f64; 3],
    /// `TelemWheelV01.mWear`: Wear as a fraction of maximum; direction unverified (A-LMU-040).
    pub wear: f64,
    /// `TelemWheelV01.mTerrainName`.
    pub terrain_name: RawString,
    /// `TelemWheelV01.mSurfaceType`.
    pub surface_type: u8,
    /// `TelemWheelV01.mFlat`.
    pub flat: bool,
    /// `TelemWheelV01.mDetached`.
    pub detached: bool,
    /// `TelemWheelV01.mStaticUndeflectedRadius`: Static undeflected radius, cm.
    pub static_undeflected_radius: u8,
    /// `TelemWheelV01.mVerticalTireDeflection`.
    pub vertical_tire_deflection: f64,
    /// `TelemWheelV01.mWheelYLocation`.
    pub wheel_y_location: f64,
    /// `TelemWheelV01.mToe`.
    pub toe: f64,
    /// `TelemWheelV01.mTireCarcassTemperature`: Carcass temperature, Kelvin.
    pub tire_carcass_temperature: f64,
    /// `TelemWheelV01.mTireInnerLayerTemperature`: Inner layer temperature left/centre/right, Kelvin.
    pub tire_inner_layer_temperature: [f64; 3],
    /// `TelemWheelV01.mOptimalTemp`: Optimal temperature; unit not stated (A-LMU-038).
    pub optimal_temp: f32,
    /// `TelemWheelV01.mCompoundIndex`.
    pub compound_index: u8,
    /// `TelemWheelV01.mCompoundType`: Compound type; meaning CLAIMED only (A-LMU-041).
    pub compound_type: u8,
    /// `TelemWheelV01.mExpansion` (18 reserved bytes, diagnostics only).
    pub expansion: ReservedBytes,
}

/// Decodes one `TelemWheelV01` from exactly `telem_wheel::SIZE` bytes.
///
/// # Errors
/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).
pub fn decode_wheel(buf: &[u8]) -> Result<RawWheel, DecodeError> {
    let r = FieldReader::new(buf, telem_wheel::SIZE, "TelemWheelV01")?;
    Ok(RawWheel {
        suspension_deflection: r.f64(telem_wheel::SUSPENSION_DEFLECTION)?,
        ride_height: r.f64(telem_wheel::RIDE_HEIGHT)?,
        susp_force: r.f64(telem_wheel::SUSP_FORCE)?,
        brake_temp: r.f64(telem_wheel::BRAKE_TEMP)?,
        brake_pressure: r.f64(telem_wheel::BRAKE_PRESSURE)?,
        rotation: r.f64(telem_wheel::ROTATION)?,
        lateral_patch_vel: r.f64(telem_wheel::LATERAL_PATCH_VEL)?,
        longitudinal_patch_vel: r.f64(telem_wheel::LONGITUDINAL_PATCH_VEL)?,
        lateral_ground_vel: r.f64(telem_wheel::LATERAL_GROUND_VEL)?,
        longitudinal_ground_vel: r.f64(telem_wheel::LONGITUDINAL_GROUND_VEL)?,
        camber: r.f64(telem_wheel::CAMBER)?,
        lateral_force: r.f64(telem_wheel::LATERAL_FORCE)?,
        longitudinal_force: r.f64(telem_wheel::LONGITUDINAL_FORCE)?,
        tire_load: r.f64(telem_wheel::TIRE_LOAD)?,
        grip_fract: r.f64(telem_wheel::GRIP_FRACT)?,
        pressure: r.f64(telem_wheel::PRESSURE)?,
        temperature: r.f64_array::<3>(telem_wheel::TEMPERATURE)?,
        wear: r.f64(telem_wheel::WEAR)?,
        terrain_name: r.string(telem_wheel::TERRAIN_NAME)?,
        surface_type: r.u8(telem_wheel::SURFACE_TYPE)?,
        flat: r.bool(telem_wheel::FLAT)?,
        detached: r.bool(telem_wheel::DETACHED)?,
        static_undeflected_radius: r.u8(telem_wheel::STATIC_UNDEFLECTED_RADIUS)?,
        vertical_tire_deflection: r.f64(telem_wheel::VERTICAL_TIRE_DEFLECTION)?,
        wheel_y_location: r.f64(telem_wheel::WHEEL_Y_LOCATION)?,
        toe: r.f64(telem_wheel::TOE)?,
        tire_carcass_temperature: r.f64(telem_wheel::TIRE_CARCASS_TEMPERATURE)?,
        tire_inner_layer_temperature: r
            .f64_array::<3>(telem_wheel::TIRE_INNER_LAYER_TEMPERATURE)?,
        optimal_temp: r.f32(telem_wheel::OPTIMAL_TEMP)?,
        compound_index: r.u8(telem_wheel::COMPOUND_INDEX)?,
        compound_type: r.u8(telem_wheel::COMPOUND_TYPE)?,
        expansion: r.reserved(telem_wheel::EXPANSION)?,
    })
}

/// Decodes the four `TelemWheelV01` entries of `TelemInfoV01.mWheel`.
fn decode_wheels(r: &FieldReader<'_>) -> Result<[RawWheel; WHEEL_COUNT], DecodeError> {
    let wheels = r.sub(telem_info::WHEEL)?;
    let wheel = |i: usize| -> Result<RawWheel, DecodeError> {
        let start = i * telem_wheel::SIZE;
        let bytes =
            wheels
                .get(start..start + telem_wheel::SIZE)
                .ok_or(DecodeError::OutOfBounds {
                    context: "TelemInfoV01.mWheel",
                    offset: start,
                    len: telem_wheel::SIZE,
                    available: wheels.len(),
                })?;
        decode_wheel(bytes)
    };
    Ok([wheel(0)?, wheel(1)?, wheel(2)?, wheel(3)?])
}

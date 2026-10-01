//! Byte offsets and sizes of the LMU `LMU_Data` shared-memory layout (D-003).
//!
//! **Generated** by `crates/lmu-reader/codegen/gen_layout.py` from
//! `docs/research/lmu_abi_layout.json`, the output of `tools/layout-probe` compiled with MSVC x64
//! against the SDK headers of the installed game (LMU 1.4.2.0). Do not edit by hand: re-run the probe
//! and the generator. The test at the bottom re-loads the JSON and fails loudly on any difference.
//!
//! * Per-struct modules hold offsets *relative to the start of that struct*.
//! * [`abs`] holds offsets *from the start of the mapping view*.
//! * `InternalsPlugin.hpp` structs are `pack(4)` (doubles only 4-aligned); the `SharedMemory*` wrappers
//!   use default packing, hence the 4-byte hole at absolute 2180 (A-LMU-007).

/// Location of one field inside its parent struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Field {
    /// Byte offset from the start of the parent struct.
    pub offset: usize,
    /// Size of the whole field in bytes (all elements for arrays).
    pub size: usize,
}

impl Field {
    /// Creates a field location.
    pub const fn new(offset: usize, size: usize) -> Self {
        Self { offset, size }
    }

    /// One past the last byte of the field.
    pub const fn end(&self) -> usize {
        self.offset + self.size
    }
}

/// Number of slots in `vehScoringInfo[]` (and the clamp applied to `mNumVehicles`).
pub const MAX_SCORING_VEHICLES: usize = 104;
/// Number of slots in `telemInfo[]` (and the clamp applied to `activeVehicles`).
pub const MAX_TELEMETRY_VEHICLES: usize = 104;
/// Capacity of `scoringStream[]` in bytes.
pub const SCORING_STREAM_CAPACITY: usize = 65536;
/// Maximum number of scoring-stream bytes we copy: one less than the capacity, because the SDK
/// helper NUL-terminates at `[scoringStreamSize]`, which is out of bounds at full capacity.
pub const SCORING_STREAM_COPY_MAX: usize = SCORING_STREAM_CAPACITY - 1;
/// Number of slots in the `generic.events[]` queue (`SME_MAX`).
pub const EVENT_SLOTS: usize = 16;
/// Length of each path in `SharedMemoryPathData` (`MAX_PATH`).
pub const PATH_LEN: usize = 260;
/// Number of wheels in `TelemInfoV01.mWheel[]` (FL, FR, RL, RR).
pub const WHEEL_COUNT: usize = 4;
/// FNV-1a 64 over every probed struct (name, sizeof) and field (name, offset, size, type), see
/// `layout_fingerprint` in `codegen/gen_layout.py`. Stored in `.lmuraw` captures so a capture is
/// never decoded with a layout that moved fields without changing `sizeof` (e.g. a re-probe).
pub const LAYOUT_FINGERPRINT: u64 = 0x90ac5f141c60a277;

/// Kernel object names (session-local namespace, no `Global\` prefix).
pub mod names {
    /// The data mapping, created by LMU; opened read-only (`FILE_MAP_READ`).
    pub const DATA_MAPPING: &str = "LMU_Data";
    /// Frame gate event 1, waited on first (opened with `SYNCHRONIZE` only).
    pub const HOLD_EVENT: &str = "LMU_Data_HoldEvent";
    /// Frame gate event 2, waited on second (opened with `SYNCHRONIZE` only).
    pub const DATA_EVENT: &str = "LMU_Data_DataEvent";
    /// The 8-byte lock-word mapping. Opened only if it already exists; never created or reset.
    pub const LOCK_MAPPING: &str = "LMU_SharedMemoryLockData";
    /// The auto-reset lock wake-up event.
    pub const LOCK_EVENT: &str = "LMU_SharedMemoryLockEvent";
}

/// `SharedMemoryEvent` codes found in `generic.events[]`.
pub mod sme {
    /// `SME_ENTER`.
    pub const ENTER: u32 = 0;
    /// `SME_EXIT`.
    pub const EXIT: u32 = 1;
    /// `SME_STARTUP`.
    pub const STARTUP: u32 = 2;
    /// `SME_SHUTDOWN`.
    pub const SHUTDOWN: u32 = 3;
    /// `SME_LOAD`.
    pub const LOAD: u32 = 4;
    /// `SME_UNLOAD`.
    pub const UNLOAD: u32 = 5;
    /// `SME_START_SESSION`.
    pub const START_SESSION: u32 = 6;
    /// `SME_END_SESSION`.
    pub const END_SESSION: u32 = 7;
    /// `SME_ENTER_REALTIME`.
    pub const ENTER_REALTIME: u32 = 8;
    /// `SME_EXIT_REALTIME`.
    pub const EXIT_REALTIME: u32 = 9;
    /// `SME_UPDATE_SCORING`.
    pub const UPDATE_SCORING: u32 = 10;
    /// `SME_UPDATE_TELEMETRY`.
    pub const UPDATE_TELEMETRY: u32 = 11;
    /// `SME_INIT_APPLICATION`.
    pub const INIT_APPLICATION: u32 = 12;
    /// `SME_UNINIT_APPLICATION`.
    pub const UNINIT_APPLICATION: u32 = 13;
    /// `SME_SET_ENVIRONMENT`.
    pub const SET_ENVIRONMENT: u32 = 14;
    /// `SME_FFB`.
    pub const FFB: u32 = 15;
    /// `SME_MAX`.
    pub const MAX: u32 = 16;
}

/// Raw values of the `IP_VehicleClass` enum (`TelemInfoV01.mVehicleClass`).
pub mod vehicle_class {
    /// `IP_VehicleClass::Hypercar`.
    pub const HYPERCAR: u8 = 0;
    /// `IP_VehicleClass::LMP2_ELMS`.
    pub const LMP2_ELMS: u8 = 2;
    /// `IP_VehicleClass::LMP2`.
    pub const LMP2: u8 = 3;
    /// `IP_VehicleClass::LMP3`.
    pub const LMP3: u8 = 4;
    /// `IP_VehicleClass::GTE`.
    pub const GTE: u8 = 5;
    /// `IP_VehicleClass::GT3`.
    pub const GT3: u8 = 6;
    /// `IP_VehicleClass::PaceCar`.
    pub const PACE_CAR: u8 = 8;
    /// `IP_VehicleClass::Unknown`.
    pub const UNKNOWN: u8 = 255;
}

/// Raw values of the `IP_VehicleChampionship` enum (`TelemInfoV01.mVehicleChampionship`).
pub mod vehicle_championship {
    /// `IP_VehicleChampionship::WEC_2023`.
    pub const WEC_2023: u8 = 0;
    /// `IP_VehicleChampionship::WEC_2024`.
    pub const WEC_2024: u8 = 1;
    /// `IP_VehicleChampionship::WEC_2025`.
    pub const WEC_2025: u8 = 2;
    /// `IP_VehicleChampionship::WEC_2026`.
    pub const WEC_2026: u8 = 3;
    /// `IP_VehicleChampionship::ELMS_2025`.
    pub const ELMS_2025: u8 = 16;
    /// `IP_VehicleChampionship::ELMS_2026`.
    pub const ELMS_2026: u8 = 17;
    /// `IP_VehicleChampionship::Unknown`.
    pub const UNKNOWN: u8 = 255;
}

/// `SharedMemoryLayout`: the whole `LMU_Data` view.
pub mod layout {
    use super::Field;

    /// `sizeof(SharedMemoryLayout)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 324824;
    /// `alignof(SharedMemoryLayout)`.
    pub const ALIGN: usize = 8;
    /// `data`: `struct SharedMemoryObjectOut`.
    pub const DATA: Field = Field::new(0, 324824);
}

/// `SharedMemoryObjectOut`: the four top-level blocks.
pub mod object_out {
    use super::Field;

    /// `sizeof(SharedMemoryObjectOut)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 324824;
    /// `alignof(SharedMemoryObjectOut)`.
    pub const ALIGN: usize = 8;
    /// `generic`: `struct SharedMemoryGeneric`.
    pub const GENERIC: Field = Field::new(0, 332);
    /// `paths`: `struct SharedMemoryPathData`.
    pub const PATHS: Field = Field::new(332, 1300);
    /// `scoring`: `struct SharedMemoryScoringData`.
    pub const SCORING: Field = Field::new(1632, 126832);
    /// `telemetry`: `struct SharedMemoryTelemetryData`.
    pub const TELEMETRY: Field = Field::new(128464, 196356);
}

/// `SharedMemoryGeneric`: event queue, game version, FFB, app state.
pub mod generic {
    use super::Field;

    /// `sizeof(SharedMemoryGeneric)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 332;
    /// `alignof(SharedMemoryGeneric)`.
    pub const ALIGN: usize = 4;
    /// `events`: `enum SharedMemoryEvent[16]`.
    pub const EVENTS: Field = Field::new(0, 64);
    /// `gameVersion`: `long`.
    pub const GAME_VERSION: Field = Field::new(64, 4);
    /// `FFBTorque`: `float`.
    pub const FFB_TORQUE: Field = Field::new(68, 4);
    /// `appInfo`: `struct ApplicationStateV01`.
    pub const APP_INFO: Field = Field::new(72, 260);
}

/// `ApplicationStateV01` (inside the generic block).
pub mod app_state {
    use super::Field;

    /// `sizeof(ApplicationStateV01)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 260;
    /// `alignof(ApplicationStateV01)`.
    pub const ALIGN: usize = 4;
    /// `mAppWindow`: `struct HWND__*`.
    pub const APP_WINDOW: Field = Field::new(0, 8);
    /// `mWidth`: `unsigned long`.
    pub const WIDTH: Field = Field::new(8, 4);
    /// `mHeight`: `unsigned long`.
    pub const HEIGHT: Field = Field::new(12, 4);
    /// `mRefreshRate`: `unsigned long`.
    pub const REFRESH_RATE: Field = Field::new(16, 4);
    /// `mWindowed`: `unsigned long`.
    pub const WINDOWED: Field = Field::new(20, 4);
    /// `mOptionsLocation`: `unsigned char`.
    pub const OPTIONS_LOCATION: Field = Field::new(24, 1);
    /// `mOptionsPage`: `char[31]`.
    pub const OPTIONS_PAGE: Field = Field::new(25, 31);
    /// `mExpansion`: `unsigned char[204]`.
    pub const EXPANSION: Field = Field::new(56, 204);
}

/// `SharedMemoryPathData`: five `char[260]` paths (PII).
pub mod paths {
    use super::Field;

    /// `sizeof(SharedMemoryPathData)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 1300;
    /// `alignof(SharedMemoryPathData)`.
    pub const ALIGN: usize = 1;
    /// `userData`: `char[260]`.
    pub const USER_DATA: Field = Field::new(0, 260);
    /// `customVariables`: `char[260]`.
    pub const CUSTOM_VARIABLES: Field = Field::new(260, 260);
    /// `stewardResults`: `char[260]`.
    pub const STEWARD_RESULTS: Field = Field::new(520, 260);
    /// `playerProfile`: `char[260]`.
    pub const PLAYER_PROFILE: Field = Field::new(780, 260);
    /// `pluginsFolder`: `char[260]`.
    pub const PLUGINS_FOLDER: Field = Field::new(1040, 260);
}

/// `SharedMemoryScoringData`: scoring wrapper block.
pub mod scoring_data {
    use super::Field;

    /// `sizeof(SharedMemoryScoringData)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 126832;
    /// `alignof(SharedMemoryScoringData)`.
    pub const ALIGN: usize = 8;
    /// `scoringInfo`: `struct ScoringInfoV01`.
    pub const SCORING_INFO: Field = Field::new(0, 548);
    /// `scoringStreamSize`: `unsigned long long`.
    pub const SCORING_STREAM_SIZE: Field = Field::new(552, 8);
    /// `vehScoringInfo`: `struct VehicleScoringInfoV01[104]`.
    pub const VEH_SCORING_INFO: Field = Field::new(560, 60736);
    /// `scoringStream`: `char[65536]`.
    pub const SCORING_STREAM: Field = Field::new(61296, 65536);
}

/// `ScoringInfoV01`: session-wide scoring info.
pub mod scoring_info {
    use super::Field;

    /// `sizeof(ScoringInfoV01)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 548;
    /// `alignof(ScoringInfoV01)`.
    pub const ALIGN: usize = 4;
    /// `mTrackName`: `char[64]`.
    pub const TRACK_NAME: Field = Field::new(0, 64);
    /// `mSession`: `long`.
    pub const SESSION: Field = Field::new(64, 4);
    /// `mCurrentET`: `double`.
    pub const CURRENT_ET: Field = Field::new(68, 8);
    /// `mEndET`: `double`.
    pub const END_ET: Field = Field::new(76, 8);
    /// `mMaxLaps`: `long`.
    pub const MAX_LAPS: Field = Field::new(84, 4);
    /// `mLapDist`: `double`.
    pub const LAP_DIST: Field = Field::new(88, 8);
    /// `mResultsStream`: `char*`.
    pub const RESULTS_STREAM: Field = Field::new(96, 8);
    /// `mNumVehicles`: `long`.
    pub const NUM_VEHICLES: Field = Field::new(104, 4);
    /// `mGamePhase`: `unsigned char`.
    pub const GAME_PHASE: Field = Field::new(108, 1);
    /// `mYellowFlagState`: `signed char`.
    pub const YELLOW_FLAG_STATE: Field = Field::new(109, 1);
    /// `mSectorFlag`: `signed char[3]`.
    pub const SECTOR_FLAG: Field = Field::new(110, 3);
    /// `mStartLight`: `unsigned char`.
    pub const START_LIGHT: Field = Field::new(113, 1);
    /// `mNumRedLights`: `unsigned char`.
    pub const NUM_RED_LIGHTS: Field = Field::new(114, 1);
    /// `mInRealtime`: `bool`.
    pub const IN_REALTIME: Field = Field::new(115, 1);
    /// `mPlayerName`: `char[32]`.
    pub const PLAYER_NAME: Field = Field::new(116, 32);
    /// `mPlrFileName`: `char[64]`.
    pub const PLR_FILE_NAME: Field = Field::new(148, 64);
    /// `mDarkCloud`: `double`.
    pub const DARK_CLOUD: Field = Field::new(212, 8);
    /// `mRaining`: `double`.
    pub const RAINING: Field = Field::new(220, 8);
    /// `mAmbientTemp`: `double`.
    pub const AMBIENT_TEMP: Field = Field::new(228, 8);
    /// `mTrackTemp`: `double`.
    pub const TRACK_TEMP: Field = Field::new(236, 8);
    /// `mWind`: `struct TelemVect3`.
    pub const WIND: Field = Field::new(244, 24);
    /// `mMinPathWetness`: `double`.
    pub const MIN_PATH_WETNESS: Field = Field::new(268, 8);
    /// `mMaxPathWetness`: `double`.
    pub const MAX_PATH_WETNESS: Field = Field::new(276, 8);
    /// `mGameMode`: `unsigned char`.
    pub const GAME_MODE: Field = Field::new(284, 1);
    /// `mIsPasswordProtected`: `bool`.
    pub const IS_PASSWORD_PROTECTED: Field = Field::new(285, 1);
    /// `mServerPort`: `unsigned short`.
    pub const SERVER_PORT: Field = Field::new(286, 2);
    /// `mServerPublicIP`: `unsigned long`.
    pub const SERVER_PUBLIC_IP: Field = Field::new(288, 4);
    /// `mMaxPlayers`: `long`.
    pub const MAX_PLAYERS: Field = Field::new(292, 4);
    /// `mServerName`: `char[32]`.
    pub const SERVER_NAME: Field = Field::new(296, 32);
    /// `mStartET`: `float`.
    pub const START_ET: Field = Field::new(328, 4);
    /// `mAvgPathWetness`: `double`.
    pub const AVG_PATH_WETNESS: Field = Field::new(332, 8);
    /// `mSessionTimeRemaining`: `float`.
    pub const SESSION_TIME_REMAINING: Field = Field::new(340, 4);
    /// `mTimeOfDay`: `float`.
    pub const TIME_OF_DAY: Field = Field::new(344, 4);
    /// `mIsFixedSetup`: `bool`.
    pub const IS_FIXED_SETUP: Field = Field::new(348, 1);
    /// `mTrackGripLevel`: `unsigned char`.
    pub const TRACK_GRIP_LEVEL: Field = Field::new(349, 1);
    /// `mCloudCoverage`: `unsigned char`.
    pub const CLOUD_COVERAGE: Field = Field::new(350, 1);
    /// `mTrackLimitsStepsPerPenalty`: `unsigned char`.
    pub const TRACK_LIMITS_STEPS_PER_PENALTY: Field = Field::new(351, 1);
    /// `mTrackLimitsStepsPerPoint`: `unsigned char`.
    pub const TRACK_LIMITS_STEPS_PER_POINT: Field = Field::new(352, 1);
    /// `mExpansion`: `unsigned char[187]`.
    pub const EXPANSION: Field = Field::new(353, 187);
    /// `mVehicle`: `struct VehicleScoringInfoV01*`.
    pub const VEHICLE: Field = Field::new(540, 8);
    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.
    /// The runtime layout checks flag any other value (a hint that fields have moved).
    pub const BOOL_FIELDS: &[(&str, Field)] = &[
        ("mInRealtime", IN_REALTIME),
        ("mIsPasswordProtected", IS_PASSWORD_PROTECTED),
        ("mIsFixedSetup", IS_FIXED_SETUP),
    ];
}

/// `VehicleScoringInfoV01`: one scoring row per car.
pub mod veh_scoring {
    use super::Field;

    /// `sizeof(VehicleScoringInfoV01)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 584;
    /// `alignof(VehicleScoringInfoV01)`.
    pub const ALIGN: usize = 4;
    /// `mID`: `long`.
    pub const ID: Field = Field::new(0, 4);
    /// `mDriverName`: `char[32]`.
    pub const DRIVER_NAME: Field = Field::new(4, 32);
    /// `mVehicleName`: `char[64]`.
    pub const VEHICLE_NAME: Field = Field::new(36, 64);
    /// `mTotalLaps`: `short`.
    pub const TOTAL_LAPS: Field = Field::new(100, 2);
    /// `mSector`: `signed char`.
    pub const SECTOR: Field = Field::new(102, 1);
    /// `mFinishStatus`: `signed char`.
    pub const FINISH_STATUS: Field = Field::new(103, 1);
    /// `mLapDist`: `double`.
    pub const LAP_DIST: Field = Field::new(104, 8);
    /// `mPathLateral`: `double`.
    pub const PATH_LATERAL: Field = Field::new(112, 8);
    /// `mTrackEdge`: `double`.
    pub const TRACK_EDGE: Field = Field::new(120, 8);
    /// `mBestSector1`: `double`.
    pub const BEST_SECTOR1: Field = Field::new(128, 8);
    /// `mBestSector2`: `double`.
    pub const BEST_SECTOR2: Field = Field::new(136, 8);
    /// `mBestLapTime`: `double`.
    pub const BEST_LAP_TIME: Field = Field::new(144, 8);
    /// `mLastSector1`: `double`.
    pub const LAST_SECTOR1: Field = Field::new(152, 8);
    /// `mLastSector2`: `double`.
    pub const LAST_SECTOR2: Field = Field::new(160, 8);
    /// `mLastLapTime`: `double`.
    pub const LAST_LAP_TIME: Field = Field::new(168, 8);
    /// `mCurSector1`: `double`.
    pub const CUR_SECTOR1: Field = Field::new(176, 8);
    /// `mCurSector2`: `double`.
    pub const CUR_SECTOR2: Field = Field::new(184, 8);
    /// `mNumPitstops`: `short`.
    pub const NUM_PITSTOPS: Field = Field::new(192, 2);
    /// `mNumPenalties`: `short`.
    pub const NUM_PENALTIES: Field = Field::new(194, 2);
    /// `mIsPlayer`: `bool`.
    pub const IS_PLAYER: Field = Field::new(196, 1);
    /// `mControl`: `signed char`.
    pub const CONTROL: Field = Field::new(197, 1);
    /// `mInPits`: `bool`.
    pub const IN_PITS: Field = Field::new(198, 1);
    /// `mPlace`: `unsigned char`.
    pub const PLACE: Field = Field::new(199, 1);
    /// `mVehicleClass`: `char[32]`.
    pub const VEHICLE_CLASS: Field = Field::new(200, 32);
    /// `mTimeBehindNext`: `double`.
    pub const TIME_BEHIND_NEXT: Field = Field::new(232, 8);
    /// `mLapsBehindNext`: `long`.
    pub const LAPS_BEHIND_NEXT: Field = Field::new(240, 4);
    /// `mTimeBehindLeader`: `double`.
    pub const TIME_BEHIND_LEADER: Field = Field::new(244, 8);
    /// `mLapsBehindLeader`: `long`.
    pub const LAPS_BEHIND_LEADER: Field = Field::new(252, 4);
    /// `mLapStartET`: `double`.
    pub const LAP_START_ET: Field = Field::new(256, 8);
    /// `mPos`: `struct TelemVect3`.
    pub const POS: Field = Field::new(264, 24);
    /// `mLocalVel`: `struct TelemVect3`.
    pub const LOCAL_VEL: Field = Field::new(288, 24);
    /// `mLocalAccel`: `struct TelemVect3`.
    pub const LOCAL_ACCEL: Field = Field::new(312, 24);
    /// `mOri`: `struct TelemVect3[3]`.
    pub const ORI: Field = Field::new(336, 72);
    /// `mLocalRot`: `struct TelemVect3`.
    pub const LOCAL_ROT: Field = Field::new(408, 24);
    /// `mLocalRotAccel`: `struct TelemVect3`.
    pub const LOCAL_ROT_ACCEL: Field = Field::new(432, 24);
    /// `mHeadlights`: `unsigned char`.
    pub const HEADLIGHTS: Field = Field::new(456, 1);
    /// `mPitState`: `unsigned char`.
    pub const PIT_STATE: Field = Field::new(457, 1);
    /// `mServerScored`: `unsigned char`.
    pub const SERVER_SCORED: Field = Field::new(458, 1);
    /// `mIndividualPhase`: `unsigned char`.
    pub const INDIVIDUAL_PHASE: Field = Field::new(459, 1);
    /// `mQualification`: `long`.
    pub const QUALIFICATION: Field = Field::new(460, 4);
    /// `mTimeIntoLap`: `double`.
    pub const TIME_INTO_LAP: Field = Field::new(464, 8);
    /// `mEstimatedLapTime`: `double`.
    pub const ESTIMATED_LAP_TIME: Field = Field::new(472, 8);
    /// `mPitGroup`: `char[24]`.
    pub const PIT_GROUP: Field = Field::new(480, 24);
    /// `mFlag`: `unsigned char`.
    pub const FLAG: Field = Field::new(504, 1);
    /// `mUnderYellow`: `bool`.
    pub const UNDER_YELLOW: Field = Field::new(505, 1);
    /// `mCountLapFlag`: `unsigned char`.
    pub const COUNT_LAP_FLAG: Field = Field::new(506, 1);
    /// `mInGarageStall`: `bool`.
    pub const IN_GARAGE_STALL: Field = Field::new(507, 1);
    /// `mUpgradePack`: `unsigned char[16]`.
    pub const UPGRADE_PACK: Field = Field::new(508, 16);
    /// `mPitLapDist`: `float`.
    pub const PIT_LAP_DIST: Field = Field::new(524, 4);
    /// `mBestLapSector1`: `float`.
    pub const BEST_LAP_SECTOR1: Field = Field::new(528, 4);
    /// `mBestLapSector2`: `float`.
    pub const BEST_LAP_SECTOR2: Field = Field::new(532, 4);
    /// `mSteamID`: `unsigned long long`.
    pub const STEAM_ID: Field = Field::new(536, 8);
    /// `mVehFilename`: `char[32]`.
    pub const VEH_FILENAME: Field = Field::new(544, 32);
    /// `mAttackMode`: `short`.
    pub const ATTACK_MODE: Field = Field::new(576, 2);
    /// `mFuelFraction`: `unsigned char`.
    pub const FUEL_FRACTION: Field = Field::new(578, 1);
    /// `mDRSState`: `bool`.
    pub const DRS_STATE: Field = Field::new(579, 1);
    /// `mExpansion`: `unsigned char[4]`.
    pub const EXPANSION: Field = Field::new(580, 4);
    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.
    /// The runtime layout checks flag any other value (a hint that fields have moved).
    pub const BOOL_FIELDS: &[(&str, Field)] = &[
        ("mIsPlayer", IS_PLAYER),
        ("mInPits", IN_PITS),
        ("mUnderYellow", UNDER_YELLOW),
        ("mInGarageStall", IN_GARAGE_STALL),
        ("mDRSState", DRS_STATE),
    ];
}

/// `SharedMemoryTelemetryData`: telemetry wrapper block.
pub mod telemetry_data {
    use super::Field;

    /// `sizeof(SharedMemoryTelemetryData)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 196356;
    /// `alignof(SharedMemoryTelemetryData)`.
    pub const ALIGN: usize = 4;
    /// `activeVehicles`: `unsigned char`.
    pub const ACTIVE_VEHICLES: Field = Field::new(0, 1);
    /// `playerVehicleIdx`: `unsigned char`.
    pub const PLAYER_VEHICLE_IDX: Field = Field::new(1, 1);
    /// `playerHasVehicle`: `bool`.
    pub const PLAYER_HAS_VEHICLE: Field = Field::new(2, 1);
    /// `telemInfo`: `struct TelemInfoV01[104]`.
    pub const TELEM_INFO: Field = Field::new(4, 196352);
    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.
    /// The runtime layout checks flag any other value (a hint that fields have moved).
    pub const BOOL_FIELDS: &[(&str, Field)] = &[("playerHasVehicle", PLAYER_HAS_VEHICLE)];
}

/// `TelemInfoV01`: one telemetry slot.
pub mod telem_info {
    use super::Field;

    /// `sizeof(TelemInfoV01)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 1888;
    /// `alignof(TelemInfoV01)`.
    pub const ALIGN: usize = 4;
    /// `mID`: `long`.
    pub const ID: Field = Field::new(0, 4);
    /// `mDeltaTime`: `double`.
    pub const DELTA_TIME: Field = Field::new(4, 8);
    /// `mElapsedTime`: `double`.
    pub const ELAPSED_TIME: Field = Field::new(12, 8);
    /// `mLapNumber`: `long`.
    pub const LAP_NUMBER: Field = Field::new(20, 4);
    /// `mLapStartET`: `double`.
    pub const LAP_START_ET: Field = Field::new(24, 8);
    /// `mVehicleName`: `char[64]`.
    pub const VEHICLE_NAME: Field = Field::new(32, 64);
    /// `mTrackName`: `char[64]`.
    pub const TRACK_NAME: Field = Field::new(96, 64);
    /// `mPos`: `struct TelemVect3`.
    pub const POS: Field = Field::new(160, 24);
    /// `mLocalVel`: `struct TelemVect3`.
    pub const LOCAL_VEL: Field = Field::new(184, 24);
    /// `mLocalAccel`: `struct TelemVect3`.
    pub const LOCAL_ACCEL: Field = Field::new(208, 24);
    /// `mOri`: `struct TelemVect3[3]`.
    pub const ORI: Field = Field::new(232, 72);
    /// `mLocalRot`: `struct TelemVect3`.
    pub const LOCAL_ROT: Field = Field::new(304, 24);
    /// `mLocalRotAccel`: `struct TelemVect3`.
    pub const LOCAL_ROT_ACCEL: Field = Field::new(328, 24);
    /// `mGear`: `long`.
    pub const GEAR: Field = Field::new(352, 4);
    /// `mEngineRPM`: `double`.
    pub const ENGINE_RPM: Field = Field::new(356, 8);
    /// `mEngineWaterTemp`: `double`.
    pub const ENGINE_WATER_TEMP: Field = Field::new(364, 8);
    /// `mEngineOilTemp`: `double`.
    pub const ENGINE_OIL_TEMP: Field = Field::new(372, 8);
    /// `mClutchRPM`: `double`.
    pub const CLUTCH_RPM: Field = Field::new(380, 8);
    /// `mUnfilteredThrottle`: `double`.
    pub const UNFILTERED_THROTTLE: Field = Field::new(388, 8);
    /// `mUnfilteredBrake`: `double`.
    pub const UNFILTERED_BRAKE: Field = Field::new(396, 8);
    /// `mUnfilteredSteering`: `double`.
    pub const UNFILTERED_STEERING: Field = Field::new(404, 8);
    /// `mUnfilteredClutch`: `double`.
    pub const UNFILTERED_CLUTCH: Field = Field::new(412, 8);
    /// `mFilteredThrottle`: `double`.
    pub const FILTERED_THROTTLE: Field = Field::new(420, 8);
    /// `mFilteredBrake`: `double`.
    pub const FILTERED_BRAKE: Field = Field::new(428, 8);
    /// `mFilteredSteering`: `double`.
    pub const FILTERED_STEERING: Field = Field::new(436, 8);
    /// `mFilteredClutch`: `double`.
    pub const FILTERED_CLUTCH: Field = Field::new(444, 8);
    /// `mSteeringShaftTorque`: `double`.
    pub const STEERING_SHAFT_TORQUE: Field = Field::new(452, 8);
    /// `mFront3rdDeflection`: `double`.
    pub const FRONT3RD_DEFLECTION: Field = Field::new(460, 8);
    /// `mRear3rdDeflection`: `double`.
    pub const REAR3RD_DEFLECTION: Field = Field::new(468, 8);
    /// `mFrontWingHeight`: `double`.
    pub const FRONT_WING_HEIGHT: Field = Field::new(476, 8);
    /// `mFrontRideHeight`: `double`.
    pub const FRONT_RIDE_HEIGHT: Field = Field::new(484, 8);
    /// `mRearRideHeight`: `double`.
    pub const REAR_RIDE_HEIGHT: Field = Field::new(492, 8);
    /// `mDrag`: `double`.
    pub const DRAG: Field = Field::new(500, 8);
    /// `mFrontDownforce`: `double`.
    pub const FRONT_DOWNFORCE: Field = Field::new(508, 8);
    /// `mRearDownforce`: `double`.
    pub const REAR_DOWNFORCE: Field = Field::new(516, 8);
    /// `mFuel`: `double`.
    pub const FUEL: Field = Field::new(524, 8);
    /// `mEngineMaxRPM`: `double`.
    pub const ENGINE_MAX_RPM: Field = Field::new(532, 8);
    /// `mScheduledStops`: `unsigned char`.
    pub const SCHEDULED_STOPS: Field = Field::new(540, 1);
    /// `mOverheating`: `bool`.
    pub const OVERHEATING: Field = Field::new(541, 1);
    /// `mDetached`: `bool`.
    pub const DETACHED: Field = Field::new(542, 1);
    /// `mHeadlights`: `bool`.
    pub const HEADLIGHTS: Field = Field::new(543, 1);
    /// `mDentSeverity`: `unsigned char[8]`.
    pub const DENT_SEVERITY: Field = Field::new(544, 8);
    /// `mLastImpactET`: `double`.
    pub const LAST_IMPACT_ET: Field = Field::new(552, 8);
    /// `mLastImpactMagnitude`: `double`.
    pub const LAST_IMPACT_MAGNITUDE: Field = Field::new(560, 8);
    /// `mLastImpactPos`: `struct TelemVect3`.
    pub const LAST_IMPACT_POS: Field = Field::new(568, 24);
    /// `mEngineTorque`: `double`.
    pub const ENGINE_TORQUE: Field = Field::new(592, 8);
    /// `mCurrentSector`: `long`.
    pub const CURRENT_SECTOR: Field = Field::new(600, 4);
    /// `mSpeedLimiter`: `unsigned char`.
    pub const SPEED_LIMITER: Field = Field::new(604, 1);
    /// `mMaxGears`: `unsigned char`.
    pub const MAX_GEARS: Field = Field::new(605, 1);
    /// `mFrontTireCompoundIndex`: `unsigned char`.
    pub const FRONT_TIRE_COMPOUND_INDEX: Field = Field::new(606, 1);
    /// `mRearTireCompoundIndex`: `unsigned char`.
    pub const REAR_TIRE_COMPOUND_INDEX: Field = Field::new(607, 1);
    /// `mFuelCapacity`: `double`.
    pub const FUEL_CAPACITY: Field = Field::new(608, 8);
    /// `mFrontFlapActivated`: `unsigned char`.
    pub const FRONT_FLAP_ACTIVATED: Field = Field::new(616, 1);
    /// `mRearFlapActivated`: `unsigned char`.
    pub const REAR_FLAP_ACTIVATED: Field = Field::new(617, 1);
    /// `mRearFlapLegalStatus`: `unsigned char`.
    pub const REAR_FLAP_LEGAL_STATUS: Field = Field::new(618, 1);
    /// `mIgnitionStarter`: `unsigned char`.
    pub const IGNITION_STARTER: Field = Field::new(619, 1);
    /// `mFrontTireCompoundName`: `char[18]`.
    pub const FRONT_TIRE_COMPOUND_NAME: Field = Field::new(620, 18);
    /// `mRearTireCompoundName`: `char[18]`.
    pub const REAR_TIRE_COMPOUND_NAME: Field = Field::new(638, 18);
    /// `mSpeedLimiterAvailable`: `unsigned char`.
    pub const SPEED_LIMITER_AVAILABLE: Field = Field::new(656, 1);
    /// `mAntiStallActivated`: `unsigned char`.
    pub const ANTI_STALL_ACTIVATED: Field = Field::new(657, 1);
    /// `mUnused`: `unsigned char[2]`.
    pub const UNUSED: Field = Field::new(658, 2);
    /// `mVisualSteeringWheelRange`: `float`.
    pub const VISUAL_STEERING_WHEEL_RANGE: Field = Field::new(660, 4);
    /// `mRearBrakeBias`: `double`.
    pub const REAR_BRAKE_BIAS: Field = Field::new(664, 8);
    /// `mTurboBoostPressure`: `double`.
    pub const TURBO_BOOST_PRESSURE: Field = Field::new(672, 8);
    /// `mPhysicsToGraphicsOffset`: `float[3]`.
    pub const PHYSICS_TO_GRAPHICS_OFFSET: Field = Field::new(680, 12);
    /// `mPhysicalSteeringWheelRange`: `float`.
    pub const PHYSICAL_STEERING_WHEEL_RANGE: Field = Field::new(692, 4);
    /// `mDeltaBest`: `double`.
    pub const DELTA_BEST: Field = Field::new(696, 8);
    /// `mBatteryChargeFraction`: `double`.
    pub const BATTERY_CHARGE_FRACTION: Field = Field::new(704, 8);
    /// `mElectricBoostMotorTorque`: `double`.
    pub const ELECTRIC_BOOST_MOTOR_TORQUE: Field = Field::new(712, 8);
    /// `mElectricBoostMotorRPM`: `double`.
    pub const ELECTRIC_BOOST_MOTOR_RPM: Field = Field::new(720, 8);
    /// `mElectricBoostMotorTemperature`: `double`.
    pub const ELECTRIC_BOOST_MOTOR_TEMPERATURE: Field = Field::new(728, 8);
    /// `mElectricBoostWaterTemperature`: `double`.
    pub const ELECTRIC_BOOST_WATER_TEMPERATURE: Field = Field::new(736, 8);
    /// `mElectricBoostMotorState`: `unsigned char`.
    pub const ELECTRIC_BOOST_MOTOR_STATE: Field = Field::new(744, 1);
    /// `mLapInvalidated`: `bool`.
    pub const LAP_INVALIDATED: Field = Field::new(745, 1);
    /// `mABSActive`: `bool`.
    pub const ABS_ACTIVE: Field = Field::new(746, 1);
    /// `mTCActive`: `bool`.
    pub const TC_ACTIVE: Field = Field::new(747, 1);
    /// `mSpeedLimiterActive`: `bool`.
    pub const SPEED_LIMITER_ACTIVE: Field = Field::new(748, 1);
    /// `mWiperState`: `unsigned char`.
    pub const WIPER_STATE: Field = Field::new(749, 1);
    /// `mTC`: `unsigned char`.
    pub const TC: Field = Field::new(750, 1);
    /// `mTCMax`: `unsigned char`.
    pub const TC_MAX: Field = Field::new(751, 1);
    /// `mTCSlip`: `unsigned char`.
    pub const TC_SLIP: Field = Field::new(752, 1);
    /// `mTCSlipMax`: `unsigned char`.
    pub const TC_SLIP_MAX: Field = Field::new(753, 1);
    /// `mTCCut`: `unsigned char`.
    pub const TC_CUT: Field = Field::new(754, 1);
    /// `mTCCutMax`: `unsigned char`.
    pub const TC_CUT_MAX: Field = Field::new(755, 1);
    /// `mABS`: `unsigned char`.
    pub const ABS: Field = Field::new(756, 1);
    /// `mABSMax`: `unsigned char`.
    pub const ABS_MAX: Field = Field::new(757, 1);
    /// `mMotorMap`: `unsigned char`.
    pub const MOTOR_MAP: Field = Field::new(758, 1);
    /// `mMotorMapMax`: `unsigned char`.
    pub const MOTOR_MAP_MAX: Field = Field::new(759, 1);
    /// `mMigration`: `unsigned char`.
    pub const MIGRATION: Field = Field::new(760, 1);
    /// `mMigrationMax`: `unsigned char`.
    pub const MIGRATION_MAX: Field = Field::new(761, 1);
    /// `mFrontAntiSway`: `unsigned char`.
    pub const FRONT_ANTI_SWAY: Field = Field::new(762, 1);
    /// `mFrontAntiSwayMax`: `unsigned char`.
    pub const FRONT_ANTI_SWAY_MAX: Field = Field::new(763, 1);
    /// `mRearAntiSway`: `unsigned char`.
    pub const REAR_ANTI_SWAY: Field = Field::new(764, 1);
    /// `mRearAntiSwayMax`: `unsigned char`.
    pub const REAR_ANTI_SWAY_MAX: Field = Field::new(765, 1);
    /// `mLiftAndCoastProgress`: `unsigned char`.
    pub const LIFT_AND_COAST_PROGRESS: Field = Field::new(766, 1);
    /// `mTrackLimitsSteps`: `unsigned char`.
    pub const TRACK_LIMITS_STEPS: Field = Field::new(767, 1);
    /// `mRegen`: `float`.
    pub const REGEN: Field = Field::new(768, 4);
    /// `mSoC`: `float`.
    pub const SOC: Field = Field::new(772, 4);
    /// `mVirtualEnergy`: `float`.
    pub const VIRTUAL_ENERGY: Field = Field::new(776, 4);
    /// `mTimeGapCarAhead`: `float`.
    pub const TIME_GAP_CAR_AHEAD: Field = Field::new(780, 4);
    /// `mTimeGapCarBehind`: `float`.
    pub const TIME_GAP_CAR_BEHIND: Field = Field::new(784, 4);
    /// `mTimeGapPlaceAhead`: `float`.
    pub const TIME_GAP_PLACE_AHEAD: Field = Field::new(788, 4);
    /// `mTimeGapPlaceBehind`: `float`.
    pub const TIME_GAP_PLACE_BEHIND: Field = Field::new(792, 4);
    /// `mVehicleModel`: `char[30]`.
    pub const VEHICLE_MODEL: Field = Field::new(796, 30);
    /// `mVehicleClass`: `enum IP_VehicleClass`.
    pub const VEHICLE_CLASS: Field = Field::new(826, 1);
    /// `mVehicleChampionship`: `enum IP_VehicleChampionship`.
    pub const VEHICLE_CHAMPIONSHIP: Field = Field::new(827, 1);
    /// `mExpansion`: `unsigned char[20]`.
    pub const EXPANSION: Field = Field::new(828, 20);
    /// `mWheel`: `struct TelemWheelV01[4]`.
    pub const WHEEL: Field = Field::new(848, 1040);
    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.
    /// The runtime layout checks flag any other value (a hint that fields have moved).
    pub const BOOL_FIELDS: &[(&str, Field)] = &[
        ("mOverheating", OVERHEATING),
        ("mDetached", DETACHED),
        ("mHeadlights", HEADLIGHTS),
        ("mLapInvalidated", LAP_INVALIDATED),
        ("mABSActive", ABS_ACTIVE),
        ("mTCActive", TC_ACTIVE),
        ("mSpeedLimiterActive", SPEED_LIMITER_ACTIVE),
    ];
}

/// `TelemWheelV01`: one wheel inside a telemetry slot.
pub mod telem_wheel {
    use super::Field;

    /// `sizeof(TelemWheelV01)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 260;
    /// `alignof(TelemWheelV01)`.
    pub const ALIGN: usize = 4;
    /// `mSuspensionDeflection`: `double`.
    pub const SUSPENSION_DEFLECTION: Field = Field::new(0, 8);
    /// `mRideHeight`: `double`.
    pub const RIDE_HEIGHT: Field = Field::new(8, 8);
    /// `mSuspForce`: `double`.
    pub const SUSP_FORCE: Field = Field::new(16, 8);
    /// `mBrakeTemp`: `double`.
    pub const BRAKE_TEMP: Field = Field::new(24, 8);
    /// `mBrakePressure`: `double`.
    pub const BRAKE_PRESSURE: Field = Field::new(32, 8);
    /// `mRotation`: `double`.
    pub const ROTATION: Field = Field::new(40, 8);
    /// `mLateralPatchVel`: `double`.
    pub const LATERAL_PATCH_VEL: Field = Field::new(48, 8);
    /// `mLongitudinalPatchVel`: `double`.
    pub const LONGITUDINAL_PATCH_VEL: Field = Field::new(56, 8);
    /// `mLateralGroundVel`: `double`.
    pub const LATERAL_GROUND_VEL: Field = Field::new(64, 8);
    /// `mLongitudinalGroundVel`: `double`.
    pub const LONGITUDINAL_GROUND_VEL: Field = Field::new(72, 8);
    /// `mCamber`: `double`.
    pub const CAMBER: Field = Field::new(80, 8);
    /// `mLateralForce`: `double`.
    pub const LATERAL_FORCE: Field = Field::new(88, 8);
    /// `mLongitudinalForce`: `double`.
    pub const LONGITUDINAL_FORCE: Field = Field::new(96, 8);
    /// `mTireLoad`: `double`.
    pub const TIRE_LOAD: Field = Field::new(104, 8);
    /// `mGripFract`: `double`.
    pub const GRIP_FRACT: Field = Field::new(112, 8);
    /// `mPressure`: `double`.
    pub const PRESSURE: Field = Field::new(120, 8);
    /// `mTemperature`: `double[3]`.
    pub const TEMPERATURE: Field = Field::new(128, 24);
    /// `mWear`: `double`.
    pub const WEAR: Field = Field::new(152, 8);
    /// `mTerrainName`: `char[16]`.
    pub const TERRAIN_NAME: Field = Field::new(160, 16);
    /// `mSurfaceType`: `unsigned char`.
    pub const SURFACE_TYPE: Field = Field::new(176, 1);
    /// `mFlat`: `bool`.
    pub const FLAT: Field = Field::new(177, 1);
    /// `mDetached`: `bool`.
    pub const DETACHED: Field = Field::new(178, 1);
    /// `mStaticUndeflectedRadius`: `unsigned char`.
    pub const STATIC_UNDEFLECTED_RADIUS: Field = Field::new(179, 1);
    /// `mVerticalTireDeflection`: `double`.
    pub const VERTICAL_TIRE_DEFLECTION: Field = Field::new(180, 8);
    /// `mWheelYLocation`: `double`.
    pub const WHEEL_Y_LOCATION: Field = Field::new(188, 8);
    /// `mToe`: `double`.
    pub const TOE: Field = Field::new(196, 8);
    /// `mTireCarcassTemperature`: `double`.
    pub const TIRE_CARCASS_TEMPERATURE: Field = Field::new(204, 8);
    /// `mTireInnerLayerTemperature`: `double[3]`.
    pub const TIRE_INNER_LAYER_TEMPERATURE: Field = Field::new(212, 24);
    /// `mOptimalTemp`: `float`.
    pub const OPTIMAL_TEMP: Field = Field::new(236, 4);
    /// `mCompoundIndex`: `unsigned char`.
    pub const COMPOUND_INDEX: Field = Field::new(240, 1);
    /// `mCompoundType`: `unsigned char`.
    pub const COMPOUND_TYPE: Field = Field::new(241, 1);
    /// `mExpansion`: `unsigned char[18]`.
    pub const EXPANSION: Field = Field::new(242, 18);
    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.
    /// The runtime layout checks flag any other value (a hint that fields have moved).
    pub const BOOL_FIELDS: &[(&str, Field)] = &[("mFlat", FLAT), ("mDetached", DETACHED)];
}

/// `TelemVect3`: three `f64` components.
pub mod vect3 {
    use super::Field;

    /// `sizeof(TelemVect3)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 24;
    /// `alignof(TelemVect3)`.
    pub const ALIGN: usize = 4;
    /// `x`: `double`.
    pub const X: Field = Field::new(0, 8);
    /// `y`: `double`.
    pub const Y: Field = Field::new(8, 8);
    /// `z`: `double`.
    pub const Z: Field = Field::new(16, 8);
    /// `data`: `double[3]` (union alternative view).
    pub const DATA: Field = Field::new(0, 24);
}

/// `SharedMemoryLock::LockData`: the two lock words.
pub mod lock_data {
    use super::Field;

    /// `sizeof(SharedMemoryLock::LockData)`; also the array stride where the struct is used in an array.
    pub const SIZE: usize = 8;
    /// `alignof(SharedMemoryLock::LockData)`.
    pub const ALIGN: usize = 4;
    /// `waiters`: `volatile long`.
    pub const WAITERS: Field = Field::new(0, 4);
    /// `busy`: `volatile long`.
    pub const BUSY: Field = Field::new(4, 4);
}

/// Absolute offsets from the start of the `LMU_Data` view (composed from the per-struct constants).
pub mod abs {
    use super::{
        Field, MAX_SCORING_VEHICLES, MAX_TELEMETRY_VEHICLES, object_out, scoring_data,
        scoring_info, telem_info, telemetry_data, veh_scoring,
    };

    /// Start of `data.generic`.
    pub const GENERIC: usize = object_out::GENERIC.offset;
    /// Start of `data.paths`.
    pub const PATHS: usize = object_out::PATHS.offset;
    /// Start of `data.scoring`.
    pub const SCORING: usize = object_out::SCORING.offset;
    /// Start of `data.scoring.scoringInfo`.
    pub const SCORING_INFO: usize = SCORING + scoring_data::SCORING_INFO.offset;
    /// The default-pack hole between `scoringInfo` and `scoringStreamSize`; must read as zero.
    pub const SCORING_PADDING: Field = Field::new(
        SCORING_INFO + scoring_info::SIZE,
        scoring_data::SCORING_STREAM_SIZE.offset
            - (scoring_data::SCORING_INFO.offset + scoring_info::SIZE),
    );
    /// `data.scoring.scoringStreamSize` (u64).
    pub const SCORING_STREAM_SIZE: usize = SCORING + scoring_data::SCORING_STREAM_SIZE.offset;
    /// Start of `data.scoring.vehScoringInfo[0]`.
    pub const VEH_SCORING_INFO: usize = SCORING + scoring_data::VEH_SCORING_INFO.offset;
    /// Start of `data.scoring.scoringStream`.
    pub const SCORING_STREAM: usize = SCORING + scoring_data::SCORING_STREAM.offset;
    /// Start of `data.telemetry`.
    pub const TELEMETRY: usize = object_out::TELEMETRY.offset;
    /// `data.telemetry.activeVehicles` (u8).
    pub const ACTIVE_VEHICLES: usize = TELEMETRY + telemetry_data::ACTIVE_VEHICLES.offset;
    /// `data.telemetry.playerVehicleIdx` (u8).
    pub const PLAYER_VEHICLE_IDX: usize = TELEMETRY + telemetry_data::PLAYER_VEHICLE_IDX.offset;
    /// `data.telemetry.playerHasVehicle` (bool).
    pub const PLAYER_HAS_VEHICLE: usize = TELEMETRY + telemetry_data::PLAYER_HAS_VEHICLE.offset;
    /// Start of `data.telemetry.telemInfo[0]`.
    pub const TELEM_INFO: usize = TELEMETRY + telemetry_data::TELEM_INFO.offset;

    /// The block copied as the "telemetry header": `activeVehicles`, `playerVehicleIdx`,
    /// `playerHasVehicle` and the 1-byte hole before `telemInfo`.
    pub const TELEMETRY_HEADER: Field = Field::new(TELEMETRY, telemetry_data::TELEM_INFO.offset);
    /// The block copied as the "scoring header": `scoringInfo`, the padding hole and `scoringStreamSize`.
    pub const SCORING_HEADER: Field = Field::new(SCORING_INFO, VEH_SCORING_INFO - SCORING_INFO);

    /// End of the last member: the minimum mapping size that holds every field (324,820).
    /// `sizeof(SharedMemoryLayout)` adds 4 bytes of tail padding.
    pub const MAPPING_END: usize = TELEMETRY + telemetry_data::SIZE;

    /// Absolute start of `vehScoringInfo[index]`, or `None` when `index` is not a valid slot.
    pub const fn veh_scoring_info(index: usize) -> Option<usize> {
        if index < MAX_SCORING_VEHICLES {
            Some(VEH_SCORING_INFO + index * veh_scoring::SIZE)
        } else {
            None
        }
    }

    /// Absolute start of `telemInfo[index]`, or `None` when `index` is not a valid slot.
    pub const fn telem_info(index: usize) -> Option<usize> {
        if index < MAX_TELEMETRY_VEHICLES {
            Some(TELEM_INFO + index * telem_info::SIZE)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    //! Every constant above is compared with the probe JSON. A mismatch means the committed
    //! offsets are stale: re-run `tools/layout-probe` and `codegen/gen_layout.py`.
    use super::*;
    use serde_json::Value;
    use std::collections::BTreeSet;

    fn probe() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/lmu_abi_layout.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
    }

    fn as_usize(v: &Value, what: &str) -> usize {
        let n = v
            .as_u64()
            .unwrap_or_else(|| panic!("{what}: not an unsigned integer in the probe JSON: {v}"));
        usize::try_from(n).unwrap_or_else(|_| panic!("{what}: {n} does not fit usize"))
    }

    /// One struct module: its probe name, SIZE, ALIGN and every field constant.
    struct Entry {
        name: &'static str,
        size: usize,
        align: usize,
        fields: Vec<(&'static str, Field)>,
    }

    fn table() -> Vec<Entry> {
        vec![
            Entry {
                name: "SharedMemoryLayout",
                size: layout::SIZE,
                align: layout::ALIGN,
                fields: vec![("data", layout::DATA)],
            },
            Entry {
                name: "SharedMemoryObjectOut",
                size: object_out::SIZE,
                align: object_out::ALIGN,
                fields: vec![
                    ("generic", object_out::GENERIC),
                    ("paths", object_out::PATHS),
                    ("scoring", object_out::SCORING),
                    ("telemetry", object_out::TELEMETRY),
                ],
            },
            Entry {
                name: "SharedMemoryGeneric",
                size: generic::SIZE,
                align: generic::ALIGN,
                fields: vec![
                    ("events", generic::EVENTS),
                    ("gameVersion", generic::GAME_VERSION),
                    ("FFBTorque", generic::FFB_TORQUE),
                    ("appInfo", generic::APP_INFO),
                ],
            },
            Entry {
                name: "ApplicationStateV01",
                size: app_state::SIZE,
                align: app_state::ALIGN,
                fields: vec![
                    ("mAppWindow", app_state::APP_WINDOW),
                    ("mWidth", app_state::WIDTH),
                    ("mHeight", app_state::HEIGHT),
                    ("mRefreshRate", app_state::REFRESH_RATE),
                    ("mWindowed", app_state::WINDOWED),
                    ("mOptionsLocation", app_state::OPTIONS_LOCATION),
                    ("mOptionsPage", app_state::OPTIONS_PAGE),
                    ("mExpansion", app_state::EXPANSION),
                ],
            },
            Entry {
                name: "SharedMemoryPathData",
                size: paths::SIZE,
                align: paths::ALIGN,
                fields: vec![
                    ("userData", paths::USER_DATA),
                    ("customVariables", paths::CUSTOM_VARIABLES),
                    ("stewardResults", paths::STEWARD_RESULTS),
                    ("playerProfile", paths::PLAYER_PROFILE),
                    ("pluginsFolder", paths::PLUGINS_FOLDER),
                ],
            },
            Entry {
                name: "SharedMemoryScoringData",
                size: scoring_data::SIZE,
                align: scoring_data::ALIGN,
                fields: vec![
                    ("scoringInfo", scoring_data::SCORING_INFO),
                    ("scoringStreamSize", scoring_data::SCORING_STREAM_SIZE),
                    ("vehScoringInfo", scoring_data::VEH_SCORING_INFO),
                    ("scoringStream", scoring_data::SCORING_STREAM),
                ],
            },
            Entry {
                name: "ScoringInfoV01",
                size: scoring_info::SIZE,
                align: scoring_info::ALIGN,
                fields: vec![
                    ("mTrackName", scoring_info::TRACK_NAME),
                    ("mSession", scoring_info::SESSION),
                    ("mCurrentET", scoring_info::CURRENT_ET),
                    ("mEndET", scoring_info::END_ET),
                    ("mMaxLaps", scoring_info::MAX_LAPS),
                    ("mLapDist", scoring_info::LAP_DIST),
                    ("mResultsStream", scoring_info::RESULTS_STREAM),
                    ("mNumVehicles", scoring_info::NUM_VEHICLES),
                    ("mGamePhase", scoring_info::GAME_PHASE),
                    ("mYellowFlagState", scoring_info::YELLOW_FLAG_STATE),
                    ("mSectorFlag", scoring_info::SECTOR_FLAG),
                    ("mStartLight", scoring_info::START_LIGHT),
                    ("mNumRedLights", scoring_info::NUM_RED_LIGHTS),
                    ("mInRealtime", scoring_info::IN_REALTIME),
                    ("mPlayerName", scoring_info::PLAYER_NAME),
                    ("mPlrFileName", scoring_info::PLR_FILE_NAME),
                    ("mDarkCloud", scoring_info::DARK_CLOUD),
                    ("mRaining", scoring_info::RAINING),
                    ("mAmbientTemp", scoring_info::AMBIENT_TEMP),
                    ("mTrackTemp", scoring_info::TRACK_TEMP),
                    ("mWind", scoring_info::WIND),
                    ("mMinPathWetness", scoring_info::MIN_PATH_WETNESS),
                    ("mMaxPathWetness", scoring_info::MAX_PATH_WETNESS),
                    ("mGameMode", scoring_info::GAME_MODE),
                    ("mIsPasswordProtected", scoring_info::IS_PASSWORD_PROTECTED),
                    ("mServerPort", scoring_info::SERVER_PORT),
                    ("mServerPublicIP", scoring_info::SERVER_PUBLIC_IP),
                    ("mMaxPlayers", scoring_info::MAX_PLAYERS),
                    ("mServerName", scoring_info::SERVER_NAME),
                    ("mStartET", scoring_info::START_ET),
                    ("mAvgPathWetness", scoring_info::AVG_PATH_WETNESS),
                    (
                        "mSessionTimeRemaining",
                        scoring_info::SESSION_TIME_REMAINING,
                    ),
                    ("mTimeOfDay", scoring_info::TIME_OF_DAY),
                    ("mIsFixedSetup", scoring_info::IS_FIXED_SETUP),
                    ("mTrackGripLevel", scoring_info::TRACK_GRIP_LEVEL),
                    ("mCloudCoverage", scoring_info::CLOUD_COVERAGE),
                    (
                        "mTrackLimitsStepsPerPenalty",
                        scoring_info::TRACK_LIMITS_STEPS_PER_PENALTY,
                    ),
                    (
                        "mTrackLimitsStepsPerPoint",
                        scoring_info::TRACK_LIMITS_STEPS_PER_POINT,
                    ),
                    ("mExpansion", scoring_info::EXPANSION),
                    ("mVehicle", scoring_info::VEHICLE),
                ],
            },
            Entry {
                name: "VehicleScoringInfoV01",
                size: veh_scoring::SIZE,
                align: veh_scoring::ALIGN,
                fields: vec![
                    ("mID", veh_scoring::ID),
                    ("mDriverName", veh_scoring::DRIVER_NAME),
                    ("mVehicleName", veh_scoring::VEHICLE_NAME),
                    ("mTotalLaps", veh_scoring::TOTAL_LAPS),
                    ("mSector", veh_scoring::SECTOR),
                    ("mFinishStatus", veh_scoring::FINISH_STATUS),
                    ("mLapDist", veh_scoring::LAP_DIST),
                    ("mPathLateral", veh_scoring::PATH_LATERAL),
                    ("mTrackEdge", veh_scoring::TRACK_EDGE),
                    ("mBestSector1", veh_scoring::BEST_SECTOR1),
                    ("mBestSector2", veh_scoring::BEST_SECTOR2),
                    ("mBestLapTime", veh_scoring::BEST_LAP_TIME),
                    ("mLastSector1", veh_scoring::LAST_SECTOR1),
                    ("mLastSector2", veh_scoring::LAST_SECTOR2),
                    ("mLastLapTime", veh_scoring::LAST_LAP_TIME),
                    ("mCurSector1", veh_scoring::CUR_SECTOR1),
                    ("mCurSector2", veh_scoring::CUR_SECTOR2),
                    ("mNumPitstops", veh_scoring::NUM_PITSTOPS),
                    ("mNumPenalties", veh_scoring::NUM_PENALTIES),
                    ("mIsPlayer", veh_scoring::IS_PLAYER),
                    ("mControl", veh_scoring::CONTROL),
                    ("mInPits", veh_scoring::IN_PITS),
                    ("mPlace", veh_scoring::PLACE),
                    ("mVehicleClass", veh_scoring::VEHICLE_CLASS),
                    ("mTimeBehindNext", veh_scoring::TIME_BEHIND_NEXT),
                    ("mLapsBehindNext", veh_scoring::LAPS_BEHIND_NEXT),
                    ("mTimeBehindLeader", veh_scoring::TIME_BEHIND_LEADER),
                    ("mLapsBehindLeader", veh_scoring::LAPS_BEHIND_LEADER),
                    ("mLapStartET", veh_scoring::LAP_START_ET),
                    ("mPos", veh_scoring::POS),
                    ("mLocalVel", veh_scoring::LOCAL_VEL),
                    ("mLocalAccel", veh_scoring::LOCAL_ACCEL),
                    ("mOri", veh_scoring::ORI),
                    ("mLocalRot", veh_scoring::LOCAL_ROT),
                    ("mLocalRotAccel", veh_scoring::LOCAL_ROT_ACCEL),
                    ("mHeadlights", veh_scoring::HEADLIGHTS),
                    ("mPitState", veh_scoring::PIT_STATE),
                    ("mServerScored", veh_scoring::SERVER_SCORED),
                    ("mIndividualPhase", veh_scoring::INDIVIDUAL_PHASE),
                    ("mQualification", veh_scoring::QUALIFICATION),
                    ("mTimeIntoLap", veh_scoring::TIME_INTO_LAP),
                    ("mEstimatedLapTime", veh_scoring::ESTIMATED_LAP_TIME),
                    ("mPitGroup", veh_scoring::PIT_GROUP),
                    ("mFlag", veh_scoring::FLAG),
                    ("mUnderYellow", veh_scoring::UNDER_YELLOW),
                    ("mCountLapFlag", veh_scoring::COUNT_LAP_FLAG),
                    ("mInGarageStall", veh_scoring::IN_GARAGE_STALL),
                    ("mUpgradePack", veh_scoring::UPGRADE_PACK),
                    ("mPitLapDist", veh_scoring::PIT_LAP_DIST),
                    ("mBestLapSector1", veh_scoring::BEST_LAP_SECTOR1),
                    ("mBestLapSector2", veh_scoring::BEST_LAP_SECTOR2),
                    ("mSteamID", veh_scoring::STEAM_ID),
                    ("mVehFilename", veh_scoring::VEH_FILENAME),
                    ("mAttackMode", veh_scoring::ATTACK_MODE),
                    ("mFuelFraction", veh_scoring::FUEL_FRACTION),
                    ("mDRSState", veh_scoring::DRS_STATE),
                    ("mExpansion", veh_scoring::EXPANSION),
                ],
            },
            Entry {
                name: "SharedMemoryTelemetryData",
                size: telemetry_data::SIZE,
                align: telemetry_data::ALIGN,
                fields: vec![
                    ("activeVehicles", telemetry_data::ACTIVE_VEHICLES),
                    ("playerVehicleIdx", telemetry_data::PLAYER_VEHICLE_IDX),
                    ("playerHasVehicle", telemetry_data::PLAYER_HAS_VEHICLE),
                    ("telemInfo", telemetry_data::TELEM_INFO),
                ],
            },
            Entry {
                name: "TelemInfoV01",
                size: telem_info::SIZE,
                align: telem_info::ALIGN,
                fields: vec![
                    ("mID", telem_info::ID),
                    ("mDeltaTime", telem_info::DELTA_TIME),
                    ("mElapsedTime", telem_info::ELAPSED_TIME),
                    ("mLapNumber", telem_info::LAP_NUMBER),
                    ("mLapStartET", telem_info::LAP_START_ET),
                    ("mVehicleName", telem_info::VEHICLE_NAME),
                    ("mTrackName", telem_info::TRACK_NAME),
                    ("mPos", telem_info::POS),
                    ("mLocalVel", telem_info::LOCAL_VEL),
                    ("mLocalAccel", telem_info::LOCAL_ACCEL),
                    ("mOri", telem_info::ORI),
                    ("mLocalRot", telem_info::LOCAL_ROT),
                    ("mLocalRotAccel", telem_info::LOCAL_ROT_ACCEL),
                    ("mGear", telem_info::GEAR),
                    ("mEngineRPM", telem_info::ENGINE_RPM),
                    ("mEngineWaterTemp", telem_info::ENGINE_WATER_TEMP),
                    ("mEngineOilTemp", telem_info::ENGINE_OIL_TEMP),
                    ("mClutchRPM", telem_info::CLUTCH_RPM),
                    ("mUnfilteredThrottle", telem_info::UNFILTERED_THROTTLE),
                    ("mUnfilteredBrake", telem_info::UNFILTERED_BRAKE),
                    ("mUnfilteredSteering", telem_info::UNFILTERED_STEERING),
                    ("mUnfilteredClutch", telem_info::UNFILTERED_CLUTCH),
                    ("mFilteredThrottle", telem_info::FILTERED_THROTTLE),
                    ("mFilteredBrake", telem_info::FILTERED_BRAKE),
                    ("mFilteredSteering", telem_info::FILTERED_STEERING),
                    ("mFilteredClutch", telem_info::FILTERED_CLUTCH),
                    ("mSteeringShaftTorque", telem_info::STEERING_SHAFT_TORQUE),
                    ("mFront3rdDeflection", telem_info::FRONT3RD_DEFLECTION),
                    ("mRear3rdDeflection", telem_info::REAR3RD_DEFLECTION),
                    ("mFrontWingHeight", telem_info::FRONT_WING_HEIGHT),
                    ("mFrontRideHeight", telem_info::FRONT_RIDE_HEIGHT),
                    ("mRearRideHeight", telem_info::REAR_RIDE_HEIGHT),
                    ("mDrag", telem_info::DRAG),
                    ("mFrontDownforce", telem_info::FRONT_DOWNFORCE),
                    ("mRearDownforce", telem_info::REAR_DOWNFORCE),
                    ("mFuel", telem_info::FUEL),
                    ("mEngineMaxRPM", telem_info::ENGINE_MAX_RPM),
                    ("mScheduledStops", telem_info::SCHEDULED_STOPS),
                    ("mOverheating", telem_info::OVERHEATING),
                    ("mDetached", telem_info::DETACHED),
                    ("mHeadlights", telem_info::HEADLIGHTS),
                    ("mDentSeverity", telem_info::DENT_SEVERITY),
                    ("mLastImpactET", telem_info::LAST_IMPACT_ET),
                    ("mLastImpactMagnitude", telem_info::LAST_IMPACT_MAGNITUDE),
                    ("mLastImpactPos", telem_info::LAST_IMPACT_POS),
                    ("mEngineTorque", telem_info::ENGINE_TORQUE),
                    ("mCurrentSector", telem_info::CURRENT_SECTOR),
                    ("mSpeedLimiter", telem_info::SPEED_LIMITER),
                    ("mMaxGears", telem_info::MAX_GEARS),
                    (
                        "mFrontTireCompoundIndex",
                        telem_info::FRONT_TIRE_COMPOUND_INDEX,
                    ),
                    (
                        "mRearTireCompoundIndex",
                        telem_info::REAR_TIRE_COMPOUND_INDEX,
                    ),
                    ("mFuelCapacity", telem_info::FUEL_CAPACITY),
                    ("mFrontFlapActivated", telem_info::FRONT_FLAP_ACTIVATED),
                    ("mRearFlapActivated", telem_info::REAR_FLAP_ACTIVATED),
                    ("mRearFlapLegalStatus", telem_info::REAR_FLAP_LEGAL_STATUS),
                    ("mIgnitionStarter", telem_info::IGNITION_STARTER),
                    (
                        "mFrontTireCompoundName",
                        telem_info::FRONT_TIRE_COMPOUND_NAME,
                    ),
                    ("mRearTireCompoundName", telem_info::REAR_TIRE_COMPOUND_NAME),
                    (
                        "mSpeedLimiterAvailable",
                        telem_info::SPEED_LIMITER_AVAILABLE,
                    ),
                    ("mAntiStallActivated", telem_info::ANTI_STALL_ACTIVATED),
                    ("mUnused", telem_info::UNUSED),
                    (
                        "mVisualSteeringWheelRange",
                        telem_info::VISUAL_STEERING_WHEEL_RANGE,
                    ),
                    ("mRearBrakeBias", telem_info::REAR_BRAKE_BIAS),
                    ("mTurboBoostPressure", telem_info::TURBO_BOOST_PRESSURE),
                    (
                        "mPhysicsToGraphicsOffset",
                        telem_info::PHYSICS_TO_GRAPHICS_OFFSET,
                    ),
                    (
                        "mPhysicalSteeringWheelRange",
                        telem_info::PHYSICAL_STEERING_WHEEL_RANGE,
                    ),
                    ("mDeltaBest", telem_info::DELTA_BEST),
                    (
                        "mBatteryChargeFraction",
                        telem_info::BATTERY_CHARGE_FRACTION,
                    ),
                    (
                        "mElectricBoostMotorTorque",
                        telem_info::ELECTRIC_BOOST_MOTOR_TORQUE,
                    ),
                    (
                        "mElectricBoostMotorRPM",
                        telem_info::ELECTRIC_BOOST_MOTOR_RPM,
                    ),
                    (
                        "mElectricBoostMotorTemperature",
                        telem_info::ELECTRIC_BOOST_MOTOR_TEMPERATURE,
                    ),
                    (
                        "mElectricBoostWaterTemperature",
                        telem_info::ELECTRIC_BOOST_WATER_TEMPERATURE,
                    ),
                    (
                        "mElectricBoostMotorState",
                        telem_info::ELECTRIC_BOOST_MOTOR_STATE,
                    ),
                    ("mLapInvalidated", telem_info::LAP_INVALIDATED),
                    ("mABSActive", telem_info::ABS_ACTIVE),
                    ("mTCActive", telem_info::TC_ACTIVE),
                    ("mSpeedLimiterActive", telem_info::SPEED_LIMITER_ACTIVE),
                    ("mWiperState", telem_info::WIPER_STATE),
                    ("mTC", telem_info::TC),
                    ("mTCMax", telem_info::TC_MAX),
                    ("mTCSlip", telem_info::TC_SLIP),
                    ("mTCSlipMax", telem_info::TC_SLIP_MAX),
                    ("mTCCut", telem_info::TC_CUT),
                    ("mTCCutMax", telem_info::TC_CUT_MAX),
                    ("mABS", telem_info::ABS),
                    ("mABSMax", telem_info::ABS_MAX),
                    ("mMotorMap", telem_info::MOTOR_MAP),
                    ("mMotorMapMax", telem_info::MOTOR_MAP_MAX),
                    ("mMigration", telem_info::MIGRATION),
                    ("mMigrationMax", telem_info::MIGRATION_MAX),
                    ("mFrontAntiSway", telem_info::FRONT_ANTI_SWAY),
                    ("mFrontAntiSwayMax", telem_info::FRONT_ANTI_SWAY_MAX),
                    ("mRearAntiSway", telem_info::REAR_ANTI_SWAY),
                    ("mRearAntiSwayMax", telem_info::REAR_ANTI_SWAY_MAX),
                    ("mLiftAndCoastProgress", telem_info::LIFT_AND_COAST_PROGRESS),
                    ("mTrackLimitsSteps", telem_info::TRACK_LIMITS_STEPS),
                    ("mRegen", telem_info::REGEN),
                    ("mSoC", telem_info::SOC),
                    ("mVirtualEnergy", telem_info::VIRTUAL_ENERGY),
                    ("mTimeGapCarAhead", telem_info::TIME_GAP_CAR_AHEAD),
                    ("mTimeGapCarBehind", telem_info::TIME_GAP_CAR_BEHIND),
                    ("mTimeGapPlaceAhead", telem_info::TIME_GAP_PLACE_AHEAD),
                    ("mTimeGapPlaceBehind", telem_info::TIME_GAP_PLACE_BEHIND),
                    ("mVehicleModel", telem_info::VEHICLE_MODEL),
                    ("mVehicleClass", telem_info::VEHICLE_CLASS),
                    ("mVehicleChampionship", telem_info::VEHICLE_CHAMPIONSHIP),
                    ("mExpansion", telem_info::EXPANSION),
                    ("mWheel", telem_info::WHEEL),
                ],
            },
            Entry {
                name: "TelemWheelV01",
                size: telem_wheel::SIZE,
                align: telem_wheel::ALIGN,
                fields: vec![
                    ("mSuspensionDeflection", telem_wheel::SUSPENSION_DEFLECTION),
                    ("mRideHeight", telem_wheel::RIDE_HEIGHT),
                    ("mSuspForce", telem_wheel::SUSP_FORCE),
                    ("mBrakeTemp", telem_wheel::BRAKE_TEMP),
                    ("mBrakePressure", telem_wheel::BRAKE_PRESSURE),
                    ("mRotation", telem_wheel::ROTATION),
                    ("mLateralPatchVel", telem_wheel::LATERAL_PATCH_VEL),
                    ("mLongitudinalPatchVel", telem_wheel::LONGITUDINAL_PATCH_VEL),
                    ("mLateralGroundVel", telem_wheel::LATERAL_GROUND_VEL),
                    (
                        "mLongitudinalGroundVel",
                        telem_wheel::LONGITUDINAL_GROUND_VEL,
                    ),
                    ("mCamber", telem_wheel::CAMBER),
                    ("mLateralForce", telem_wheel::LATERAL_FORCE),
                    ("mLongitudinalForce", telem_wheel::LONGITUDINAL_FORCE),
                    ("mTireLoad", telem_wheel::TIRE_LOAD),
                    ("mGripFract", telem_wheel::GRIP_FRACT),
                    ("mPressure", telem_wheel::PRESSURE),
                    ("mTemperature", telem_wheel::TEMPERATURE),
                    ("mWear", telem_wheel::WEAR),
                    ("mTerrainName", telem_wheel::TERRAIN_NAME),
                    ("mSurfaceType", telem_wheel::SURFACE_TYPE),
                    ("mFlat", telem_wheel::FLAT),
                    ("mDetached", telem_wheel::DETACHED),
                    (
                        "mStaticUndeflectedRadius",
                        telem_wheel::STATIC_UNDEFLECTED_RADIUS,
                    ),
                    (
                        "mVerticalTireDeflection",
                        telem_wheel::VERTICAL_TIRE_DEFLECTION,
                    ),
                    ("mWheelYLocation", telem_wheel::WHEEL_Y_LOCATION),
                    ("mToe", telem_wheel::TOE),
                    (
                        "mTireCarcassTemperature",
                        telem_wheel::TIRE_CARCASS_TEMPERATURE,
                    ),
                    (
                        "mTireInnerLayerTemperature",
                        telem_wheel::TIRE_INNER_LAYER_TEMPERATURE,
                    ),
                    ("mOptimalTemp", telem_wheel::OPTIMAL_TEMP),
                    ("mCompoundIndex", telem_wheel::COMPOUND_INDEX),
                    ("mCompoundType", telem_wheel::COMPOUND_TYPE),
                    ("mExpansion", telem_wheel::EXPANSION),
                ],
            },
            Entry {
                name: "TelemVect3",
                size: vect3::SIZE,
                align: vect3::ALIGN,
                fields: vec![
                    ("x", vect3::X),
                    ("y", vect3::Y),
                    ("z", vect3::Z),
                    ("data", vect3::DATA),
                ],
            },
            Entry {
                name: "SharedMemoryLock::LockData",
                size: lock_data::SIZE,
                align: lock_data::ALIGN,
                fields: vec![("waiters", lock_data::WAITERS), ("busy", lock_data::BUSY)],
            },
        ]
    }

    #[test]
    fn every_struct_constant_matches_the_probe() {
        let j = probe();
        assert_eq!(
            j["checks"]["all_ok"],
            Value::Bool(true),
            "probe self-checks did not all pass"
        );
        let mut failures = Vec::new();
        for Entry {
            name: sname,
            size,
            align,
            fields,
        } in table()
        {
            let s = &j["structs"][sname];
            assert!(s.is_object(), "struct {sname} missing from the probe JSON");
            let probe_size = as_usize(&s["sizeof"], sname);
            let probe_align = as_usize(&s["alignof"], sname);
            if probe_size != size {
                failures.push(format!(
                    "{sname}: SIZE constant {size} != probe sizeof {probe_size}"
                ));
            }
            if probe_align != align {
                failures.push(format!(
                    "{sname}: ALIGN constant {align} != probe alignof {probe_align}"
                ));
            }
            let probe_fields = s["fields"]
                .as_array()
                .unwrap_or_else(|| panic!("{sname}: no fields array"));
            let ours: BTreeSet<&str> = fields.iter().map(|(n, _)| *n).collect();
            for pf in probe_fields {
                let name = pf["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{sname}: field without a name"));
                if !ours.contains(name) {
                    failures.push(format!(
                        "{sname}.{name}: present in the probe JSON but has no constant (new field?)"
                    ));
                }
            }
            for (name, field) in fields {
                let Some(pf) = probe_fields.iter().find(|pf| pf["name"] == *name) else {
                    failures.push(format!(
                        "{sname}.{name}: constant exists but the field is gone from the probe JSON"
                    ));
                    continue;
                };
                let off = as_usize(&pf["offset"], name);
                let size = as_usize(&pf["size"], name);
                if off != field.offset || size != field.size {
                    failures.push(format!(
                        "{sname}.{name}: constant (offset {}, size {}) != probe (offset {off}, size {size})",
                        field.offset, field.size
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "LAYOUT MISMATCH vs docs/research/lmu_abi_layout.json:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn absolute_offsets_match_the_probe() {
        let j = probe();
        let ours: Vec<(&str, usize, usize)> = vec![
            ("data", 0, layout::SIZE),
            ("data.generic", abs::GENERIC, generic::SIZE),
            (
                "data.generic.events",
                abs::GENERIC + generic::EVENTS.offset,
                generic::EVENTS.size,
            ),
            (
                "data.generic.gameVersion",
                abs::GENERIC + generic::GAME_VERSION.offset,
                generic::GAME_VERSION.size,
            ),
            (
                "data.generic.FFBTorque",
                abs::GENERIC + generic::FFB_TORQUE.offset,
                generic::FFB_TORQUE.size,
            ),
            (
                "data.generic.appInfo",
                abs::GENERIC + generic::APP_INFO.offset,
                app_state::SIZE,
            ),
            ("data.paths", abs::PATHS, paths::SIZE),
            (
                "data.paths.userData",
                abs::PATHS + paths::USER_DATA.offset,
                paths::USER_DATA.size,
            ),
            (
                "data.paths.customVariables",
                abs::PATHS + paths::CUSTOM_VARIABLES.offset,
                paths::CUSTOM_VARIABLES.size,
            ),
            (
                "data.paths.stewardResults",
                abs::PATHS + paths::STEWARD_RESULTS.offset,
                paths::STEWARD_RESULTS.size,
            ),
            (
                "data.paths.playerProfile",
                abs::PATHS + paths::PLAYER_PROFILE.offset,
                paths::PLAYER_PROFILE.size,
            ),
            (
                "data.paths.pluginsFolder",
                abs::PATHS + paths::PLUGINS_FOLDER.offset,
                paths::PLUGINS_FOLDER.size,
            ),
            ("data.scoring", abs::SCORING, scoring_data::SIZE),
            (
                "data.scoring.scoringInfo",
                abs::SCORING_INFO,
                scoring_info::SIZE,
            ),
            (
                "data.scoring.scoringInfo.mNumVehicles",
                abs::SCORING_INFO + scoring_info::NUM_VEHICLES.offset,
                scoring_info::NUM_VEHICLES.size,
            ),
            (
                "data.scoring.scoringInfo.mVehicle",
                abs::SCORING_INFO + scoring_info::VEHICLE.offset,
                scoring_info::VEHICLE.size,
            ),
            (
                "data.scoring.scoringStreamSize",
                abs::SCORING_STREAM_SIZE,
                scoring_data::SCORING_STREAM_SIZE.size,
            ),
            (
                "data.scoring.vehScoringInfo",
                abs::VEH_SCORING_INFO,
                scoring_data::VEH_SCORING_INFO.size,
            ),
            (
                "data.scoring.vehScoringInfo[1]",
                abs_index(abs::veh_scoring_info(1)),
                veh_scoring::SIZE,
            ),
            (
                "data.scoring.vehScoringInfo[103]",
                abs_index(abs::veh_scoring_info(103)),
                veh_scoring::SIZE,
            ),
            (
                "data.scoring.scoringStream",
                abs::SCORING_STREAM,
                SCORING_STREAM_CAPACITY,
            ),
            ("data.telemetry", abs::TELEMETRY, telemetry_data::SIZE),
            (
                "data.telemetry.activeVehicles",
                abs::ACTIVE_VEHICLES,
                telemetry_data::ACTIVE_VEHICLES.size,
            ),
            (
                "data.telemetry.playerVehicleIdx",
                abs::PLAYER_VEHICLE_IDX,
                telemetry_data::PLAYER_VEHICLE_IDX.size,
            ),
            (
                "data.telemetry.playerHasVehicle",
                abs::PLAYER_HAS_VEHICLE,
                telemetry_data::PLAYER_HAS_VEHICLE.size,
            ),
            (
                "data.telemetry.telemInfo",
                abs::TELEM_INFO,
                telemetry_data::TELEM_INFO.size,
            ),
            (
                "data.telemetry.telemInfo[0].mDeltaTime",
                abs::TELEM_INFO + telem_info::DELTA_TIME.offset,
                telem_info::DELTA_TIME.size,
            ),
            (
                "data.telemetry.telemInfo[0].mWheel",
                abs::TELEM_INFO + telem_info::WHEEL.offset,
                telem_info::WHEEL.size,
            ),
            (
                "data.telemetry.telemInfo[0].mWheel[1]",
                abs::TELEM_INFO + telem_info::WHEEL.offset + telem_wheel::SIZE,
                telem_wheel::SIZE,
            ),
            (
                "data.telemetry.telemInfo[1]",
                abs_index(abs::telem_info(1)),
                telem_info::SIZE,
            ),
            (
                "data.telemetry.telemInfo[103]",
                abs_index(abs::telem_info(103)),
                telem_info::SIZE,
            ),
        ];
        let paths = j["absolute_offsets"]["paths"]
            .as_array()
            .unwrap_or_else(|| panic!("no absolute_offsets.paths"));
        assert_eq!(
            paths.len(),
            ours.len(),
            "the probe lists a different set of absolute paths; regenerate"
        );
        let mut failures = Vec::new();
        for (path, off, size) in ours {
            let Some(p) = paths.iter().find(|p| p["path"] == path) else {
                failures.push(format!("{path}: not in the probe JSON"));
                continue;
            };
            let (po, ps) = (as_usize(&p["offset"], path), as_usize(&p["size"], path));
            if (po, ps) != (off, size) {
                failures.push(format!(
                    "{path}: ours (offset {off}, size {size}) != probe (offset {po}, size {ps})"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "ABSOLUTE LAYOUT MISMATCH:\n{}",
            failures.join("\n")
        );
        assert_eq!(
            as_usize(
                &j["absolute_offsets"]["sizeof_SharedMemoryLayout"],
                "sizeof"
            ),
            layout::SIZE
        );
    }

    fn abs_index(v: Option<usize>) -> usize {
        v.unwrap_or_else(|| panic!("slot index out of range"))
    }

    #[test]
    fn derived_absolute_offsets_are_the_documented_values() {
        // Values quoted in docs/LMU_TELEMETRY_RESEARCH.md section 4 and the channel map section 0.5.
        assert_eq!(abs::SCORING_PADDING, Field::new(2180, 4));
        assert_eq!(abs::SCORING_STREAM_SIZE, 2184);
        assert_eq!(abs::VEH_SCORING_INFO, 2192);
        assert_eq!(abs::SCORING_STREAM, 62_928);
        assert_eq!(abs::TELEMETRY, 128_464);
        assert_eq!(abs::TELEM_INFO, 128_468);
        assert_eq!(abs::MAPPING_END, 324_820);
        assert_eq!(abs::SCORING_HEADER, Field::new(1632, 560));
        assert_eq!(abs::TELEMETRY_HEADER, Field::new(128_464, 4));
        assert_eq!(abs::SCORING_INFO + scoring_info::CURRENT_ET.offset, 1700);
        assert_eq!(abs::veh_scoring_info(104), None);
        assert_eq!(abs::telem_info(104), None);
        assert_eq!(layout::SIZE - abs::MAPPING_END, 4, "tail padding");
    }

    #[test]
    fn constants_enums_and_names_match_the_probe() {
        let j = probe();
        let c = &j["constants"];
        assert_eq!(
            as_usize(&c["scoring_vehicle_slots"], "slots"),
            MAX_SCORING_VEHICLES
        );
        assert_eq!(
            as_usize(&c["telemetry_vehicle_slots"], "slots"),
            MAX_TELEMETRY_VEHICLES
        );
        assert_eq!(
            as_usize(&c["scoring_stream_capacity"], "capacity"),
            SCORING_STREAM_CAPACITY
        );
        assert_eq!(as_usize(&c["SME_MAX"], "SME_MAX"), EVENT_SLOTS);
        assert_eq!(as_usize(&c["MAX_PATH"], "MAX_PATH"), PATH_LEN);
        assert_eq!(telem_info::WHEEL.size, WHEEL_COUNT * telem_wheel::SIZE);
        let n = &c["names"];
        assert_eq!(n["LMU_SHARED_MEMORY_FILE"]["value"], names::DATA_MAPPING);
        assert_eq!(
            n["LMU_SHARED_MEMORY_HOLD_EVENT"]["value"],
            names::HOLD_EVENT
        );
        assert_eq!(
            n["LMU_SHARED_MEMORY_DATA_EVENT"]["value"],
            names::DATA_EVENT
        );
        assert_eq!(n["lock_mapping"]["value"], names::LOCK_MAPPING);
        assert_eq!(n["lock_event"]["value"], names::LOCK_EVENT);
        let e = &j["enums"];
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_ENTER"],
            u64::from(sme::ENTER)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_EXIT"],
            u64::from(sme::EXIT)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_STARTUP"],
            u64::from(sme::STARTUP)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_SHUTDOWN"],
            u64::from(sme::SHUTDOWN)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_LOAD"],
            u64::from(sme::LOAD)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_UNLOAD"],
            u64::from(sme::UNLOAD)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_START_SESSION"],
            u64::from(sme::START_SESSION)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_END_SESSION"],
            u64::from(sme::END_SESSION)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_ENTER_REALTIME"],
            u64::from(sme::ENTER_REALTIME)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_EXIT_REALTIME"],
            u64::from(sme::EXIT_REALTIME)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_UPDATE_SCORING"],
            u64::from(sme::UPDATE_SCORING)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_UPDATE_TELEMETRY"],
            u64::from(sme::UPDATE_TELEMETRY)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_INIT_APPLICATION"],
            u64::from(sme::INIT_APPLICATION)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_UNINIT_APPLICATION"],
            u64::from(sme::UNINIT_APPLICATION)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_SET_ENVIRONMENT"],
            u64::from(sme::SET_ENVIRONMENT)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_FFB"],
            u64::from(sme::FFB)
        );
        assert_eq!(
            e["SharedMemoryEvent"]["values"]["SME_MAX"],
            u64::from(sme::MAX)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["Hypercar"],
            u64::from(vehicle_class::HYPERCAR)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["LMP2_ELMS"],
            u64::from(vehicle_class::LMP2_ELMS)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["LMP2"],
            u64::from(vehicle_class::LMP2)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["LMP3"],
            u64::from(vehicle_class::LMP3)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["GTE"],
            u64::from(vehicle_class::GTE)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["GT3"],
            u64::from(vehicle_class::GT3)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["PaceCar"],
            u64::from(vehicle_class::PACE_CAR)
        );
        assert_eq!(
            e["IP_VehicleClass"]["values"]["Unknown"],
            u64::from(vehicle_class::UNKNOWN)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["WEC_2023"],
            u64::from(vehicle_championship::WEC_2023)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["WEC_2024"],
            u64::from(vehicle_championship::WEC_2024)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["WEC_2025"],
            u64::from(vehicle_championship::WEC_2025)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["WEC_2026"],
            u64::from(vehicle_championship::WEC_2026)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["ELMS_2025"],
            u64::from(vehicle_championship::ELMS_2025)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["ELMS_2026"],
            u64::from(vehicle_championship::ELMS_2026)
        );
        assert_eq!(
            e["IP_VehicleChampionship"]["values"]["Unknown"],
            u64::from(vehicle_championship::UNKNOWN)
        );
        // Value counts the hand-written enums in raw.rs are built for (SharedMemoryEvent: 16 codes
        // plus the SME_MAX sentinel). A new value means raw.rs needs a new variant.
        for (name, expected) in [
            ("SharedMemoryEvent", 17),
            ("IP_VehicleClass", 8),
            ("IP_VehicleChampionship", 7),
        ] {
            let count = e[name]["values"].as_object().map_or(0, |m| m.len());
            assert_eq!(
                count, expected,
                "{name}: the probe lists {count} values; regenerate"
            );
        }
    }

    #[test]
    fn bool_field_lists_match_the_probe() {
        let j = probe();
        let lists: Vec<(&str, &[(&str, Field)])> = vec![
            ("ScoringInfoV01", scoring_info::BOOL_FIELDS),
            ("VehicleScoringInfoV01", veh_scoring::BOOL_FIELDS),
            ("SharedMemoryTelemetryData", telemetry_data::BOOL_FIELDS),
            ("TelemInfoV01", telem_info::BOOL_FIELDS),
            ("TelemWheelV01", telem_wheel::BOOL_FIELDS),
        ];
        let mut with_bools = 0;
        for (sname, s) in j["structs"]
            .as_object()
            .unwrap_or_else(|| panic!("no structs"))
        {
            let fields = s["fields"]
                .as_array()
                .unwrap_or_else(|| panic!("{sname}: no fields array"));
            let expected: Vec<(&str, usize, usize)> = fields
                .iter()
                .filter(|f| f["kind"] == "bool" && f["union_alt"].as_u64().unwrap_or(0) == 0)
                .map(|f| {
                    (
                        f["name"].as_str().unwrap_or_default(),
                        as_usize(&f["offset"], sname),
                        as_usize(&f["size"], sname),
                    )
                })
                .collect();
            let Some((_, list)) = lists.iter().find(|(n, _)| n == sname) else {
                assert!(
                    expected.is_empty(),
                    "{sname} has bool members but no BOOL_FIELDS list (struct not mirrored?)"
                );
                continue;
            };
            with_bools += 1;
            let ours: Vec<(&str, usize, usize)> =
                list.iter().map(|(n, f)| (*n, f.offset, f.size)).collect();
            assert_eq!(
                ours, expected,
                "{sname}: BOOL_FIELDS differ from the probe; regenerate"
            );
        }
        assert_eq!(
            with_bools,
            lists.len(),
            "a BOOL_FIELDS list names a struct missing from the probe"
        );
    }
}

//! Typed scoring state (session + standings), session identity and capability maps.
//!
//! These are serde structs designed for postcard (no `flatten`, no untagged enums, no skipped fields).
//! Anything LMU may not provide is an `Option`; a missing value is `None`, never a zero or an invented
//! default state. Field semantics and raw conversions follow docs/LMU_CHANNEL_MAP.md §1, §3, §14–§16.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::channel::{Availability, ChannelId, Source};
use crate::enums::{
    CarFlag, ControlKind, CountLapFlag, FinishStatus, GameMode, GamePhase, IndividualPhase,
    PitState, SessionType, VehicleClass, YellowFlagState,
};
use crate::registry;

/// `mMaxLaps` sentinel for timed sessions (INT32_MAX, LMU_CHANNEL_MAP §1).
pub const LMU_MAX_LAPS_UNLIMITED: i32 = i32::MAX;
/// Number of sectors per lap.
pub const SECTORS_PER_LAP: u8 = 3;
/// Raw `mSector` values: 0 = S3, 1 = S1, 2 = S2.
const LMU_SECTOR_RAW_S3: i8 = 0;
const LMU_SECTOR_RAW_S1: i8 = 1;
const LMU_SECTOR_RAW_S2: i8 = 2;

/// LMU lap/sector times use `<= 0` for "no time yet"; returns `None` for those and for non-finite values.
pub fn lmu_time(raw_s: f64) -> Option<f64> {
    (raw_s.is_finite() && raw_s > 0.0).then_some(raw_s)
}

/// Decodes `mMaxLaps`: the INT32_MAX sentinel (timed session) and negative values become `None`.
pub fn lmu_max_laps(raw: i32) -> Option<u32> {
    if raw == LMU_MAX_LAPS_UNLIMITED {
        None
    } else {
        u32::try_from(raw).ok()
    }
}

/// Session time remaining from `mEndET − mCurrentET`, at least 0 once a timed session has run out
/// (LMU_CHANNEL_MAP §18 `session.time_remaining_calc_s`). `None` when `mEndET` is not a real end time (not
/// finite or ≤ 0, e.g. the UNVERIFIED lap-race sentinel) or the result is not finite: never a fabricated
/// "0 remaining". The single implementation for every producer and consumer of this value.
pub fn lmu_session_time_remaining(end_et_s: f64, current_et_s: f64) -> Option<f64> {
    if !(end_et_s.is_finite() && end_et_s > 0.0) {
        return None;
    }
    let remaining = end_et_s - current_et_s;
    remaining.is_finite().then(|| remaining.max(0.0))
}

/// Decodes the raw `mSector` (0 = S3, 1 = S1, 2 = S2) into a 1-based sector number.
pub fn lmu_sector(raw: i8) -> Option<u8> {
    match raw {
        LMU_SECTOR_RAW_S1 => Some(1),
        LMU_SECTOR_RAW_S2 => Some(2),
        LMU_SECTOR_RAW_S3 => Some(SECTORS_PER_LAP),
        _ => None,
    }
}

/// Weather as reported by the scoring block (internal units: °C, fractions, m/s).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoringWeather {
    pub ambient_temp_c: Option<f64>,
    pub track_temp_c: Option<f64>,
    /// Rain intensity 0..1 (`mRaining`).
    pub raining_frac: Option<f64>,
    /// Dark cloud 0..1 (`mDarkCloud`).
    pub dark_cloud_frac: Option<f64>,
    /// Raw cloud coverage (`mCloudCoverage`, scale UNVERIFIED).
    pub cloud_coverage_raw: Option<u8>,
    pub path_wetness_min_frac: Option<f64>,
    pub path_wetness_max_frac: Option<f64>,
    pub path_wetness_avg_frac: Option<f64>,
    /// Wind vector in world coordinates (m/s), `[x, y, z]`.
    pub wind_mps: Option<[f64; 3]>,
    /// Raw track grip level (`mTrackGripLevel`, scale UNVERIFIED).
    pub track_grip_level_raw: Option<u8>,
}

/// Session-wide scoring state (`scoring.scoringInfo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionScoring {
    pub track_name: Option<String>,
    /// Decoded `mSession` (0 test, 1–4 practice, 5–8 qualifying, 9 warm-up, 10–13 race).
    pub session_type: Option<SessionType>,
    pub game_phase: Option<GamePhase>,
    pub yellow_flag_state: Option<YellowFlagState>,
    /// Raw `mSectorFlag[0..3]` (index → sector order UNVERIFIED, A-LMU-030; 1 = yellow, 11 = clear CLAIMED).
    pub sector_flags_raw: Option<[i8; 3]>,
    /// Start light frame (`mStartLight`, track dependent).
    pub start_light: Option<u8>,
    pub num_red_lights: Option<u8>,
    /// Vehicles in the session (`mNumVehicles`).
    pub num_vehicles: Option<u32>,
    /// Elapsed session time at this scoring update (`mCurrentET`).
    pub current_et_s: Option<f64>,
    /// Session end time (`mEndET`); not a real end time when ≤ 0 (lap-race sentinel, UNVERIFIED).
    pub end_et_s: Option<f64>,
    /// Time remaining as reported by LMU (`mSessionTimeRemaining`, unit assumed seconds, A-LMU-035).
    pub time_remaining_s: Option<f64>,
    /// Lap limit; `None` for timed sessions (`mMaxLaps` sentinel) or when unknown.
    pub max_laps: Option<u32>,
    /// Track length (`scoringInfo.mLapDist`).
    pub track_length_m: Option<f64>,
    pub in_realtime: Option<bool>,
    pub game_mode: Option<GameMode>,
    pub max_players: Option<i32>,
    /// Time of day in seconds since midnight.
    pub time_of_day_s: Option<f64>,
    pub fixed_setup: Option<bool>,
    pub weather: ScoringWeather,
}

impl SessionScoring {
    /// `max(0, end − current)` (cross-check for `time_remaining_s`), `None` unless both are known and the end
    /// time is real; see [`lmu_session_time_remaining`].
    pub fn time_remaining_calc_s(&self) -> Option<f64> {
        lmu_session_time_remaining(self.end_et_s?, self.current_et_s?)
    }
}

/// One competitor row (`scoring.vehScoringInfo[i]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompetitorEntry {
    /// LMU slot id (`mID`), the key of the row.
    pub slot_id: i32,
    pub driver: Option<String>,
    pub vehicle_name: Option<String>,
    pub vehicle_filename: Option<String>,
    /// Class string (`mVehicleClass`), the grouping key for class positions.
    pub class_name: Option<String>,
    /// Team name, usually (`mPitGroup`).
    pub pit_group: Option<String>,
    /// Overall position, 1-based (`mPlace`).
    pub place: Option<u8>,
    /// Position within `class_name`, derived by [`ScoringState::assign_class_places`].
    pub class_place: Option<u8>,
    /// Completed laps (`mTotalLaps`).
    pub laps: Option<i32>,
    pub lap_dist_m: Option<f64>,
    /// Current sector, 1-based (decoded from `mSector` by [`lmu_sector`]).
    pub sector: Option<u8>,
    pub best_lap_s: Option<f64>,
    pub last_lap_s: Option<f64>,
    pub best_sector1_s: Option<f64>,
    /// Best sector 1 + 2 (LMU stores sector 2 cumulatively).
    pub best_sector12_s: Option<f64>,
    pub last_sector1_s: Option<f64>,
    pub last_sector12_s: Option<f64>,
    pub cur_sector1_s: Option<f64>,
    pub cur_sector12_s: Option<f64>,
    pub time_behind_next_s: Option<f64>,
    pub laps_behind_next: Option<i32>,
    pub time_behind_leader_s: Option<f64>,
    pub laps_behind_leader: Option<i32>,
    pub time_into_lap_s: Option<f64>,
    pub estimated_lap_s: Option<f64>,
    pub lap_start_et_s: Option<f64>,
    /// `mInPits` (unreliable for remote cars; prefer `pit_state`).
    pub in_pits: Option<bool>,
    pub in_garage_stall: Option<bool>,
    pub pit_state: Option<PitState>,
    pub num_pitstops: Option<i32>,
    pub num_penalties: Option<i32>,
    pub finish_status: Option<FinishStatus>,
    pub flag: Option<CarFlag>,
    pub under_yellow: Option<bool>,
    pub individual_phase: Option<IndividualPhase>,
    pub count_lap_flag: Option<CountLapFlag>,
    pub control: Option<ControlKind>,
    pub is_player: bool,
    pub qualification: Option<i32>,
    /// Fuel **or battery / virtual energy** fraction 0..1 (`mFuelFraction`; meaning is car dependent,
    /// A-LMU-044).
    pub energy_or_fuel_frac: Option<f64>,
    /// World position x (m) for the track map.
    pub pos_x_m: Option<f64>,
    /// World position z (m) for the track map (y is up).
    pub pos_z_m: Option<f64>,
}

impl CompetitorEntry {
    /// A row with only its slot id known.
    pub fn new(slot_id: i32) -> Self {
        Self {
            slot_id,
            ..Self::default()
        }
    }

    /// Last lap sector 2 alone (`last_sector12 − last_sector1`).
    pub fn last_sector2_s(&self) -> Option<f64> {
        positive_difference(self.last_sector12_s?, self.last_sector1_s?)
    }

    /// Last lap sector 3 (`last_lap − last_sector12`).
    pub fn last_sector3_s(&self) -> Option<f64> {
        positive_difference(self.last_lap_s?, self.last_sector12_s?)
    }

    /// `best_sector12 − best_sector1`. The two bests may come from different laps, so this is only a true
    /// best sector 2 when they come from the same lap; engines track per-sector bests themselves.
    pub fn best_sector2_s(&self) -> Option<f64> {
        positive_difference(self.best_sector12_s?, self.best_sector1_s?)
    }

    /// Whether this car is in the pit lane or stall according to the pit state machine. The observed garage
    /// state ([`PitState::Garage`], A-LMU-063) counts: LMU reports it in the stall and on the way out of it.
    pub fn in_pit_lane(&self) -> bool {
        matches!(
            self.pit_state,
            Some(PitState::Entering | PitState::Stopped | PitState::Exiting | PitState::Garage)
        ) || self.in_garage_stall == Some(true)
    }
}

fn positive_difference(a: f64, b: f64) -> Option<f64> {
    let d = a - b;
    (d.is_finite() && d > 0.0).then_some(d)
}

/// Full scoring state: session plus every competitor row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoringState {
    pub session: SessionScoring,
    pub vehicles: Vec<CompetitorEntry>,
}

impl ScoringState {
    /// The player's row (exactly one expected while driving).
    pub fn player(&self) -> Option<&CompetitorEntry> {
        self.vehicles.iter().find(|v| v.is_player)
    }

    /// Row by slot id.
    pub fn by_slot(&self, slot_id: i32) -> Option<&CompetitorEntry> {
        self.vehicles.iter().find(|v| v.slot_id == slot_id)
    }

    /// Overall leader (place 1).
    pub fn leader(&self) -> Option<&CompetitorEntry> {
        self.vehicles.iter().find(|v| v.place == Some(1))
    }

    /// True when every slot id is unique and at most one row is the player (LMU_CHANNEL_MAP §16: duplicate
    /// ids make the standings SUSPECT).
    pub fn is_consistent(&self) -> bool {
        let mut ids: Vec<i32> = self.vehicles.iter().map(|v| v.slot_id).collect();
        ids.sort_unstable();
        let unique = ids.windows(2).all(|w| w[0] != w[1]);
        unique && self.vehicles.iter().filter(|v| v.is_player).count() <= 1
    }

    /// Derives `class_place` for every row: rank by overall `place` within the same `class_name`. Rows
    /// without a place or class get `None`.
    pub fn assign_class_places(&mut self) {
        let mut order: Vec<usize> = (0..self.vehicles.len()).collect();
        order.sort_by_key(|&i| (self.vehicles[i].place.is_none(), self.vehicles[i].place));
        let mut counters: BTreeMap<String, u8> = BTreeMap::new();
        for i in order {
            let row = &mut self.vehicles[i];
            row.class_place = match (&row.class_name, row.place) {
                (Some(class), Some(_)) => {
                    let n = counters.entry(class.clone()).or_insert(0);
                    *n = n.saturating_add(1);
                    Some(*n)
                }
                _ => None,
            };
        }
    }

    /// Vehicles sorted by overall place (rows without a place last).
    pub fn standings(&self) -> Vec<&CompetitorEntry> {
        let mut rows: Vec<&CompetitorEntry> = self.vehicles.iter().collect();
        rows.sort_by_key(|v| (v.place.is_none(), v.place, v.slot_id));
        rows
    }
}

/// Who/what/where of the current session; changes trigger `SessionChanged`/`VehicleChanged`/`TrackChanged`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionIdentity {
    pub vehicle_name: Option<String>,
    pub vehicle_model: Option<String>,
    pub vehicle_class: Option<VehicleClass>,
    /// Class string from scoring (`mVehicleClass` text).
    pub class_name: Option<String>,
    pub track_name: Option<String>,
    pub session_type: Option<SessionType>,
    /// `generic.gameVersion`.
    pub game_version: Option<i32>,
}

/// Runtime status of one channel for the current car and session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapabilityStatus {
    /// Valid values are being received.
    Available,
    /// The source can provide it but currently does not (e.g. not driving yet).
    Unavailable,
    /// The source or this car can never provide it (e.g. hybrid on a GT3 car).
    NotSupported,
    /// Was available; not refreshed within its staleness budget.
    Stale,
    /// Values are arriving but fail validation.
    Invalid,
    /// Produced by a calculation engine from measured inputs.
    Calculated,
}

/// Capability note of [`Availability::Experimental`] channels in a fresh [`CapabilityMap`].
pub const NOTE_EXPERIMENTAL: &str = "experimental: not published until verified live";

/// Capability of one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelCapability {
    pub status: CapabilityStatus,
    pub source: Source,
    /// Measured update rate (never assumed); `None` until measured.
    pub native_rate_hz: Option<f32>,
    /// Short human explanation (e.g. "car has no hybrid system").
    pub note: Option<String>,
}

/// Per-channel capability for the current car and session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CapabilityMap {
    pub channels: BTreeMap<ChannelId, ChannelCapability>,
}

impl CapabilityMap {
    /// Initial map from the registry, before anything is observed: channels no source provides are
    /// `NotSupported`; every other channel, calculated ones included, is `Unavailable` until a valid sample
    /// is seen (the producer then reports `Available` or `Calculated`). Experimental channels carry
    /// [`NOTE_EXPERIMENTAL`]: they are not published until verified live.
    pub fn from_registry() -> Self {
        let channels = registry::all()
            .iter()
            .map(|def| {
                let (status, note) = match def.availability {
                    Availability::NotProvided => (CapabilityStatus::NotSupported, None),
                    Availability::Experimental => (
                        CapabilityStatus::Unavailable,
                        Some(NOTE_EXPERIMENTAL.to_owned()),
                    ),
                    Availability::Always | Availability::VehicleDependent => {
                        (CapabilityStatus::Unavailable, None)
                    }
                };
                (
                    def.id,
                    ChannelCapability {
                        status,
                        source: def.source,
                        native_rate_hz: None,
                        note,
                    },
                )
            })
            .collect();
        Self { channels }
    }

    /// Capability of `id`.
    pub fn get(&self, id: ChannelId) -> Option<&ChannelCapability> {
        self.channels.get(&id)
    }

    /// Sets the status and note of `id` (keeps the measured rate); returns false for channels not in the
    /// map.
    pub fn set_status(
        &mut self,
        id: ChannelId,
        status: CapabilityStatus,
        note: Option<String>,
    ) -> bool {
        match self.channels.get_mut(&id) {
            Some(cap) => {
                cap.status = status;
                cap.note = note;
                true
            }
            None => false,
        }
    }

    /// Records a measured native rate for `id`; returns false for channels not in the map and for
    /// non-finite or negative rates.
    pub fn set_rate(&mut self, id: ChannelId, rate_hz: f32) -> bool {
        if !(rate_hz.is_finite() && rate_hz >= 0.0) {
            return false;
        }
        match self.channels.get_mut(&id) {
            Some(cap) => {
                cap.native_rate_hz = Some(rate_hz);
                true
            }
            None => false,
        }
    }

    /// Number of channels with `status`.
    pub fn count(&self, status: CapabilityStatus) -> usize {
        self.channels
            .values()
            .filter(|c| c.status == status)
            .count()
    }
}

/// Coarse per-car feature flags, decided at runtime from telemetry; `None` = not determined yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleCapabilities {
    /// Hybrid system present (`mElectricBoostMotorState != Unavailable`).
    pub supports_hybrid: Option<bool>,
    /// Virtual energy rules apply (`mVirtualEnergy` meaningful).
    pub supports_virtual_energy: Option<bool>,
    /// Tyre wear reported.
    pub supports_tyre_wear: Option<bool>,
    /// ABS system present (setting `mABSMax > 0`).
    pub supports_abs: Option<bool>,
    /// TC system present (setting `mTCMax > 0`).
    pub supports_tc: Option<bool>,
    /// ABS activity signal (`mABSActive`) reported. LMU exposes activity only as a boolean, never an
    /// intervention amount (spec §15).
    pub supports_abs_intervention: Option<bool>,
    /// TC activity signal (`mTCActive`) reported (boolean only, spec §15).
    pub supports_tc_intervention: Option<bool>,
    /// Brake migration setting present (`mMigrationMax > 0`).
    pub supports_brake_migration: Option<bool>,
    /// Engine/motor map setting present (`mMotorMapMax > 0`).
    pub supports_motor_map: Option<bool>,
    /// Rear flap (DRS-like) present.
    pub supports_rear_flap: Option<bool>,
    /// Anti-roll bars adjustable from the cockpit.
    pub supports_arb_adjustment: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ch;

    fn row(slot: i32, place: u8, class: &str) -> CompetitorEntry {
        CompetitorEntry {
            place: Some(place),
            class_name: Some(class.to_owned()),
            ..CompetitorEntry::new(slot)
        }
    }

    #[test]
    fn raw_decoders() {
        assert_eq!(lmu_time(0.0), None);
        assert_eq!(lmu_time(-1.0), None);
        assert_eq!(lmu_time(f64::NAN), None);
        assert_eq!(lmu_time(92.5), Some(92.5));
        assert_eq!(lmu_max_laps(i32::MAX), None);
        assert_eq!(lmu_max_laps(-3), None);
        assert_eq!(lmu_max_laps(30), Some(30));
        assert_eq!(lmu_sector(0), Some(3));
        assert_eq!(lmu_sector(1), Some(1));
        assert_eq!(lmu_sector(2), Some(2));
        assert_eq!(lmu_sector(3), None);
        assert_eq!(lmu_session_time_remaining(3_600.0, 100.0), Some(3_500.0));
        assert_eq!(lmu_session_time_remaining(0.0, 100.0), None);
    }

    #[test]
    fn defaults_invent_nothing() {
        let r = CompetitorEntry::new(5);
        assert_eq!(
            (r.pit_state, r.flag, r.finish_status, r.in_pits),
            (None, None, None, None)
        );
        assert!(!r.in_pit_lane());
        let s = SessionScoring::default();
        assert_eq!(
            (s.session_type, s.game_phase, s.num_vehicles),
            (None, None, None)
        );
    }

    #[test]
    fn class_places_are_derived_per_class() {
        let mut state = ScoringState {
            session: SessionScoring::default(),
            vehicles: vec![
                row(10, 3, "LMGT3"),
                row(11, 1, "Hypercar"),
                row(12, 2, "LMGT3"),
                CompetitorEntry::new(13),
            ],
        };
        state.vehicles[3].class_name = Some("LMGT3".into());
        state.assign_class_places();
        let places: Vec<_> = state.vehicles.iter().map(|v| v.class_place).collect();
        assert_eq!(places, vec![Some(2), Some(1), Some(1), None]);
        assert_eq!(state.leader().map(|v| v.slot_id), Some(11));
        assert_eq!(
            state
                .standings()
                .iter()
                .map(|v| v.slot_id)
                .collect::<Vec<_>>(),
            vec![11, 12, 10, 13]
        );
        assert!(state.is_consistent());
        state.vehicles.push(row(10, 9, "LMP2"));
        assert!(
            !state.is_consistent(),
            "duplicate slot ids are inconsistent"
        );
    }

    #[test]
    fn sector_splits() {
        let mut r = CompetitorEntry::new(1);
        r.last_sector1_s = Some(30.0);
        r.last_sector12_s = Some(65.0);
        r.last_lap_s = Some(100.0);
        assert_eq!(r.last_sector2_s(), Some(35.0));
        assert_eq!(r.last_sector3_s(), Some(35.0));
        assert_eq!(r.best_sector2_s(), None);
        r.pit_state = Some(PitState::Stopped);
        assert!(r.in_pit_lane());
        // Observed garage state (A-LMU-063): in the stall or driving out of it.
        r.pit_state = Some(PitState::Garage);
        assert!(r.in_pit_lane());
        r.pit_state = Some(PitState::Requested);
        assert!(!r.in_pit_lane(), "a request is made on track");
    }

    #[test]
    fn session_helpers() {
        let mut s = SessionScoring::default();
        assert_eq!(s.time_remaining_calc_s(), None);
        s.end_et_s = Some(3_600.0);
        s.current_et_s = Some(3_700.0);
        assert_eq!(
            s.time_remaining_calc_s(),
            Some(0.0),
            "expired timed session"
        );
        s.current_et_s = Some(100.0);
        assert_eq!(s.time_remaining_calc_s(), Some(3_500.0));
    }

    /// LMU_CHANNEL_MAP §18: no data when `mEndET` is not finite or ≤ 0 (the lap-race sentinel), never a
    /// fabricated "0:00 remaining".
    #[test]
    fn time_remaining_needs_a_real_end_time() {
        let mut s = SessionScoring {
            current_et_s: Some(1_234.5),
            ..SessionScoring::default()
        };
        for end in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            s.end_et_s = Some(end);
            assert_eq!(s.time_remaining_calc_s(), None, "mEndET {end}");
        }
        s.end_et_s = Some(3_600.0);
        s.current_et_s = Some(f64::NAN);
        assert_eq!(s.time_remaining_calc_s(), None, "unknown current time");
    }

    #[test]
    fn postcard_roundtrip() {
        let mut state = ScoringState {
            session: SessionScoring::default(),
            vehicles: vec![row(1, 1, "Hypercar")],
        };
        state.session.session_type = Some(SessionType::from_lmu(11));
        state.session.weather.wind_mps = Some([1.0, 0.0, -2.0]);
        state.vehicles[0].is_player = true;
        state.vehicles[0].flag = Some(CarFlag::from_raw(3));
        let bytes = postcard::to_stdvec(&state).expect("serialize");
        let back: ScoringState = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(back, state);

        let caps = CapabilityMap::from_registry();
        let bytes = postcard::to_stdvec(&caps).expect("serialize");
        let back: CapabilityMap = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(back, caps);

        let identity = SessionIdentity {
            vehicle_class: Some(VehicleClass::from_raw(6)),
            ..Default::default()
        };
        let bytes = postcard::to_stdvec(&identity).expect("serialize");
        assert_eq!(
            postcard::from_bytes::<SessionIdentity>(&bytes).expect("deserialize"),
            identity
        );
    }

    #[test]
    fn capability_map_from_registry() {
        let mut caps = CapabilityMap::from_registry();
        assert_eq!(caps.channels.len(), registry::len());
        assert_eq!(
            caps.get(ch::TC_INTERVENTION).map(|c| c.status),
            Some(CapabilityStatus::NotSupported)
        );
        // Nothing is advertised before it has been observed: calculated channels start unavailable, and
        // experimental ones say why they will not be published.
        assert_eq!(
            caps.get(ch::SPEED).map(|c| c.status),
            Some(CapabilityStatus::Unavailable)
        );
        for id in [ch::SLIP_RATIO_FL, ch::SLIP_ANGLE_RR, ch::LOCAL_YELLOW_S1] {
            let cap = caps.get(id).expect("experimental channel");
            assert_eq!(cap.status, CapabilityStatus::Unavailable, "{id:?}");
            assert_eq!(cap.note.as_deref(), Some(NOTE_EXPERIMENTAL), "{id:?}");
        }
        assert_eq!(caps.count(CapabilityStatus::Calculated), 0);
        assert_eq!(
            caps.get(ch::RPM).map(|c| c.status),
            Some(CapabilityStatus::Unavailable)
        );
        assert!(caps.set_status(ch::RPM, CapabilityStatus::Available, None));
        assert!(caps.set_rate(ch::RPM, 100.0));
        assert!(!caps.set_rate(ch::RPM, f32::NAN));
        assert!(!caps.set_status(ChannelId(0xFFFF), CapabilityStatus::Available, None));
        assert_eq!(
            caps.get(ch::RPM).and_then(|c| c.native_rate_hz),
            Some(100.0)
        );
        assert!(caps.count(CapabilityStatus::NotSupported) > 0);
    }
}

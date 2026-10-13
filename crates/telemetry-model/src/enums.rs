//! Decoded LMU enumerations (docs/LMU_CHANNEL_MAP.md).
//!
//! Every enum keeps unknown raw values verbatim in an `Unknown(raw)` variant instead of coercing them to a
//! known variant, so a new LMU value can never be silently misread. Construct values with `from_raw`;
//! `raw()` returns the exact LMU value for recording and export.
//!
//! Serde (postcard on the wire and in recordings, JSON elsewhere) carries the **raw LMU value**, never the
//! declaration index: adding a known variant in a later version cannot shift the encoding of `Unknown(raw)`
//! or of any later field, and decoding always yields the canonical variant (a peer's `Unknown(5)` reads as
//! `GamePhase::GreenFlag`).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Declares an LMU enum with explicit raw values, an `Unknown(raw)` fallback, `from_raw`, `raw` and `label`.
macro_rules! lmu_enum {
    (
        $(#[$meta:meta])*
        $name:ident : $raw:ty {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal => $label:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A raw value this version does not know, kept verbatim.
            Unknown($raw),
        }

        /// Serialised as the raw LMU value (module docs).
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.raw().serialize(serializer)
            }
        }

        /// Decoded from the raw LMU value into the canonical variant (module docs).
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                <$raw>::deserialize(deserializer).map(Self::from_raw)
            }
        }

        impl $name {
            /// Decodes the raw LMU value; values this version does not know become [`Self::Unknown`].
            pub const fn from_raw(raw: $raw) -> Self {
                match raw {
                    $( $value => Self::$variant, )+
                    other => Self::Unknown(other),
                }
            }

            /// The exact raw LMU value.
            pub const fn raw(self) -> $raw {
                match self {
                    $( Self::$variant => $value, )+
                    Self::Unknown(other) => other,
                }
            }

            /// Short human-readable label.
            pub const fn label(self) -> &'static str {
                match self {
                    $( Self::$variant => $label, )+
                    Self::Unknown(_) => "Unknown",
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::Unknown(raw) => write!(f, "Unknown ({raw})"),
                    known => f.write_str(known.label()),
                }
            }
        }
    };
}

lmu_enum! {
    /// Session-wide game phase (`scoring.scoringInfo.mGamePhase`).
    GamePhase: u8 {
        BeforeSession = 0 => "Before session",
        Reconnaissance = 1 => "Reconnaissance laps",
        GridWalk = 2 => "Grid walk",
        Formation = 3 => "Formation lap",
        Countdown = 4 => "Starting light countdown",
        GreenFlag = 5 => "Green flag",
        /// Full-course yellow or safety car.
        FullCourseYellow = 6 => "Full-course yellow / safety car",
        SessionStopped = 7 => "Session stopped",
        SessionOver = 8 => "Session over",
        Paused = 9 => "Paused",
    }
}

lmu_enum! {
    /// Full-course yellow state (`scoring.scoringInfo.mYellowFlagState`).
    YellowFlagState: i8 {
        Invalid = -1 => "Invalid",
        NoFlag = 0 => "None",
        Pending = 1 => "Pending",
        PitsClosed = 2 => "Pits closed",
        PitLeadLap = 3 => "Pit lead lap",
        PitsOpen = 4 => "Pits open",
        LastLap = 5 => "Last lap",
        Resume = 6 => "Resume",
        RaceHalt = 7 => "Race halt",
    }
}

lmu_enum! {
    /// Per-vehicle phase (`vehScoringInfo[i].mIndividualPhase`): game phases plus three per-car states.
    IndividualPhase: u8 {
        BeforeSession = 0 => "Before session",
        Reconnaissance = 1 => "Reconnaissance laps",
        GridWalk = 2 => "Grid walk",
        Formation = 3 => "Formation lap",
        Countdown = 4 => "Starting light countdown",
        GreenFlag = 5 => "Green flag",
        FullCourseYellow = 6 => "Full-course yellow / safety car",
        SessionStopped = 7 => "Session stopped",
        SessionOver = 8 => "Session over",
        AfterFormation = 9 => "After formation",
        UnderYellow = 10 => "Under yellow",
        UnderBlue = 11 => "Under blue",
    }
}

lmu_enum! {
    /// Pit state (`vehScoringInfo[i].mPitState`). 0..4 are documented by the header; 5 is observed only.
    PitState: u8 {
        Idle = 0 => "None",
        Requested = 1 => "Requested",
        Entering = 2 => "Entering",
        Stopped = 3 => "Stopped",
        Exiting = 4 => "Exiting",
        /// OBSERVED, NOT DOCUMENTED (A-LMU-063): LMU 1.4.2.0 reports 5 while the car sits in its garage stall
        /// and while it drives from the garage down the pit lane to the track (seen 0 → 1 → 5 → 0 on a return
        /// to the garage); never on track. The meaning is inferred from where it was seen.
        Garage = 5 => "Garage / leaving garage",
    }
}

lmu_enum! {
    /// Finish status (`vehScoringInfo[i].mFinishStatus`).
    FinishStatus: i8 {
        NotFinished = 0 => "None",
        Finished = 1 => "Finished",
        Dnf = 2 => "DNF",
        Dq = 3 => "DQ",
    }
}

lmu_enum! {
    /// Who controls a vehicle (`vehScoringInfo[i].mControl`).
    ControlKind: i8 {
        Nobody = -1 => "Nobody",
        LocalPlayer = 0 => "Local player",
        LocalAi = 1 => "Local AI",
        Remote = 2 => "Remote",
        Replay = 3 => "Replay",
    }
}

lmu_enum! {
    /// Per-car flag (`vehScoringInfo[i].mFlag`). LMU currently uses only green and blue.
    CarFlag: u8 {
        Green = 0 => "Green",
        Blue = 6 => "Blue",
    }
}

lmu_enum! {
    /// What a lap counts for (`vehScoringInfo[i].mCountLapFlag`).
    CountLapFlag: u8 {
        CountNothing = 0 => "Count nothing",
        LapOnly = 1 => "Count lap only",
        LapAndTime = 2 => "Count lap and time",
    }
}

lmu_enum! {
    /// Vehicle class (`telemInfo[p].mVehicleClass`, `IP_VehicleClass`). LMU's own "Unknown" (255) decodes
    /// to [`VehicleClass::Unknown`].
    VehicleClass: u8 {
        Hypercar = 0 => "Hypercar",
        Lmp2Elms = 2 => "LMP2 (ELMS)",
        Lmp2 = 3 => "LMP2",
        Lmp3 = 4 => "LMP3",
        Gte = 5 => "GTE",
        Gt3 = 6 => "LMGT3",
        PaceCar = 8 => "Pace car",
    }
}

lmu_enum! {
    /// Championship (`telemInfo[p].mVehicleChampionship`, `IP_VehicleChampionship`).
    VehicleChampionship: u8 {
        Wec2023 = 0 => "WEC 2023",
        Wec2024 = 1 => "WEC 2024",
        Wec2025 = 2 => "WEC 2025",
        Wec2026 = 3 => "WEC 2026",
        Elms2025 = 16 => "ELMS 2025",
        Elms2026 = 17 => "ELMS 2026",
    }
}

lmu_enum! {
    /// Network role (`scoring.scoringInfo.mGameMode`). 0 = offline is an inference (LMU_CHANNEL_MAP §1).
    GameMode: u8 {
        Offline = 0 => "Offline",
        Server = 1 => "Server",
        Client = 2 => "Client",
        ServerAndClient = 3 => "Server and client",
    }
}

lmu_enum! {
    /// Surface under a wheel (`mWheel[w].mSurfaceType`).
    SurfaceType: u8 {
        Dry = 0 => "Dry",
        Wet = 1 => "Wet",
        Grass = 2 => "Grass",
        Dirt = 3 => "Dirt",
        Gravel = 4 => "Gravel",
        RumbleStrip = 5 => "Rumble strip",
        Special = 6 => "Special",
    }
}

lmu_enum! {
    /// Tyre compound type (`mWheel[w].mCompoundType`). Mapping is CLAIMED (pyLMU), ASSUMPTION A-LMU-041.
    CompoundType: u8 {
        Soft = 0 => "Soft",
        Medium = 1 => "Medium",
        Hard = 2 => "Hard",
        Wet = 3 => "Wet",
    }
}

lmu_enum! {
    /// Electric boost motor state (`telemInfo[p].mElectricBoostMotorState`). `Unavailable` means the car
    /// has no hybrid system: every hybrid channel is NOT_SUPPORTED.
    HybridMotorState: u8 {
        Unavailable = 0 => "Unavailable",
        Inactive = 1 => "Inactive",
        Propulsion = 2 => "Propulsion",
        Regeneration = 3 => "Regeneration",
        /// OBSERVED, NOT DOCUMENTED (A-LMU-064): LMU 1.4.2.0 (Hypercar) reports 4 for 1.64 s right after the car
        /// is released from the garage stall (stationary, battery full, no torque), then 1. Meaning unknown
        /// beyond that; the car is hybrid while it is reported.
        GarageRelease = 4 => "After garage release (undocumented)",
    }
}

lmu_enum! {
    /// Ignition/starter state (`telemInfo[p].mIgnitionStarter`).
    IgnitionStarter: u8 {
        Off = 0 => "Off",
        Ignition = 1 => "Ignition",
        IgnitionAndStarter = 2 => "Ignition + starter",
    }
}

lmu_enum! {
    /// Rear-flap (DRS-style) legal status (`telemInfo[p].mRearFlapLegalStatus`).
    RearFlapLegalStatus: u8 {
        Disallowed = 0 => "Disallowed",
        CriteriaDetected = 1 => "Criteria detected",
        Allowed = 2 => "Allowed",
    }
}

lmu_enum! {
    /// Where the LMU UI is (`generic.appInfo.mOptionsLocation`). Drives the PAUSED state.
    OptionsLocation: u8 {
        MainUi = 0 => "Main UI",
        TrackLoading = 1 => "Track loading",
        Monitor = 2 => "Monitor",
        OnTrack = 3 => "On track",
    }
}

lmu_enum! {
    /// Dent severity at one of 8 body locations (`telemInfo[p].mDentSeverity[n]`; location order UNVERIFIED).
    DentSeverity: u8 {
        Intact = 0 => "None",
        Light = 1 => "Some",
        Heavy = 2 => "More",
    }
}

/// First raw `mSession` value of the practice block (practice 1).
const LMU_SESSION_PRACTICE_FIRST: i32 = 1;
/// Last raw `mSession` value of the practice block (practice 4).
const LMU_SESSION_PRACTICE_LAST: i32 = 4;
/// First raw `mSession` value of the qualifying block.
const LMU_SESSION_QUALIFYING_FIRST: i32 = 5;
/// Last raw `mSession` value of the qualifying block.
const LMU_SESSION_QUALIFYING_LAST: i32 = 8;
/// Raw `mSession` value of the warm-up.
const LMU_SESSION_WARMUP: i32 = 9;
/// First raw `mSession` value of the race block.
const LMU_SESSION_RACE_FIRST: i32 = 10;
/// Last raw `mSession` value of the race block.
const LMU_SESSION_RACE_LAST: i32 = 13;
/// Raw `mSession` value of the test day.
const LMU_SESSION_TEST_DAY: i32 = 0;

/// Session type decoded from `scoring.scoringInfo.mSession`: 0 test day, 1–4 practice, 5–8 qualifying,
/// 9 warm-up, 10–13 race (header comment, VERIFIED read). Numbered sessions carry their 1-based number
/// within the block (raw 6 = `Qualifying(2)`). Serde carries the raw `mSession` value (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionType {
    TestDay,
    Practice(u8),
    Qualifying(u8),
    Warmup,
    Race(u8),
    /// A raw value outside 0..=13, kept verbatim.
    Unknown(i32),
}

impl SessionType {
    /// Decodes the raw `mSession` value.
    pub const fn from_lmu(raw: i32) -> Self {
        match raw {
            LMU_SESSION_TEST_DAY => Self::TestDay,
            LMU_SESSION_PRACTICE_FIRST..=LMU_SESSION_PRACTICE_LAST => {
                Self::Practice(block_number(raw, LMU_SESSION_PRACTICE_FIRST))
            }
            LMU_SESSION_QUALIFYING_FIRST..=LMU_SESSION_QUALIFYING_LAST => {
                Self::Qualifying(block_number(raw, LMU_SESSION_QUALIFYING_FIRST))
            }
            LMU_SESSION_WARMUP => Self::Warmup,
            LMU_SESSION_RACE_FIRST..=LMU_SESSION_RACE_LAST => {
                Self::Race(block_number(raw, LMU_SESSION_RACE_FIRST))
            }
            other => Self::Unknown(other),
        }
    }

    /// The raw `mSession` value. A numbered variant whose number is outside its block (only possible when
    /// constructed by hand) maps to the block's first value.
    pub const fn raw(self) -> i32 {
        match self {
            Self::TestDay => LMU_SESSION_TEST_DAY,
            Self::Practice(n) => {
                block_raw(n, LMU_SESSION_PRACTICE_FIRST, LMU_SESSION_PRACTICE_LAST)
            }
            Self::Qualifying(n) => {
                block_raw(n, LMU_SESSION_QUALIFYING_FIRST, LMU_SESSION_QUALIFYING_LAST)
            }
            Self::Warmup => LMU_SESSION_WARMUP,
            Self::Race(n) => block_raw(n, LMU_SESSION_RACE_FIRST, LMU_SESSION_RACE_LAST),
            Self::Unknown(raw) => raw,
        }
    }

    /// Session kind without its number.
    pub const fn kind_label(self) -> &'static str {
        match self {
            Self::TestDay => "Test day",
            Self::Practice(_) => "Practice",
            Self::Qualifying(_) => "Qualifying",
            Self::Warmup => "Warm-up",
            Self::Race(_) => "Race",
            Self::Unknown(_) => "Unknown",
        }
    }

    /// True for race sessions.
    pub const fn is_race(self) -> bool {
        matches!(self, Self::Race(_))
    }
}

/// Serialised as the raw `mSession` value; a hand-made numbered variant outside its block (only possible when
/// constructed by hand, see [`SessionType::raw`]) is written as the block's first value.
impl Serialize for SessionType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw().serialize(serializer)
    }
}

/// Decoded from the raw `mSession` value, exactly like [`SessionType::from_lmu`].
impl<'de> Deserialize<'de> for SessionType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        i32::deserialize(deserializer).map(Self::from_lmu)
    }
}

/// 1-based position of `raw` within a block starting at `first`. Callers guarantee `first <= raw` and a block
/// shorter than 256 values, so the narrowing cannot truncate.
const fn block_number(raw: i32, first: i32) -> u8 {
    (raw - first + 1) as u8
}

/// Inverse of [`block_number`], clamped to the block.
const fn block_raw(n: u8, first: i32, last: i32) -> i32 {
    let raw = first + n as i32 - 1;
    if raw < first || raw > last {
        first
    } else {
        raw
    }
}

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Practice(n) | Self::Qualifying(n) | Self::Race(n) => {
                write!(f, "{} {n}", self.kind_label())
            }
            Self::Unknown(raw) => write!(f, "Unknown ({raw})"),
            Self::TestDay | Self::Warmup => f.write_str(self.kind_label()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_mapping_matches_header() {
        assert_eq!(SessionType::from_lmu(0), SessionType::TestDay);
        assert_eq!(SessionType::from_lmu(1), SessionType::Practice(1));
        assert_eq!(SessionType::from_lmu(4), SessionType::Practice(4));
        assert_eq!(SessionType::from_lmu(5), SessionType::Qualifying(1));
        assert_eq!(SessionType::from_lmu(8), SessionType::Qualifying(4));
        assert_eq!(SessionType::from_lmu(9), SessionType::Warmup);
        assert_eq!(SessionType::from_lmu(10), SessionType::Race(1));
        assert_eq!(SessionType::from_lmu(13), SessionType::Race(4));
        assert_eq!(SessionType::from_lmu(14), SessionType::Unknown(14));
        assert_eq!(SessionType::from_lmu(-1), SessionType::Unknown(-1));
        for raw in -2..=15 {
            assert_eq!(
                SessionType::from_lmu(raw).raw(),
                raw,
                "raw {raw} must round-trip"
            );
        }
        assert_eq!(SessionType::Qualifying(2).to_string(), "Qualifying 2");
        assert!(SessionType::Race(1).is_race());
    }

    #[test]
    fn unknown_values_are_preserved_not_coerced() {
        assert_eq!(GamePhase::from_raw(42), GamePhase::Unknown(42));
        assert_eq!(GamePhase::from_raw(42).raw(), 42);
        assert_eq!(YellowFlagState::from_raw(-1), YellowFlagState::Invalid);
        assert_eq!(YellowFlagState::from_raw(-5), YellowFlagState::Unknown(-5));
        assert_eq!(VehicleClass::from_raw(255), VehicleClass::Unknown(255));
        assert_eq!(VehicleClass::from_raw(6), VehicleClass::Gt3);
        assert_eq!(ControlKind::from_raw(-1), ControlKind::Nobody);
        assert_eq!(CarFlag::from_raw(6), CarFlag::Blue);
        assert_eq!(CarFlag::from_raw(3).to_string(), "Unknown (3)");
        for raw in 0..=u8::MAX {
            assert_eq!(PitState::from_raw(raw).raw(), raw);
            assert_eq!(SurfaceType::from_raw(raw).raw(), raw);
            assert_eq!(HybridMotorState::from_raw(raw).raw(), raw);
        }
    }

    /// Gate 1 (LMU 1.4.2.0): values outside the header's documented sets that real data showed are named
    /// variants now, and the next unknown value still stays verbatim.
    #[test]
    fn values_observed_at_gate1_are_known() {
        assert_eq!(PitState::from_raw(5), PitState::Garage);
        assert_eq!(PitState::Garage.raw(), 5);
        assert_eq!(PitState::from_raw(6), PitState::Unknown(6));
        assert_eq!(
            HybridMotorState::from_raw(4),
            HybridMotorState::GarageRelease
        );
        assert_eq!(HybridMotorState::GarageRelease.raw(), 4);
        assert_eq!(HybridMotorState::from_raw(5), HybridMotorState::Unknown(5));
    }

    /// Serialised as the raw LMU value, not the declaration index: adding a known variant can never shift the
    /// encoding of `Unknown(raw)` (old recordings stay readable) and decoding is always canonical.
    #[test]
    fn serialised_as_raw_lmu_values() {
        fn bytes<T: Serialize>(v: &T) -> Vec<u8> {
            postcard::to_stdvec(v).expect("serialize")
        }
        assert_eq!(bytes(&CarFlag::Blue), vec![6]);
        assert_eq!(bytes(&VehicleClass::Gt3), vec![6]);
        assert_eq!(bytes(&VehicleClass::Unknown(255)), vec![255]);
        assert_eq!(bytes(&YellowFlagState::Invalid), vec![0xFF], "i8 -1");
        assert_eq!(bytes(&GamePhase::GreenFlag), vec![5]);
        // i32 raw: zigzag varint, raw 6 → 12.
        assert_eq!(bytes(&SessionType::Qualifying(2)), vec![12]);

        // A raw value always decodes to its canonical variant, whatever a peer wrote.
        let phase: GamePhase = postcard::from_bytes(&[5]).expect("decode");
        assert_eq!(phase, GamePhase::GreenFlag);
        let unknown: GamePhase = postcard::from_bytes(&[200]).expect("decode");
        assert_eq!(unknown, GamePhase::Unknown(200));
        let session: SessionType = postcard::from_bytes(&[12]).expect("decode");
        assert_eq!(session, SessionType::Qualifying(2));
        // A hand-made non-canonical value is normalised by a round trip.
        let back: GamePhase = postcard::from_bytes(&bytes(&GamePhase::Unknown(5))).expect("decode");
        assert_eq!(back, GamePhase::GreenFlag);
        for raw in 0..=u8::MAX {
            let v = PitState::from_raw(raw);
            let back: PitState = postcard::from_bytes(&bytes(&v)).expect("decode");
            assert_eq!(back.raw(), raw);
        }
        for raw in -3..=16 {
            let v = SessionType::from_lmu(raw);
            let back: SessionType = postcard::from_bytes(&bytes(&v)).expect("decode");
            assert_eq!(back, v, "raw {raw}");
        }
        assert_eq!(
            serde_json::to_string(&CarFlag::Blue).expect("json"),
            "6",
            "JSON also carries the raw value"
        );
    }

    #[test]
    fn game_phase_labels() {
        assert_eq!(
            GamePhase::from_raw(6).label(),
            "Full-course yellow / safety car"
        );
        assert_eq!(
            IndividualPhase::from_raw(9),
            IndividualPhase::AfterFormation
        );
        assert_eq!(GamePhase::from_raw(9), GamePhase::Paused);
    }
}

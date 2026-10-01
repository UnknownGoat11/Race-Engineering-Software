//! Compatibility checks and the [`CompatibilityReport`] (spec §189-191, D-003).
//!
//! Two kinds of checks run on every new frame:
//! * **structural** checks read raw bytes that discriminate between layouts (mapping size, the
//!   default-pack hole at 2180, `scoringStreamSize`, the slot counts, the `playerHasVehicle` bool).
//!   A failure means the bytes cannot be trusted at all: the frame is not decoded and, if failures
//!   persist, the reader stops with [`crate::LmuError::IncompatibleInterface`]. It never guesses.
//! * **content** checks look at decoded values (versions, ETs, strings, bool bytes, player
//!   identity). On their own they only produce warnings (`SUPPORTED_WITH_WARNINGS`): the runtime
//!   behaviour of LMU is still UNVERIFIED (research §13) and the owner reviews these warnings during
//!   Gate 1.
//! * **layout-symptom** checks (A-LMU-007) combine content symptoms that together mean the offsets
//!   are wrong: the player slot with an implausible ET/`mDeltaTime`, unprintable names *and* an
//!   `mID` that matches no player row; or every scoring row's driver name unprintable together with
//!   the track name. They fail like structural checks: the frame is not delivered.
//!
//! The game-version verdict is reported separately ([`CompatibilityReport::version_verified`]) and
//! does not lower the per-frame / per-section support levels: until Gate 1 verifies a version it
//! would mark every frame `SUPPORTED_WITH_WARNINGS` and the level would carry no information.
//!
//! Per frame only cheap verdicts are computed; the strings of the report are rendered on demand.

use std::fmt;

use serde::Serialize;

use crate::blocks::{PADDING_LEN, RawBlocks};
use crate::bytes::RawString;
use crate::offsets::{
    MAX_SCORING_VEHICLES, MAX_TELEMETRY_VEHICLES, SCORING_STREAM_CAPACITY, WHEEL_COUNT, abs,
    layout, scoring_info, telem_info, telem_wheel, veh_scoring,
};
use crate::raw::SharedMemoryEvent;
use crate::snapshot::{PlayerIdentity, PlayerTelemetry, ScoringBlock, TelemetryHeader};
use crate::state::{LockMode, SyncMode};

/// The probe output these offsets come from.
pub const LAYOUT_SOURCE: &str = "docs/research/lmu_abi_layout.json";

/// The LMU build whose SDK headers were probed (research §3).
pub const PROBED_LMU_BUILD: &str = "LMU 1.4.2.0 (Steam build 25513257)";

/// `gameVersion` values that have been verified against this layout with LMU running.
/// Empty until the Gate-1 session records one (A-LMU-010); until then every version is reported as
/// `UNKNOWN_VERSION` (a warning) and decoding continues as long as the structural checks pass.
pub const VERIFIED_GAME_VERSIONS: &[i32] = &[];

/// Upper bound of a plausible `mDeltaTime` (research §13: in (0, 0.1] s).
const MAX_PLAUSIBLE_DELTA_TIME_S: f64 = 0.1;

/// Page size used to round the expected `LMU_Data` region size.
const PAGE_SIZE: usize = 4_096;

/// Largest `VirtualQuery` region a section of exactly `sizeof(SharedMemoryLayout)` bytes produces
/// (327,680). A larger region is evidence that LMU's layout grew.
pub const MAPPING_EXPECTED_MAX: usize = layout::SIZE.div_ceil(PAGE_SIZE) * PAGE_SIZE;

/// Support level of one section or of the whole interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SupportLevel {
    /// Every check passed.
    Supported,
    /// Structural checks passed; some content checks warned.
    SupportedWithWarnings,
    /// A structural check failed: nothing is decoded.
    Unsupported,
}

impl SupportLevel {
    /// Upper-case label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "SUPPORTED",
            Self::SupportedWithWarnings => "SUPPORTED_WITH_WARNINGS",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

impl fmt::Display for SupportLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A decoded section of `LMU_Data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Section {
    /// `data.generic`.
    Generic,
    /// `data.scoring`.
    Scoring,
    /// `data.telemetry`.
    Telemetry,
}

/// Result of one check on one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    /// As expected.
    Pass,
    /// Unexpected but not structural; decoding continues.
    Warn,
    /// Structural failure; the frame is not decoded.
    Fail,
}

/// Identifies a compatibility check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckId {
    /// `VirtualQuery` region size of the view ≥ end of the last member (324,820).
    MappingSize,
    /// The 4 default-pack padding bytes at absolute 2180 are zero.
    ScoringPadding,
    /// `scoringStreamSize` ≤ 65,536.
    ScoringStreamSize,
    /// `mNumVehicles` in 0..=104.
    NumVehicles,
    /// `activeVehicles` ≤ 104.
    ActiveVehicles,
    /// `playerHasVehicle` byte is 0 or 1.
    PlayerHasVehicle,
    /// `gameVersion` recorded and compared with the verified list.
    GameVersion,
    /// Every `events[]` value is a known `SharedMemoryEvent` code.
    EventCodes,
    /// `playerVehicleIdx < min(activeVehicles, 104)` while `playerHasVehicle`.
    PlayerIndex,
    /// Scoring and telemetry vehicle counts agree.
    CountAgreement,
    /// Exactly one `mIsPlayer` scoring row, with the player telemetry `mID`.
    PlayerScoringRow,
    /// `mCurrentET` finite and ≥ 0.
    ScoringEt,
    /// Track and per-row names printable and NUL-terminated.
    ScoringStrings,
    /// Every C++ `bool` byte in the scoring block is 0 or 1.
    ScoringBools,
    /// Player `mElapsedTime` finite and ≥ 0, `mDeltaTime` in (0, 0.1] s.
    TelemetryEt,
    /// Player `mElapsedTime` did not go backwards.
    TelemetryEtMonotonic,
    /// Player vehicle and track names printable and NUL-terminated.
    TelemetryStrings,
    /// Every C++ `bool` byte in the player telemetry slot is 0 or 1.
    TelemetryBools,
    /// A-LMU-007 layout symptoms in the player slot: implausible ET / `mDeltaTime`, bad names and an
    /// `mID` that matches no player row, all on the same frame.
    TelemetryLayout,
    /// A-LMU-007 layout symptoms in scoring: every row's names and the track name unprintable.
    ScoringLayout,
}

/// Every check, in report order.
pub const ALL_CHECKS: [CheckId; 20] = [
    CheckId::MappingSize,
    CheckId::ScoringPadding,
    CheckId::ScoringStreamSize,
    CheckId::NumVehicles,
    CheckId::ActiveVehicles,
    CheckId::PlayerHasVehicle,
    CheckId::GameVersion,
    CheckId::EventCodes,
    CheckId::PlayerIndex,
    CheckId::CountAgreement,
    CheckId::PlayerScoringRow,
    CheckId::ScoringEt,
    CheckId::ScoringStrings,
    CheckId::ScoringBools,
    CheckId::TelemetryEt,
    CheckId::TelemetryEtMonotonic,
    CheckId::TelemetryStrings,
    CheckId::TelemetryBools,
    CheckId::TelemetryLayout,
    CheckId::ScoringLayout,
];

const CHECK_COUNT: usize = ALL_CHECKS.len();

impl CheckId {
    /// Checks whose failure stops decoding (raw structural checks and layout symptoms); the others
    /// only warn.
    pub fn is_structural(self) -> bool {
        self.is_raw_structural() || self.is_layout_symptom()
    }

    /// Structural checks evaluated on raw bytes before anything is decoded.
    fn is_raw_structural(self) -> bool {
        matches!(
            self,
            Self::MappingSize
                | Self::ScoringPadding
                | Self::ScoringStreamSize
                | Self::NumVehicles
                | Self::ActiveVehicles
                | Self::PlayerHasVehicle
        )
    }

    /// Layout-symptom checks (A-LMU-007), evaluated on decoded values but failing like structural ones.
    fn is_layout_symptom(self) -> bool {
        matches!(self, Self::TelemetryLayout | Self::ScoringLayout)
    }

    /// `false` for the game-version check, which is reported separately and does not lower the
    /// support levels.
    fn affects_support(self) -> bool {
        self != Self::GameVersion
    }

    /// Sections whose bytes the check vouches for.
    pub fn sections(self) -> &'static [Section] {
        match self {
            Self::MappingSize => &[Section::Generic, Section::Scoring, Section::Telemetry],
            // The hole and the stream size sit before vehScoringInfo and telemetry: a different
            // packing shifts both sections.
            Self::ScoringPadding | Self::ScoringStreamSize => {
                &[Section::Scoring, Section::Telemetry]
            }
            Self::GameVersion | Self::EventCodes => &[Section::Generic],
            Self::NumVehicles
            | Self::CountAgreement
            | Self::PlayerScoringRow
            | Self::ScoringEt
            | Self::ScoringStrings
            | Self::ScoringBools => &[Section::Scoring],
            // Scoring sits before telemetry: a shifted scoring layout shifts both.
            Self::ScoringLayout => &[Section::Scoring, Section::Telemetry],
            Self::ActiveVehicles
            | Self::PlayerHasVehicle
            | Self::PlayerIndex
            | Self::TelemetryEt
            | Self::TelemetryEtMonotonic
            | Self::TelemetryStrings
            | Self::TelemetryBools
            | Self::TelemetryLayout => &[Section::Telemetry],
        }
    }

    /// Short human-readable title.
    pub fn title(self) -> &'static str {
        match self {
            Self::MappingSize => "LMU_Data region size",
            Self::ScoringPadding => "padding bytes at offset 2180",
            Self::ScoringStreamSize => "scoringStreamSize at offset 2184",
            Self::NumVehicles => "scoringInfo.mNumVehicles",
            Self::ActiveVehicles => "telemetry.activeVehicles",
            Self::PlayerHasVehicle => "telemetry.playerHasVehicle byte",
            Self::GameVersion => "generic.gameVersion",
            Self::EventCodes => "generic.events[] codes",
            Self::PlayerIndex => "telemetry.playerVehicleIdx",
            Self::CountAgreement => "scoring vs telemetry vehicle count",
            Self::PlayerScoringRow => "player scoring row (mIsPlayer, mID)",
            Self::ScoringEt => "scoringInfo.mCurrentET",
            Self::ScoringStrings => "scoring strings",
            Self::ScoringBools => "scoring bool bytes",
            Self::TelemetryEt => "player mElapsedTime / mDeltaTime",
            Self::TelemetryEtMonotonic => "player mElapsedTime monotonic",
            Self::TelemetryStrings => "player telemetry strings",
            Self::TelemetryBools => "player telemetry bool bytes",
            Self::TelemetryLayout => "player telemetry slot layout (A-LMU-007)",
            Self::ScoringLayout => "scoring layout (A-LMU-007)",
        }
    }

    fn index(self) -> usize {
        ALL_CHECKS.iter().position(|&c| c == self).unwrap_or(0)
    }
}

/// Raw values the structural checks look at, read from the copied bytes without decoding.
/// `None` means "not read" (e.g. the mapping was too small to copy anything).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct StructuralFacts {
    pub(crate) mapping_size: Option<usize>,
    pub(crate) padding: Option<[u8; PADDING_LEN]>,
    pub(crate) stream_size: Option<u64>,
    pub(crate) num_vehicles: Option<i32>,
    pub(crate) active_vehicles: Option<u8>,
    pub(crate) player_idx: Option<u8>,
    pub(crate) player_has_vehicle: Option<u8>,
}

impl StructuralFacts {
    /// Facts for a view that could not be copied (only its size is known).
    pub(crate) fn mapping_only(mapping_size: usize) -> Self {
        Self {
            mapping_size: Some(mapping_size),
            ..Self::default()
        }
    }

    /// Reads the structural facts from a copied frame.
    pub(crate) fn from_blocks(blocks: &RawBlocks, mapping_size: Option<usize>) -> Self {
        Self {
            mapping_size,
            padding: blocks.scoring_padding(),
            stream_size: blocks.scoring_stream_size_raw(),
            num_vehicles: blocks.num_vehicles_raw(),
            active_vehicles: blocks.active_vehicles_raw(),
            player_idx: blocks.player_vehicle_idx_raw(),
            player_has_vehicle: blocks.player_has_vehicle_raw(),
        }
    }
}

/// One C++ `bool` byte that is neither 0 nor 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BadBool {
    field: &'static str,
    /// Scoring row or wheel index, when the field is inside an array element.
    index: Option<usize>,
    value: u8,
}

/// Printability of one fixed-size string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StrOk {
    printable: bool,
    terminated: bool,
}

impl StrOk {
    fn of(s: &RawString) -> Self {
        Self {
            printable: s.printable,
            terminated: s.nul_terminated,
        }
    }

    fn ok(self) -> bool {
        self.printable && self.terminated
    }
}

/// Values the player-telemetry checks look at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlayerFacts {
    slot: u8,
    id: i32,
    elapsed_time: f64,
    delta_time: f64,
    vehicle_name: StrOk,
    track_name: StrOk,
    bad_bools: usize,
    first_bad_bool: Option<BadBool>,
}

/// Values the content checks look at, taken from the decoded frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContentFacts {
    game_version: i32,
    unknown_events: usize,
    first_unknown_event: Option<(usize, u32)>,
    scoring_et: f64,
    scoring_rows: usize,
    header: TelemetryHeader,
    player: Option<PlayerFacts>,
    player_rows: usize,
    player_row_id: Option<i32>,
    track_name: StrOk,
    bad_string_rows: usize,
    first_bad_string_row: Option<usize>,
    scoring_bad_bools: usize,
    first_scoring_bad_bool: Option<BadBool>,
    et_regression: Option<(f64, f64)>,
    identity: PlayerIdentity,
}

/// Inputs of [`ContentFacts::collect`] besides the copied blocks.
pub(crate) struct ContentInput<'a> {
    pub(crate) game_version: i32,
    pub(crate) events: &'a [u32],
    pub(crate) scoring: &'a ScoringBlock,
    pub(crate) header: TelemetryHeader,
    /// The (unverified) slot named by `playerVehicleIdx`: the layout checks look at it.
    pub(crate) index_slot: Option<&'a PlayerTelemetry>,
    /// `(previous, current)` player `mElapsedTime` when it went backwards for the same car.
    pub(crate) et_regression: Option<(f64, f64)>,
    /// How the player was identified.
    pub(crate) identity: PlayerIdentity,
}

impl ContentFacts {
    /// Collects the content facts of one decoded frame.
    pub(crate) fn collect(blocks: &RawBlocks, input: &ContentInput<'_>) -> Self {
        let ContentInput {
            game_version,
            events,
            scoring,
            header,
            index_slot: player,
            et_regression,
            identity,
        } = *input;
        let first_unknown_event = events
            .iter()
            .enumerate()
            .find(|(_, code)| !SharedMemoryEvent::from_raw(**code).is_known())
            .map(|(slot, &code)| (slot, code));
        let unknown_events = events
            .iter()
            .filter(|code| !SharedMemoryEvent::from_raw(**code).is_known())
            .count();

        let row_ok = |r: &crate::raw_structs::RawVehicleScoring| {
            StrOk::of(&r.driver_name).ok()
                && StrOk::of(&r.vehicle_name).ok()
                && StrOk::of(&r.vehicle_class).ok()
        };
        let first_bad_string_row = scoring.vehicles.iter().position(|r| !row_ok(r));
        let bad_string_rows = scoring.vehicles.iter().filter(|r| !row_ok(r)).count();
        let player_rows = scoring.vehicles.iter().filter(|r| r.is_player).count();
        let player_row_id = scoring.player_row().map(|r| r.id);
        let (scoring_bad_bools, first_scoring_bad_bool) = scoring_bad_bools(blocks);

        let player = player.map(|p| {
            let (bad_bools, first_bad_bool) = telemetry_bad_bools(blocks);
            PlayerFacts {
                slot: p.slot,
                id: p.telemetry.id,
                elapsed_time: p.telemetry.elapsed_time,
                delta_time: p.telemetry.delta_time,
                vehicle_name: StrOk::of(&p.telemetry.vehicle_name),
                track_name: StrOk::of(&p.telemetry.track_name),
                bad_bools,
                first_bad_bool,
            }
        });

        Self {
            game_version,
            unknown_events,
            first_unknown_event,
            scoring_et: scoring.info.current_et,
            scoring_rows: scoring.vehicles.len(),
            header,
            player,
            player_rows,
            player_row_id,
            track_name: StrOk::of(&scoring.info.track_name),
            bad_string_rows,
            first_bad_string_row,
            scoring_bad_bools,
            first_scoring_bad_bool,
            et_regression,
            identity,
        }
    }

    /// A-LMU-007: the slot `playerVehicleIdx` names shows every symptom of a wrong layout at once.
    fn telemetry_layout_symptoms(&self) -> Option<bool> {
        let p = self.player?;
        let id_matches = self.player_rows == 1 && self.player_row_id == Some(p.id);
        Some(!telemetry_et_ok(&p) && !(p.vehicle_name.ok() && p.track_name.ok()) && !id_matches)
    }

    /// A-LMU-007: every scoring row's names and the track name are unprintable.
    fn scoring_layout_symptoms(&self) -> Option<bool> {
        (self.scoring_rows > 0)
            .then_some(self.bad_string_rows == self.scoring_rows && !self.track_name.ok())
    }
}

/// Verdict of [`CompatTracker::observe_content`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentVerdict {
    /// Support level of the frame (game version excluded).
    pub(crate) support: SupportLevel,
    /// A layout-symptom check failed: the frame must not be delivered.
    pub(crate) reject: bool,
    /// `gameVersion` is in [`VERIFIED_GAME_VERSIONS`].
    pub(crate) version_verified: bool,
}

/// Counts non-0/1 bool bytes in `scoringInfo` and the copied vehicle rows.
fn scoring_bad_bools(blocks: &RawBlocks) -> (usize, Option<BadBool>) {
    let mut found = BadBools::default();
    let info = blocks.scoring_header();
    for &(field, f) in scoring_info::BOOL_FIELDS {
        found.check(info, f.offset, f.size, field, None);
    }
    for (row, bytes) in blocks
        .vehicles()
        .as_chunks::<{ veh_scoring::SIZE }>()
        .0
        .iter()
        .enumerate()
    {
        for &(field, f) in veh_scoring::BOOL_FIELDS {
            found.check(bytes, f.offset, f.size, field, Some(row));
        }
    }
    (found.count, found.first)
}

/// Counts non-0/1 bool bytes in the copied player telemetry slot (including its wheels).
fn telemetry_bad_bools(blocks: &RawBlocks) -> (usize, Option<BadBool>) {
    let mut found = BadBools::default();
    let Some((_, slot)) = blocks.player_index_slot() else {
        return (0, None);
    };
    for &(field, f) in telem_info::BOOL_FIELDS {
        found.check(slot, f.offset, f.size, field, None);
    }
    for wheel in 0..WHEEL_COUNT {
        let base = telem_info::WHEEL.offset + wheel * telem_wheel::SIZE;
        for &(field, f) in telem_wheel::BOOL_FIELDS {
            found.check(slot, base + f.offset, f.size, field, Some(wheel));
        }
    }
    (found.count, found.first)
}

#[derive(Default)]
struct BadBools {
    count: usize,
    first: Option<BadBool>,
}

impl BadBools {
    fn check(
        &mut self,
        bytes: &[u8],
        offset: usize,
        size: usize,
        field: &'static str,
        index: Option<usize>,
    ) {
        let Some(values) = bytes.get(offset..offset + size) else {
            return;
        };
        for &value in values {
            if value > 1 {
                self.count += 1;
                self.first.get_or_insert(BadBool {
                    field,
                    index,
                    value,
                });
            }
        }
    }
}

/// Cheap per-frame verdict of one check (`None` = not applicable to this frame).
fn verdict(id: CheckId, s: &StructuralFacts, c: Option<&ContentFacts>) -> Option<Outcome> {
    let pass_or = |ok: bool, otherwise: Outcome| if ok { Outcome::Pass } else { otherwise };
    let fail = |ok: bool| pass_or(ok, Outcome::Fail);
    let warn = |ok: bool| pass_or(ok, Outcome::Warn);
    match id {
        CheckId::MappingSize => s.mapping_size.map(|size| {
            if size < abs::MAPPING_END {
                Outcome::Fail
            } else if size > MAPPING_EXPECTED_MAX {
                // Larger than a section of sizeof(SharedMemoryLayout): the layout may have grown.
                Outcome::Warn
            } else {
                Outcome::Pass
            }
        }),
        CheckId::ScoringPadding => s.padding.map(|p| fail(p == [0; PADDING_LEN])),
        CheckId::ScoringStreamSize => s
            .stream_size
            .map(|n| fail(n <= SCORING_STREAM_CAPACITY as u64)),
        CheckId::NumVehicles => s
            .num_vehicles
            .map(|n| fail(count_in_range(n, MAX_SCORING_VEHICLES))),
        CheckId::ActiveVehicles => s
            .active_vehicles
            .map(|n| fail(usize::from(n) <= MAX_TELEMETRY_VEHICLES)),
        CheckId::PlayerHasVehicle => s.player_has_vehicle.map(|b| fail(b <= 1)),
        CheckId::GameVersion => c.map(|c| warn(VERIFIED_GAME_VERSIONS.contains(&c.game_version))),
        CheckId::EventCodes => c.map(|c| warn(c.unknown_events == 0)),
        CheckId::PlayerIndex => c.map(|c| warn(player_index_ok(c.header))),
        CheckId::CountAgreement => c.map(|c| warn(c.scoring_rows == clamped_active(c.header))),
        CheckId::PlayerScoringRow => {
            let c = c?;
            c.player
                .map(|p| warn(c.player_rows == 1 && c.player_row_id == Some(p.id)))
        }
        CheckId::ScoringEt => c.map(|c| warn(c.scoring_et.is_finite() && c.scoring_et >= 0.0)),
        CheckId::ScoringStrings => c.map(|c| warn(c.track_name.ok() && c.bad_string_rows == 0)),
        CheckId::ScoringBools => c.map(|c| warn(c.scoring_bad_bools == 0)),
        CheckId::TelemetryEt => c?.player.map(|p| warn(telemetry_et_ok(&p))),
        CheckId::TelemetryEtMonotonic => {
            let c = c?;
            c.player.map(|_| warn(c.et_regression.is_none()))
        }
        CheckId::TelemetryStrings => c?
            .player
            .map(|p| warn(p.vehicle_name.ok() && p.track_name.ok())),
        CheckId::TelemetryBools => c?.player.map(|p| warn(p.bad_bools == 0)),
        CheckId::TelemetryLayout => c?.telemetry_layout_symptoms().map(|bad| fail(!bad)),
        CheckId::ScoringLayout => c?.scoring_layout_symptoms().map(|bad| fail(!bad)),
    }
}

fn count_in_range(n: i32, max: usize) -> bool {
    usize::try_from(n).is_ok_and(|n| n <= max)
}

fn clamped_active(h: TelemetryHeader) -> usize {
    usize::from(h.active_vehicles).min(MAX_TELEMETRY_VEHICLES)
}

fn player_index_ok(h: TelemetryHeader) -> bool {
    h.player_has_vehicle == 0 || usize::from(h.player_vehicle_idx) < clamped_active(h)
}

fn telemetry_et_ok(p: &PlayerFacts) -> bool {
    p.elapsed_time.is_finite()
        && p.elapsed_time >= 0.0
        && p.delta_time.is_finite()
        && p.delta_time > 0.0
        && p.delta_time <= MAX_PLAUSIBLE_DELTA_TIME_S
}

fn identity_text(identity: PlayerIdentity) -> String {
    match identity {
        PlayerIdentity::NoVehicle => "no vehicle".to_owned(),
        PlayerIdentity::Verified => "verified".to_owned(),
        PlayerIdentity::FoundById { .. } => {
            "found by mID in another slot (A-LMU-020 fallback)".to_owned()
        }
        PlayerIdentity::Unverified { reason } => format!("UNAVAILABLE ({reason:?})"),
    }
}

fn opt<T: fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "not read".to_owned(), |v| v.to_string())
}

fn str_state(s: StrOk) -> &'static str {
    match (s.printable, s.terminated) {
        (true, true) => "printable, NUL-terminated",
        (true, false) => "printable, NOT NUL-terminated",
        (false, true) => "NOT printable",
        (false, false) => "NOT printable, NOT NUL-terminated",
    }
}

fn bad_bool_text(count: usize, first: Option<BadBool>) -> String {
    match first {
        None => "all 0/1".to_owned(),
        Some(b) => {
            let at = b.index.map_or_else(String::new, |i| format!("[{i}]"));
            format!(
                "{count} byte(s) not 0/1, first {}{at} = {}",
                b.field, b.value
            )
        }
    }
}

/// Renders `(detected, expected)` for the report.
fn describe(id: CheckId, s: &StructuralFacts, c: Option<&ContentFacts>) -> (String, String) {
    let na = || ("not evaluated (no data)".to_owned(), String::new());
    match id {
        CheckId::MappingSize => (
            s.mapping_size.map_or_else(
                || "not measured (not attached, or a capture replay)".to_owned(),
                |n| format!("{n} bytes"),
            ),
            format!(
                "{} ..= {MAPPING_EXPECTED_MAX} bytes (larger: the layout may have grown)",
                abs::MAPPING_END
            ),
        ),
        CheckId::ScoringPadding => (
            s.padding
                .map_or_else(|| "not read".to_owned(), |p| format!("{p:02x?}")),
            "[00, 00, 00, 00] (MSVC default packing, A-LMU-007)".to_owned(),
        ),
        CheckId::ScoringStreamSize => (opt(s.stream_size), format!("<= {SCORING_STREAM_CAPACITY}")),
        CheckId::NumVehicles => (opt(s.num_vehicles), format!("0..={MAX_SCORING_VEHICLES}")),
        CheckId::ActiveVehicles => (
            opt(s.active_vehicles),
            format!("0..={MAX_TELEMETRY_VEHICLES}"),
        ),
        CheckId::PlayerHasVehicle => (opt(s.player_has_vehicle), "0 or 1".to_owned()),
        CheckId::GameVersion => match c {
            None => na(),
            Some(c) => {
                let state = if VERIFIED_GAME_VERSIONS.contains(&c.game_version) {
                    "verified"
                } else if c.game_version == 0 {
                    "UNKNOWN_VERSION (0: not published yet?)"
                } else {
                    "UNKNOWN_VERSION (not yet verified at runtime; decoding continues while structural checks pass)"
                };
                let expected = format!(
                    "a runtime-verified value, verified list {VERIFIED_GAME_VERSIONS:?} (A-LMU-010: empty until \
                     Gate 1 records one; probably 14200 on {PROBED_LMU_BUILD})"
                );
                (format!("{} {state}", c.game_version), expected)
            }
        },
        CheckId::EventCodes => match c {
            None => na(),
            Some(c) => (
                match c.first_unknown_event {
                    None => "all codes < SME_MAX".to_owned(),
                    Some((slot, code)) => format!(
                        "{} unknown code(s), first events[{slot}] = {code}",
                        c.unknown_events
                    ),
                },
                "codes 0..=15".to_owned(),
            ),
        },
        CheckId::PlayerIndex => match c {
            None => na(),
            Some(c) => (
                format!(
                    "playerHasVehicle={} playerVehicleIdx={} activeVehicles={}",
                    c.header.player_has_vehicle,
                    c.header.player_vehicle_idx,
                    c.header.active_vehicles
                ),
                "no vehicle, or idx < min(activeVehicles, 104)".to_owned(),
            ),
        },
        CheckId::CountAgreement => match c {
            None => na(),
            Some(c) => (
                format!(
                    "{} scoring rows, {} telemetry slots",
                    c.scoring_rows,
                    clamped_active(c.header)
                ),
                "equal (may differ briefly while cars join)".to_owned(),
            ),
        },
        CheckId::PlayerScoringRow => match c.and_then(|c| c.player.map(|p| (c, p))) {
            None => ("no player telemetry".to_owned(), String::new()),
            Some((c, p)) => (
                format!(
                    "{} mIsPlayer row(s), row mID {}, telemInfo[{}] mID {}; player {}",
                    c.player_rows,
                    opt(c.player_row_id),
                    p.slot,
                    p.id,
                    identity_text(c.identity)
                ),
                "exactly one row, same mID as the slot playerVehicleIdx names".to_owned(),
            ),
        },
        CheckId::TelemetryLayout => match c.and_then(|c| c.telemetry_layout_symptoms()) {
            None => ("no player telemetry".to_owned(), String::new()),
            Some(bad) => (
                if bad {
                    "implausible ET/dt, bad names AND no matching mID on the same frame: the \
                     telemetry offsets are probably wrong"
                        .to_owned()
                } else {
                    "no combined layout symptoms".to_owned()
                },
                "not all symptoms at once".to_owned(),
            ),
        },
        CheckId::ScoringLayout => match c.and_then(|c| c.scoring_layout_symptoms().map(|b| (c, b)))
        {
            None => ("no scoring rows".to_owned(), String::new()),
            Some((c, bad)) => (
                if bad {
                    format!(
                        "all {} row(s) and the track name unprintable: the scoring offsets are probably wrong",
                        c.scoring_rows
                    )
                } else {
                    "readable names present".to_owned()
                },
                "at least one printable name".to_owned(),
            ),
        },
        CheckId::ScoringEt => match c {
            None => na(),
            Some(c) => (format!("{}", c.scoring_et), "finite, >= 0".to_owned()),
        },
        CheckId::ScoringStrings => match c {
            None => na(),
            Some(c) => (
                format!(
                    "track name {}; {} row(s) with bad names{}",
                    str_state(c.track_name),
                    c.bad_string_rows,
                    c.first_bad_string_row
                        .map_or_else(String::new, |r| format!(" (first row {r})"))
                ),
                "printable, NUL-terminated".to_owned(),
            ),
        },
        CheckId::ScoringBools => match c {
            None => na(),
            Some(c) => (
                bad_bool_text(c.scoring_bad_bools, c.first_scoring_bad_bool),
                "0 or 1".to_owned(),
            ),
        },
        CheckId::TelemetryEt => match c.and_then(|c| c.player) {
            None => ("no player telemetry".to_owned(), String::new()),
            Some(p) => (
                format!(
                    "mElapsedTime={} mDeltaTime={}",
                    p.elapsed_time, p.delta_time
                ),
                format!("ET finite >= 0, 0 < dt <= {MAX_PLAUSIBLE_DELTA_TIME_S}"),
            ),
        },
        CheckId::TelemetryEtMonotonic => match c {
            Some(ContentFacts {
                player: Some(p),
                et_regression,
                ..
            }) => (
                et_regression.map_or_else(
                    || format!("slot {} mID {}: non-decreasing", p.slot, p.id),
                    |(prev, now)| format!("went back from {prev} to {now} (session restart?)"),
                ),
                "non-decreasing within a session (A-LMU-031)".to_owned(),
            ),
            _ => ("no player telemetry".to_owned(), String::new()),
        },
        CheckId::TelemetryStrings => match c.and_then(|c| c.player) {
            None => ("no player telemetry".to_owned(), String::new()),
            Some(p) => (
                format!(
                    "vehicle name {}; track name {}",
                    str_state(p.vehicle_name),
                    str_state(p.track_name)
                ),
                "printable, NUL-terminated".to_owned(),
            ),
        },
        CheckId::TelemetryBools => match c.and_then(|c| c.player) {
            None => ("no player telemetry".to_owned(), String::new()),
            Some(p) => (
                bad_bool_text(p.bad_bools, p.first_bad_bool),
                "0 or 1".to_owned(),
            ),
        },
    }
}

/// One check in a [`CompatibilityReport`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CheckReport {
    /// The check.
    pub id: CheckId,
    /// Human-readable title.
    pub title: &'static str,
    /// Sections the check vouches for.
    pub sections: &'static [Section],
    /// Structural (failure stops decoding) or content (warning only).
    pub structural: bool,
    /// Verdict on the latest checked frame (`None` = not applicable / no data).
    pub outcome: Option<Outcome>,
    /// What the latest frame showed.
    pub detected: String,
    /// What a compatible layout shows.
    pub expected: String,
    /// Frames on which the check warned since the connection started.
    pub warn_frames: u64,
    /// Frames on which the check failed since the connection started.
    pub fail_frames: u64,
    /// `detected` of the most recent frame that did not pass, if any.
    pub last_problem: Option<String>,
}

/// Compatibility of the running LMU with the layout this build decodes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompatibilityReport {
    /// Where the offsets come from.
    pub layout_source: &'static str,
    /// The LMU build the layout was probed from.
    pub probed_build: &'static str,
    /// `VirtualQuery` region size of the mapped view.
    pub mapping_size: Option<usize>,
    /// Minimum region size (end of the last member).
    pub mapping_required: usize,
    /// Raw `generic.gameVersion` of the latest decoded frame.
    pub game_version: Option<i32>,
    /// First `gameVersion` seen on this connection.
    pub first_game_version: Option<i32>,
    /// The latest `gameVersion` is in [`VERIFIED_GAME_VERSIONS`] (`None` before a decoded frame).
    /// Reported separately: it does not lower the support levels below.
    pub version_verified: Option<bool>,
    /// Frame synchronisation in use (filled in by the reader).
    pub sync_mode: Option<SyncMode>,
    /// Copy protection in use (filled in by the reader).
    pub lock_mode: Option<LockMode>,
    /// Frames checked on this connection.
    pub frames_checked: u64,
    /// Frames rejected by structural checks on this connection.
    pub frames_rejected: u64,
    /// `data.generic` support (`None` until a frame was checked).
    pub generic: Option<SupportLevel>,
    /// `data.scoring` support.
    pub scoring: Option<SupportLevel>,
    /// `data.telemetry` support.
    pub telemetry: Option<SupportLevel>,
    /// Worst of the three sections.
    pub overall: Option<SupportLevel>,
    /// Every check.
    pub checks: Vec<CheckReport>,
}

impl CompatibilityReport {
    /// `true` once a frame was checked and no structural check failed.
    pub fn is_decodable(&self) -> bool {
        matches!(self.overall, Some(level) if level != SupportLevel::Unsupported)
    }

    /// The checks whose latest verdict is a structural failure.
    pub fn failures(&self) -> impl Iterator<Item = &CheckReport> {
        self.checks
            .iter()
            .filter(|c| c.outcome == Some(Outcome::Fail))
    }

    /// The checks whose latest verdict is a warning.
    pub fn warnings(&self) -> impl Iterator<Item = &CheckReport> {
        self.checks
            .iter()
            .filter(|c| c.outcome == Some(Outcome::Warn))
    }

    /// Actionable message for [`crate::LmuError::IncompatibleInterface`].
    pub fn incompatibility_message(&self) -> String {
        let details: Vec<String> = self
            .failures()
            .map(|c| {
                format!(
                    "{}: detected {}, expected {}",
                    c.title, c.detected, c.expected
                )
            })
            .collect();
        let version = self
            .game_version
            .map_or_else(|| "unknown".to_owned(), |v| v.to_string());
        format!(
            "LMU shared-memory layout is INCOMPATIBLE with this build ({}); telemetry is NOT decoded. \
             Layout probed from {} ({}); running LMU gameVersion {version}. \
             LMU was probably updated: install an LMU Race Engineering update that supports this LMU \
             version, or re-run tools/layout-probe and crates/lmu-reader/codegen/gen_layout.py.",
            details.join("; "),
            self.probed_build,
            self.layout_source,
        )
    }
}

/// Accumulates verdicts per connection and renders [`CompatibilityReport`]s.
#[derive(Debug, Clone, Default)]
pub struct CompatTracker {
    structural: StructuralFacts,
    content: Option<ContentFacts>,
    outcomes: [Option<Outcome>; CHECK_COUNT],
    warn_frames: [u64; CHECK_COUNT],
    fail_frames: [u64; CHECK_COUNT],
    last_problem: [Option<(StructuralFacts, Option<ContentFacts>)>; CHECK_COUNT],
    frames_checked: u64,
    frames_rejected: u64,
    first_game_version: Option<i32>,
}

impl CompatTracker {
    /// A tracker with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the structural verdicts of a frame. Returns `false` (frame must not be decoded) if
    /// any structural check failed; content verdicts are then cleared.
    pub(crate) fn observe_structural(&mut self, facts: StructuralFacts) -> bool {
        self.structural = facts;
        self.content = None;
        self.frames_checked += 1;
        let mut ok = true;
        for id in ALL_CHECKS {
            if id.is_raw_structural() {
                let outcome = verdict(id, &facts, None);
                if outcome == Some(Outcome::Fail) {
                    ok = false;
                }
                self.record(id, outcome);
            } else {
                // Content verdicts belong to the previous frame; forget them until decoded.
                if let Some(o) = self.outcomes.get_mut(id.index()) {
                    *o = None;
                }
            }
        }
        if !ok {
            self.frames_rejected += 1;
        }
        ok
    }

    /// Records the content and layout-symptom verdicts of a decoded frame.
    pub(crate) fn observe_content(&mut self, facts: ContentFacts) -> ContentVerdict {
        self.first_game_version.get_or_insert(facts.game_version);
        self.content = Some(facts);
        let mut reject = false;
        for id in ALL_CHECKS.into_iter().filter(|c| !c.is_raw_structural()) {
            let outcome = verdict(id, &self.structural, Some(&facts));
            reject |= outcome == Some(Outcome::Fail);
            self.record(id, outcome);
        }
        if reject {
            self.frames_rejected += 1;
        }
        ContentVerdict {
            support: self.overall().unwrap_or(SupportLevel::Supported),
            reject,
            version_verified: VERIFIED_GAME_VERSIONS.contains(&facts.game_version),
        }
    }

    fn record(&mut self, id: CheckId, outcome: Option<Outcome>) {
        let i = id.index();
        if let Some(slot) = self.outcomes.get_mut(i) {
            *slot = outcome;
        }
        let counter = match outcome {
            Some(Outcome::Warn) => self.warn_frames.get_mut(i),
            Some(Outcome::Fail) => self.fail_frames.get_mut(i),
            _ => None,
        };
        if let Some(counter) = counter {
            *counter += 1;
            if let Some(slot) = self.last_problem.get_mut(i) {
                *slot = Some((self.structural, self.content));
            }
        }
    }

    fn section_level(&self, section: Section) -> Option<SupportLevel> {
        let mut level = None;
        for id in ALL_CHECKS
            .into_iter()
            .filter(|c| c.affects_support() && c.sections().contains(&section))
        {
            let this = match self.outcomes.get(id.index()).copied().flatten() {
                None => continue,
                Some(Outcome::Pass) => SupportLevel::Supported,
                Some(Outcome::Warn) => SupportLevel::SupportedWithWarnings,
                Some(Outcome::Fail) => SupportLevel::Unsupported,
            };
            level = Some(level.map_or(this, |l: SupportLevel| l.max(this)));
        }
        level
    }

    /// Worst section level of the latest frame (`None` before any frame).
    pub fn overall(&self) -> Option<SupportLevel> {
        [Section::Generic, Section::Scoring, Section::Telemetry]
            .into_iter()
            .filter_map(|s| self.section_level(s))
            .max()
    }

    /// `true` if the latest checked frame failed a structural check.
    pub fn structural_failure(&self) -> bool {
        self.overall() == Some(SupportLevel::Unsupported)
    }

    /// Frames checked so far.
    pub fn frames_checked(&self) -> u64 {
        self.frames_checked
    }

    /// Renders the report (mode fields are left `None`; the reader fills them in).
    pub fn report(&self) -> CompatibilityReport {
        let checks = ALL_CHECKS
            .into_iter()
            .map(|id| {
                let i = id.index();
                let (detected, expected) = describe(id, &self.structural, self.content.as_ref());
                let last_problem = self
                    .last_problem
                    .get(i)
                    .copied()
                    .flatten()
                    .map(|(s, c)| describe(id, &s, c.as_ref()).0);
                CheckReport {
                    id,
                    title: id.title(),
                    sections: id.sections(),
                    structural: id.is_structural(),
                    outcome: self.outcomes.get(i).copied().flatten(),
                    detected,
                    expected,
                    warn_frames: self.warn_frames.get(i).copied().unwrap_or(0),
                    fail_frames: self.fail_frames.get(i).copied().unwrap_or(0),
                    last_problem,
                }
            })
            .collect();
        CompatibilityReport {
            layout_source: LAYOUT_SOURCE,
            probed_build: PROBED_LMU_BUILD,
            mapping_size: self.structural.mapping_size,
            mapping_required: abs::MAPPING_END,
            game_version: self.content.map(|c| c.game_version),
            first_game_version: self.first_game_version,
            version_verified: self
                .content
                .map(|c| VERIFIED_GAME_VERSIONS.contains(&c.game_version)),
            sync_mode: None,
            lock_mode: None,
            frames_checked: self.frames_checked,
            frames_rejected: self.frames_rejected,
            generic: self.section_level(Section::Generic),
            scoring: self.section_level(Section::Scoring),
            telemetry: self.section_level(Section::Telemetry),
            overall: self.overall(),
            checks,
        }
    }
}

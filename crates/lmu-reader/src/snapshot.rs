//! Decoded frames ([`RawSnapshot`]) and the pure decoders that build them from [`RawBlocks`].
//!
//! Everything here keeps raw C semantics: no unit conversion, no clamping of values, NaN and
//! infinities pass through (the normalizer validates and converts). Only the slot counts are
//! clamped, and that already happened inside the copy.

use std::fmt;
use std::sync::Arc;

use serde::{Serialize, Serializer};

use crate::blocks::RawBlocks;
use crate::bytes::{DecodeError, read_le};
use crate::compat::SupportLevel;
use crate::offsets::{scoring_info, telem_info, veh_scoring};
use crate::raw::SharedMemoryEvent;
use crate::raw_structs::{
    RawGeneric, RawPaths, RawScoringInfo, RawTelemetry, RawVehicleScoring, decode_scoring_info,
    decode_telemetry, decode_vehicle_scoring,
};
use crate::state::FrameSource;

/// The raw telemetry header bytes (`data.telemetry`, absolute offset 128,464).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TelemetryHeader {
    /// Raw `activeVehicles` (not clamped; values above 104 fail the structural checks).
    pub active_vehicles: u8,
    /// Raw `playerVehicleIdx`: an index into `telemInfo[]`, not an `mID` (A-LMU-020).
    pub player_vehicle_idx: u8,
    /// Raw `playerHasVehicle` byte (a C++ `bool`: 0 or 1).
    pub player_has_vehicle: u8,
}

/// One decoded telemetry slot (the player's, or any other car's).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerTelemetry {
    /// Index into `telemInfo[]` the slot was copied from.
    pub slot: u8,
    /// The decoded `TelemInfoV01`.
    pub telemetry: RawTelemetry,
}

/// Why the player's telemetry is UNAVAILABLE for a frame although the header names a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlayerUnverified {
    /// No scoring row is flagged `mIsPlayer` (scoring not published yet, spectating?).
    NoPlayerRow,
    /// More than one scoring row is flagged `mIsPlayer`.
    SeveralPlayerRows,
    /// The slot named by `playerVehicleIdx` has another `mID`, and no copied slot has the player's.
    IdNotFound,
    /// The slot named by `playerVehicleIdx` has another `mID`, and several copied slots have the
    /// player's (cannot tell which one is the player).
    IdAmbiguous,
}

/// How [`RawSnapshot::player`] was identified (A-LMU-020, research §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "kind")]
pub enum PlayerIdentity {
    /// `playerHasVehicle` is 0 or `playerVehicleIdx` is outside the active slots: no player car.
    NoVehicle,
    /// The slot named by `playerVehicleIdx` carries the `mID` of the single `mIsPlayer` scoring row.
    Verified,
    /// The slot named by `playerVehicleIdx` carried another `mID`; the player's `mID` was found in
    /// exactly one other copied slot, which is used instead.
    FoundById {
        /// The slot `playerVehicleIdx` named (not the player's).
        index_slot: u8,
    },
    /// The player could not be verified: [`RawSnapshot::player`] is `None` for this frame.
    Unverified {
        /// Why.
        reason: PlayerUnverified,
    },
}

/// The raw bytes of every copied telemetry slot (`telemInfo[0..min(activeVehicles, 104)]`),
/// decoded on demand so frames that nobody inspects cost no decode time.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TelemetrySlots {
    bytes: Box<[u8]>,
}

impl fmt::Debug for TelemetrySlots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TelemetrySlots")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl TelemetrySlots {
    /// Copies the telemetry slots of a frame.
    pub fn from_blocks(blocks: &RawBlocks) -> Self {
        Self {
            bytes: blocks.telemetry_slots().into(),
        }
    }

    /// Number of copied slots.
    pub fn len(&self) -> usize {
        self.bytes.len() / telem_info::SIZE
    }

    /// `true` if no slot was copied.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn slot_bytes(&self, slot: usize) -> Option<&[u8]> {
        let start = slot.checked_mul(telem_info::SIZE)?;
        self.bytes.get(start..start.checked_add(telem_info::SIZE)?)
    }

    /// Raw `mID` of a slot.
    pub fn id(&self, slot: usize) -> Option<i32> {
        read_le(self.slot_bytes(slot)?, telem_info::ID.offset).map(i32::from_le_bytes)
    }

    /// Slots whose `mID` equals `id`.
    pub fn slots_with_id(&self, id: i32) -> impl Iterator<Item = usize> + '_ {
        (0..self.len()).filter(move |&slot| self.id(slot) == Some(id))
    }

    /// Decodes one slot (`Ok(None)` beyond the copied slots).
    ///
    /// # Errors
    /// [`DecodeError`] if the slot bytes do not have the layout size (an internal invariant).
    pub fn decode(&self, slot: usize) -> Result<Option<PlayerTelemetry>, DecodeError> {
        let Some(bytes) = self.slot_bytes(slot) else {
            return Ok(None);
        };
        let Ok(slot) = u8::try_from(slot) else {
            return Ok(None);
        };
        Ok(Some(PlayerTelemetry {
            slot,
            telemetry: decode_telemetry(bytes)?,
        }))
    }

    /// Decodes the single slot whose `mID` is `id` (`Ok(None)` when no slot or several slots have
    /// it: an ambiguous identity is never guessed).
    ///
    /// # Errors
    /// See [`TelemetrySlots::decode`].
    pub fn decode_by_id(&self, id: i32) -> Result<Option<PlayerTelemetry>, DecodeError> {
        let mut matches = self.slots_with_id(id);
        match (matches.next(), matches.next()) {
            (Some(slot), None) => self.decode(slot),
            _ => Ok(None),
        }
    }
}

/// The scoring block: session-wide info plus the valid vehicle rows.
///
/// Consecutive snapshots share one `Arc` while the scoring bytes are unchanged, so consumers can
/// detect "same scoring" with [`Arc::ptr_eq`] and the reader does not re-decode 104 rows per frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoringBlock {
    /// `scoringInfo`.
    pub info: RawScoringInfo,
    /// Raw `mNumVehicles` (not clamped).
    pub num_vehicles_raw: i32,
    /// Raw `scoringStreamSize` (not clamped).
    pub scoring_stream_size_raw: u64,
    /// `vehScoringInfo[0..min(mNumVehicles, 104)]` (rows beyond the count are never decoded).
    pub vehicles: Vec<RawVehicleScoring>,
}

impl ScoringBlock {
    /// The first row flagged `mIsPlayer`.
    pub fn player_row(&self) -> Option<&RawVehicleScoring> {
        self.vehicles.iter().find(|r| r.is_player)
    }

    /// The row with the given `mID` (unique within one snapshot, reused in multiplayer, A-LMU-023).
    pub fn row_by_id(&self, id: i32) -> Option<&RawVehicleScoring> {
        self.vehicles.iter().find(|r| r.id == id)
    }
}

fn serialize_arc<S: Serializer, T: Serialize>(
    value: &Arc<T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    T::serialize(value, serializer)
}

/// One decoded frame of `LMU_Data`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawSnapshot {
    /// Snapshot sequence number (1, 2, ...), monotonic for the lifetime of the reader.
    pub seq: u64,
    /// Capture time: nanoseconds of the reader's [`crate::MonoClock`], taken right after the copy.
    pub capture_mono_ns: u64,
    /// Connection generation (increments on every attach to `LMU_Data`).
    pub connection_id: u64,
    /// Frame synchronisation and copy consistency.
    pub source: FrameSource,
    /// The player telemetry `mElapsedTime` (or the player slot / `mID`) changed since the previous
    /// snapshot: this frame carries a new telemetry sample (A-LMU-031).
    pub telemetry_updated: bool,
    /// The scoring bytes changed since the previous snapshot.
    pub scoring_updated: bool,
    /// Compatibility of this frame (never `Unsupported`: such frames are not decoded). The game
    /// version verdict is not part of it, see [`RawSnapshot::version_verified`].
    pub support: SupportLevel,
    /// `generic.gameVersion` is in [`crate::compat::VERIFIED_GAME_VERSIONS`] (A-LMU-010). `false`
    /// means the layout has not been confirmed against this LMU build at runtime yet.
    pub version_verified: bool,
    /// `data.generic`.
    pub generic: RawGeneric,
    /// `data.scoring` (shared with the previous snapshot while unchanged).
    #[serde(serialize_with = "serialize_arc")]
    pub scoring: Arc<ScoringBlock>,
    /// `data.telemetry` header.
    pub telemetry_header: TelemetryHeader,
    /// The player's telemetry slot, present only when its identity is verified against the single
    /// `mIsPlayer` scoring row by `mID` (see [`RawSnapshot::player_identity`]); otherwise the player
    /// is UNAVAILABLE for this frame (research §11.4). Never another car's data.
    pub player: Option<PlayerTelemetry>,
    /// How [`RawSnapshot::player`] was identified, or why it is unavailable.
    pub player_identity: PlayerIdentity,
    /// Every copied telemetry slot, including the opponents' (decoded on demand). Not serialized.
    #[serde(skip)]
    pub vehicle_telemetry: Arc<TelemetrySlots>,
    /// `data.paths`, only on frames where it was copied (**PII**: Windows user paths).
    pub paths: Option<RawPaths>,
    /// `scoringStream[0..min(scoringStreamSize, 65535)]`, only when stream copying is enabled.
    pub scoring_stream: Option<Vec<u8>>,
}

impl RawSnapshot {
    /// The `generic.events[]` queue as named codes (advisory only, A-LMU-009).
    pub fn events(&self) -> impl Iterator<Item = SharedMemoryEvent> + '_ {
        self.generic
            .events
            .iter()
            .map(|&code| SharedMemoryEvent::from_raw(code))
    }

    /// Player telemetry `mElapsedTime`, if a player slot was copied.
    pub fn player_et(&self) -> Option<f64> {
        self.player.as_ref().map(|p| p.telemetry.elapsed_time)
    }

    /// The player's scoring row: the `mIsPlayer` row whose `mID` equals the player telemetry `mID`.
    /// `None` when there is no player telemetry or the two disagree (research §11.4: the player is
    /// then UNAVAILABLE for this frame).
    pub fn player_scoring(&self) -> Option<&RawVehicleScoring> {
        let id = self.player.as_ref()?.telemetry.id;
        self.scoring
            .vehicles
            .iter()
            .find(|r| r.is_player && r.id == id)
    }

    /// Decodes the telemetry of the car with scoring `mID` `id` (opponents included). `Ok(None)` when
    /// no copied slot or several slots carry that `mID` (never guessed).
    ///
    /// # Errors
    /// [`DecodeError`] if a slot does not have the layout size (an internal invariant).
    pub fn telemetry_for_id(&self, id: i32) -> Result<Option<PlayerTelemetry>, DecodeError> {
        self.vehicle_telemetry.decode_by_id(id)
    }

    /// The scoring stream as lossy UTF-8 text.
    pub fn scoring_stream_text(&self) -> Option<String> {
        self.scoring_stream
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

/// Decodes the scoring header and the copied vehicle rows.
///
/// # Errors
/// [`DecodeError`] if a block does not have the layout size (an internal invariant).
pub fn decode_scoring_block(blocks: &RawBlocks) -> Result<ScoringBlock, DecodeError> {
    let header = blocks.scoring_header();
    let info_bytes = header
        .get(..scoring_info::SIZE)
        .ok_or(DecodeError::WrongSize {
            context: "scoring header",
            expected: scoring_info::SIZE,
            actual: header.len(),
        })?;
    let vehicles = blocks
        .vehicles()
        .as_chunks::<{ veh_scoring::SIZE }>()
        .0
        .iter()
        .map(|row| decode_vehicle_scoring(row))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ScoringBlock {
        info: decode_scoring_info(info_bytes)?,
        num_vehicles_raw: blocks.num_vehicles_raw().unwrap_or_default(),
        scoring_stream_size_raw: blocks.scoring_stream_size_raw().unwrap_or_default(),
        vehicles,
    })
}

/// The raw telemetry header of a frame.
pub fn decode_telemetry_header(blocks: &RawBlocks) -> TelemetryHeader {
    TelemetryHeader {
        active_vehicles: blocks.active_vehicles_raw().unwrap_or_default(),
        player_vehicle_idx: blocks.player_vehicle_idx_raw().unwrap_or_default(),
        player_has_vehicle: blocks.player_has_vehicle_raw().unwrap_or_default(),
    }
}

/// Decodes the slot named by `playerVehicleIdx` (unverified, see [`select_player`]).
///
/// # Errors
/// [`DecodeError`] if the slot block does not have the layout size (an internal invariant).
pub fn decode_index_slot(blocks: &RawBlocks) -> Result<Option<PlayerTelemetry>, DecodeError> {
    blocks
        .player_index_slot()
        .map(|(slot, bytes)| {
            Ok(PlayerTelemetry {
                slot,
                telemetry: decode_telemetry(bytes)?,
            })
        })
        .transpose()
}

/// Picks the player's telemetry (A-LMU-020, research §11.4): the slot named by `playerVehicleIdx`
/// when its `mID` equals the `mID` of the single `mIsPlayer` scoring row; otherwise the single
/// copied slot carrying that `mID`; otherwise none (the player is UNAVAILABLE, never guessed).
///
/// # Errors
/// [`DecodeError`] if a slot does not have the layout size (an internal invariant).
pub fn select_player(
    index_slot: Option<PlayerTelemetry>,
    scoring: &ScoringBlock,
    slots: &TelemetrySlots,
) -> Result<(Option<PlayerTelemetry>, PlayerIdentity), DecodeError> {
    let Some(indexed) = index_slot else {
        return Ok((None, PlayerIdentity::NoVehicle));
    };
    let unverified = |reason| Ok((None, PlayerIdentity::Unverified { reason }));
    let mut rows = scoring.vehicles.iter().filter(|r| r.is_player);
    let player_id = match (rows.next(), rows.next()) {
        (Some(row), None) => row.id,
        (None, _) => return unverified(PlayerUnverified::NoPlayerRow),
        (Some(_), Some(_)) => return unverified(PlayerUnverified::SeveralPlayerRows),
    };
    if indexed.telemetry.id == player_id {
        return Ok((Some(indexed), PlayerIdentity::Verified));
    }
    let mut matches = slots.slots_with_id(player_id);
    match (matches.next(), matches.next()) {
        (Some(slot), None) => Ok((
            slots.decode(slot)?,
            PlayerIdentity::FoundById {
                index_slot: indexed.slot,
            },
        )),
        (None, _) => unverified(PlayerUnverified::IdNotFound),
        (Some(_), Some(_)) => unverified(PlayerUnverified::IdAmbiguous),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{CopyPlan, copy_frame};
    use crate::offsets::{telem_info, telem_wheel};
    use crate::testutil::LayoutBuilder;

    fn copy(b: &LayoutBuilder) -> RawBlocks {
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        blocks
    }

    #[test]
    fn decodes_a_synthetic_on_track_frame() {
        let blocks = copy(&LayoutBuilder::on_track_frame());
        let scoring = decode_scoring_block(&blocks).expect("scoring");
        assert_eq!(scoring.vehicles.len(), 2);
        assert_eq!(scoring.info.track_name.as_str(), "Synthetic Ring");
        assert_eq!(scoring.info.current_et, 120.0);
        assert_eq!(scoring.player_row().map(|r| r.id), Some(7));
        assert_eq!(
            scoring.row_by_id(9).map(|r| r.driver_name.as_str()),
            Some("Synthetic Rival")
        );

        let header = decode_telemetry_header(&blocks);
        assert_eq!(
            header,
            TelemetryHeader {
                active_vehicles: 2,
                player_vehicle_idx: 0,
                player_has_vehicle: 1
            }
        );
        let player = decode_index_slot(&blocks)
            .expect("decode")
            .expect("player slot");
        assert_eq!(player.slot, 0);
        assert_eq!(player.telemetry.id, 7);
        assert_eq!(player.telemetry.gear, 4);
        assert_eq!(player.telemetry.local_vel.z, -50.0);
        assert_eq!(player.telemetry.wheels[3].pressure, 170.0);
        assert_eq!(player.telemetry.wheels[1].temperature, [353.15; 3]);
        assert_eq!(player.telemetry.vehicle_name.as_str(), "Synthetic Car #7");
    }

    #[test]
    fn nan_and_infinity_pass_through_unchanged() {
        let mut b = LayoutBuilder::on_track_frame();
        let slot = LayoutBuilder::telem_base(0);
        b.set(slot, telem_info::ENGINE_RPM, f64::NAN);
        b.set(slot, telem_info::FUEL, f64::INFINITY);
        b.set(slot, telem_info::REGEN, f32::NAN);
        b.set_elem(
            LayoutBuilder::wheel_base(0, 2),
            telem_wheel::TEMPERATURE,
            1,
            f64::NEG_INFINITY,
        );
        let player = decode_index_slot(&copy(&b))
            .expect("decode")
            .expect("player");
        assert!(player.telemetry.engine_rpm.is_nan());
        assert_eq!(player.telemetry.fuel, f64::INFINITY);
        assert!(player.telemetry.regen.is_nan());
        assert_eq!(player.telemetry.wheels[2].temperature[1], f64::NEG_INFINITY);
    }

    #[test]
    fn rows_beyond_the_count_are_not_decoded() {
        let mut b = LayoutBuilder::on_track_frame();
        b.vehicle_row(2, 11, "Stale Row", false, 3);
        let scoring = decode_scoring_block(&copy(&b)).expect("scoring");
        assert_eq!(
            scoring.vehicles.len(),
            2,
            "mNumVehicles is 2: row 2 is stale (A-LMU-021)"
        );
        assert!(scoring.row_by_id(11).is_none());
    }

    #[test]
    fn strings_without_terminator_or_with_garbage_are_flagged() {
        let mut b = LayoutBuilder::on_track_frame();
        let row = LayoutBuilder::vehicle_base(1);
        b.set_raw(
            row,
            veh_scoring::DRIVER_NAME,
            &[b'X'; veh_scoring::DRIVER_NAME.size],
        );
        b.set_raw(
            LayoutBuilder::telem_base(0),
            telem_info::VEHICLE_NAME,
            &[b'C', 0x07, b'r', 0],
        );
        let blocks = copy(&b);
        let scoring = decode_scoring_block(&blocks).expect("scoring");
        let name = &scoring.vehicles[1].driver_name;
        assert_eq!(
            name.as_str().len(),
            veh_scoring::DRIVER_NAME.size,
            "decoded within the array bounds"
        );
        assert!(!name.nul_terminated);
        let player = decode_index_slot(&blocks).expect("decode").expect("player");
        assert!(!player.telemetry.vehicle_name.printable);
    }

    #[test]
    fn player_scoring_requires_matching_ids() {
        let blocks = copy(&LayoutBuilder::on_track_frame());
        let snapshot = RawSnapshot {
            seq: 1,
            capture_mono_ns: 0,
            connection_id: 1,
            source: FrameSource {
                sync: None,
                consistency: crate::state::Consistency::Replayed { locked: true },
            },
            telemetry_updated: true,
            scoring_updated: true,
            support: SupportLevel::Supported,
            version_verified: false,
            generic: crate::raw_structs::decode_generic(blocks.generic()).expect("generic"),
            scoring: Arc::new(decode_scoring_block(&blocks).expect("scoring")),
            telemetry_header: decode_telemetry_header(&blocks),
            player: decode_index_slot(&blocks).expect("decode"),
            player_identity: PlayerIdentity::Verified,
            vehicle_telemetry: Arc::new(TelemetrySlots::from_blocks(&blocks)),
            paths: None,
            scoring_stream: None,
        };
        assert_eq!(
            snapshot
                .telemetry_for_id(9)
                .expect("decode")
                .map(|t| t.slot),
            Some(1),
            "opponent telemetry is reachable by mID"
        );
        assert_eq!(snapshot.player_scoring().map(|r| r.id), Some(7));
        assert_eq!(snapshot.player_et(), Some(120.0));

        let mut other = snapshot.clone();
        if let Some(p) = other.player.as_mut() {
            p.telemetry.id = 9;
        }
        assert!(
            other.player_scoring().is_none(),
            "mIsPlayer row and telemetry disagree on mID"
        );
        let json = serde_json::to_value(&snapshot).expect("serialize");
        assert_eq!(json["scoring"]["vehicles"][0]["id"], 7);
    }

    fn select(b: &LayoutBuilder) -> (Option<PlayerTelemetry>, PlayerIdentity) {
        let blocks = copy(b);
        let scoring = decode_scoring_block(&blocks).expect("scoring");
        let slots = TelemetrySlots::from_blocks(&blocks);
        select_player(
            decode_index_slot(&blocks).expect("decode"),
            &scoring,
            &slots,
        )
        .expect("select")
    }

    #[test]
    fn player_is_verified_by_id_and_never_taken_from_another_car() {
        // Index slot carries the mIsPlayer row's mID: verified.
        let (player, identity) = select(&LayoutBuilder::on_track_frame());
        assert_eq!(identity, PlayerIdentity::Verified);
        assert_eq!(player.map(|p| (p.slot, p.telemetry.id)), Some((0, 7)));

        // playerVehicleIdx points at the rival's slot: the player's mID is found in slot 0.
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(2, 1, 1);
        let (player, identity) = select(&b);
        assert_eq!(identity, PlayerIdentity::FoundById { index_slot: 1 });
        assert_eq!(player.map(|p| (p.slot, p.telemetry.id)), Some((0, 7)));

        // No slot carries the player's mID: UNAVAILABLE, not the rival's data.
        let mut b = LayoutBuilder::on_track_frame();
        b.set(LayoutBuilder::telem_base(0), telem_info::ID, 9i32);
        let (player, identity) = select(&b);
        assert!(player.is_none());
        assert_eq!(
            identity,
            PlayerIdentity::Unverified {
                reason: PlayerUnverified::IdNotFound
            }
        );

        // Two other slots carry the player's mID: ambiguous, never guessed.
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(3, 2, 1);
        b.telemetry_slot(2, 11, 120.0);
        b.set(LayoutBuilder::telem_base(1), telem_info::ID, 7i32);
        let (player, identity) = select(&b);
        assert!(player.is_none());
        assert_eq!(
            identity,
            PlayerIdentity::Unverified {
                reason: PlayerUnverified::IdAmbiguous
            }
        );

        // No mIsPlayer row: cannot verify.
        let mut b = LayoutBuilder::on_track_frame();
        b.set(
            LayoutBuilder::vehicle_base(0),
            veh_scoring::IS_PLAYER,
            false,
        );
        assert_eq!(
            select(&b),
            (
                None,
                PlayerIdentity::Unverified {
                    reason: PlayerUnverified::NoPlayerRow
                }
            )
        );

        // Two mIsPlayer rows: cannot verify.
        let mut b = LayoutBuilder::on_track_frame();
        b.set(LayoutBuilder::vehicle_base(1), veh_scoring::IS_PLAYER, true);
        assert_eq!(
            select(&b).1,
            PlayerIdentity::Unverified {
                reason: PlayerUnverified::SeveralPlayerRows
            }
        );

        // No vehicle.
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(2, 0, 0);
        assert_eq!(select(&b), (None, PlayerIdentity::NoVehicle));
    }
}

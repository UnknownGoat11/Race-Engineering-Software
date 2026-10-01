//! The bytes of one frame, copied out of `LMU_Data` into preallocated buffers.
//!
//! [`copy_frame`] is the body of the lock critical section: it performs only bounded `memcpy`s into
//! buffers allocated up front, clamps every count *inside* the copy (A-LMU-021) and never allocates,
//! logs or panics. Counts are read once, from the bytes just copied, so the variable-length copies and
//! the later decode always agree.

use std::fmt;

use thiserror::Error;

use crate::bytes::read_le;
use crate::offsets::{
    MAX_SCORING_VEHICLES, MAX_TELEMETRY_VEHICLES, SCORING_STREAM_COPY_MAX, abs, generic, paths,
    scoring_info, telem_info, telemetry_data, veh_scoring,
};

/// A read outside the readable source range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("read of {len} bytes at offset {offset} exceeds the {available}-byte source")]
pub struct OutOfBounds {
    /// Requested offset.
    pub offset: usize,
    /// Requested length.
    pub len: usize,
    /// Readable size of the source.
    pub available: usize,
}

/// Something raw `LMU_Data` bytes can be copied from: the live mapping, or a buffer in tests and replays.
pub trait ByteSource {
    /// Readable size in bytes.
    fn len(&self) -> usize;

    /// `true` if nothing is readable.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies `dst.len()` bytes starting at `offset` into `dst`.
    ///
    /// Implementations must not allocate, log or panic: this runs while the LMU lock is held.
    ///
    /// # Errors
    /// [`OutOfBounds`] if the range is not entirely readable (nothing is copied).
    fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds>;
}

impl ByteSource for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds> {
        let oob = OutOfBounds {
            offset,
            len: dst.len(),
            available: <[u8]>::len(self),
        };
        let end = offset.checked_add(dst.len()).ok_or(oob)?;
        let src = self.get(offset..end).ok_or(oob)?;
        dst.copy_from_slice(src);
        Ok(())
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn copy_to(&self, offset: usize, dst: &mut [u8]) -> Result<(), OutOfBounds> {
        self.as_slice().copy_to(offset, dst)
    }
}

/// Which optional regions a frame copy includes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyPlan {
    /// Copy `data.paths` (1,300 bytes; PII). Normally only once after attaching.
    pub paths: bool,
    /// Copy `scoringStream[0..min(scoringStreamSize, 65535)]`. Off by default.
    pub scoring_stream: bool,
}

/// Length of the copied telemetry header (`activeVehicles`, `playerVehicleIdx`, `playerHasVehicle`, hole).
pub const TELEMETRY_HEADER_LEN: usize = abs::TELEMETRY_HEADER.size;
/// Length of the copied scoring header (`scoringInfo`, padding hole, `scoringStreamSize`).
pub const SCORING_HEADER_LEN: usize = abs::SCORING_HEADER.size;
/// Capacity of the vehicle-scoring buffer (all 104 rows).
pub const VEHICLES_CAPACITY: usize = MAX_SCORING_VEHICLES * veh_scoring::SIZE;
/// Capacity of the telemetry-slot buffer (all 104 `TelemInfoV01` slots, 196,352 bytes).
pub const TELEMETRY_CAPACITY: usize = MAX_TELEMETRY_VEHICLES * telem_info::SIZE;

/// Offset of `scoringStreamSize` inside the scoring header block.
const STREAM_SIZE_IN_HEADER: usize = abs::SCORING_STREAM_SIZE - abs::SCORING_INFO;
/// Offset of the padding hole inside the scoring header block.
const PADDING_IN_HEADER: usize = abs::SCORING_PADDING.offset - abs::SCORING_INFO;
/// Size of the padding hole.
pub const PADDING_LEN: usize = abs::SCORING_PADDING.size;

/// Identifies one copied block (also the block id in `.lmuraw` capture files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BlockKind {
    /// `data.generic`.
    Generic = 1,
    /// `scoringInfo` + padding + `scoringStreamSize`.
    ScoringHeader = 2,
    /// `vehScoringInfo[0..n]`.
    Vehicles = 3,
    /// `activeVehicles`, `playerVehicleIdx`, `playerHasVehicle` + hole.
    TelemetryHeader = 4,
    /// `telemInfo[0..min(activeVehicles, 104)]` (every active slot, not only the player's).
    Telemetry = 5,
    /// `data.paths`.
    Paths = 6,
    /// `scoringStream[0..len]`.
    ScoringStream = 7,
}

impl BlockKind {
    /// Parses a block id.
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Generic,
            2 => Self::ScoringHeader,
            3 => Self::Vehicles,
            4 => Self::TelemetryHeader,
            5 => Self::Telemetry,
            6 => Self::Paths,
            7 => Self::ScoringStream,
            _ => return None,
        })
    }
}

/// A copied block does not fit the layout (only possible when loading capture files).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BlockError {
    /// Offset or length does not match the block kind.
    #[error("{kind:?} block at offset {offset} with {len} bytes does not match the layout")]
    Misplaced {
        /// Block kind.
        kind: BlockKind,
        /// Absolute offset recorded for the block.
        offset: usize,
        /// Recorded length.
        len: usize,
    },
    /// A mandatory block is missing from a frame.
    #[error("frame is missing its {0:?} block")]
    Missing(BlockKind),
    /// Blocks disagree with each other (e.g. vehicle rows vs. `mNumVehicles`).
    #[error("frame blocks are inconsistent: {0}")]
    Inconsistent(&'static str),
}

/// The raw bytes of one frame. Buffers are allocated once and reused for every copy.
#[derive(Clone)]
pub struct RawBlocks {
    generic: Box<[u8]>,
    scoring_header: Box<[u8]>,
    telemetry_header: Box<[u8]>,
    vehicles: Box<[u8]>,
    vehicle_count: usize,
    telemetry: Box<[u8]>,
    telemetry_count: usize,
    player_slot: Option<u8>,
    paths: Box<[u8]>,
    paths_valid: bool,
    stream: Box<[u8]>,
    stream_len: usize,
    stream_valid: bool,
}

impl fmt::Debug for RawBlocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawBlocks")
            .field("vehicle_count", &self.vehicle_count)
            .field("telemetry_count", &self.telemetry_count)
            .field("player_slot", &self.player_slot)
            .field("paths_valid", &self.paths_valid)
            .field("stream_len", &self.stream_valid.then_some(self.stream_len))
            .finish_non_exhaustive()
    }
}

impl Default for RawBlocks {
    fn default() -> Self {
        Self::new()
    }
}

fn zeroed(len: usize) -> Box<[u8]> {
    vec![0u8; len].into_boxed_slice()
}

impl RawBlocks {
    /// Allocates all fixed buffers (about 260 KiB). The scoring-stream buffer is allocated only by
    /// [`RawBlocks::enable_scoring_stream`].
    pub fn new() -> Self {
        Self {
            generic: zeroed(generic::SIZE),
            scoring_header: zeroed(SCORING_HEADER_LEN),
            telemetry_header: zeroed(TELEMETRY_HEADER_LEN),
            vehicles: zeroed(VEHICLES_CAPACITY),
            vehicle_count: 0,
            telemetry: zeroed(TELEMETRY_CAPACITY),
            telemetry_count: 0,
            player_slot: None,
            paths: zeroed(paths::SIZE),
            paths_valid: false,
            stream: Box::default(),
            stream_len: 0,
            stream_valid: false,
        }
    }

    /// Allocates the 65,535-byte scoring-stream buffer (call outside the lock).
    pub fn enable_scoring_stream(&mut self) {
        if self.stream.is_empty() {
            self.stream = zeroed(SCORING_STREAM_COPY_MAX);
        }
    }

    /// `data.generic` bytes.
    pub fn generic(&self) -> &[u8] {
        &self.generic
    }

    /// `scoringInfo` + padding + `scoringStreamSize` bytes.
    pub fn scoring_header(&self) -> &[u8] {
        &self.scoring_header
    }

    /// The 4-byte telemetry header.
    pub fn telemetry_header(&self) -> &[u8] {
        &self.telemetry_header
    }

    /// `vehScoringInfo[0..vehicle_count]` bytes.
    pub fn vehicles(&self) -> &[u8] {
        self.vehicles
            .get(..self.vehicle_count * veh_scoring::SIZE)
            .unwrap_or_default()
    }

    /// Number of scoring rows copied (`mNumVehicles` clamped to 0..=104).
    pub fn vehicle_count(&self) -> usize {
        self.vehicle_count
    }

    /// Number of telemetry slots copied (`activeVehicles` clamped to 0..=104).
    pub fn telemetry_count(&self) -> usize {
        self.telemetry_count
    }

    /// `telemInfo[0..telemetry_count]` bytes.
    pub fn telemetry_slots(&self) -> &[u8] {
        self.telemetry
            .get(..self.telemetry_count * telem_info::SIZE)
            .unwrap_or_default()
    }

    /// The bytes of copied telemetry slot `slot` (`None` beyond the copied count).
    pub fn telemetry_slot(&self, slot: usize) -> Option<&[u8]> {
        if slot >= self.telemetry_count {
            return None;
        }
        let start = slot.checked_mul(telem_info::SIZE)?;
        self.telemetry
            .get(start..start.checked_add(telem_info::SIZE)?)
    }

    /// The slot named by `playerVehicleIdx` and its bytes, if `playerHasVehicle` is set and the index
    /// is inside the copied slots. **Unverified**: whether this slot really is the player's car is
    /// decided by the `mID` check in the processor (A-LMU-020, research §11.4).
    pub fn player_index_slot(&self) -> Option<(u8, &[u8])> {
        let slot = self.player_slot?;
        self.telemetry_slot(usize::from(slot)).map(|b| (slot, b))
    }

    /// `data.paths` bytes, if copied with this frame.
    pub fn paths(&self) -> Option<&[u8]> {
        self.paths_valid.then_some(&*self.paths)
    }

    /// `scoringStream[0..len]`, if copied with this frame.
    pub fn scoring_stream(&self) -> Option<&[u8]> {
        if self.stream_valid {
            self.stream.get(..self.stream_len)
        } else {
            None
        }
    }

    /// Raw `mNumVehicles` (unclamped).
    pub fn num_vehicles_raw(&self) -> Option<i32> {
        read_le(&self.scoring_header, scoring_info::NUM_VEHICLES.offset).map(i32::from_le_bytes)
    }

    /// Raw `scoringStreamSize` (unclamped).
    pub fn scoring_stream_size_raw(&self) -> Option<u64> {
        read_le(&self.scoring_header, STREAM_SIZE_IN_HEADER).map(u64::from_le_bytes)
    }

    /// The 4 padding bytes at absolute 2180 (expected zero).
    pub fn scoring_padding(&self) -> Option<[u8; PADDING_LEN]> {
        read_le(&self.scoring_header, PADDING_IN_HEADER)
    }

    /// Raw `scoringInfo.mCurrentET`.
    pub fn scoring_current_et(&self) -> Option<f64> {
        read_le(&self.scoring_header, scoring_info::CURRENT_ET.offset).map(f64::from_le_bytes)
    }

    /// Raw `telemetry.activeVehicles` (unclamped).
    pub fn active_vehicles_raw(&self) -> Option<u8> {
        self.telemetry_header
            .get(telemetry_data::ACTIVE_VEHICLES.offset)
            .copied()
    }

    /// Raw `telemetry.playerVehicleIdx`.
    pub fn player_vehicle_idx_raw(&self) -> Option<u8> {
        self.telemetry_header
            .get(telemetry_data::PLAYER_VEHICLE_IDX.offset)
            .copied()
    }

    /// Raw `telemetry.playerHasVehicle` byte (a C++ `bool`: 0 or 1 when the layout is right).
    pub fn player_has_vehicle_raw(&self) -> Option<u8> {
        self.telemetry_header
            .get(telemetry_data::PLAYER_HAS_VEHICLE.offset)
            .copied()
    }

    /// Raw `telemInfo[playerVehicleIdx].mElapsedTime`, if that slot was copied.
    pub fn player_elapsed_time(&self) -> Option<f64> {
        let (_, bytes) = self.player_index_slot()?;
        read_le(bytes, telem_info::ELAPSED_TIME.offset).map(f64::from_le_bytes)
    }

    /// Raw `generic.gameVersion`.
    pub fn game_version(&self) -> Option<i32> {
        read_le(&self.generic, generic::GAME_VERSION.offset).map(i32::from_le_bytes)
    }

    /// `true` if the scoring header and the copied vehicle rows are byte-identical to `other`'s.
    pub fn same_scoring(&self, other: &Self) -> bool {
        self.scoring_header == other.scoring_header && self.vehicles() == other.vehicles()
    }

    /// `true` if every block valid in either frame is byte-identical (a duplicate frame).
    pub fn same_content(&self, other: &Self) -> bool {
        self.generic == other.generic
            && self.telemetry_header == other.telemetry_header
            && self.same_scoring(other)
            && self.player_slot == other.player_slot
            && self.telemetry_slots() == other.telemetry_slots()
            && self.paths() == other.paths()
            && self.scoring_stream() == other.scoring_stream()
    }

    /// Every block present in this frame with its absolute offset in `LMU_Data`.
    pub fn present_blocks(&self) -> Vec<(BlockKind, usize, &[u8])> {
        let mut out = vec![
            (BlockKind::Generic, abs::GENERIC, self.generic()),
            (
                BlockKind::ScoringHeader,
                abs::SCORING_HEADER.offset,
                self.scoring_header(),
            ),
            (
                BlockKind::TelemetryHeader,
                abs::TELEMETRY_HEADER.offset,
                self.telemetry_header(),
            ),
            (BlockKind::Vehicles, abs::VEH_SCORING_INFO, self.vehicles()),
            (
                BlockKind::Telemetry,
                abs::TELEM_INFO,
                self.telemetry_slots(),
            ),
        ];
        if let Some(bytes) = self.paths() {
            out.push((BlockKind::Paths, abs::PATHS, bytes));
        }
        if let Some(bytes) = self.scoring_stream() {
            out.push((BlockKind::ScoringStream, abs::SCORING_STREAM, bytes));
        }
        out
    }

    /// Clears per-frame validity before [`RawBlocks::load_block`] calls for a new frame.
    pub(crate) fn begin_load(&mut self) {
        self.vehicle_count = 0;
        self.telemetry_count = 0;
        self.player_slot = None;
        self.paths_valid = false;
        self.stream_valid = false;
        self.stream_len = 0;
    }

    /// Loads one block read from a capture file, validating its placement against the layout.
    pub(crate) fn load_block(
        &mut self,
        kind: BlockKind,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), BlockError> {
        let misplaced = BlockError::Misplaced {
            kind,
            offset,
            len: bytes.len(),
        };
        let fixed = |expected_offset: usize, dst: &mut [u8]| -> Result<(), BlockError> {
            if offset != expected_offset || bytes.len() != dst.len() {
                return Err(misplaced.clone());
            }
            dst.copy_from_slice(bytes);
            Ok(())
        };
        match kind {
            BlockKind::Generic => fixed(abs::GENERIC, &mut self.generic),
            BlockKind::ScoringHeader => fixed(abs::SCORING_HEADER.offset, &mut self.scoring_header),
            BlockKind::TelemetryHeader => {
                fixed(abs::TELEMETRY_HEADER.offset, &mut self.telemetry_header)
            }
            BlockKind::Paths => {
                fixed(abs::PATHS, &mut self.paths)?;
                self.paths_valid = true;
                Ok(())
            }
            BlockKind::Vehicles => {
                if offset != abs::VEH_SCORING_INFO || !bytes.len().is_multiple_of(veh_scoring::SIZE)
                {
                    return Err(misplaced);
                }
                let dst = self.vehicles.get_mut(..bytes.len()).ok_or(misplaced)?;
                dst.copy_from_slice(bytes);
                self.vehicle_count = bytes.len() / veh_scoring::SIZE;
                Ok(())
            }
            BlockKind::Telemetry => {
                if offset != abs::TELEM_INFO || !bytes.len().is_multiple_of(telem_info::SIZE) {
                    return Err(misplaced);
                }
                let dst = self.telemetry.get_mut(..bytes.len()).ok_or(misplaced)?;
                dst.copy_from_slice(bytes);
                self.telemetry_count = bytes.len() / telem_info::SIZE;
                Ok(())
            }
            BlockKind::ScoringStream => {
                if offset != abs::SCORING_STREAM || bytes.len() > SCORING_STREAM_COPY_MAX {
                    return Err(misplaced);
                }
                self.enable_scoring_stream();
                let dst = self.stream.get_mut(..bytes.len()).ok_or(misplaced)?;
                dst.copy_from_slice(bytes);
                self.stream_len = bytes.len();
                self.stream_valid = true;
                Ok(())
            }
        }
    }

    /// Checks that a loaded frame is internally consistent (same rules [`copy_frame`] applies) and
    /// derives the player index slot from the loaded header.
    pub(crate) fn finish_load(&mut self, seen: &[BlockKind]) -> Result<(), BlockError> {
        for required in [
            BlockKind::Generic,
            BlockKind::ScoringHeader,
            BlockKind::TelemetryHeader,
            BlockKind::Vehicles,
            BlockKind::Telemetry,
        ] {
            if !seen.contains(&required) {
                return Err(BlockError::Missing(required));
            }
        }
        let expected_rows = clamp_count(self.num_vehicles_raw(), MAX_SCORING_VEHICLES);
        if expected_rows != self.vehicle_count {
            return Err(BlockError::Inconsistent(
                "vehicle rows do not match the clamped mNumVehicles",
            ));
        }
        let header: [u8; TELEMETRY_HEADER_LEN] = read_le(&self.telemetry_header, 0)
            .ok_or(BlockError::Missing(BlockKind::TelemetryHeader))?;
        if active_slots(header) != self.telemetry_count {
            return Err(BlockError::Inconsistent(
                "telemetry slots do not match the clamped activeVehicles",
            ));
        }
        self.player_slot = player_slot_from_header(header).and_then(|s| u8::try_from(s).ok());
        Ok(())
    }

    /// The bytes of every present block, owned and trimmed to what was copied (for queues and
    /// capture files; a [`RawBlocks`] clone copies the full ~260 KiB of buffers).
    pub fn to_owned_blocks(&self) -> OwnedBlocks {
        OwnedBlocks {
            blocks: self
                .present_blocks()
                .into_iter()
                .map(|(kind, offset, bytes)| (kind, offset, bytes.to_vec()))
                .collect(),
        }
    }
}

/// The present blocks of one frame, owned and trimmed (see [`RawBlocks::to_owned_blocks`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedBlocks {
    blocks: Vec<(BlockKind, usize, Vec<u8>)>,
}

impl OwnedBlocks {
    /// Total payload bytes (used to bound queues by memory, not only by message count).
    pub fn byte_len(&self) -> usize {
        self.blocks.iter().map(|(_, _, b)| b.len()).sum()
    }

    /// `(kind, absolute offset, bytes)` of every block, as [`RawBlocks::present_blocks`] returns them.
    pub fn blocks(&self) -> Vec<(BlockKind, usize, &[u8])> {
        self.blocks
            .iter()
            .map(|(kind, offset, bytes)| (*kind, *offset, bytes.as_slice()))
            .collect()
    }
}

/// Clamps a raw C count to `0..=max` (negative counts become 0).
fn clamp_count(raw: Option<i32>, max: usize) -> usize {
    raw.and_then(|n| usize::try_from(n).ok())
        .map_or(0, |n| n.min(max))
}

fn header_byte(header: [u8; TELEMETRY_HEADER_LEN], field: crate::offsets::Field) -> u8 {
    header.get(field.offset).copied().unwrap_or_default()
}

/// `min(activeVehicles, 104)` from a raw telemetry header (A-LMU-021).
fn active_slots(header: [u8; TELEMETRY_HEADER_LEN]) -> usize {
    usize::from(header_byte(header, telemetry_data::ACTIVE_VEHICLES)).min(MAX_TELEMETRY_VEHICLES)
}

/// The slot named by `playerVehicleIdx`, if `playerHasVehicle` is set and
/// `playerVehicleIdx < min(activeVehicles, 104)` (A-LMU-020/021).
pub(crate) fn player_slot_from_header(header: [u8; TELEMETRY_HEADER_LEN]) -> Option<usize> {
    let byte = |field| header_byte(header, field);
    let active = active_slots(header);
    let idx = usize::from(byte(telemetry_data::PLAYER_VEHICLE_IDX));
    let has = byte(telemetry_data::PLAYER_HAS_VEHICLE) != 0;
    (has && idx < active).then_some(idx)
}

/// Copies one frame from `src` into `blocks` (the lock critical section).
///
/// Allocation-free, logging-free and panic-free. All counts are clamped here: `mNumVehicles` and
/// `activeVehicles` to 104 slots, `scoringStreamSize` to 65,535 bytes. Every active telemetry slot
/// is copied (research §5.3 step 4.3, A-LMU-009), so the player can be verified by `mID` and the
/// opponents' telemetry is available.
///
/// # Errors
/// [`OutOfBounds`] if the source is smaller than the layout (the frame must then be discarded).
pub fn copy_frame<S: ByteSource + ?Sized>(
    src: &S,
    plan: CopyPlan,
    blocks: &mut RawBlocks,
) -> Result<(), OutOfBounds> {
    src.copy_to(abs::GENERIC, &mut blocks.generic)?;
    src.copy_to(abs::SCORING_HEADER.offset, &mut blocks.scoring_header)?;
    src.copy_to(abs::TELEMETRY_HEADER.offset, &mut blocks.telemetry_header)?;

    // Scoring rows: the count comes from the header bytes copied just above (read exactly once).
    let rows = clamp_count(blocks.num_vehicles_raw(), MAX_SCORING_VEHICLES);
    let len = rows * veh_scoring::SIZE;
    let oob = OutOfBounds {
        offset: abs::VEH_SCORING_INFO,
        len,
        available: blocks.vehicles.len(),
    };
    src.copy_to(
        abs::VEH_SCORING_INFO,
        blocks.vehicles.get_mut(..len).ok_or(oob)?,
    )?;
    blocks.vehicle_count = rows;

    // Telemetry slots 0..min(activeVehicles, 104), counted from the header copied above.
    blocks.player_slot = None;
    blocks.telemetry_count = 0;
    let header: [u8; TELEMETRY_HEADER_LEN] =
        read_le(&blocks.telemetry_header, 0).ok_or(OutOfBounds {
            offset: abs::TELEMETRY_HEADER.offset,
            len: TELEMETRY_HEADER_LEN,
            available: 0,
        })?;
    let slots = active_slots(header);
    let len = slots * telem_info::SIZE;
    let oob = OutOfBounds {
        offset: abs::TELEM_INFO,
        len,
        available: blocks.telemetry.len(),
    };
    src.copy_to(abs::TELEM_INFO, blocks.telemetry.get_mut(..len).ok_or(oob)?)?;
    blocks.telemetry_count = slots;
    blocks.player_slot = player_slot_from_header(header).and_then(|s| u8::try_from(s).ok());

    blocks.stream_valid = false;
    blocks.stream_len = 0;
    if plan.scoring_stream && !blocks.stream.is_empty() {
        let size = blocks.scoring_stream_size_raw().unwrap_or(0);
        let len = usize::try_from(size)
            .unwrap_or(usize::MAX)
            .min(SCORING_STREAM_COPY_MAX);
        let oob = OutOfBounds {
            offset: abs::SCORING_STREAM,
            len,
            available: blocks.stream.len(),
        };
        src.copy_to(
            abs::SCORING_STREAM,
            blocks.stream.get_mut(..len).ok_or(oob)?,
        )?;
        blocks.stream_len = len;
        blocks.stream_valid = true;
    }

    blocks.paths_valid = false;
    if plan.paths {
        src.copy_to(abs::PATHS, &mut blocks.paths)?;
        blocks.paths_valid = true;
    }
    Ok(())
}

/// Values that change with every publish; equal before and after a lock-free copy means the copy
/// was not torn by a concurrent write (best effort, used only without the lock or while polling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Witness {
    scoring_et_bits: u64,
    num_vehicles: i32,
    telemetry_header: [u8; TELEMETRY_HEADER_LEN],
    player_et_bits: Option<u64>,
}

/// Reads the witness values directly from the source.
pub(crate) fn read_witness<S: ByteSource + ?Sized>(src: &S) -> Result<Witness, OutOfBounds> {
    let mut et = [0u8; 8];
    src.copy_to(abs::SCORING_INFO + scoring_info::CURRENT_ET.offset, &mut et)?;
    let mut count = [0u8; 4];
    src.copy_to(
        abs::SCORING_INFO + scoring_info::NUM_VEHICLES.offset,
        &mut count,
    )?;
    let mut header = [0u8; TELEMETRY_HEADER_LEN];
    src.copy_to(abs::TELEMETRY_HEADER.offset, &mut header)?;
    let player_et_bits = match player_slot_from_header(header).and_then(abs::telem_info) {
        Some(slot_offset) => {
            let mut player_et = [0u8; 8];
            src.copy_to(
                slot_offset + telem_info::ELAPSED_TIME.offset,
                &mut player_et,
            )?;
            Some(u64::from_le_bytes(player_et))
        }
        None => None,
    };
    Ok(Witness {
        scoring_et_bits: u64::from_le_bytes(et),
        num_vehicles: i32::from_le_bytes(count),
        telemetry_header: header,
        player_et_bits,
    })
}

/// The witness values as captured in `blocks`.
pub(crate) fn witness_of(blocks: &RawBlocks) -> Option<Witness> {
    Some(Witness {
        scoring_et_bits: blocks.scoring_current_et()?.to_bits(),
        num_vehicles: blocks.num_vehicles_raw()?,
        telemetry_header: read_le(&blocks.telemetry_header, 0)?,
        player_et_bits: blocks.player_elapsed_time().map(f64::to_bits),
    })
}

/// Outcome of a witness-verified copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifiedCopy {
    /// Witness identical before, inside and after the copy.
    Consistent {
        /// Number of copy attempts used (1 = first try).
        attempts: u32,
    },
    /// Every attempt saw the witness change; the blocks must not be used.
    Torn {
        /// Number of copy attempts made.
        attempts: u32,
    },
}

/// Copies a frame and verifies it with before/after witness reads, retrying up to `max_attempts`.
pub(crate) fn copy_frame_verified<S: ByteSource + ?Sized>(
    src: &S,
    plan: CopyPlan,
    blocks: &mut RawBlocks,
    max_attempts: u32,
) -> Result<VerifiedCopy, OutOfBounds> {
    let attempts = max_attempts.max(1);
    for attempt in 1..=attempts {
        let before = read_witness(src)?;
        copy_frame(src, plan, blocks)?;
        let after = read_witness(src)?;
        if before == after && witness_of(blocks) == Some(before) {
            return Ok(VerifiedCopy::Consistent { attempts: attempt });
        }
    }
    Ok(VerifiedCopy::Torn { attempts })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::LayoutBuilder;

    #[test]
    fn copy_clamps_vehicle_count_and_ignores_rows_beyond_it() {
        let mut b = LayoutBuilder::new();
        b.num_vehicles(500);
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert_eq!(blocks.vehicle_count(), MAX_SCORING_VEHICLES);
        assert_eq!(
            blocks.num_vehicles_raw(),
            Some(500),
            "the raw value is kept for compatibility checks"
        );

        b.num_vehicles(-3);
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert_eq!(blocks.vehicle_count(), 0);
        assert!(blocks.vehicles().is_empty());
    }

    #[test]
    fn copy_takes_the_player_slot_only_when_valid() {
        let mut b = LayoutBuilder::new();
        b.telemetry_header(3, 2, 1);
        b.player_elapsed_time(2, 12.5);
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert_eq!(blocks.player_index_slot().map(|(slot, _)| slot), Some(2));
        assert_eq!(blocks.player_elapsed_time(), Some(12.5));
        assert_eq!(blocks.telemetry_count(), 3);

        // Index beyond activeVehicles: no player slot.
        b.telemetry_header(2, 2, 1);
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert!(blocks.player_index_slot().is_none());
        assert_eq!(blocks.telemetry_count(), 2);

        // activeVehicles above the slot count is clamped: index 200 is never copied.
        b.telemetry_header(255, 200, 1);
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert!(blocks.player_index_slot().is_none());
        assert_eq!(blocks.telemetry_count(), MAX_TELEMETRY_VEHICLES);

        // No vehicle: slots are still copied (opponents), but there is no player slot.
        b.telemetry_header(3, 1, 0);
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert!(blocks.player_index_slot().is_none());
        assert_eq!(blocks.telemetry_count(), 3);
    }

    #[test]
    fn every_active_telemetry_slot_is_copied() {
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(3, 0, 1);
        b.telemetry_slot(2, 11, 42.0);
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert_eq!(blocks.telemetry_slots().len(), 3 * telem_info::SIZE);
        let slot2 = blocks.telemetry_slot(2).expect("slot 2 copied");
        assert_eq!(
            read_le(slot2, telem_info::ID.offset).map(i32::from_le_bytes),
            Some(11)
        );
        assert!(blocks.telemetry_slot(3).is_none(), "beyond activeVehicles");
    }

    #[test]
    fn scoring_stream_is_clamped_and_only_copied_when_planned() {
        let mut b = LayoutBuilder::new();
        b.scoring_stream_size(1 << 40);
        let mut blocks = RawBlocks::new();
        let plan = CopyPlan {
            scoring_stream: true,
            paths: false,
        };
        copy_frame(b.bytes(), plan, &mut blocks).expect("copy");
        assert!(blocks.scoring_stream().is_none(), "no buffer allocated yet");

        blocks.enable_scoring_stream();
        copy_frame(b.bytes(), plan, &mut blocks).expect("copy");
        assert_eq!(
            blocks.scoring_stream().map(<[u8]>::len),
            Some(SCORING_STREAM_COPY_MAX)
        );

        b.scoring_stream_size(5);
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert!(
            blocks.scoring_stream().is_none(),
            "not planned for this frame"
        );
        copy_frame(b.bytes(), plan, &mut blocks).expect("copy");
        assert_eq!(blocks.scoring_stream().map(<[u8]>::len), Some(5));
    }

    #[test]
    fn paths_are_copied_only_when_planned() {
        let b = LayoutBuilder::new();
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        assert!(blocks.paths().is_none());
        copy_frame(
            b.bytes(),
            CopyPlan {
                paths: true,
                scoring_stream: false,
            },
            &mut blocks,
        )
        .expect("copy");
        assert!(blocks.paths().is_some());
    }

    #[test]
    fn copy_from_a_short_source_fails_without_panicking() {
        let short = vec![0u8; abs::TELEMETRY];
        let mut blocks = RawBlocks::new();
        assert!(copy_frame(&short, CopyPlan::default(), &mut blocks).is_err());
    }

    #[test]
    fn witness_detects_a_change_between_reads() {
        let mut b = LayoutBuilder::new();
        b.telemetry_header(1, 0, 1);
        b.player_elapsed_time(0, 1.0);
        let w1 = read_witness(b.bytes()).expect("witness");
        b.player_elapsed_time(0, 1.01);
        let w2 = read_witness(b.bytes()).expect("witness");
        assert_ne!(w1, w2);
        let mut blocks = RawBlocks::new();
        let outcome =
            copy_frame_verified(b.bytes(), CopyPlan::default(), &mut blocks, 3).expect("copy");
        assert_eq!(outcome, VerifiedCopy::Consistent { attempts: 1 });
        assert_eq!(witness_of(&blocks), Some(w2));
    }

    #[test]
    fn present_blocks_round_trip_through_load_block() {
        let mut b = LayoutBuilder::new();
        b.num_vehicles(2);
        b.telemetry_header(2, 1, 1);
        b.player_elapsed_time(1, 3.0);
        let mut blocks = RawBlocks::new();
        blocks.enable_scoring_stream();
        b.scoring_stream_size(3);
        copy_frame(
            b.bytes(),
            CopyPlan {
                paths: true,
                scoring_stream: true,
            },
            &mut blocks,
        )
        .expect("copy");

        let mut loaded = RawBlocks::new();
        loaded.begin_load();
        let mut seen = Vec::new();
        for (kind, offset, bytes) in blocks.present_blocks() {
            loaded.load_block(kind, offset, bytes).expect("load");
            seen.push(kind);
        }
        loaded.finish_load(&seen).expect("consistent");
        assert_eq!(loaded.present_blocks(), blocks.present_blocks());
    }

    /// Informational timing of the copy for a full frame (104 scoring rows, 104 telemetry slots):
    /// `cargo test --release -p lmu-reader copy_timing -- --ignored --nocapture`.
    ///
    /// **Hot-cache, in-process only**: the source is a `Vec` in this process that stays in cache
    /// across rounds, so cross-process / cross-core cache traffic of the real mapped view is not
    /// exercised. The evidence for the 50 µs lock-hold budget is the `lock_hold` histogram that
    /// `lmu-diag live` records against the running game at Gate 1.
    #[test]
    #[ignore = "timing measurement; run in release mode"]
    fn copy_timing_of_a_full_frame() {
        const ROUNDS: usize = 20_000;
        const PERCENTILE_99: usize = 99;
        let mut b = LayoutBuilder::on_track_frame();
        b.num_vehicles(i32::try_from(MAX_SCORING_VEHICLES).expect("count"));
        b.telemetry_header(u8::try_from(MAX_TELEMETRY_VEHICLES).expect("count"), 0, 1);
        let mut blocks = RawBlocks::new();
        let mut samples: Vec<u128> = Vec::with_capacity(ROUNDS);
        for _ in 0..ROUNDS {
            let started = std::time::Instant::now();
            copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
            samples.push(started.elapsed().as_nanos());
        }
        samples.sort_unstable();
        eprintln!(
            "full-frame copy ({} bytes, hot cache): median {} ns, p99 {} ns, max {} ns",
            generic::SIZE
                + SCORING_HEADER_LEN
                + VEHICLES_CAPACITY
                + TELEMETRY_HEADER_LEN
                + TELEMETRY_CAPACITY,
            samples[ROUNDS / 2],
            samples[ROUNDS * PERCENTILE_99 / 100],
            samples[ROUNDS - 1]
        );
    }

    #[test]
    fn load_block_rejects_misplaced_or_inconsistent_blocks() {
        let mut blocks = RawBlocks::new();
        blocks.begin_load();
        assert!(
            blocks
                .load_block(BlockKind::Generic, 1, &[0u8; generic::SIZE])
                .is_err()
        );
        assert!(
            blocks
                .load_block(BlockKind::Vehicles, abs::VEH_SCORING_INFO, &[0u8; 10])
                .is_err()
        );
        assert!(
            blocks
                .load_block(
                    BlockKind::Telemetry,
                    abs::TELEM_INFO + 1,
                    &[0u8; telem_info::SIZE]
                )
                .is_err()
        );
        assert!(
            blocks
                .load_block(
                    BlockKind::Telemetry,
                    abs::TELEM_INFO,
                    &vec![0u8; TELEMETRY_CAPACITY + telem_info::SIZE]
                )
                .is_err(),
            "more than 104 slots"
        );
        assert!(matches!(
            blocks.finish_load(&[]),
            Err(BlockError::Missing(BlockKind::Generic))
        ));
    }

    #[test]
    fn owned_blocks_hold_only_the_present_bytes() {
        let b = LayoutBuilder::on_track_frame();
        let mut blocks = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy");
        let owned = blocks.to_owned_blocks();
        assert_eq!(owned.blocks(), blocks.present_blocks());
        let expected = generic::SIZE
            + SCORING_HEADER_LEN
            + TELEMETRY_HEADER_LEN
            + 2 * veh_scoring::SIZE
            + 2 * telem_info::SIZE;
        assert_eq!(owned.byte_len(), expected);
        assert!(owned.byte_len() < VEHICLES_CAPACITY + TELEMETRY_CAPACITY);
    }
}

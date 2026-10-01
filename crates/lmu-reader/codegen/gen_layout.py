#!/usr/bin/env python3
"""Generate `src/offsets.rs` and `src/raw_structs.rs` from the layout probe JSON.

Input : docs/research/lmu_abi_layout.json (produced by tools/layout-probe against the *installed* SDK
        headers; it contains identifiers, sizes and offsets only, never header text).
Output: crates/lmu-reader/src/offsets.rs      named offset/size constants + a test that re-loads the JSON
        crates/lmu-reader/src/raw_structs.rs  typed raw structs + bounds-checked decoders

Run after every re-probe (LMU update that touches the SDK), then `cargo fmt -p lmu-reader` and review
the diff. The unit test in offsets.rs fails loudly whenever the committed constants and the JSON differ,
so a stale generation cannot go unnoticed.

    python crates/lmu-reader/codegen/gen_layout.py
"""

from __future__ import annotations

import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent
CRATE = HERE.parent
REPO = CRATE.parent.parent
JSON_PATH = REPO / "docs" / "research" / "lmu_abi_layout.json"
OFFSETS_RS = CRATE / "src" / "offsets.rs"
RAW_RS = CRATE / "src" / "raw_structs.rs"

# (JSON struct name, Rust module name, short description)
STRUCT_MODULES = [
    ("SharedMemoryLayout", "layout", "`SharedMemoryLayout`: the whole `LMU_Data` view"),
    ("SharedMemoryObjectOut", "object_out", "`SharedMemoryObjectOut`: the four top-level blocks"),
    ("SharedMemoryGeneric", "generic", "`SharedMemoryGeneric`: event queue, game version, FFB, app state"),
    ("ApplicationStateV01", "app_state", "`ApplicationStateV01` (inside the generic block)"),
    ("SharedMemoryPathData", "paths", "`SharedMemoryPathData`: five `char[260]` paths (PII)"),
    ("SharedMemoryScoringData", "scoring_data", "`SharedMemoryScoringData`: scoring wrapper block"),
    ("ScoringInfoV01", "scoring_info", "`ScoringInfoV01`: session-wide scoring info"),
    ("VehicleScoringInfoV01", "veh_scoring", "`VehicleScoringInfoV01`: one scoring row per car"),
    ("SharedMemoryTelemetryData", "telemetry_data", "`SharedMemoryTelemetryData`: telemetry wrapper block"),
    ("TelemInfoV01", "telem_info", "`TelemInfoV01`: one telemetry slot"),
    ("TelemWheelV01", "telem_wheel", "`TelemWheelV01`: one wheel inside a telemetry slot"),
    ("TelemVect3", "vect3", "`TelemVect3`: three `f64` components"),
    ("SharedMemoryLock::LockData", "lock_data", "`SharedMemoryLock::LockData`: the two lock words"),
]

NAME_OVERRIDES = {"mSoC": "soc", "mID": "id"}


def snake(name: str) -> str:
    """C field name -> Rust snake_case (drops the `m` member prefix)."""
    if name in NAME_OVERRIDES:
        return NAME_OVERRIDES[name]
    n = name
    if re.match(r"^m[A-Z]", n):
        n = n[1:]
    n = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", n)
    n = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", n)
    return n.lower()


def const_name(name: str) -> str:
    return snake(name).upper()


# Short unit / semantics hints, paraphrased from docs/LMU_TELEMETRY_RESEARCH.md and
# docs/LMU_CHANNEL_MAP.md (evidence labels live there). No header text.
HINTS = {
    ("TelemInfoV01", "mID"): "Slot id; unique within a snapshot, reused in multiplayer (A-LMU-023).",
    ("TelemInfoV01", "mDeltaTime"): "Time since the previous update, s.",
    ("TelemInfoV01", "mElapsedTime"): "Game session time, s. Telemetry freshness key (A-LMU-031).",
    ("TelemInfoV01", "mLapNumber"): "Current lap number.",
    ("TelemInfoV01", "mLapStartET"): "Session time at which the current lap started, s.",
    ("TelemInfoV01", "mPos"): "World position, m (left-handed world, +y up).",
    ("TelemInfoV01", "mLocalVel"): "Velocity in the local vehicle frame, m/s (+x left, +y up, +z rear).",
    ("TelemInfoV01", "mLocalAccel"): "Acceleration in the local vehicle frame, m/s^2 (gravity handling: A-LMU-032).",
    ("TelemInfoV01", "mOri"): "Orientation matrix rows (world frame).",
    ("TelemInfoV01", "mLocalRot"): "Rotation rate in the local frame, rad/s (+y yaws right).",
    ("TelemInfoV01", "mLocalRotAccel"): "Rotational acceleration in the local frame, rad/s^2.",
    ("TelemInfoV01", "mGear"): "-1 reverse, 0 neutral, 1.. forward.",
    ("TelemInfoV01", "mEngineRPM"): "Engine speed, rpm.",
    ("TelemInfoV01", "mEngineWaterTemp"): "Engine water temperature, degrees Celsius per header.",
    ("TelemInfoV01", "mEngineOilTemp"): "Engine oil temperature, degrees Celsius per header.",
    ("TelemInfoV01", "mClutchRPM"): "Clutch speed, rpm.",
    ("TelemInfoV01", "mUnfilteredThrottle"): "Raw driver input, 0..1.",
    ("TelemInfoV01", "mUnfilteredBrake"): "Raw driver input, 0..1.",
    ("TelemInfoV01", "mUnfilteredSteering"): "Raw driver input, -1..1.",
    ("TelemInfoV01", "mUnfilteredClutch"): "Raw driver input, 0..1.",
    ("TelemInfoV01", "mFilteredThrottle"): "Input after aids, 0..1.",
    ("TelemInfoV01", "mFilteredBrake"): "Input after aids, 0..1.",
    ("TelemInfoV01", "mFilteredSteering"): "Input after aids, -1..1.",
    ("TelemInfoV01", "mFilteredClutch"): "Input after aids, 0..1.",
    ("TelemInfoV01", "mSteeringShaftTorque"): "Steering shaft torque; unit not stated (assumed N*m, A-LMU-053).",
    ("TelemInfoV01", "mFuel"): "Fuel in the tank, litres (A-LMU-018).",
    ("TelemInfoV01", "mFuelCapacity"): "Tank capacity, litres (A-LMU-018).",
    ("TelemInfoV01", "mEngineMaxRPM"): "Rev limit, rpm.",
    ("TelemInfoV01", "mDentSeverity"): "Dent severity per body location, 0/1/2.",
    ("TelemInfoV01", "mEngineTorque"): "Engine torque; unit not stated (assumed N*m, A-LMU-053).",
    ("TelemInfoV01", "mCurrentSector"): "Zero-based sector in the low bits; bit 31 set while in the pit lane (A-LMU-026).",
    ("TelemInfoV01", "mRearBrakeBias"): "Fraction of brake force on the rear axle.",
    ("TelemInfoV01", "mTurboBoostPressure"): "Boost pressure; unit not stated (A-LMU-027).",
    ("TelemInfoV01", "mDeltaBest"): "Delta to best lap; unit not stated (assumed s, A-LMU-033).",
    ("TelemInfoV01", "mBatteryChargeFraction"): "Battery charge, 0..1.",
    ("TelemInfoV01", "mElectricBoostMotorState"): "0 unavailable, 1 inactive, 2 propulsion, 3 regeneration.",
    ("TelemInfoV01", "mABSActive"): "ABS intervening right now.",
    ("TelemInfoV01", "mTCActive"): "TC intervening right now.",
    ("TelemInfoV01", "mTC"): "TC setting level.",
    ("TelemInfoV01", "mABS"): "ABS setting level.",
    ("TelemInfoV01", "mRegen"): "Regeneration power, kW per header.",
    ("TelemInfoV01", "mSoC"): "State of charge; unit not stated (A-LMU-014).",
    ("TelemInfoV01", "mVirtualEnergy"): "Virtual energy; unit not stated (A-LMU-013).",
    ("TelemInfoV01", "mTimeGapCarAhead"): "Gap to the car physically ahead; unit not stated (A-LMU-034).",
    ("TelemInfoV01", "mTimeGapCarBehind"): "Gap to the car physically behind; unit not stated (A-LMU-034).",
    ("TelemInfoV01", "mTimeGapPlaceAhead"): "Gap to the car ahead in the classification; unit not stated (A-LMU-034).",
    ("TelemInfoV01", "mTimeGapPlaceBehind"): "Gap to the car behind in the classification; unit not stated (A-LMU-034).",
    ("TelemInfoV01", "mVehicleClass"): "Raw `IP_VehicleClass` value, see [`crate::VehicleClass`].",
    ("TelemInfoV01", "mVehicleChampionship"): "Raw `IP_VehicleChampionship` value, see [`crate::VehicleChampionship`].",
    ("TelemInfoV01", "mPhysicalSteeringWheelRange"): "Steering wheel range; unit not stated (assumed degrees, A-LMU-037).",
    ("TelemWheelV01", "mBrakeTemp"): "Brake temperature; header says Celsius, community says Kelvin (A-LMU-015).",
    ("TelemWheelV01", "mBrakePressure"): "Normalised 0..1 (pedal x bias), not a line pressure (A-LMU-017).",
    ("TelemWheelV01", "mRotation"): "Wheel rotation, rad/s (sign: A-LMU-039).",
    ("TelemWheelV01", "mPressure"): "Tyre pressure, kPa.",
    ("TelemWheelV01", "mTemperature"): "Tread temperature left/centre/right in the car frame, Kelvin (A-LMU-016).",
    ("TelemWheelV01", "mWear"): "Wear as a fraction of maximum; direction unverified (A-LMU-040).",
    ("TelemWheelV01", "mTireCarcassTemperature"): "Carcass temperature, Kelvin.",
    ("TelemWheelV01", "mTireInnerLayerTemperature"): "Inner layer temperature left/centre/right, Kelvin.",
    ("TelemWheelV01", "mOptimalTemp"): "Optimal temperature; unit not stated (A-LMU-038).",
    ("TelemWheelV01", "mStaticUndeflectedRadius"): "Static undeflected radius, cm.",
    ("TelemWheelV01", "mCompoundType"): "Compound type; meaning CLAIMED only (A-LMU-041).",
    ("ScoringInfoV01", "mSession"): "0 test day, 1-4 practice, 5-8 qualifying, 9 warm-up, 10-13 race.",
    ("ScoringInfoV01", "mCurrentET"): "Current session time, s. Scoring freshness key (A-LMU-031).",
    ("ScoringInfoV01", "mEndET"): "Session end time, s.",
    ("ScoringInfoV01", "mMaxLaps"): "Maximum laps; INT32_MAX in timed races (A-LMU-043).",
    ("ScoringInfoV01", "mLapDist"): "Track length, m.",
    ("ScoringInfoV01", "mNumVehicles"): "Number of valid `vehScoringInfo` rows (clamped to 104 when copied).",
    ("ScoringInfoV01", "mGamePhase"): "0 before session .. 5 green, 6 FCY/safety car, 7 stopped, 8 over, 9 paused.",
    ("ScoringInfoV01", "mYellowFlagState"): "Full-course yellow state machine value.",
    ("ScoringInfoV01", "mSectorFlag"): "Per-sector flags; 1 = yellow, 11 = clear (CLAIMED, A-LMU-030); index order unknown.",
    ("ScoringInfoV01", "mPlayerName"): "Player name (PII, never transmitted).",
    ("ScoringInfoV01", "mPlrFileName"): "Player file name (PII, never transmitted).",
    ("ScoringInfoV01", "mDarkCloud"): "Cloud darkness, 0..1.",
    ("ScoringInfoV01", "mRaining"): "Rain severity, 0..1.",
    ("ScoringInfoV01", "mAmbientTemp"): "Ambient temperature, degrees Celsius.",
    ("ScoringInfoV01", "mTrackTemp"): "Track temperature, degrees Celsius.",
    ("ScoringInfoV01", "mWind"): "Wind vector; frame and unit unverified (A-LMU-042).",
    ("ScoringInfoV01", "mMinPathWetness"): "Minimum wetness on the racing line, 0..1.",
    ("ScoringInfoV01", "mMaxPathWetness"): "Maximum wetness on the racing line, 0..1.",
    ("ScoringInfoV01", "mAvgPathWetness"): "Average wetness on the racing line, 0..1.",
    ("ScoringInfoV01", "mServerName"): "Server name (privacy: not transmitted by default).",
    ("ScoringInfoV01", "mServerPublicIP"): "Server public IP (privacy: not transmitted).",
    ("ScoringInfoV01", "mServerPort"): "Server port (privacy: not transmitted).",
    ("ScoringInfoV01", "mStartET"): "Start time of day, seconds since midnight.",
    ("ScoringInfoV01", "mSessionTimeRemaining"): "Session time remaining; unit not stated (assumed s, A-LMU-035).",
    ("ScoringInfoV01", "mTimeOfDay"): "Time of day; scale unverified (A-LMU-036).",
    ("VehicleScoringInfoV01", "mID"): "Slot id; join key with telemetry, reused in multiplayer (A-LMU-023).",
    ("VehicleScoringInfoV01", "mDriverName"): "Driver name (PII).",
    ("VehicleScoringInfoV01", "mSector"): "Sector: 0 = sector 3, 1 = sector 1, 2 = sector 2 (A-LMU-026).",
    ("VehicleScoringInfoV01", "mLapDist"): "Distance around the track, m.",
    ("VehicleScoringInfoV01", "mBestSector1"): "Best sector 1, s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mBestSector2"): "Best sector 1 + 2 (cumulative), s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mBestLapTime"): "Best lap time, s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mLastSector1"): "Last sector 1, s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mLastSector2"): "Last sector 1 + 2 (cumulative), s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mLastLapTime"): "Last lap time, s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mCurSector1"): "Current lap sector 1, s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mCurSector2"): "Current lap sector 1 + 2 (cumulative), s (<= 0 means none).",
    ("VehicleScoringInfoV01", "mIsPlayer"): "Row belongs to the local player.",
    ("VehicleScoringInfoV01", "mInPits"): "In the pit lane; unreliable for remote cars (A-LMU-029).",
    ("VehicleScoringInfoV01", "mPlace"): "Position in the classification, 1-based.",
    ("VehicleScoringInfoV01", "mVehicleClass"): "Class name text (not the telemetry enum, A-LMU-055).",
    ("VehicleScoringInfoV01", "mPitState"): "0 none, 1 request, 2 entering, 3 stopped, 4 exiting.",
    ("VehicleScoringInfoV01", "mFlag"): "Flag shown to the car (0 green, 6 blue).",
    ("VehicleScoringInfoV01", "mSteamID"): "Steam id (PII, never transmitted).",
    ("VehicleScoringInfoV01", "mFuelFraction"): "Fuel or battery fraction, 0..255 (A-LMU-044).",
    ("SharedMemoryGeneric", "events"): "Queue of `SharedMemoryEvent` codes, not flags (A-LMU-009).",
    ("SharedMemoryGeneric", "gameVersion"): "Game version number (recorded, A-LMU-010).",
    ("SharedMemoryGeneric", "FFBTorque"): "Force-feedback output; scale unverified.",
    ("ApplicationStateV01", "mOptionsLocation"): "0 main UI, 1 track loading, 2 monitor, 3 on track.",
}

# Scalars: JSON kind -> (rust type, reader method)
SCALARS = {
    "i8": ("i8", "i8"),
    "u8": ("u8", "u8"),
    "bool": ("bool", "bool"),
    "i16": ("i16", "i16"),
    "u16": ("u16", "u16"),
    "i32": ("i32", "i32"),
    "u32": ("u32", "u32"),
    "f32": ("f32", "f32"),
    "f64": ("f64", "f64"),
    "u64": ("u64", "u64"),
    "enum:u8": ("u8", "u8"),
    "enum:u32": ("u32", "u32"),
}

# Arrays of this many bytes or fewer stay fixed-size arrays; serde derives stop at 32.
MAX_FIXED_ARRAY = 32

# Fields that are reserved/padding bytes (kept for diagnostics only).
RESERVED_NAMES = {"mExpansion", "mUnused"}


def rust_field(struct: str, f: dict):
    """Return (rust_type, decode_expr) for a field, or None when the field is never decoded."""
    name, kind, dims, ftype = f["name"], f["kind"], f.get("dims", []), f["type"]
    const = f"{MODULE_OF[struct]}::{const_name(name)}"
    if kind == "ptr":
        return None  # pointers into LMU's address space: never decoded, never dereferenced
    if f.get("union_alt", 0) != 0:
        return None  # union alternative view of the same bytes
    if kind == "char":
        return ("RawString", f"r.string({const})?")
    if kind == "struct":
        if "TelemVect3" in ftype:
            if dims:
                return (f"[Vec3; {dims[0]}]", f"r.vec3_array::<{dims[0]}>({const})?")
            return ("Vec3", f"r.vec3({const})?")
        if "TelemWheelV01" in ftype:
            return None  # decoded separately (see RawTelemetry::wheels)
        return None  # nested wrapper structs are decoded block by block
    if dims:
        (n,) = dims
        if kind == "u8" and (name in RESERVED_NAMES or n > MAX_FIXED_ARRAY):
            return ("ReservedBytes", f"r.reserved({const})?")
        base, method = SCALARS[kind]
        return (f"[{base}; {n}]", f"r.{method}_array::<{n}>({const})?")
    base, method = SCALARS[kind]
    return (base, f"r.{method}({const})?")


MODULE_OF = {s: m for s, m, _ in STRUCT_MODULES}


def bool_fields(s: dict) -> list:
    """The C++ `bool` members of a struct (never union alternatives)."""
    return [f for f in s["fields"] if f["kind"] == "bool" and not f.get("union_alt", 0)]


FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3


def layout_fingerprint(j: dict) -> int:
    """FNV-1a 64 over `"{struct}:{sizeof}\\n"` and `"{field}@{offset}+{size}:{type}\\n"` lines, structs
    sorted by name, fields in probe order. The Rust test `layout_fingerprint_matches_the_probe` in
    `src/capture.rs` recomputes it from the JSON."""
    lines = []
    for name in sorted(j["structs"]):
        s = j["structs"][name]
        lines.append(f"{name}:{s['sizeof']}\n")
        for f in s["fields"]:
            lines.append(f"{f['name']}@{f['offset']}+{f['size']}:{f['type']}\n")
    h = FNV_OFFSET
    for byte in "".join(lines).encode("utf-8"):
        h ^= byte
        h = (h * FNV_PRIME) & 0xFFFFFFFFFFFFFFFF
    return h


def gen_offsets(j: dict) -> str:
    out = []
    w = out.append
    w("//! Byte offsets and sizes of the LMU `LMU_Data` shared-memory layout (D-003).")
    w("//!")
    w("//! **Generated** by `crates/lmu-reader/codegen/gen_layout.py` from")
    w("//! `docs/research/lmu_abi_layout.json`, the output of `tools/layout-probe` compiled with MSVC x64")
    w("//! against the SDK headers of the installed game (LMU 1.4.2.0). Do not edit by hand: re-run the probe")
    w("//! and the generator. The test at the bottom re-loads the JSON and fails loudly on any difference.")
    w("//!")
    w("//! * Per-struct modules hold offsets *relative to the start of that struct*.")
    w("//! * [`abs`] holds offsets *from the start of the mapping view*.")
    w("//! * `InternalsPlugin.hpp` structs are `pack(4)` (doubles only 4-aligned); the `SharedMemory*` wrappers")
    w("//!   use default packing, hence the 4-byte hole at absolute 2180 (A-LMU-007).")
    w("")
    w("/// Location of one field inside its parent struct.")
    w("#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
    w("pub struct Field {")
    w("    /// Byte offset from the start of the parent struct.")
    w("    pub offset: usize,")
    w("    /// Size of the whole field in bytes (all elements for arrays).")
    w("    pub size: usize,")
    w("}")
    w("")
    w("impl Field {")
    w("    /// Creates a field location.")
    w("    pub const fn new(offset: usize, size: usize) -> Self {")
    w("        Self { offset, size }")
    w("    }")
    w("")
    w("    /// One past the last byte of the field.")
    w("    pub const fn end(&self) -> usize {")
    w("        self.offset + self.size")
    w("    }")
    w("}")
    w("")
    c = j["constants"]
    w("/// Number of slots in `vehScoringInfo[]` (and the clamp applied to `mNumVehicles`).")
    w(f"pub const MAX_SCORING_VEHICLES: usize = {c['scoring_vehicle_slots']};")
    w("/// Number of slots in `telemInfo[]` (and the clamp applied to `activeVehicles`).")
    w(f"pub const MAX_TELEMETRY_VEHICLES: usize = {c['telemetry_vehicle_slots']};")
    w("/// Capacity of `scoringStream[]` in bytes.")
    w(f"pub const SCORING_STREAM_CAPACITY: usize = {c['scoring_stream_capacity']};")
    w("/// Maximum number of scoring-stream bytes we copy: one less than the capacity, because the SDK")
    w("/// helper NUL-terminates at `[scoringStreamSize]`, which is out of bounds at full capacity.")
    w("pub const SCORING_STREAM_COPY_MAX: usize = SCORING_STREAM_CAPACITY - 1;")
    w("/// Number of slots in the `generic.events[]` queue (`SME_MAX`).")
    w(f"pub const EVENT_SLOTS: usize = {c['SME_MAX']};")
    w("/// Length of each path in `SharedMemoryPathData` (`MAX_PATH`).")
    w(f"pub const PATH_LEN: usize = {c['MAX_PATH']};")
    w("/// Number of wheels in `TelemInfoV01.mWheel[]` (FL, FR, RL, RR).")
    w("pub const WHEEL_COUNT: usize = 4;")
    w("/// FNV-1a 64 over every probed struct (name, sizeof) and field (name, offset, size, type), see")
    w("/// `layout_fingerprint` in `codegen/gen_layout.py`. Stored in `.lmuraw` captures so a capture is")
    w("/// never decoded with a layout that moved fields without changing `sizeof` (e.g. a re-probe).")
    w(f"pub const LAYOUT_FINGERPRINT: u64 = {layout_fingerprint(j):#018x};")
    w("")
    names = c["names"]
    w("/// Kernel object names (session-local namespace, no `Global\\` prefix).")
    w("pub mod names {")
    w("    /// The data mapping, created by LMU; opened read-only (`FILE_MAP_READ`).")
    w(f"    pub const DATA_MAPPING: &str = \"{names['LMU_SHARED_MEMORY_FILE']['value']}\";")
    w("    /// Frame gate event 1, waited on first (opened with `SYNCHRONIZE` only).")
    w(f"    pub const HOLD_EVENT: &str = \"{names['LMU_SHARED_MEMORY_HOLD_EVENT']['value']}\";")
    w("    /// Frame gate event 2, waited on second (opened with `SYNCHRONIZE` only).")
    w(f"    pub const DATA_EVENT: &str = \"{names['LMU_SHARED_MEMORY_DATA_EVENT']['value']}\";")
    w("    /// The 8-byte lock-word mapping. Opened only if it already exists; never created or reset.")
    w(f"    pub const LOCK_MAPPING: &str = \"{names['lock_mapping']['value']}\";")
    w("    /// The auto-reset lock wake-up event.")
    w(f"    pub const LOCK_EVENT: &str = \"{names['lock_event']['value']}\";")
    w("}")
    w("")
    w("/// `SharedMemoryEvent` codes found in `generic.events[]`.")
    w("pub mod sme {")
    for k, v in j["enums"]["SharedMemoryEvent"]["values"].items():
        w(f"    /// `{k}`.")
        w(f"    pub const {k.removeprefix('SME_')}: u32 = {v};")
    w("}")
    w("")
    w("/// Raw values of the `IP_VehicleClass` enum (`TelemInfoV01.mVehicleClass`).")
    w("pub mod vehicle_class {")
    for k, v in j["enums"]["IP_VehicleClass"]["values"].items():
        w(f"    /// `IP_VehicleClass::{k}`.")
        w(f"    pub const {const_name(k)}: u8 = {v};")
    w("}")
    w("")
    w("/// Raw values of the `IP_VehicleChampionship` enum (`TelemInfoV01.mVehicleChampionship`).")
    w("pub mod vehicle_championship {")
    for k, v in j["enums"]["IP_VehicleChampionship"]["values"].items():
        w(f"    /// `IP_VehicleChampionship::{k}`.")
        w(f"    pub const {const_name(k).replace('__', '_')}: u8 = {v};")
    w("}")
    w("")
    for sname, mod, desc in STRUCT_MODULES:
        s = j["structs"][sname]
        w(f"/// {desc}.")
        w(f"pub mod {mod} {{")
        w("    use super::Field;")
        w("")
        w(f"    /// `sizeof({sname})`; also the array stride where the struct is used in an array.")
        w(f"    pub const SIZE: usize = {s['sizeof']};")
        w(f"    /// `alignof({sname})`.")
        w(f"    pub const ALIGN: usize = {s['alignof']};")
        for f in s["fields"]:
            alt = " (union alternative view)" if f.get("union_alt", 0) else ""
            w(f"    /// `{f['name']}`: `{f['type']}`{alt}.")
            w(f"    pub const {const_name(f['name'])}: Field = Field::new({f['offset']}, {f['size']});")
        bools = bool_fields(s)
        if bools:
            w("    /// Every C++ `bool` member: one byte that reads 0 or 1 while the layout is right.")
            w("    /// The runtime layout checks flag any other value (a hint that fields have moved).")
            w("    pub const BOOL_FIELDS: &[(&str, Field)] = &[")
            for f in bools:
                w(f'        ("{f["name"]}", {const_name(f["name"])}),')
            w("    ];")
        w("}")
        w("")
    w(ABS_MODULE)
    w(gen_offsets_test(j))
    return "\n".join(out)


ABS_MODULE = '''/// Absolute offsets from the start of the `LMU_Data` view (composed from the per-struct constants).
pub mod abs {
    use super::{
        Field, MAX_SCORING_VEHICLES, MAX_TELEMETRY_VEHICLES, object_out, scoring_data, scoring_info,
        telem_info, telemetry_data, veh_scoring,
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
        scoring_data::SCORING_STREAM_SIZE.offset - (scoring_data::SCORING_INFO.offset + scoring_info::SIZE),
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
        if index < MAX_SCORING_VEHICLES { Some(VEH_SCORING_INFO + index * veh_scoring::SIZE) } else { None }
    }

    /// Absolute start of `telemInfo[index]`, or `None` when `index` is not a valid slot.
    pub const fn telem_info(index: usize) -> Option<usize> {
        if index < MAX_TELEMETRY_VEHICLES { Some(TELEM_INFO + index * telem_info::SIZE) } else { None }
    }
}
'''


def gen_offsets_test(j: dict) -> str:
    lines = []
    w = lines.append
    w("#[cfg(test)]")
    w("mod tests {")
    w("    //! Every constant above is compared with the probe JSON. A mismatch means the committed")
    w("    //! offsets are stale: re-run `tools/layout-probe` and `codegen/gen_layout.py`.")
    w("    use super::*;")
    w("    use serde_json::Value;")
    w("    use std::collections::BTreeSet;")
    w("")
    w("    fn probe() -> Value {")
    w('        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/research/lmu_abi_layout.json");')
    w('        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));')
    w('        serde_json::from_str(&text).unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))')
    w("    }")
    w("")
    w("    fn as_usize(v: &Value, what: &str) -> usize {")
    w('        let n = v.as_u64().unwrap_or_else(|| panic!("{what}: not an unsigned integer in the probe JSON: {v}"));')
    w('        usize::try_from(n).unwrap_or_else(|_| panic!("{what}: {n} does not fit usize"))')
    w("    }")
    w("")
    w("    /// One struct module: its probe name, SIZE, ALIGN and every field constant.")
    w("    struct Entry {")
    w("        name: &'static str,")
    w("        size: usize,")
    w("        align: usize,")
    w("        fields: Vec<(&'static str, Field)>,")
    w("    }")
    w("")
    w("    fn table() -> Vec<Entry> {")
    w("        vec![")
    for sname, mod, _ in STRUCT_MODULES:
        s = j["structs"][sname]
        w(f'            Entry {{ name: "{sname}", size: {mod}::SIZE, align: {mod}::ALIGN, fields: vec![')
        for f in s["fields"]:
            w(f'                ("{f["name"]}", {mod}::{const_name(f["name"])}),')
        w("            ] },")
    w("        ]")
    w("    }")
    w("")
    w("    #[test]")
    w("    fn every_struct_constant_matches_the_probe() {")
    w("        let j = probe();")
    w('        assert_eq!(j["checks"]["all_ok"], Value::Bool(true), "probe self-checks did not all pass");')
    w("        let mut failures = Vec::new();")
    w("        for Entry { name: sname, size, align, fields } in table() {")
    w('            let s = &j["structs"][sname];')
    w('            assert!(s.is_object(), "struct {sname} missing from the probe JSON");')
    w('            let probe_size = as_usize(&s["sizeof"], sname);')
    w('            let probe_align = as_usize(&s["alignof"], sname);')
    w("            if probe_size != size {")
    w('                failures.push(format!("{sname}: SIZE constant {size} != probe sizeof {probe_size}"));')
    w("            }")
    w("            if probe_align != align {")
    w('                failures.push(format!("{sname}: ALIGN constant {align} != probe alignof {probe_align}"));')
    w("            }")
    w('            let probe_fields = s["fields"].as_array().unwrap_or_else(|| panic!("{sname}: no fields array"));')
    w("            let ours: BTreeSet<&str> = fields.iter().map(|(n, _)| *n).collect();")
    w("            for pf in probe_fields {")
    w('                let name = pf["name"].as_str().unwrap_or_else(|| panic!("{sname}: field without a name"));')
    w("                if !ours.contains(name) {")
    w('                    failures.push(format!("{sname}.{name}: present in the probe JSON but has no constant (new field?)"));')
    w("                }")
    w("            }")
    w("            for (name, field) in fields {")
    w("                let Some(pf) = probe_fields.iter().find(|pf| pf[\"name\"] == *name) else {")
    w('                    failures.push(format!("{sname}.{name}: constant exists but the field is gone from the probe JSON"));')
    w("                    continue;")
    w("                };")
    w('                let off = as_usize(&pf["offset"], name);')
    w('                let size = as_usize(&pf["size"], name);')
    w("                if off != field.offset || size != field.size {")
    w("                    failures.push(format!(")
    w('                        "{sname}.{name}: constant (offset {}, size {}) != probe (offset {off}, size {size})",')
    w("                        field.offset, field.size")
    w("                    ));")
    w("                }")
    w("            }")
    w("        }")
    w('        assert!(failures.is_empty(), "LAYOUT MISMATCH vs docs/research/lmu_abi_layout.json:\\n{}", failures.join("\\n"));')
    w("    }")
    w("")
    # absolute paths
    w("    #[test]")
    w("    fn absolute_offsets_match_the_probe() {")
    w("        let j = probe();")
    w("        let ours: Vec<(&str, usize, usize)> = vec![")
    absmap = {
        "data": ("0", "layout::SIZE"),
        "data.generic": ("abs::GENERIC", "generic::SIZE"),
        "data.generic.events": ("abs::GENERIC + generic::EVENTS.offset", "generic::EVENTS.size"),
        "data.generic.gameVersion": ("abs::GENERIC + generic::GAME_VERSION.offset", "generic::GAME_VERSION.size"),
        "data.generic.FFBTorque": ("abs::GENERIC + generic::FFB_TORQUE.offset", "generic::FFB_TORQUE.size"),
        "data.generic.appInfo": ("abs::GENERIC + generic::APP_INFO.offset", "app_state::SIZE"),
        "data.paths": ("abs::PATHS", "paths::SIZE"),
        "data.paths.userData": ("abs::PATHS + paths::USER_DATA.offset", "paths::USER_DATA.size"),
        "data.paths.customVariables": ("abs::PATHS + paths::CUSTOM_VARIABLES.offset", "paths::CUSTOM_VARIABLES.size"),
        "data.paths.stewardResults": ("abs::PATHS + paths::STEWARD_RESULTS.offset", "paths::STEWARD_RESULTS.size"),
        "data.paths.playerProfile": ("abs::PATHS + paths::PLAYER_PROFILE.offset", "paths::PLAYER_PROFILE.size"),
        "data.paths.pluginsFolder": ("abs::PATHS + paths::PLUGINS_FOLDER.offset", "paths::PLUGINS_FOLDER.size"),
        "data.scoring": ("abs::SCORING", "scoring_data::SIZE"),
        "data.scoring.scoringInfo": ("abs::SCORING_INFO", "scoring_info::SIZE"),
        "data.scoring.scoringInfo.mNumVehicles": ("abs::SCORING_INFO + scoring_info::NUM_VEHICLES.offset", "scoring_info::NUM_VEHICLES.size"),
        "data.scoring.scoringInfo.mVehicle": ("abs::SCORING_INFO + scoring_info::VEHICLE.offset", "scoring_info::VEHICLE.size"),
        "data.scoring.scoringStreamSize": ("abs::SCORING_STREAM_SIZE", "scoring_data::SCORING_STREAM_SIZE.size"),
        "data.scoring.vehScoringInfo": ("abs::VEH_SCORING_INFO", "scoring_data::VEH_SCORING_INFO.size"),
        "data.scoring.vehScoringInfo[1]": ("abs_index(abs::veh_scoring_info(1))", "veh_scoring::SIZE"),
        "data.scoring.vehScoringInfo[103]": ("abs_index(abs::veh_scoring_info(103))", "veh_scoring::SIZE"),
        "data.scoring.scoringStream": ("abs::SCORING_STREAM", "SCORING_STREAM_CAPACITY"),
        "data.telemetry": ("abs::TELEMETRY", "telemetry_data::SIZE"),
        "data.telemetry.activeVehicles": ("abs::ACTIVE_VEHICLES", "telemetry_data::ACTIVE_VEHICLES.size"),
        "data.telemetry.playerVehicleIdx": ("abs::PLAYER_VEHICLE_IDX", "telemetry_data::PLAYER_VEHICLE_IDX.size"),
        "data.telemetry.playerHasVehicle": ("abs::PLAYER_HAS_VEHICLE", "telemetry_data::PLAYER_HAS_VEHICLE.size"),
        "data.telemetry.telemInfo": ("abs::TELEM_INFO", "telemetry_data::TELEM_INFO.size"),
        "data.telemetry.telemInfo[0].mDeltaTime": ("abs::TELEM_INFO + telem_info::DELTA_TIME.offset", "telem_info::DELTA_TIME.size"),
        "data.telemetry.telemInfo[0].mWheel": ("abs::TELEM_INFO + telem_info::WHEEL.offset", "telem_info::WHEEL.size"),
        "data.telemetry.telemInfo[0].mWheel[1]": ("abs::TELEM_INFO + telem_info::WHEEL.offset + telem_wheel::SIZE", "telem_wheel::SIZE"),
        "data.telemetry.telemInfo[1]": ("abs_index(abs::telem_info(1))", "telem_info::SIZE"),
        "data.telemetry.telemInfo[103]": ("abs_index(abs::telem_info(103))", "telem_info::SIZE"),
    }
    probe_paths = [p["path"] for p in j["absolute_offsets"]["paths"]]
    missing = [p for p in probe_paths if p not in absmap]
    if missing:
        raise SystemExit(f"generator: no mapping for absolute paths {missing}")
    for p in probe_paths:
        off, size = absmap[p]
        w(f'            ("{p}", {off}, {size}),')
    w("        ];")
    w('        let paths = j["absolute_offsets"]["paths"].as_array().unwrap_or_else(|| panic!("no absolute_offsets.paths"));')
    w('        assert_eq!(paths.len(), ours.len(), "the probe lists a different set of absolute paths; regenerate");')
    w("        let mut failures = Vec::new();")
    w("        for (path, off, size) in ours {")
    w('            let Some(p) = paths.iter().find(|p| p["path"] == path) else {')
    w('                failures.push(format!("{path}: not in the probe JSON"));')
    w("                continue;")
    w("            };")
    w('            let (po, ps) = (as_usize(&p["offset"], path), as_usize(&p["size"], path));')
    w("            if (po, ps) != (off, size) {")
    w('                failures.push(format!("{path}: ours (offset {off}, size {size}) != probe (offset {po}, size {ps})"));')
    w("            }")
    w("        }")
    w('        assert!(failures.is_empty(), "ABSOLUTE LAYOUT MISMATCH:\\n{}", failures.join("\\n"));')
    w('        assert_eq!(as_usize(&j["absolute_offsets"]["sizeof_SharedMemoryLayout"], "sizeof"), layout::SIZE);')
    w("    }")
    w("")
    w("    fn abs_index(v: Option<usize>) -> usize {")
    w('        v.unwrap_or_else(|| panic!("slot index out of range"))')
    w("    }")
    w("")
    w("    #[test]")
    w("    fn derived_absolute_offsets_are_the_documented_values() {")
    w("        // Values quoted in docs/LMU_TELEMETRY_RESEARCH.md section 4 and the channel map section 0.5.")
    w("        assert_eq!(abs::SCORING_PADDING, Field::new(2180, 4));")
    w("        assert_eq!(abs::SCORING_STREAM_SIZE, 2184);")
    w("        assert_eq!(abs::VEH_SCORING_INFO, 2192);")
    w("        assert_eq!(abs::SCORING_STREAM, 62_928);")
    w("        assert_eq!(abs::TELEMETRY, 128_464);")
    w("        assert_eq!(abs::TELEM_INFO, 128_468);")
    w("        assert_eq!(abs::MAPPING_END, 324_820);")
    w("        assert_eq!(abs::SCORING_HEADER, Field::new(1632, 560));")
    w("        assert_eq!(abs::TELEMETRY_HEADER, Field::new(128_464, 4));")
    w("        assert_eq!(abs::SCORING_INFO + scoring_info::CURRENT_ET.offset, 1700);")
    w("        assert_eq!(abs::veh_scoring_info(104), None);")
    w("        assert_eq!(abs::telem_info(104), None);")
    w("        assert_eq!(layout::SIZE - abs::MAPPING_END, 4, \"tail padding\");")
    w("    }")
    w("")
    w("    #[test]")
    w("    fn constants_enums_and_names_match_the_probe() {")
    w("        let j = probe();")
    w('        let c = &j["constants"];')
    w('        assert_eq!(as_usize(&c["scoring_vehicle_slots"], "slots"), MAX_SCORING_VEHICLES);')
    w('        assert_eq!(as_usize(&c["telemetry_vehicle_slots"], "slots"), MAX_TELEMETRY_VEHICLES);')
    w('        assert_eq!(as_usize(&c["scoring_stream_capacity"], "capacity"), SCORING_STREAM_CAPACITY);')
    w('        assert_eq!(as_usize(&c["SME_MAX"], "SME_MAX"), EVENT_SLOTS);')
    w('        assert_eq!(as_usize(&c["MAX_PATH"], "MAX_PATH"), PATH_LEN);')
    w('        assert_eq!(telem_info::WHEEL.size, WHEEL_COUNT * telem_wheel::SIZE);')
    w('        let n = &c["names"];')
    w('        assert_eq!(n["LMU_SHARED_MEMORY_FILE"]["value"], names::DATA_MAPPING);')
    w('        assert_eq!(n["LMU_SHARED_MEMORY_HOLD_EVENT"]["value"], names::HOLD_EVENT);')
    w('        assert_eq!(n["LMU_SHARED_MEMORY_DATA_EVENT"]["value"], names::DATA_EVENT);')
    w('        assert_eq!(n["lock_mapping"]["value"], names::LOCK_MAPPING);')
    w('        assert_eq!(n["lock_event"]["value"], names::LOCK_EVENT);')
    w('        let e = &j["enums"];')
    for k, v in j["enums"]["SharedMemoryEvent"]["values"].items():
        w(f'        assert_eq!(e["SharedMemoryEvent"]["values"]["{k}"], u64::from(sme::{k.removeprefix("SME_")}));')
    for k in j["enums"]["IP_VehicleClass"]["values"]:
        w(f'        assert_eq!(e["IP_VehicleClass"]["values"]["{k}"], u64::from(vehicle_class::{const_name(k)}));')
    for k in j["enums"]["IP_VehicleChampionship"]["values"]:
        w(f'        assert_eq!(e["IP_VehicleChampionship"]["values"]["{k}"], u64::from(vehicle_championship::{const_name(k).replace("__", "_")}));')
    w("        // Value counts the hand-written enums in raw.rs are built for (SharedMemoryEvent: 16 codes")
    w("        // plus the SME_MAX sentinel). A new value means raw.rs needs a new variant.")
    w('        for (name, expected) in [("SharedMemoryEvent", 17), ("IP_VehicleClass", 8), ("IP_VehicleChampionship", 7)] {')
    w('            let count = e[name]["values"].as_object().map_or(0, |m| m.len());')
    w('            assert_eq!(count, expected, "{name}: the probe lists {count} values; regenerate");')
    w("        }")
    w("    }")
    w("")
    w("    #[test]")
    w("    fn bool_field_lists_match_the_probe() {")
    w("        let j = probe();")
    w("        let lists: Vec<(&str, &[(&str, Field)])> = vec![")
    for sname, mod, _ in STRUCT_MODULES:
        if bool_fields(j["structs"][sname]):
            w(f'            ("{sname}", {mod}::BOOL_FIELDS),')
    w("        ];")
    w("        let mut with_bools = 0;")
    w('        for (sname, s) in j["structs"].as_object().unwrap_or_else(|| panic!("no structs")) {')
    w('            let fields = s["fields"].as_array().unwrap_or_else(|| panic!("{sname}: no fields array"));')
    w("            let expected: Vec<(&str, usize, usize)> = fields")
    w("                .iter()")
    w('                .filter(|f| f["kind"] == "bool" && f["union_alt"].as_u64().unwrap_or(0) == 0)')
    w('                .map(|f| (f["name"].as_str().unwrap_or_default(), as_usize(&f["offset"], sname), as_usize(&f["size"], sname)))')
    w("                .collect();")
    w("            let Some((_, list)) = lists.iter().find(|(n, _)| n == sname) else {")
    w('                assert!(expected.is_empty(), "{sname} has bool members but no BOOL_FIELDS list (struct not mirrored?)");')
    w("                continue;")
    w("            };")
    w("            with_bools += 1;")
    w("            let ours: Vec<(&str, usize, usize)> = list.iter().map(|(n, f)| (*n, f.offset, f.size)).collect();")
    w('            assert_eq!(ours, expected, "{sname}: BOOL_FIELDS differ from the probe; regenerate");')
    w("        }")
    w('        assert_eq!(with_bools, lists.len(), "a BOOL_FIELDS list names a struct missing from the probe");')
    w("    }")
    w("}")
    return "\n".join(lines)


# ----------------------------------------------------------------------------------------------
# raw_structs.rs

RAW_STRUCTS = [
    # (JSON struct, Rust type, decode fn, doc)
    ("SharedMemoryGeneric", "RawGeneric", "decode_generic",
     "Decoded `SharedMemoryGeneric` (`data.generic`, absolute offset 0). Raw C semantics, no unit conversion."),
    ("ApplicationStateV01", "RawAppState", "decode_app_state",
     "Decoded `ApplicationStateV01` (`data.generic.appInfo`). The window handle is never decoded."),
    ("SharedMemoryPathData", "RawPaths", "decode_paths",
     "Decoded `SharedMemoryPathData` (`data.paths`). **PII**: contains Windows user paths; never transmit."),
    ("ScoringInfoV01", "RawScoringInfo", "decode_scoring_info",
     "Decoded `ScoringInfoV01` (`data.scoring.scoringInfo`). Pointer fields are never decoded."),
    ("VehicleScoringInfoV01", "RawVehicleScoring", "decode_vehicle_scoring",
     "Decoded `VehicleScoringInfoV01`: one row of `data.scoring.vehScoringInfo[]`."),
    ("TelemInfoV01", "RawTelemetry", "decode_telemetry",
     "Decoded `TelemInfoV01`: one slot of `data.telemetry.telemInfo[]` (normally the player's)."),
    ("TelemWheelV01", "RawWheel", "decode_wheel",
     "Decoded `TelemWheelV01`: one wheel; index 0 FL, 1 FR, 2 RL, 3 RR (A-LMU-019)."),
]


def gen_raw(j: dict) -> str:
    out = []
    w = out.append
    w("//! Typed raw LMU structs and their bounds-checked decoders.")
    w("//!")
    w("//! **Generated** by `crates/lmu-reader/codegen/gen_layout.py` from `docs/research/lmu_abi_layout.json`;")
    w("//! do not edit by hand. Every field keeps raw C semantics: no unit conversion, no clamping, NaN and")
    w("//! infinities pass through unchanged (the normalizer validates and converts). Field docs name the C")
    w("//! field; unit hints paraphrase docs/LMU_CHANNEL_MAP.md and docs/ASSUMPTIONS.md.")
    w("")
    w("use serde::Serialize;")
    w("")
    w("use crate::bytes::{DecodeError, FieldReader, RawString, ReservedBytes, Vec3};")
    w("use crate::offsets::{")
    w("    WHEEL_COUNT, app_state, generic, paths, scoring_info, telem_info, telem_wheel, veh_scoring,")
    w("};")
    w("")
    for sname, rname, fn, doc in RAW_STRUCTS:
        s = j["structs"][sname]
        mod = MODULE_OF[sname]
        w(f"/// {doc}")
        w("#[derive(Debug, Clone, PartialEq, Serialize)]")
        w(f"pub struct {rname} {{")
        decode_lines = []
        for f in s["fields"]:
            name = f["name"]
            if sname == "SharedMemoryGeneric" and name == "appInfo":
                w(f"    /// `{sname}.appInfo`.")
                w("    pub app_info: RawAppState,")
                decode_lines.append(
                    "app_info: decode_app_state(r.sub(generic::APP_INFO)?)?,")
                continue
            if sname == "TelemInfoV01" and name == "mWheel":
                w(f"    /// `{sname}.mWheel[4]`: FL, FR, RL, RR (A-LMU-019).")
                w("    pub wheels: [RawWheel; WHEEL_COUNT],")
                decode_lines.append("wheels: decode_wheels(&r)?,")
                continue
            rf = rust_field(sname, f)
            if rf is None:
                continue
            rtype, expr = rf
            hint = HINTS.get((sname, name))
            doc_line = f"`{sname}.{name}`" + (f": {hint}" if hint else ".")
            if rtype == "ReservedBytes":
                doc_line = f"`{sname}.{name}` ({f['size']} reserved bytes, diagnostics only)."
            w(f"    /// {doc_line}")
            w(f"    pub {snake(name)}: {rtype},")
            decode_lines.append(f"{snake(name)}: {expr},")
        w("}")
        w("")
        w(f"/// Decodes one `{sname}` from exactly `{mod}::SIZE` bytes.")
        w("///")
        w("/// # Errors")
        w("/// [`DecodeError`] if `buf` is not exactly the struct size (no field is read past the buffer).")
        w(f"pub fn {fn}(buf: &[u8]) -> Result<{rname}, DecodeError> {{")
        w(f'    let r = FieldReader::new(buf, {mod}::SIZE, "{sname}")?;')
        w(f"    Ok({rname} {{")
        for line in decode_lines:
            w(f"        {line}")
        w("    })")
        w("}")
        w("")
    w("/// Decodes the four `TelemWheelV01` entries of `TelemInfoV01.mWheel`.")
    w("fn decode_wheels(r: &FieldReader<'_>) -> Result<[RawWheel; WHEEL_COUNT], DecodeError> {")
    w("    let wheels = r.sub(telem_info::WHEEL)?;")
    w("    let wheel = |i: usize| -> Result<RawWheel, DecodeError> {")
    w("        let start = i * telem_wheel::SIZE;")
    w('        let bytes = wheels.get(start..start + telem_wheel::SIZE).ok_or(DecodeError::OutOfBounds {')
    w('            context: "TelemInfoV01.mWheel",')
    w("            offset: start,")
    w("            len: telem_wheel::SIZE,")
    w("            available: wheels.len(),")
    w("        })?;")
    w("        decode_wheel(bytes)")
    w("    };")
    w("    Ok([wheel(0)?, wheel(1)?, wheel(2)?, wheel(3)?])")
    w("}")
    return "\n".join(out)


def main() -> None:
    j = json.loads(JSON_PATH.read_text(encoding="utf-8"))
    if not j.get("checks", {}).get("all_ok"):
        raise SystemExit("probe JSON reports failed self-checks; refusing to generate")
    OFFSETS_RS.write_text(gen_offsets(j) + "\n", encoding="utf-8", newline="\n")
    RAW_RS.write_text(gen_raw(j) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {OFFSETS_RS}")
    print(f"wrote {RAW_RS}")


if __name__ == "__main__":
    main()

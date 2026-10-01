//! Read-only acquisition of Le Mans Ultimate's native `LMU_Data` shared memory (Gate 1).
//!
//! * **Strictly read-only toward LMU (D-018).** `LMU_Data` is mapped with `FILE_MAP_READ`; the gate
//!   events are opened with `SYNCHRONIZE`; the SDK reader lock is used exactly as the SDK documents
//!   (existing objects only, never created, never reset) and held only for the `memcpy`.
//! * **Explicit offsets (D-003).** [`offsets`] is generated from the layout probe JSON and unit-tested
//!   against it; decoding uses bounds-checked little-endian reads, never transmutes.
//! * **Never guesses.** [`compat`] checks every frame; structural failures stop decoding with
//!   [`LmuError::IncompatibleInterface`]. Values keep raw C semantics (no unit conversion; NaN
//!   passes through); the normalizer converts and validates.
//!
//! The entry point is [`LmuReader`], a blocking API for a dedicated OS thread. [`capture`] records and
//! replays the copied bytes (`.lmuraw`) for offline tests.

pub mod blocks;
pub mod bytes;
pub mod capture;
pub mod clock;
pub mod compat;
pub mod error;
pub mod offsets;
pub mod processor;
pub mod raw;
pub mod raw_structs;
pub mod reader;
pub mod snapshot;
pub mod state;
pub mod stats;
mod sys;
#[cfg(any(test, feature = "test-support"))]
pub mod testutil;

pub use blocks::{BlockKind, CopyPlan, OwnedBlocks, RawBlocks};
pub use bytes::{DecodeError, RawString, ReservedBytes, Vec3};
pub use capture::{
    CaptureError, CaptureReader, CaptureReplay, CaptureWriter, CapturedFrame, FrameInfo,
};
pub use clock::MonoClock;
pub use compat::{CheckId, CheckReport, CompatibilityReport, Outcome, Section, SupportLevel};
pub use error::LmuError;
pub use processor::{FrameMeta, FrameOutcome, FrameProcessor};
pub use raw::{SharedMemoryEvent, VehicleChampionship, VehicleClass, split_current_sector};
pub use raw_structs::{
    RawAppState, RawGeneric, RawPaths, RawScoringInfo, RawTelemetry, RawVehicleScoring, RawWheel,
};
pub use reader::{LmuReader, ReaderConfig, ReaderDiagnostics};
pub use snapshot::{
    PlayerIdentity, PlayerTelemetry, PlayerUnverified, RawSnapshot, ScoringBlock, TelemetryHeader,
    TelemetrySlots,
};
pub use state::{Consistency, FrameSource, LockMode, ObjectNames, ReaderState, SyncMode};
pub use stats::{DurationStats, RateMeter, ReaderStats};

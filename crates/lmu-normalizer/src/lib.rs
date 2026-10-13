//! LMU raw snapshot → canonical telemetry (the ONLY place LMU fields are mapped to channels and converted
//! to internal units; docs/ARCHITECTURE.md, docs/LMU_CHANNEL_MAP.md).
//!
//! Inputs: `lmu_reader::RawSnapshot` (raw C semantics, no unit conversion, NaN passes through).
//! Outputs: `telemetry_model::TelemetryFrame` (one per new physics sample), `ScoringState`,
//! `SessionIdentity` with `SessionEvent`s, `VehicleCapabilities`, a `CapabilityMap`, and
//! [`DiagnosticNotice`]s for unit hazards.
//!
//! Guarantees:
//! - **No silent corruption.** NaN/±Inf, out-of-range values and implausible jumps become
//!   `Quality::Invalid` (registry validation); missing data stays `Unavailable` / `NotSupported`. A value is
//!   never replaced by 0 or clamped. A value whose unit or scale is not yet decided by evidence (brake
//!   temperature K/°C, VE/SoC 0..1 vs %, `mTimeOfDay`, zero tyre wear) is withheld as `Unavailable`, never
//!   published `Valid` on an assumption.
//! - **One physics sample per LMU update.** A snapshot whose player `mElapsedTime` equals the previous one
//!   is a duplicate and produces no frame.
//! - **Units in one place.** All conversions use `telemetry_model::units::convert`; sign conventions are
//!   named constants in [`convention`]; the tyre side mapping is one function in [`tyre`]; unit hazards
//!   (brake temperature K/°C, brake pressure fraction, VE/SoC scale…) are explicit detectors in [`detect`].
//! - **Known discontinuities are not rejections.** A garage teleport, a realtime entry or a pit service
//!   resets the rate history of the affected channels ([`discontinuity`]); uncorroborated jumps are still
//!   rejected.
//! - **Deterministic, no global state.** [`Normalizer`] holds all state; the same inputs give the same
//!   outputs.

pub mod capability;
pub mod convention;
pub mod detect;
pub mod discontinuity;
pub mod identity;
pub mod mapping;
pub mod normalizer;
pub mod notice;
pub mod standings;
pub mod tyre;
pub mod writer;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;

pub use capability::{CAPABILITY_DECISION_SAMPLES, VehicleCapabilityTracker};
pub use detect::{
    BrakeTempUnit, BrakeTempUnitDecision, BrakeTempUnitDetector, FillState, FractionScale,
    FractionScaleDetector, StuckValueDetector, UnitDecisionState, VERIFIED_BRAKE_TEMP_UNITS,
    VERIFIED_VIRTUAL_ENERGY_SCALES, verified_brake_temp_unit, verified_virtual_energy_scale,
};
pub use discontinuity::{CarSample, Discontinuity, DiscontinuityDetector};
pub use identity::{IdentityChange, IdentityTracker};
pub use normalizer::{
    NormalizedOutput, Normalizer, NormalizerInput, NormalizerStats, SUSTAINED_REJECTION_FRAMES,
    TelemetryDisposition,
};
pub use notice::{DiagnosticNotice, NoticeCode};
pub use tyre::{TyreEdges, edge_indices, lcr_to_edges};

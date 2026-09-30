//! Channel definitions: the metadata every channel must carry (spec §12, §350).

use serde::{Deserialize, Serialize};

use crate::units::Unit;
use crate::value::ValueType;

/// Stable numeric channel identifier used on the wire and in recordings.
/// IDs are append-only and never reused (docs/CHANNEL_IDS.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub u16);

/// Where a channel's value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    /// LMU `LMU_Data` shared memory, telemetry block (player vehicle).
    LmuSharedMemoryTelemetry,
    /// LMU `LMU_Data` shared memory, scoring block.
    LmuSharedMemoryScoring,
    /// LMU `LMU_Data` shared memory, generic/application block.
    LmuSharedMemoryGeneric,
    /// LMU REST API (supplementary, low rate, feature-flagged).
    LmuRest,
    /// Computed by `race-engines` from measured channels.
    Calculated,
    /// Produced by this application itself (network/recording diagnostics).
    Application,
}

/// Measured vs calculated classification shown to the user (spec §107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Provenance {
    /// Directly from LMU (after unit conversion only).
    Measured,
    /// Deterministically calculated from measured data.
    Derived,
    /// Calculated with statistical assumptions (e.g. fuel/lap window).
    Estimated,
}

/// Network priority class (spec §171).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    P0 = 0,
    P1 = 1,
    P2 = 2,
}

/// Nominal update class. Actual source rates are measured at runtime, never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrequencyClass {
    /// Updated on every LMU telemetry frame.
    Physics,
    /// Updated on every LMU scoring update.
    Scoring,
    /// Slow environmental data (weather, temperatures that change slowly).
    Environment,
    /// Event-driven (lap complete, pit event…).
    Event,
    /// Static per session (vehicle name, track…).
    Static,
}

/// Whether a value is continuous (may be interpolated for display) or discrete (never interpolated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interpolation {
    Continuous,
    Discrete,
}

/// Static definition of a channel. The registry is the single source of truth driving the protocol,
/// graph selectors, diagnostics, export column names and UI labels.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelDef {
    pub id: ChannelId,
    /// Canonical dotted name, e.g. `vehicle.speed`.
    pub name: &'static str,
    /// Export/MoTeC-style name, e.g. `Speed`, `TyrePressure_FL`.
    pub export_name: &'static str,
    /// Short human label for UI.
    pub label: &'static str,
    /// Internal (SI) unit of the stored value.
    pub unit: Unit,
    pub value_type: ValueType,
    pub source: Source,
    pub provenance: Provenance,
    pub priority: Priority,
    pub frequency: FrequencyClass,
    pub interpolation: Interpolation,
    /// Exact origin, e.g. `telemetry.telemInfo[player].mWheel[0].mPressure` or a formula id.
    pub origin: &'static str,
    /// Plausibility range in internal units (inclusive). Values outside are marked INVALID.
    pub valid_range: Option<(f64, f64)>,
    /// Maximum plausible change per second in internal units; larger jumps are marked INVALID.
    pub max_rate_per_s: Option<f64>,
}

//! Channel definitions: the metadata every channel must carry (spec §12, §350).

use serde::{Deserialize, Serialize};

use crate::units::Unit;
use crate::value::{Quality, Sample, ValueType};

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
    /// LMU's own DuckDB telemetry recordings, opened `read_only` for offline import (D-018). Only used by
    /// channels that no live source provides.
    LmuDuckDb,
}

impl Source {
    /// True for the three `LMU_Data` shared-memory blocks.
    pub const fn is_lmu_shared_memory(self) -> bool {
        matches!(
            self,
            Source::LmuSharedMemoryTelemetry
                | Source::LmuSharedMemoryScoring
                | Source::LmuSharedMemoryGeneric
        )
    }
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

/// Network priority class (spec §171, docs/PROTOCOL.md): P0 vehicle-critical state, timing, fuel, position,
/// race status; P1 tyres, brakes, energy; P2 detail.
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
    /// Periodic application statistics (rates, network and recording health), about once per second.
    Diagnostic,
}

/// Whether a value is continuous (may be interpolated for display) or discrete (never interpolated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interpolation {
    Continuous,
    Discrete,
}

/// Static availability of a channel, independent of the car currently driven. The per-car, per-session
/// answer lives in `scoring::CapabilityMap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Availability {
    /// Present and meaningful for every car (LMU_CHANNEL_MAP "AVAILABLE").
    Always,
    /// Meaningful only for some classes or cars ("VEHICLE-DEPENDENT"); decided at runtime.
    VehicleDependent,
    /// No live source provides it today ("UNAVAILABLE", LMU_CHANNEL_MAP §19). The channel exists so the UI
    /// and capability map can say "not provided" instead of showing 0. Frames carry it as `NotSupported`.
    NotProvided,
    /// Planned derived channel that must not be published until its inputs/semantics are verified live
    /// (e.g. slip ratio, local yellow flags).
    Experimental,
}

/// Static definition of a channel. The registry is the single source of truth driving the protocol,
/// graph selectors, diagnostics, export column names and UI labels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ChannelDef {
    pub id: ChannelId,
    /// Canonical dotted name, e.g. `vehicle.speed_mps`.
    pub name: &'static str,
    /// Export/MoTeC-style name, e.g. `Speed`, `TyrePressure_FL`.
    pub export_name: &'static str,
    /// Short human label for UI.
    pub label: &'static str,
    /// Internal (SI) unit of the stored value.
    pub unit: Unit,
    /// Wire/storage value type. Every `Valid` sample of this channel carries exactly this type.
    pub value_type: ValueType,
    pub source: Source,
    pub provenance: Provenance,
    pub priority: Priority,
    pub frequency: FrequencyClass,
    pub interpolation: Interpolation,
    /// Static availability (see [`Availability`]).
    pub availability: Availability,
    /// Exact origin: an LMU path such as `telemetry.telemInfo[player].mWheel[0].mPressure`, a formula id
    /// (`formula:<id>`), an application metric (`app:<id>`) or `unavailable:<where it might exist>`.
    pub origin: &'static str,
    /// Plausibility range in internal units (inclusive). Values outside are marked INVALID.
    pub valid_range: Option<(f64, f64)>,
    /// Maximum plausible change per second in internal units; larger jumps are marked INVALID.
    pub max_rate_per_s: Option<f64>,
}

impl ChannelDef {
    /// The "no data" sample a fresh frame carries for this channel: `NotSupported` for channels no source
    /// provides, otherwise `Unavailable`. Never a zero.
    pub fn default_sample(&self) -> Sample {
        Sample {
            value: crate::value::Value::None,
            quality: self.default_quality(),
        }
    }

    /// Quality of [`ChannelDef::default_sample`].
    pub const fn default_quality(&self) -> Quality {
        match self.availability {
            Availability::NotProvided => Quality::NotSupported,
            Availability::Always | Availability::VehicleDependent | Availability::Experimental => {
                Quality::Unavailable
            }
        }
    }

    /// True if the stored value is numeric (graphable). Strings and `None` are not.
    pub const fn is_numeric(&self) -> bool {
        !matches!(self.value_type, ValueType::Str | ValueType::None)
    }

    /// True if the stored value is a floating-point number.
    pub const fn is_float(&self) -> bool {
        matches!(self.value_type, ValueType::F32 | ValueType::F64)
    }
}

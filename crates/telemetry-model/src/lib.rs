//! Canonical telemetry model for LMU Race Engineering.
//!
//! One schema for live, recorded and replayed data (spec §11, §272, addendum §11):
//! - [`registry`]: every channel (id, names, unit, type, source, provenance, ranges) — single source of
//!   truth (D-017), `registry::ch::*` id constants;
//! - [`frame`]: the dense, immutable-once-published [`frame::TelemetryFrame`] and its TLV conversion;
//! - [`validate`]: per-sample plausibility checks driven by the registry;
//! - [`ring`]: fixed-capacity time-series storage for graphs;
//! - [`scoring`]: typed session/standings state, session identity and capability maps;
//! - [`events`]: session events (laps, pits, flags, network, recording, alerts);
//! - [`enums`]: decoded LMU enumerations that never coerce unknown raw values;
//! - [`uibatch`]: the `UiBatch` v1 envelope that carries TLV blocks to the desktop UI (D-023).

pub mod channel;
pub mod codec;
pub mod enums;
pub mod events;
pub mod frame;
pub mod registry;
pub mod ring;
pub mod scoring;
pub mod uibatch;
pub mod units;
pub mod validate;
pub mod value;

pub use channel::{
    Availability, ChannelDef, ChannelId, FrequencyClass, Interpolation, Priority, Provenance,
    Source,
};
pub use frame::{SharedFrame, TelemetryFrame};
pub use registry::ch;
pub use units::Unit;
pub use value::{Quality, Sample, Value, ValueType};

/// Version of the canonical telemetry schema (channel set + semantics). Independent of the wire
/// protocol version. Bump when channels are added or their meaning/unit changes.
pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;

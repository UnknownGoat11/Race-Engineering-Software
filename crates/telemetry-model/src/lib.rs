//! Canonical telemetry model for LMU Race Engineering.
//!
//! One schema for live, recorded and replayed data (spec Â§11, Â§272, addendum Â§11).

pub mod channel;
pub mod codec;
pub mod units;
pub mod value;

pub use channel::{ChannelDef, ChannelId, FrequencyClass, Interpolation, Priority, Provenance, Source};
pub use units::Unit;
pub use value::{Quality, Sample, Value, ValueType};

/// Version of the canonical telemetry schema (channel set + semantics). Independent of the wire
/// protocol version. Bump when channels are added or their meaning/unit changes.
pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;

//! **DEV / TEST / DEMO ONLY.** Synthetic telemetry and network faults for LMU Race Engineering.
//!
//! Nothing here may be used by production acquisition (docs/ARCHITECTURE.md §2): the provider can only be
//! constructed with an explicit [`DemoMode`] or [`TestMode`] token, and everything it produces is labelled
//! synthetic ([`DataOrigin::Synthetic`], identity strings prefixed with [`SYNTHETIC_NAME_PREFIX`]).
//!
//! - [`provider`]: [`SyntheticTelemetryProvider`], the seeded deterministic frame + scoring generator;
//! - [`scenario`]: [`Scenario`] builder and presets (normal, pit, invalid lap, rain, hybrid, non-hybrid,
//!   stale, malformed, safety car, demo);
//! - [`track`]: a closed ~4.7 km circuit (corners, sectors, pit lane) built from straights and arcs;
//! - [`vehicle`] / [`profile`]: hybrid Hypercar and LMGT3 profiles and their racing-line speed profiles;
//! - [`car`], [`dynamics`], [`weather`], [`session`], [`writer`]: the simulation and frame/scoring output;
//! - [`inject`]: malformed-value injection and a raw-NaN wire helper;
//! - [`fault`]: a pure, seeded network [`FaultModel`] for unit tests;
//! - [`proxy`]: a frame-aware TCP fault proxy that only ever drops or delays whole protocol frames;
//! - [`mode`]: construction tokens and the synthetic labels.

pub mod car;
pub mod dynamics;
pub mod error;
pub mod fault;
pub mod inject;
pub mod mode;
pub mod profile;
pub mod provider;
pub mod proxy;
pub mod scenario;
pub mod session;
pub mod track;
pub mod vehicle;
pub mod weather;
pub mod writer;

pub use error::SyntheticError;
pub use fault::{
    BlackoutPolicy, DropReason, FaultConfig, FaultModel, FaultStats, Jitter, LossPreset, Verdict,
    Window,
};
pub use inject::{Injection, encode_snapshot_with_injections};
pub use mode::{
    DataOrigin, DemoMode, SYNTHETIC_LABEL, SYNTHETIC_NAME_PREFIX, SyntheticMode, SyntheticPurpose,
    TestMode, is_synthetic_name,
};
pub use provider::{RealtimeStream, SyntheticEvent, SyntheticTelemetryProvider, SyntheticTick};
pub use proxy::{FaultProxy, FaultProxyConfig, ProxyError, ProxyMode, ProxyStats};
pub use scenario::{
    CautionKind, CautionSpec, MalformedKind, MalformedSpec, PitStopSpec, RainSpec, Scenario,
    ScenarioBuilder, StaleMode, StaleSpec,
};
pub use session::ScoringSnapshot;
pub use track::{Track, TrackSpec};
pub use vehicle::{ChannelFill, VehicleKind, VehicleProfile};

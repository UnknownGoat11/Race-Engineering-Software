//! Deterministic race-engineering calculation engines (spec §143, D-010).
//!
//! Every engine:
//! - consumes canonical telemetry (`telemetry_model::TelemetryFrame` / `ScoringState`, or small typed
//!   samples built from them);
//! - is pure and deterministic: no I/O, no clocks, no randomness — time always comes from the input (LMU
//!   elapsed time). The only exception is [`track::TrackMapStore`], the explicit, directory-scoped cache of
//!   track-map files;
//! - outputs typed metrics that carry their provenance (`Derived` / `Estimated`, or `Measured` for
//!   pass-through scoring values) and [`CALCULATION_ENGINE_VERSION`] (see [`common::Metric`]);
//! - never turns a missing input into a zero: an unknown result is `None`.
//!
//! Modules: [`timing`] (laps, sectors, references, distance-aligned delta), [`race`] (positions, gaps,
//! clock, pit state machine, flags), [`track`] (track map generation, projection, cache), [`fuel`]
//! (rolling consumption, confidence, reserve, fuel to finish), [`stint`] (stint tracking and table) and
//! [`publish`] (writes engine outputs into the registry's calculated channels), [`tyre`] (pressures,
//! temperatures, deltas, balances, rise rate), [`brake`] (temperatures, trend, distribution, bias),
//! [`energy`] (virtual energy, SoC, deployment / harvest / regen per lap), [`aids`] (TC / ABS duty cycle per lap) and [`alert`] (rule-based alerts
//! with hysteresis, minimum duration and cooldown).

pub mod aids;
pub mod alert;
pub mod brake;
pub mod common;
pub mod energy;
pub mod fuel;
pub mod publish;
pub mod race;
pub mod stint;
pub mod timing;
pub mod track;
pub mod tyre;

pub use common::{CompatibilityKey, Metric};

/// Version of the race-engines formulas. Every derived output carries it (D-010) and it is published as
/// `app.calculation_engine_version`. Bump whenever a formula, threshold or exclusion rule changes the
/// numbers an engine produces.
pub const CALCULATION_ENGINE_VERSION: u32 = 1;

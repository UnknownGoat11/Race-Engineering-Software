//! `EngineHub`: the pure, deterministic per-frame engine sequence.
//!
//! Order (blueprint section 6): timing, race, the lap-completion hooks (fuel, energy, stint), the per-frame
//! updates (fuel, stint, tyre, brake, energy), the engine snapshots, then `publish::publish` and
//! `energy::publish_energy` into the frame. A non-empty `PublishReport.errors` is a bug and is counted.
//! Unavailable inputs never yield valid zeros.
//!
//! Stub: implemented in Phase 1 lane C.

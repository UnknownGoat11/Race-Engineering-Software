//! Demo mode (feature `demo`): runs the synthetic provider into the same pipeline.
//!
//! Built with `SyntheticTelemetryProvider::new(DemoMode::acknowledge(), scenario)` and fed through its
//! realtime stream. The run's origin is `Synthetic` and it has no recorder and no network sink, so demo data
//! is display-only on the local PC (owner decision Q1).
//!
//! Stub: implemented in increment I-4.

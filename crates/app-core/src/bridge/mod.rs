//! UI data bridge: delivers telemetry to the WebView as `UiBatch` envelopes (D-023).
//!
//! One [`hub`] serves both modes. The [`pacer`] flushes it at 60 Hz into a [`sink`] that the Tauri adapter
//! implements. [`status`] holds the JSON event DTOs.

pub mod hub;
pub mod pacer;
pub mod sink;
pub mod status;

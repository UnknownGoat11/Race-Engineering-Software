//! UI data bridge: delivers telemetry to the WebView as `UiBatch` envelopes (D-023).
//!
//! One [`hub`] serves both modes. The [`pacer`] flushes it at 60 Hz into a [`sink`] that the Tauri adapter
//! implements. [`status`] holds the JSON event DTOs.

pub mod hub;
pub mod pacer;
pub mod sink;
pub mod status;

pub use hub::{
    MAX_HISTORY_BYTES, MAX_HISTORY_DURATION_NS, MAX_SUB_QUEUE_BLOCKS, MAX_SUB_QUEUE_BYTES, UiHub,
};
pub use pacer::{DEFAULT_FLUSH_HZ, DEFAULT_STATUS_HZ, PacerConfig, UiBridgePacer};
pub use sink::{SinkClosed, SubscriptionId, UiSink};
pub use status::{
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, EngineerDto, ErrorDto, LmuSourceStatus,
    NetworkStatusDto, Origin, RatesDto, RecordingStatusDto, SessionStatusDto, UiEvent,
};

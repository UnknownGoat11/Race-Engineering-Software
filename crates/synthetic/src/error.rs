//! Errors of the synthetic generator.

use telemetry_model::codec::CodecError;
use telemetry_model::frame::FrameError;
use thiserror::Error;

/// Errors raised while configuring or running the synthetic generator.
#[derive(Debug, Error)]
pub enum SyntheticError {
    /// A scenario, track or vehicle parameter is out of its supported range.
    #[error("invalid synthetic configuration `{field}`: {reason}")]
    InvalidConfig { field: &'static str, reason: String },
    /// Writing a sample into a frame failed: a generator bug (wrong type or non-finite value), never
    /// silently ignored.
    #[error("synthetic frame rejected a sample")]
    Frame(#[from] FrameError),
    /// TLV encoding failed.
    #[error("TLV encoding failed")]
    Codec(#[from] CodecError),
    /// A real-time stream was requested outside a tokio runtime.
    #[error("no tokio runtime is running")]
    NoRuntime(#[source] tokio::runtime::TryCurrentError),
    /// The real-time provider task panicked (a generator bug).
    #[error("the real-time synthetic provider task failed")]
    RealtimeTask(#[source] tokio::task::JoinError),
}

impl SyntheticError {
    /// Shorthand for [`SyntheticError::InvalidConfig`].
    pub(crate) fn config(field: &'static str, reason: impl Into<String>) -> Self {
        SyntheticError::InvalidConfig {
            field,
            reason: reason.into(),
        }
    }
}

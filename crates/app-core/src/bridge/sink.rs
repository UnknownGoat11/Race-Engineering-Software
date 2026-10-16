//! `UiSink`: where batches and JSON events leave app-core.
//!
//! Defined trait:
//! `UiSink: Send + Sync + 'static {
//!     fn send_batch(&self, sub: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed>;
//!     fn emit(&self, event: UiEvent);
//!     fn is_visible(&self) -> bool { true }
//! }`
//!
//! A send error drops the subscription and the UI resubscribes.

use std::fmt;
use thiserror::Error;

use crate::bridge::status::UiEvent;

/// Uniquely identifies a UI subscription attached to the [`crate::bridge::hub::UiHub`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubscriptionId(pub u64);

impl fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sub-{}", self.0)
    }
}

/// The sink has closed or disconnected (e.g. Tauri webview channel closed or IPC error).
///
/// Receiving this error causes the `UiHub` to drop the subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("UI sink channel closed")]
pub struct SinkClosed;

/// Interface through which `UiHub` and `AppCore` deliver batches and JSON events to the UI.
///
/// Headless tests implement an in-memory sink; Tauri implements it over an IPC Channel.
pub trait UiSink: Send + Sync + 'static {
    /// Delivers an encoded `UiBatch` envelope to the frontend for a given subscription.
    fn send_batch(&self, sub: SubscriptionId, batch: Vec<u8>) -> Result<(), SinkClosed>;

    /// Emits a structured JSON event to the frontend (e.g. `core:status`).
    fn emit(&self, event: UiEvent);

    /// Whether the view consuming this sink is currently visible.
    ///
    /// When `false`, the hub pauses batch delivery for the subscription. When visibility
    /// resumes, the subscription is re-primed with a `RESET | BACKFILL` batch.
    fn is_visible(&self) -> bool {
        true
    }
}

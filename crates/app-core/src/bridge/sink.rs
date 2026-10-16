//! `UiSink`: where batches and JSON events leave app-core.
//!
//! Planned trait: `UiSink: Send + Sync { send_batch(sub, Vec<u8>) -> Result<(), SinkClosed>;
//! emit(UiEvent); is_visible() -> bool }`. The Tauri adapter implements it over `tauri::ipc::Channel`;
//! tests implement it in memory. A send error drops the subscription and the UI resubscribes.
//!
//! Stub: implemented in Phase 1 lane A.

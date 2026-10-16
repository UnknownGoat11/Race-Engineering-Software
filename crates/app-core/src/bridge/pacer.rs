//! The 60 Hz flush loop (`"ui-bridge"` thread).
//!
//! Ticks on the [`crate::hires`] high-resolution timer and flushes every subscription's pending blocks as one
//! `UiBatch`. The 4 Hz `core:status` emitter runs on the same thread.
//!
//! Stub: implemented in Phase 1 lane A.

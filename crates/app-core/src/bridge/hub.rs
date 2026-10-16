//! `UiHub`: telemetry history and UI subscriptions.
//!
//! Holds encoded TLV blocks as `Arc<[u8]>` with a keyframe flag (a full snapshot at session start, every 1 s
//! and after a discontinuity), trimmed only at keyframe boundaries (60 s, hard cap 48 MiB). A subscribe takes
//! a consistent cut under the hub lock and sends `RESET|BACKFILL` batches (oldest keyframe first, each at
//! most 4 MiB) before any live batch. Each subscription has a bounded queue (1024 blocks, 8 MiB); overflow
//! drops the oldest block and flags `GAP_BEFORE`. An invisible sink pauses its subscription, which is
//! re-primed when visible again. Envelopes come from `telemetry_model::uibatch`.
//!
//! Stub: implemented in Phase 1 lane A.

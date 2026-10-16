//! `NetPublisher`: streams the driver's frames to the relay.
//!
//! Sends `SchemaNegotiate` first in each sequence space, then per-priority (P0/P1/P2) deltas through
//! `DriverClient::send_live`, a 1 Hz and on-request snapshot, scoring at 5 Hz, weather on change at most
//! 1 Hz, and `DriverStatus`/`RecordingStatus` at 1 Hz. When a latest-wins slot reports `replaced`, the lost
//! delta is repaired by re-pushing the union of both channel sets (blueprint decision 4). Reliable messages
//! go through `try_send_reliable` with a bounded pending list. All priority classes are sent at the native
//! 100 Hz (owner decision Q2). Never receives synthetic data.
//!
//! Stub: implemented in Phase 1 lane D.

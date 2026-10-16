//! Stream age and `net.*` channels on the engineer side.
//!
//! Computes the stream age locally from `relay_time_of` and writes the `net.*` channels. Drives the
//! engineer-side `AlertEngine` at 4 Hz even when no frame arrives, so a lost driver shows STALE and then
//! NO DATA instead of frozen values.
//!
//! Stub: implemented in Phase 1 lane G.

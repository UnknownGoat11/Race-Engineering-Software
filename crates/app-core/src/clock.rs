//! `AppClock`: the single monotonic time base of a run.
//!
//! Wraps `lmu_reader::MonoClock`. Its epoch is passed to transport's `ClientConfig::new(.., mono_epoch)` and
//! to the recorder's `SessionOptions::with_capture_clock`, so capture timestamps from the reader, the network
//! headers and the recording all share one clock. Also hosts the wall-versus-monotonic skew helper used by
//! the sleep/resume check (a skew above 5 s is a discontinuity).
//!
//! Stub: implemented in Phase 1 lane B.

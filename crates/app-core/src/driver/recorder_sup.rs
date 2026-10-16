//! Recorder supervisor: one `RecordingSession` per LMU session identity.
//!
//! Opens the `RecordingStore`, starts sessions with a disk probe, and appends the canonical TLV block of
//! every frame plus scoring (5 Hz), session events, capability maps and markers. Records laps, stints and
//! events from engine outputs. Drains `RecorderEvent`s at 1 Hz and maps `RecorderStats` to the `rec.*`
//! channels and `RecordingStatus`. A recorder failure never stops live processing. Never receives synthetic
//! data.
//!
//! Stub: implemented in Phase 1 lane E.

//! The LMU reader thread (`"lmu-reader"`, ABOVE_NORMAL priority).
//!
//! Loops `LmuReader::next_snapshot(100ms)` and hands each `RawSnapshot` to the pipeline with `try_send` on a
//! bounded queue of 256 snapshots. A full queue drops the newest snapshot, counts `reader_overrun` and marks
//! a discontinuity. Publishes `ReaderDiagnostics` at 4 Hz through an `ArcSwap`. Never touches the network,
//! the recorder or the UI. Stop is an `AtomicBool` honoured within 100 ms. LMU is only ever read
//! (`FILE_MAP_READ` in `lmu-reader`).
//!
//! Stub: implemented in increment I-3.

//! `EngineerCore`: wraps transport's `EngineerClient`.
//!
//! Consumes `EngineerEvent`s without ever blocking (byte copy and queue push only). Forwards TLV blocks
//! unchanged after a schema guard, and coalesces the per-priority blocks of one capture (equal
//! `capture_mono_ns`, flushed on change or after 8 ms) into one block for the [`crate::bridge::hub`]. Maps
//! scoring, status and session events to JSON. Pre-gap history is kept across a blip, with a visible gap
//! (owner decision Q4).
//!
//! Stub: implemented in Phase 1 lane G.

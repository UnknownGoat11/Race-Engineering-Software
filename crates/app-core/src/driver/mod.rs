//! Driver side: acquisition, processing, recording and streaming on the driver PC.
//!
//! Data path (blueprint section 2): reader thread, then pipeline thread (normalize, engines, app channels,
//! alerts), then a non-blocking fan-out to the recorder, the network publisher and the UI hub. Every queue
//! between threads is bounded, and a network failure shares no lock or queue with acquisition or
//! recording.

pub mod engines;
pub mod net;
pub mod pipeline;
pub mod reader_thread;
pub mod recorder_sup;
pub mod session;

#[cfg(feature = "demo")]
pub mod demo;

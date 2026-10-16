//! The pipeline thread (`"pipeline"`, NORMAL priority).
//!
//! Owns the `Normalizer`. Input: `PipelineInput::{Lmu(Box<RawSnapshot>), Demo(Box<SyntheticTick>),
//! Command(PipelineCommand), Tick}`. Per frame it runs normalization, the [`super::engines`] `EngineHub`,
//! the app channels (`SRC_*`, `PIPELINE_LATENCY`, `net.*`, `rec.*`) and the `AlertEngine`, then encodes one
//! canonical TLV block shared as `Arc<[u8]>` and fans it out without blocking. Event-driven: no timer sits
//! in the 100 Hz path. A `recv_timeout(100ms)` expiry is the 10 Hz housekeeping tick (source-age ticker,
//! alert clock, recorder event drain, sleep/resume check).
//!
//! Stub: implemented in increment I-3.

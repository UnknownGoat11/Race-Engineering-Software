//! `lmu-verify`: the offline Gate-1 evidence tool.
//!
//! * [`report`] replays `.lmuraw` captures (written by `lmu-diag capture`) through `lmu-reader`'s
//!   [`lmu_reader::CaptureReplay`] and `lmu-normalizer` exactly as the live app does, and turns them into a
//!   reproducible verification report (`report.md` + `report.json`): session facts, rates and timing, a
//!   per-channel table and explicit Gate-1 checks with PASS / FAIL / INCONCLUSIVE verdicts.
//! * [`rest`] is the opt-in, GET-only probe of LMU's supplementary REST endpoints on `127.0.0.1:6397`.
//!
//! **Read-only toward LMU (D-018).** The report never touches LMU: it reads capture files. The REST probe
//! only sends one-shot HTTP `GET` requests to the loopback address when explicitly invoked.
//!
//! **Privacy.** Captures contain driver, player and server names. The report never prints them: every
//! string taken from a capture passes through [`redact::Redactor`].
//!
//! **Bounded memory.** Every statistic is streaming ([`stats`]); a capture of any length is analysed in
//! constant space.

pub mod analysis;
pub mod error;
pub mod redact;
pub mod report;
pub mod rest;
pub mod stats;

pub use error::VerifyError;

/// `component` field of every tracing event of this tool.
pub const COMPONENT: &str = "lmu-verify";

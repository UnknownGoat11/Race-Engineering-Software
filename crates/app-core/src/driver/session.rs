//! `DriverSessionMachine`: the pure driver-session state machine (blueprint section 5).
//!
//! Inputs are plain values (reader state, identity, login and relay state, settings). Outputs are actions
//! such as `StartRecording`, `FinishRecording`, `StartStreaming` and `SendStaleBlock`. Three orthogonal axes
//! feed the UI: LMU source (NotRunning, Connecting, Live, Paused, Stale, Error), recording (Off, Active,
//! Failed, Suspended) and network. Recording runs independent of login and network (owner decision Q9). A
//! track or car change keeps the relay session and is signalled by a session event (Q3). Auto-stream is off
//! by default (Q7).
//!
//! Stub: implemented in Phase 1 lane F.

//! Serde DTOs for the JSON events sent to the UI (blueprint section 9).
//!
//! Events: `core:status` (4 Hz), `core:scoring` (at most 5 Hz), `core:session_event`, `core:alert`,
//! `core:capabilities`, `core:engine_state` (at most 2 Hz), `core:engineer` and `core:error`. The connection
//! code appears only in `core:status`. It is never logged and never persisted.
//!
//! Stub: implemented in Phase 1 lane A.

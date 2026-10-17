//! Engineer side: the client that follows a driver's stream through the relay.
//!
//! A session is only ever joined with a code the user typed (`engineer_join`); app-core never auto-joins.
//! No driver, code or ticket is persisted (D-022).

pub mod age;
pub mod core;

pub use age::{StreamAgeReport, StreamAgeTracker, StreamHealth};
pub use core::{
    COALESCE_TIMEOUT_NS, EngineerCore, EngineerCoreStats, EngineerStatusSnapshot,
    EngineerTickOutput, MAX_ENGINEER_SESSION_EVENTS,
};

//! Engineer side: the client that follows a driver's stream through the relay.
//!
//! A session is only ever joined with a code the user typed (`engineer_join`); app-core never auto-joins.
//! No driver, code or ticket is persisted (D-022).

pub mod age;
pub mod core;

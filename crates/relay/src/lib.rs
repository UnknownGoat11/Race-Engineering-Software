//! `lmu-relay`: the lmure relay server (docs/PROTOCOL.md, ARCHITECTURE.md §3 "Relay", D-004,
//! D-007, D-008, D-009, D-016, D-022).
//!
//! * [`server`]: startup ([`RelayBuilder`]), the TLS 1.3 accept loop, housekeeping, shutdown.
//! * [`conn`] (internal): per-connection reader state machine and writer task; the hot path parses
//!   and validates headers only and never decodes telemetry payloads.
//! * [`registry`]: sessions, engineer slots, one-time connection codes, resume tickets, join
//!   cache and fan-out (relay memory only).
//! * [`outbox`]: per-receiver reliable queue plus latest-wins live slots (bounded memory).
//! * [`auth`], [`jwt`]: Argon2id accounts, EdDSA access tokens, rotating refresh tokens. A login
//!   that may no longer be used (revoked with `lmu-relay revoke-tokens` or by refresh-token reuse,
//!   account deleted or disabled) also loses its open connections within
//!   `Timeouts::revocation_check_interval` ([`state::Logins`]).
//! * [`tickets`]: engineer resume tickets (random 256 bits + HMAC tag).
//! * [`store`]: PostgreSQL / in-memory persistence of accounts, refresh-token digests and session
//!   metadata including engineer slots (never telemetry, codes, tickets or ticket digests).
//! * [`admin`] (internal): `/health` and `/metrics` on a separate bearer-protected listener.
//! * [`tls`]: PEM loading with hot reload, development certificates.
//! * [`cli`]: command line / environment configuration of the `lmu-relay` binary.
//!
//! Relay restart semantics (D-019, D-020, D-022): accounts, refresh tokens and session metadata
//! (including which engineers hold a slot, and since when their connection is down) live in
//! PostgreSQL and survive a restart; relay memory (registry, codes, ticket digests, join cache) is
//! rebuilt as clients come back, without user action:
//! * a driver re-authenticates with its refresh (or still valid access) token and resumes the same
//!   session with its resume token; the session is rebuilt from the store with its engineer slots
//!   shown as `Reconnecting`;
//! * a running engineer app re-authenticates and resumes with its in-memory ticket, before or after
//!   the driver: the ticket's HMAC tag (key derived from `JWT_KEY_FILE`) proves it belongs to the
//!   stored slot, and the slot's ticket window still applies (`ENGINEER_TICKET_TTL_S` after the
//!   drop, which a graceful shutdown records);
//! * a slot that ended before the restart (DISCONNECT, ENGINEER_KICK, expiry, session end) has no
//!   store row, so its tickets stay dead; connection codes do not survive (the driver gets a fresh
//!   one when it resumes and a slot is free).
//!
//! In development without `JWT_KEY_FILE` (ephemeral signing key, in-memory store) nothing survives
//! a restart.

mod admin;
pub mod auth;
pub mod cli;
pub mod codes;
pub mod config;
mod conn;
pub mod jwt;
pub mod metrics;
pub mod outbox;
pub mod ratelimit;
pub mod registry;
pub mod secrets;
pub mod server;
pub mod state;
pub mod store;
pub mod tickets;
pub mod time;
pub mod tls;

pub use config::RelayConfig;
pub use server::{RelayBuilder, RelayHandle, StartError};
pub use store::{MemoryStore, PgStore, Store};
pub use tls::ReloadingTls;

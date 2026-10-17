//! `lmure` wire protocol v1 (docs/PROTOCOL.md). Pure logic, no I/O:
//!
//! - [`frame`]: 40-byte header, message types, flags, priorities, relay trailer, CRC-32C, validation
//!   in the §1 order, relay fast paths (append/replace the trailer without decoding the payload);
//! - [`codec`]: streaming [`codec::FrameDecoder`] / [`codec::FrameEncoder`] for TCP byte streams;
//! - [`messages`]: postcard control/metadata messages (including the D-022 one-time connection
//!   codes), TLV telemetry payloads, the typed [`messages::Message`] enum and the
//!   engineer-direction whitelist;
//! - [`sequence`]: per-stream sequence tracking, per-key live gating, join-replay / restart
//!   handling, SNAPSHOT_REQUEST rate limiting and the 64-frame archival reorder buffer (§5);
//! - [`clock`]: NTP-style clock-offset estimation against the relay (§8);
//! - [`version`]: protocol / schema version negotiation and environment separation (§6).
//!
//! There is deliberately no message type able to carry commands to LMU, the vehicle, driver inputs
//! or the driver's operating system.

pub mod clock;
pub mod codec;
pub mod frame;
pub mod messages;
pub mod sequence;
pub mod version;

pub use codec::{CodecError, FrameDecoder, FrameEncoder};
pub use frame::{
    Endpoint, Flags, Frame, FrameError, FrameHeader, FrameMeta, FramePriority, HEADER_BYTES,
    HeaderRuleError, MAX_FRAME_BYTES, MAX_PAYLOAD_BYTES, MsgType, RelayTrailer, TrailerStamper,
    check_client_frame, check_session_binding, driver_accepts,
};
pub use messages::{ErrorCode, ErrorMsg, Message, MessageError, normalize_connect_code};
pub use sequence::{LiveGate, Reception, SnapshotRequestLimiter, StreamReceiver};
pub use version::{PROTOCOL_MAJOR, PROTOCOL_MINOR, ProtocolVersion};

/// TLS ALPN protocol id (§0, D-004).
pub const ALPN_PROTOCOL: &[u8] = b"lmure/1";
/// Default relay port.
pub const DEFAULT_PORT: u16 = 443;

/// Heartbeat and timeout defaults (§7). All configurable by the transport layer.
pub mod timing {
    /// Client PING period.
    pub const HEARTBEAT_INTERVAL_MS: u64 = 1_000;
    /// No PONG for this long → connection DEGRADED.
    pub const HEARTBEAT_DEGRADED_MS: u64 = 2_500;
    /// No inbound bytes for this long → RECONNECTING.
    pub const HEARTBEAT_TIMEOUT_MS: u64 = 5_000;
    /// Minimum reconnect backoff.
    pub const RECONNECT_MIN_MS: u64 = 250;
    /// Maximum reconnect backoff.
    pub const RECONNECT_MAX_MS: u64 = 5_000;
    /// Driver periodic full snapshot interval.
    pub const SNAPSHOT_INTERVAL_MS: u64 = 1_000;
    /// Minimum interval between request-triggered snapshots at the driver, and between
    /// SNAPSHOT_REQUESTs the relay forwards per engineer (§5,
    /// [`crate::sequence::SnapshotRequestLimiter`]).
    pub const SNAPSHOT_REQUEST_MIN_INTERVAL_MS: u64 = 250;
    /// The relay keeps a session alive this long after a driver disconnect.
    pub const SESSION_DRIVER_GRACE_S: u64 = 600;
    /// An unused connection code expires this long after it was issued (D-022, relay config).
    pub const CONNECT_CODE_TTL_S: u64 = 600;
    /// An engineer's resume ticket stays valid this long after its connection dropped (D-022,
    /// relay config).
    pub const ENGINEER_TICKET_TTL_S: u64 = 600;
}

/// Engineer access limits (D-022). Relay configuration defaults.
pub mod limits {
    /// Engineers that may hold a slot in one driver session at the same time
    /// (`max_engineers_per_session`), to avoid overloading the driver's uplink.
    pub const MAX_ENGINEERS_PER_SESSION: u16 = 2;
    /// SESSION_JOIN code redemptions allowed per account, and per source IP, per minute.
    pub const CODE_REDEMPTIONS_PER_MINUTE: u32 = 5;
    /// SESSION_JOIN code redemptions allowed per account, and per source IP, per hour.
    pub const CODE_REDEMPTIONS_PER_HOUR: u32 = 20;
}

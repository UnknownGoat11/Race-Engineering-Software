//! Frame layer (docs/PROTOCOL.md §1–§3): a 40-byte little-endian header, the payload, an optional
//! 24-byte relay trailer, then a CRC-32C over every preceding byte.
//!
//! Received bytes are validated in the order §1 requires:
//! 1. magic, 2. protocol major (before any payload byte is looked at), 3. `payload_len` against the
//!    limit (before anything is allocated for the frame), 4. CRC-32C, 5. reserved bits and fields
//!    (plus the priority byte).
//!
//! Steps 1–3 need only the header ([`peek_frame_len`]). A [`Frame`] exists only after all five
//! steps passed ([`Frame::decode`], [`crate::codec::FrameDecoder`]) or because this crate encoded it
//! ([`Frame::encode_with`]). The relay can therefore never give a corrupted frame a fresh, valid CRC
//! when it adds its trailer ([`Frame::with_relay_trailer`], [`TrailerStamper`]).
//!
//! Every framing error is fatal: the stream can no longer be trusted, so the connection is closed.
//! Header *rule* violations ([`HeaderRuleError`]) and unknown message types leave the framing
//! intact. Those are reported per frame, and the receiver skips the frame.

use std::fmt;

use bytes::Bytes;
use thiserror::Error;

use crate::messages::ErrorCode;
use crate::version::{PROTOCOL_MAJOR, PROTOCOL_MINOR};

/// Frame magic, ASCII `"LR"` (`0x4C 0x52`).
pub const MAGIC: [u8; 2] = *b"LR";
/// Size of the fixed frame header.
pub const HEADER_BYTES: usize = 40;
/// Size of the relay trailer, present only if [`Flags::RELAY_TRAILER`] is set.
pub const RELAY_TRAILER_BYTES: usize = 24;
/// Size of the CRC-32C field that ends every frame.
pub const CRC_BYTES: usize = 4;
/// Smallest possible frame: header, empty payload, CRC.
pub const MIN_FRAME_BYTES: usize = HEADER_BYTES + CRC_BYTES;
/// Maximum `payload_len` (payload bytes only).
pub const MAX_PAYLOAD_BYTES: usize = 262_144;
/// Largest possible frame on the wire: header, maximum payload, relay trailer, CRC.
pub const MAX_FRAME_BYTES: usize =
    HEADER_BYTES + MAX_PAYLOAD_BYTES + RELAY_TRAILER_BYTES + CRC_BYTES;
/// Sequence number carried by every control frame (§5).
pub const CONTROL_SEQUENCE: u64 = 0;
/// Sequence number of the first stream frame of every sequence space (§5): a sender starts at 1
/// and adds exactly 1 per stream frame.
pub const FIRST_SEQUENCE: u64 = 1;
/// `session_id` before a session exists.
pub const NO_SESSION: u64 = 0;
/// Relay trailer `relay_flags` bit0: the relay replayed this frame from its per-session cache when
/// the receiver joined (§6). Receivers apply it but never feed it to sequence tracking (§5).
pub const TRAILER_FLAG_REPLAYED: u32 = 0b1;
/// Relay trailer `relay_flags` bits 1–31: reserved, must be zero (step 5).
pub const TRAILER_FLAGS_RESERVED_MASK: u32 = !TRAILER_FLAG_REPLAYED;

/// First and last message type id of the stream (telemetry-class) range, 0x20–0x28 (§5).
pub(crate) const STREAM_TYPE_FIRST: u8 = 0x20;
pub(crate) const STREAM_TYPE_LAST: u8 = 0x28;
/// Last id of the id range reserved for the stream class in major 1 (§3, §5): 0x20–0x2F. A type a
/// later minor adds in 0x29–0x2F is a stream type and consumes a sequence number of its sender's
/// sequence space, so a receiver that does not know it still observes its sequence (no false loss,
/// [`crate::sequence::StreamReceiver::observe_unknown`]) but never applies it.
pub const STREAM_CLASS_LAST_ID: u8 = 0x2F;

/// Whether a raw message type id lies in the stream-class range 0x20–0x2F reserved for major 1,
/// known to this version or not.
pub const fn is_stream_class_id(id: u8) -> bool {
    id >= STREAM_TYPE_FIRST && id <= STREAM_CLASS_LAST_ID
}

/// Byte offsets inside the 40-byte header (§1).
mod hdr {
    pub const MAGIC: usize = 0;
    pub const MAJOR: usize = 2;
    pub const MINOR: usize = 3;
    pub const MSG_TYPE: usize = 4;
    pub const FLAGS: usize = 5;
    pub const PRIORITY: usize = 6;
    pub const RESERVED: usize = 7;
    pub const PAYLOAD_LEN: usize = 8;
    pub const SESSION_ID: usize = 12;
    pub const SEQUENCE: usize = 20;
    pub const SOURCE_TS: usize = 28;
    pub const RESERVED2: usize = 36;
}

/// Byte offsets inside the 24-byte relay trailer (§1).
mod trl {
    pub const RX_NS: usize = 0;
    pub const TX_NS: usize = 8;
    pub const DROPPED: usize = 16;
    pub const FLAGS: usize = 20;
}

/// Bit masks of the per-type direction table (§3 "dir"): which links a message type may travel.
const ROUTE_D_R: u8 = 0b0001;
const ROUTE_E_R: u8 = 0b0010;
const ROUTE_R_D: u8 = 0b0100;
const ROUTE_R_E: u8 = 0b1000;
const ROUTE_C_R: u8 = ROUTE_D_R | ROUTE_E_R;
const ROUTE_R_C: u8 = ROUTE_R_D | ROUTE_R_E;
const ROUTE_D_R_E: u8 = ROUTE_D_R | ROUTE_R_E;
const ROUTE_ANY: u8 = ROUTE_C_R | ROUTE_R_C;

/// Message type ids (§3).
///
/// There is deliberately **no** message type that could carry commands to LMU, the vehicle, driver
/// inputs or the driver's operating system. Engineer→driver traffic is limited to
/// [`MsgType::SnapshotRequest`] and session control, see [`driver_accepts`]. Engineers cannot
/// discover or list drivers: they join only by redeeming a one-time connection code (D-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum MsgType {
    Hello = 0x01,
    Auth = 0x02,
    AuthOk = 0x03,
    AccountRegister = 0x04,
    SessionCreate = 0x10,
    SessionJoin = 0x11,
    SessionInfo = 0x12,
    SessionResume = 0x13,
    SessionClose = 0x14,
    ConnectCodeRequest = 0x15,
    ConnectCode = 0x16,
    EngineerList = 0x17,
    EngineerKick = 0x18,
    /// Added in 1.1 (control class): the driver's presence, relay → engineer.
    DriverPresence = 0x19,
    SchemaNegotiate = 0x20,
    TelemetryDelta = 0x21,
    TelemetrySnapshot = 0x22,
    ScoringUpdate = 0x23,
    WeatherUpdate = 0x24,
    SessionEvent = 0x25,
    SnapshotRequest = 0x26,
    DriverStatus = 0x27,
    RecordingStatus = 0x28,
    Ping = 0x30,
    Pong = 0x31,
    Disconnect = 0x3E,
    Error = 0x3F,
}

impl MsgType {
    /// Every defined message type, in id order.
    pub const ALL: [MsgType; 27] = [
        MsgType::Hello,
        MsgType::Auth,
        MsgType::AuthOk,
        MsgType::AccountRegister,
        MsgType::SessionCreate,
        MsgType::SessionJoin,
        MsgType::SessionInfo,
        MsgType::SessionResume,
        MsgType::SessionClose,
        MsgType::ConnectCodeRequest,
        MsgType::ConnectCode,
        MsgType::EngineerList,
        MsgType::EngineerKick,
        MsgType::DriverPresence,
        MsgType::SchemaNegotiate,
        MsgType::TelemetryDelta,
        MsgType::TelemetrySnapshot,
        MsgType::ScoringUpdate,
        MsgType::WeatherUpdate,
        MsgType::SessionEvent,
        MsgType::SnapshotRequest,
        MsgType::DriverStatus,
        MsgType::RecordingStatus,
        MsgType::Ping,
        MsgType::Pong,
        MsgType::Disconnect,
        MsgType::Error,
    ];

    /// Wire id of this type.
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Maps a wire id to a known type. Unknown ids yield `None`. A receiver skips such a frame and
    /// keeps the connection, which keeps the protocol forward compatible.
    pub const fn from_id(id: u8) -> Option<MsgType> {
        Some(match id {
            0x01 => MsgType::Hello,
            0x02 => MsgType::Auth,
            0x03 => MsgType::AuthOk,
            0x04 => MsgType::AccountRegister,
            0x10 => MsgType::SessionCreate,
            0x11 => MsgType::SessionJoin,
            0x12 => MsgType::SessionInfo,
            0x13 => MsgType::SessionResume,
            0x14 => MsgType::SessionClose,
            0x15 => MsgType::ConnectCodeRequest,
            0x16 => MsgType::ConnectCode,
            0x17 => MsgType::EngineerList,
            0x18 => MsgType::EngineerKick,
            0x19 => MsgType::DriverPresence,
            0x20 => MsgType::SchemaNegotiate,
            0x21 => MsgType::TelemetryDelta,
            0x22 => MsgType::TelemetrySnapshot,
            0x23 => MsgType::ScoringUpdate,
            0x24 => MsgType::WeatherUpdate,
            0x25 => MsgType::SessionEvent,
            0x26 => MsgType::SnapshotRequest,
            0x27 => MsgType::DriverStatus,
            0x28 => MsgType::RecordingStatus,
            0x30 => MsgType::Ping,
            0x31 => MsgType::Pong,
            0x3E => MsgType::Disconnect,
            0x3F => MsgType::Error,
            _ => return None,
        })
    }

    /// Upper-case protocol name as written in PROTOCOL.md (for logs and diagnostics).
    pub const fn name(self) -> &'static str {
        match self {
            MsgType::Hello => "HELLO",
            MsgType::Auth => "AUTH",
            MsgType::AuthOk => "AUTH_OK",
            MsgType::AccountRegister => "ACCOUNT_REGISTER",
            MsgType::SessionCreate => "SESSION_CREATE",
            MsgType::SessionJoin => "SESSION_JOIN",
            MsgType::SessionInfo => "SESSION_INFO",
            MsgType::SessionResume => "SESSION_RESUME",
            MsgType::SessionClose => "SESSION_CLOSE",
            MsgType::ConnectCodeRequest => "CONNECT_CODE_REQUEST",
            MsgType::ConnectCode => "CONNECT_CODE",
            MsgType::EngineerList => "ENGINEER_LIST",
            MsgType::EngineerKick => "ENGINEER_KICK",
            MsgType::DriverPresence => "DRIVER_PRESENCE",
            MsgType::SchemaNegotiate => "SCHEMA_NEGOTIATE",
            MsgType::TelemetryDelta => "TELEMETRY_DELTA",
            MsgType::TelemetrySnapshot => "TELEMETRY_SNAPSHOT",
            MsgType::ScoringUpdate => "SCORING_UPDATE",
            MsgType::WeatherUpdate => "WEATHER_UPDATE",
            MsgType::SessionEvent => "SESSION_EVENT",
            MsgType::SnapshotRequest => "SNAPSHOT_REQUEST",
            MsgType::DriverStatus => "DRIVER_STATUS",
            MsgType::RecordingStatus => "RECORDING_STATUS",
            MsgType::Ping => "PING",
            MsgType::Pong => "PONG",
            MsgType::Disconnect => "DISCONNECT",
            MsgType::Error => "ERROR",
        }
    }

    /// Stream (telemetry-class) types, 0x20–0x28. They carry a per-sender sequence ≥ 1 and a
    /// session id ≠ 0 (§5). Every other type is a control type with sequence 0.
    pub const fn is_stream(self) -> bool {
        let id = self.id();
        id >= STREAM_TYPE_FIRST && id <= STREAM_TYPE_LAST
    }

    /// Queueing class of this type (§2).
    pub const fn delivery(self) -> Delivery {
        match self {
            MsgType::TelemetryDelta
            | MsgType::TelemetrySnapshot
            | MsgType::ScoringUpdate
            | MsgType::WeatherUpdate
            | MsgType::DriverStatus
            | MsgType::RecordingStatus => Delivery::LatestWins,
            _ => Delivery::Reliable,
        }
    }

    /// Priority a sender uses unless it has a reason to choose another allowed one. Deltas are sent
    /// as one frame per channel priority class.
    pub const fn default_priority(self) -> FramePriority {
        match self {
            MsgType::WeatherUpdate => FramePriority::P2,
            MsgType::TelemetryDelta
            | MsgType::TelemetrySnapshot
            | MsgType::ScoringUpdate
            | MsgType::DriverStatus
            | MsgType::RecordingStatus => FramePriority::P0,
            _ => FramePriority::Control,
        }
    }

    /// Whether `priority` is allowed for this type. Reliable types must use `0xFF`. Latest-wins types
    /// must use P0–P2.
    pub const fn allows_priority(self, priority: FramePriority) -> bool {
        let is_control = matches!(priority, FramePriority::Control);
        match self.delivery() {
            Delivery::Reliable => is_control,
            Delivery::LatestWins => !is_control,
        }
    }

    /// Direction bit set for this type (§3 "dir"; C = driver or engineer client, R = relay).
    const fn routes(self) -> u8 {
        match self {
            MsgType::Hello => ROUTE_ANY,
            MsgType::Auth | MsgType::AccountRegister | MsgType::SessionResume | MsgType::Ping => {
                ROUTE_C_R
            }
            MsgType::AuthOk | MsgType::SessionInfo | MsgType::Pong => ROUTE_R_C,
            MsgType::SessionCreate | MsgType::ConnectCodeRequest | MsgType::EngineerKick => {
                ROUTE_D_R
            }
            MsgType::SessionJoin => ROUTE_E_R,
            MsgType::SessionClose => ROUTE_D_R | ROUTE_R_E,
            MsgType::DriverPresence => ROUTE_R_E,
            MsgType::ConnectCode | MsgType::EngineerList => ROUTE_R_D,
            MsgType::SchemaNegotiate
            | MsgType::TelemetryDelta
            | MsgType::TelemetrySnapshot
            | MsgType::ScoringUpdate
            | MsgType::WeatherUpdate
            | MsgType::SessionEvent
            | MsgType::DriverStatus
            | MsgType::RecordingStatus => ROUTE_D_R_E,
            MsgType::SnapshotRequest => ROUTE_E_R | ROUTE_R_D,
            MsgType::Disconnect | MsgType::Error => ROUTE_ANY,
        }
    }

    /// Whether this type may travel from `from` to `to` (§3). Clients never talk to each other
    /// directly. Everything passes through the relay.
    pub const fn may_send(self, from: Endpoint, to: Endpoint) -> bool {
        let link = match (from, to) {
            (Endpoint::Driver, Endpoint::Relay) => ROUTE_D_R,
            (Endpoint::Engineer, Endpoint::Relay) => ROUTE_E_R,
            (Endpoint::Relay, Endpoint::Driver) => ROUTE_R_D,
            (Endpoint::Relay, Endpoint::Engineer) => ROUTE_R_E,
            _ => return false,
        };
        (self.routes() & link) != 0
    }

    /// Where the relay forwards a frame of this type received from `from`, if anywhere. Driver
    /// stream frames go to engineers. An engineer's SNAPSHOT_REQUEST goes to the driver. Everything
    /// else ends at the relay.
    pub const fn forward_target(self, from: Endpoint) -> Option<Endpoint> {
        if !self.is_stream() {
            return None;
        }
        let to = match from {
            Endpoint::Driver => Endpoint::Engineer,
            Endpoint::Engineer => Endpoint::Driver,
            Endpoint::Relay => return None,
        };
        if self.may_send(from, Endpoint::Relay) && self.may_send(Endpoint::Relay, to) {
            Some(to)
        } else {
            None
        }
    }
}

impl TryFrom<u8> for MsgType {
    type Error = HeaderRuleError;

    fn try_from(id: u8) -> Result<Self, HeaderRuleError> {
        MsgType::from_id(id).ok_or(HeaderRuleError::UnknownMsgType(id))
    }
}

impl fmt::Display for MsgType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The engineer-direction whitelist (§3): the message types a driver app processes when they
/// arrive from the relay. Those are SNAPSHOT_REQUEST (the only type forwarded from an engineer),
/// session control originated by the relay (SESSION_INFO, CONNECT_CODE, ENGINEER_LIST) and
/// connection housekeeping (HELLO, AUTH_OK, PONG, DISCONNECT, ERROR). The driver ignores every
/// other type it receives.
pub const fn driver_accepts(msg_type: MsgType) -> bool {
    msg_type.may_send(Endpoint::Relay, Endpoint::Driver)
}

/// A participant in the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Driver,
    Engineer,
    Relay,
}

/// Queueing class of a message type (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Delivery {
    /// Small reliable queue, never dropped. Priority must be [`FramePriority::Control`].
    Reliable,
    /// Per-receiver latest-wins slot keyed by `(msg_type, priority)`. Priority must be P0–P2. A pending
    /// snapshot is never superseded by a delta: it is sent first.
    LatestWins,
}

/// Header priority byte (§1, §2): `0`/`1`/`2` = P0/P1/P2, `0xFF` = control. Every other value is a
/// step-5 violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FramePriority {
    P0 = 0,
    P1 = 1,
    P2 = 2,
    Control = 0xFF,
}

impl FramePriority {
    /// Parses the priority byte.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(FramePriority::P0),
            1 => Some(FramePriority::P1),
            2 => Some(FramePriority::P2),
            0xFF => Some(FramePriority::Control),
            _ => None,
        }
    }

    /// Wire value.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl From<telemetry_model::Priority> for FramePriority {
    fn from(p: telemetry_model::Priority) -> Self {
        match p {
            telemetry_model::Priority::P0 => FramePriority::P0,
            telemetry_model::Priority::P1 => FramePriority::P1,
            telemetry_model::Priority::P2 => FramePriority::P2,
        }
    }
}

/// Header flags byte (§1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(u8);

impl Flags {
    /// bit0: a relay trailer follows the payload. Only the relay sets it.
    pub const RELAY_TRAILER: Flags = Flags(0b0000_0001);
    /// bit1: the payload is zstd-compressed. Not used by protocol 1.0 senders; see
    /// [`crate::messages::MessageError::CompressedPayload`].
    pub const PAYLOAD_ZSTD: Flags = Flags(0b0000_0010);
    /// bits 2–7: reserved, must be zero (step 5).
    pub const RESERVED_MASK: u8 = 0b1111_1100;

    /// No flags.
    pub const fn empty() -> Self {
        Flags(0)
    }

    /// Wraps a raw flags byte (including any reserved bits).
    pub const fn from_bits(bits: u8) -> Self {
        Flags(bits)
    }

    /// Raw flags byte.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether every bit of `other` is set.
    pub const fn contains(self, other: Flags) -> bool {
        (self.0 & other.0) == other.0
    }

    /// `self` with the bits of `other` set.
    pub const fn with(self, other: Flags) -> Self {
        Flags(self.0 | other.0)
    }

    /// `self` with the bits of `other` cleared.
    pub const fn without(self, other: Flags) -> Self {
        Flags(self.0 & !other.0)
    }

    /// The reserved bits that are set (must be 0 on the wire).
    pub const fn reserved_bits(self) -> u8 {
        self.0 & Self::RESERVED_MASK
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Flags({:#010b})", self.0)
    }
}

/// Decoded frame header. Magic, major and the reserved fields are implicit: they were validated on
/// decode and are written as constants on encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    /// Protocol minor of the sender. It is informational: receivers accept every minor of
    /// [`PROTOCOL_MAJOR`].
    pub minor: u8,
    /// Raw message type id. Unknown ids are kept for forward compatibility; see
    /// [`FrameHeader::check_rules`].
    pub msg_type: u8,
    pub flags: Flags,
    pub priority: FramePriority,
    /// Payload length in bytes (payload only).
    pub payload_len: u32,
    /// Relay-assigned session id, [`NO_SESSION`] before a session exists.
    pub session_id: u64,
    /// Per-(sender, session) stream sequence. [`CONTROL_SEQUENCE`] for control frames.
    pub sequence: u64,
    /// Sender monotonic clock, ns since the sender's process epoch.
    pub source_ts_ns: u64,
}

impl FrameHeader {
    /// Parses and validates a header **without** verifying the frame CRC. It runs steps 1–3 and the
    /// header part of step 5. This is a fast path for routing and diagnostics. Never forward or trust a
    /// frame on the strength of this alone: use [`Frame::decode`] or the
    /// [`crate::codec::FrameDecoder`].
    pub fn parse(bytes: &[u8]) -> Result<FrameHeader, FrameError> {
        if peek_frame_len(bytes, MAX_PAYLOAD_BYTES)?.is_none() {
            return Err(FrameError::Truncated {
                needed: HEADER_BYTES,
                available: bytes.len(),
            });
        }
        let Some(header) = bytes.first_chunk::<HEADER_BYTES>() else {
            return Err(FrameError::Truncated {
                needed: HEADER_BYTES,
                available: bytes.len(),
            });
        };
        parse_header_fields(header)
    }

    /// Serializes the header (magic, [`PROTOCOL_MAJOR`], reserved fields zero).
    pub fn to_bytes(&self) -> [u8; HEADER_BYTES] {
        let mut b = [0u8; HEADER_BYTES];
        b[hdr::MAGIC..hdr::MAGIC + MAGIC.len()].copy_from_slice(&MAGIC);
        b[hdr::MAJOR] = PROTOCOL_MAJOR;
        b[hdr::MINOR] = self.minor;
        b[hdr::MSG_TYPE] = self.msg_type;
        b[hdr::FLAGS] = self.flags.bits();
        b[hdr::PRIORITY] = self.priority.as_u8();
        // hdr::RESERVED and hdr::RESERVED2 stay zero.
        b[hdr::PAYLOAD_LEN..hdr::PAYLOAD_LEN + 4].copy_from_slice(&self.payload_len.to_le_bytes());
        b[hdr::SESSION_ID..hdr::SESSION_ID + 8].copy_from_slice(&self.session_id.to_le_bytes());
        b[hdr::SEQUENCE..hdr::SEQUENCE + 8].copy_from_slice(&self.sequence.to_le_bytes());
        b[hdr::SOURCE_TS..hdr::SOURCE_TS + 8].copy_from_slice(&self.source_ts_ns.to_le_bytes());
        b
    }

    /// The message type, if the id is one this version knows.
    pub const fn known_msg_type(&self) -> Option<MsgType> {
        MsgType::from_id(self.msg_type)
    }

    /// Checks the per-type header rules (§2, §5): the type must be known, its priority allowed,
    /// and its sequence and session id consistent with its class. Returns the typed message type.
    pub fn check_rules(&self) -> Result<MsgType, HeaderRuleError> {
        let msg_type = MsgType::try_from(self.msg_type)?;
        check_header_rules(msg_type, self.priority, self.session_id, self.sequence)?;
        Ok(msg_type)
    }

    /// Length of the relay trailer announced by the flags (0 or [`RELAY_TRAILER_BYTES`]).
    pub const fn trailer_len(&self) -> usize {
        if self.flags.contains(Flags::RELAY_TRAILER) {
            RELAY_TRAILER_BYTES
        } else {
            0
        }
    }

    /// Total frame length on the wire announced by this header.
    pub const fn frame_len(&self) -> usize {
        HEADER_BYTES + self.payload_len as usize + self.trailer_len() + CRC_BYTES
    }
}

/// Sender-chosen header fields. The encoder derives everything else: message type, flags,
/// payload length, magic, versions and the CRC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    pub priority: FramePriority,
    pub session_id: u64,
    pub sequence: u64,
    pub source_ts_ns: u64,
}

impl FrameMeta {
    /// Metadata for a control frame: priority `0xFF`, sequence [`CONTROL_SEQUENCE`].
    pub const fn control(session_id: u64, source_ts_ns: u64) -> Self {
        Self {
            priority: FramePriority::Control,
            session_id,
            sequence: CONTROL_SEQUENCE,
            source_ts_ns,
        }
    }

    /// Metadata for a stream (telemetry-class) frame.
    pub const fn stream(
        priority: FramePriority,
        session_id: u64,
        sequence: u64,
        source_ts_ns: u64,
    ) -> Self {
        Self {
            priority,
            session_id,
            sequence,
            source_ts_ns,
        }
    }
}

/// Relay trailer (§1), appended by the relay when it forwards to a receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RelayTrailer {
    /// Relay wall clock (ns since the Unix epoch) when the frame was received from the sender.
    pub relay_rx_ns: u64,
    /// Relay wall clock (ns since the Unix epoch) when the frame was handed to this receiver's socket.
    pub relay_tx_ns: u64,
    /// Live frames dropped for this receiver (latest-wins) since the previously forwarded frame.
    pub relay_dropped: u32,
    /// `relay_flags` bit0 ([`TRAILER_FLAG_REPLAYED`]): the frame is a cached copy replayed on join
    /// (§6), not part of the live sequence flow.
    pub replayed: bool,
}

impl RelayTrailer {
    /// Serializes the trailer (reserved `relay_flags` bits zero).
    pub fn to_bytes(&self) -> [u8; RELAY_TRAILER_BYTES] {
        let mut b = [0u8; RELAY_TRAILER_BYTES];
        b[trl::RX_NS..trl::RX_NS + 8].copy_from_slice(&self.relay_rx_ns.to_le_bytes());
        b[trl::TX_NS..trl::TX_NS + 8].copy_from_slice(&self.relay_tx_ns.to_le_bytes());
        b[trl::DROPPED..trl::DROPPED + 4].copy_from_slice(&self.relay_dropped.to_le_bytes());
        let flags = if self.replayed {
            TRAILER_FLAG_REPLAYED
        } else {
            0
        };
        b[trl::FLAGS..trl::FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        b
    }

    /// Parses a trailer located at `frame_offset` inside its frame. The offset is used for error
    /// reports only.
    fn parse(b: &[u8; RELAY_TRAILER_BYTES], frame_offset: usize) -> Result<Self, FrameError> {
        let flags = read_u32(b, trl::FLAGS);
        if flags & TRAILER_FLAGS_RESERVED_MASK != 0 {
            return Err(FrameError::ReservedField {
                offset: frame_offset + trl::FLAGS,
            });
        }
        Ok(Self {
            relay_rx_ns: read_u64(b, trl::RX_NS),
            relay_tx_ns: read_u64(b, trl::TX_NS),
            relay_dropped: read_u32(b, trl::DROPPED),
            replayed: flags & TRAILER_FLAG_REPLAYED != 0,
        })
    }
}

/// Framing errors. Every variant is fatal for the connection: framing can no longer be trusted, so
/// the receiver sends `ERROR` with [`FrameError::error_code`] (where useful) and closes.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameError {
    #[error("bad frame magic {found:02X?} (expected \"LR\")")]
    BadMagic { found: [u8; 2] },
    #[error("unsupported protocol major {found} (this build speaks {PROTOCOL_MAJOR})")]
    IncompatibleMajor { found: u8 },
    #[error("payload length {len} exceeds the limit of {limit} bytes")]
    PayloadTooLarge { len: u64, limit: usize },
    #[error("CRC-32C mismatch: frame carries {received:#010x}, computed {computed:#010x}")]
    CrcMismatch { received: u32, computed: u32 },
    #[error("reserved flag bits set: {flags:#04x}")]
    ReservedFlags { flags: u8 },
    #[error("reserved field at frame offset {offset} is not zero")]
    ReservedField { offset: usize },
    #[error("invalid priority byte {value:#04x}")]
    InvalidPriority { value: u8 },
    #[error("frame truncated: {available} bytes available, {needed} needed")]
    Truncated { needed: usize, available: usize },
    #[error("buffer holds {actual} bytes but the header announces a {declared}-byte frame")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("a protocol 1.0 sender may not set flags {flags:#04x} (any flag bit)")]
    InvalidSendFlags { flags: u8 },
    #[error("payload writer removed bytes from the frame header region")]
    HeaderOverwritten,
    #[error("frame header violates protocol rules")]
    HeaderRule(#[from] HeaderRuleError),
}

impl FrameError {
    /// Wire error code to report to the peer.
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            FrameError::BadMagic { .. } => ErrorCode::BadMagic,
            FrameError::IncompatibleMajor { .. } => ErrorCode::Incompatible,
            FrameError::PayloadTooLarge { .. } => ErrorCode::TooLarge,
            FrameError::CrcMismatch { .. } => ErrorCode::CrcMismatch,
            FrameError::ReservedFlags { .. }
            | FrameError::ReservedField { .. }
            | FrameError::InvalidPriority { .. }
            | FrameError::Truncated { .. }
            | FrameError::LengthMismatch { .. }
            | FrameError::HeaderRule(_) => ErrorCode::Malformed,
            // Encode-side errors are bugs of the local sender, never sent to a peer as theirs.
            FrameError::InvalidSendFlags { .. } | FrameError::HeaderOverwritten => {
                ErrorCode::Internal
            }
        }
    }
}

/// Violations of the per-type header rules (§2, §5). The frame itself is intact (CRC valid), so
/// these are **not** fatal: the receiver skips the frame and counts it.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum HeaderRuleError {
    #[error("unknown message type {0:#04x}")]
    UnknownMsgType(u8),
    #[error("priority {priority:?} is not allowed for {msg_type}")]
    PriorityNotAllowed {
        msg_type: MsgType,
        priority: FramePriority,
    },
    #[error("control frame {msg_type} must carry sequence 0, has {sequence}")]
    ControlSequence { msg_type: MsgType, sequence: u64 },
    #[error("stream frame {msg_type} must carry a sequence >= 1")]
    StreamSequenceZero { msg_type: MsgType },
    #[error("stream frame {msg_type} must carry a session id")]
    StreamWithoutSession { msg_type: MsgType },
    /// The connection's role may not send this type to the relay (§3 direction table), e.g. an
    /// engineer sending telemetry or ENGINEER_KICK.
    #[error("{msg_type} may not be sent to the relay by a {from:?} connection")]
    DirectionNotAllowed { msg_type: MsgType, from: Endpoint },
    /// A client set a flag bit. A 1.0 client sets none: RELAY_TRAILER is appended by the relay
    /// only, and PAYLOAD_ZSTD is refused by every 1.0 receiver (§1).
    #[error("a client frame sets flags {flags:#04x}; a protocol 1.0 client sets none")]
    FlagsFromClient { flags: u8 },
    /// A client used a non-zero session id before its connection was bound to a session (§1).
    #[error("session id {found} used before the connection is bound to a session")]
    SessionNotBound { found: u64 },
    /// A client used a session id other than the one bound to its connection (§1).
    #[error("session id {found} does not match the connection's session {bound}")]
    SessionMismatch { bound: u64, found: u64 },
}

/// Session isolation rule for frames a relay receives from a client (§1, §2). The relay routes by
/// the session **bound to the authenticated connection** (after SESSION_CREATE / SESSION_JOIN /
/// SESSION_RESUME was accepted), never by the sender-controlled header `session_id`. The header must
/// agree with that binding:
/// * before binding (`bound = None`) the header carries [`NO_SESSION`];
/// * after binding, stream frames carry exactly the bound id; control frames carry the bound id or
///   [`NO_SESSION`].
///
/// A violation is a header-rule violation: the relay skips and counts the frame (or answers
/// `ERROR NotAuthorized`), and never forwards it.
pub fn check_session_binding(
    header: &FrameHeader,
    bound: Option<u64>,
) -> Result<(), HeaderRuleError> {
    let found = header.session_id;
    match bound {
        None if found == NO_SESSION => Ok(()),
        None => Err(HeaderRuleError::SessionNotBound { found }),
        Some(bound) if found == bound => Ok(()),
        Some(_)
            if found == NO_SESSION && header.known_msg_type().is_some_and(|t| !t.is_stream()) =>
        {
            Ok(())
        }
        Some(bound) => Err(HeaderRuleError::SessionMismatch { bound, found }),
    }
}

/// Every header rule a relay applies to a frame received from a client connection (§1–§3):
/// * the per-type rules ([`FrameHeader::check_rules`]);
/// * the direction table: `from` is the role the connection authenticated as
///   ([`Endpoint::Driver`] or [`Endpoint::Engineer`]), and only types that role may send to the
///   relay pass ([`MsgType::may_send`]). [`Endpoint::Relay`] passes nothing;
/// * no flag bit (a 1.0 client sets none, §1);
/// * the session binding ([`check_session_binding`]).
///
/// Returns the typed message type. A violation is not fatal: the relay skips and counts the frame
/// (or answers `ERROR NotAuthorized`) and never forwards, caches or otherwise acts on it.
pub fn check_client_frame(
    header: &FrameHeader,
    from: Endpoint,
    bound: Option<u64>,
) -> Result<MsgType, HeaderRuleError> {
    let msg_type = header.check_rules()?;
    if !msg_type.may_send(from, Endpoint::Relay) {
        return Err(HeaderRuleError::DirectionNotAllowed { msg_type, from });
    }
    if header.flags != Flags::empty() {
        return Err(HeaderRuleError::FlagsFromClient {
            flags: header.flags.bits(),
        });
    }
    check_session_binding(header, bound)?;
    Ok(msg_type)
}

/// Checks the per-type header rules shared by encoder and decoder (§2, §5):
/// * the priority must be allowed for the type ([`MsgType::allows_priority`]);
/// * control types carry sequence 0;
/// * stream types carry a sequence ≥ 1 and a session id ≠ 0.
pub const fn check_header_rules(
    msg_type: MsgType,
    priority: FramePriority,
    session_id: u64,
    sequence: u64,
) -> Result<(), HeaderRuleError> {
    if !msg_type.allows_priority(priority) {
        return Err(HeaderRuleError::PriorityNotAllowed { msg_type, priority });
    }
    if msg_type.is_stream() {
        if sequence == CONTROL_SEQUENCE {
            return Err(HeaderRuleError::StreamSequenceZero { msg_type });
        }
        if session_id == NO_SESSION {
            return Err(HeaderRuleError::StreamWithoutSession { msg_type });
        }
    } else if sequence != CONTROL_SEQUENCE {
        return Err(HeaderRuleError::ControlSequence { msg_type, sequence });
    }
    Ok(())
}

/// Validation steps 1–3 on the start of a buffer. Returns the total length of the frame that starts
/// at `buf[0]`, or `Ok(None)` if fewer than [`HEADER_BYTES`] bytes are available.
///
/// The magic and the major are checked as soon as their bytes are present, so garbage (for example
/// a stray HTTP request) is rejected without waiting for a full header. `max_payload` lets a
/// receiver apply a tighter limit than [`MAX_PAYLOAD_BYTES`]; larger values are clamped to it. No
/// memory may be reserved for the frame before this function returned `Ok(Some(_))`.
pub fn peek_frame_len(buf: &[u8], max_payload: usize) -> Result<Option<usize>, FrameError> {
    // Step 1: magic.
    if let Some(found) = buf.first_chunk::<2>()
        && *found != MAGIC
    {
        return Err(FrameError::BadMagic { found: *found });
    }
    // Step 2: major, before any payload byte is considered.
    if let Some(&found) = buf.get(hdr::MAJOR)
        && found != PROTOCOL_MAJOR
    {
        return Err(FrameError::IncompatibleMajor { found });
    }
    let Some(header) = buf.first_chunk::<HEADER_BYTES>() else {
        return Ok(None);
    };
    // Step 3: payload_len, before anything is allocated for the frame.
    let payload_len = read_u32(header, hdr::PAYLOAD_LEN);
    let limit = max_payload.min(MAX_PAYLOAD_BYTES);
    if u64::from(payload_len) > limit as u64 {
        return Err(FrameError::PayloadTooLarge {
            len: u64::from(payload_len),
            limit,
        });
    }
    let trailer = if Flags::from_bits(header[hdr::FLAGS]).contains(Flags::RELAY_TRAILER) {
        RELAY_TRAILER_BYTES
    } else {
        0
    };
    // Cannot overflow: payload_len <= MAX_PAYLOAD_BYTES was checked above.
    Ok(Some(
        HEADER_BYTES + payload_len as usize + trailer + CRC_BYTES,
    ))
}

/// Step 5 for the header plus typed field extraction. Steps 1–3 must already have passed.
fn parse_header_fields(h: &[u8; HEADER_BYTES]) -> Result<FrameHeader, FrameError> {
    let flags = Flags::from_bits(h[hdr::FLAGS]);
    if flags.reserved_bits() != 0 {
        return Err(FrameError::ReservedFlags {
            flags: flags.bits(),
        });
    }
    if h[hdr::RESERVED] != 0 {
        return Err(FrameError::ReservedField {
            offset: hdr::RESERVED,
        });
    }
    if read_u32(h, hdr::RESERVED2) != 0 {
        return Err(FrameError::ReservedField {
            offset: hdr::RESERVED2,
        });
    }
    let priority = FramePriority::from_u8(h[hdr::PRIORITY]).ok_or(FrameError::InvalidPriority {
        value: h[hdr::PRIORITY],
    })?;
    Ok(FrameHeader {
        minor: h[hdr::MINOR],
        msg_type: h[hdr::MSG_TYPE],
        flags,
        priority,
        payload_len: read_u32(h, hdr::PAYLOAD_LEN),
        session_id: read_u64(h, hdr::SESSION_ID),
        sequence: read_u64(h, hdr::SEQUENCE),
        source_ts_ns: read_u64(h, hdr::SOURCE_TS),
    })
}

/// Runs validation steps 1–5 on a buffer that must hold exactly one frame.
pub(crate) fn validate_frame(
    buf: &[u8],
    max_payload: usize,
) -> Result<(FrameHeader, Option<RelayTrailer>), FrameError> {
    // Steps 1–3.
    let Some(frame_len) = peek_frame_len(buf, max_payload)? else {
        return Err(FrameError::Truncated {
            needed: HEADER_BYTES,
            available: buf.len(),
        });
    };
    if buf.len() != frame_len {
        return Err(FrameError::LengthMismatch {
            declared: frame_len,
            actual: buf.len(),
        });
    }
    // Step 4: CRC over every byte before the CRC field. frame_len >= MIN_FRAME_BYTES here.
    let Some((body, crc_field)) = buf.split_last_chunk::<CRC_BYTES>() else {
        return Err(FrameError::Truncated {
            needed: MIN_FRAME_BYTES,
            available: buf.len(),
        });
    };
    let received = u32::from_le_bytes(*crc_field);
    let computed = crc32c::crc32c(body);
    if received != computed {
        return Err(FrameError::CrcMismatch { received, computed });
    }
    // Step 5.
    let Some(header_bytes) = body.first_chunk::<HEADER_BYTES>() else {
        return Err(FrameError::Truncated {
            needed: HEADER_BYTES,
            available: buf.len(),
        });
    };
    let header = parse_header_fields(header_bytes)?;
    let trailer = if header.flags.contains(Flags::RELAY_TRAILER) {
        let at = HEADER_BYTES + header.payload_len as usize;
        let Some(bytes) = body
            .get(at..)
            .and_then(|t| t.first_chunk::<RELAY_TRAILER_BYTES>())
        else {
            return Err(FrameError::Truncated {
                needed: at + RELAY_TRAILER_BYTES,
                available: body.len(),
            });
        };
        Some(RelayTrailer::parse(bytes, at)?)
    } else {
        None
    };
    Ok((header, trailer))
}

/// A complete frame whose CRC and reserved fields were verified, or that this crate encoded.
///
/// Invariant (established by every constructor): `bytes.len() == header.frame_len()` and the
/// relay trailer is `Some` exactly when `header.flags` contains [`Flags::RELAY_TRAILER`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    header: FrameHeader,
    trailer: Option<RelayTrailer>,
    bytes: Bytes,
}

impl Frame {
    /// Decodes and fully validates (steps 1–5) a buffer that holds exactly one frame.
    pub fn decode(bytes: Bytes) -> Result<Frame, FrameError> {
        let (header, trailer) = validate_frame(&bytes, MAX_PAYLOAD_BYTES)?;
        Ok(Frame {
            header,
            trailer,
            bytes,
        })
    }

    /// Assembles a frame from parts produced by [`validate_frame`] on exactly `bytes`.
    pub(crate) fn from_validated(
        header: FrameHeader,
        trailer: Option<RelayTrailer>,
        bytes: Bytes,
    ) -> Frame {
        Frame {
            header,
            trailer,
            bytes,
        }
    }

    /// Encodes a frame around an already encoded payload. The payload is copied once.
    pub fn build(msg_type: MsgType, meta: &FrameMeta, payload: &[u8]) -> Result<Frame, FrameError> {
        Frame::encode_with(msg_type, meta, Flags::empty(), payload.len(), |buf| {
            buf.extend_from_slice(payload);
            Ok::<(), FrameError>(())
        })
    }

    /// Encodes a frame whose payload `write_payload` appends directly to the frame buffer. Nothing is
    /// copied, which is the hot path for TLV telemetry built with `telemetry_model::codec::TlvEncoder`.
    ///
    /// The closure must only append to the buffer it is given. The header region before the payload
    /// is overwritten afterwards. `payload_capacity` is a size hint for the initial allocation and is
    /// clamped to [`MAX_PAYLOAD_BYTES`]. In protocol 1.0 `flags` must be empty: RELAY_TRAILER is
    /// set by the relay only, and every 1.0 receiver refuses PAYLOAD_ZSTD (§1), so a frame carrying
    /// either would be unusable ([`FrameError::InvalidSendFlags`]). Fails if the header rules
    /// ([`check_header_rules`]) are violated or the payload exceeds [`MAX_PAYLOAD_BYTES`].
    pub fn encode_with<E, F>(
        msg_type: MsgType,
        meta: &FrameMeta,
        flags: Flags,
        payload_capacity: usize,
        write_payload: F,
    ) -> Result<Frame, E>
    where
        E: From<FrameError>,
        F: FnOnce(&mut Vec<u8>) -> Result<(), E>,
    {
        check_header_rules(msg_type, meta.priority, meta.session_id, meta.sequence)
            .map_err(FrameError::HeaderRule)?;
        if flags != Flags::empty() {
            return Err(FrameError::InvalidSendFlags {
                flags: flags.bits(),
            }
            .into());
        }
        let capacity = HEADER_BYTES + payload_capacity.min(MAX_PAYLOAD_BYTES) + CRC_BYTES;
        let mut buf = Vec::with_capacity(capacity);
        buf.extend_from_slice(&[0u8; HEADER_BYTES]);
        write_payload(&mut buf)?;
        let payload_len = buf
            .len()
            .checked_sub(HEADER_BYTES)
            .ok_or(FrameError::HeaderOverwritten)?;
        if payload_len > MAX_PAYLOAD_BYTES {
            return Err(FrameError::PayloadTooLarge {
                len: payload_len as u64,
                limit: MAX_PAYLOAD_BYTES,
            }
            .into());
        }
        let header = FrameHeader {
            minor: PROTOCOL_MINOR,
            msg_type: msg_type.id(),
            flags,
            priority: meta.priority,
            // Fits: payload_len <= MAX_PAYLOAD_BYTES < u32::MAX.
            payload_len: payload_len as u32,
            session_id: meta.session_id,
            sequence: meta.sequence,
            source_ts_ns: meta.source_ts_ns,
        };
        let Some(header_slot) = buf.first_chunk_mut::<HEADER_BYTES>() else {
            return Err(FrameError::HeaderOverwritten.into());
        };
        *header_slot = header.to_bytes();
        let crc = crc32c::crc32c(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(Frame {
            header,
            trailer: None,
            bytes: Bytes::from(buf),
        })
    }

    /// The decoded header.
    pub const fn header(&self) -> &FrameHeader {
        &self.header
    }

    /// The message type, if known to this version.
    pub const fn msg_type(&self) -> Option<MsgType> {
        self.header.known_msg_type()
    }

    /// Payload bytes (without trailer and CRC).
    pub fn payload(&self) -> &[u8] {
        // In bounds by the type invariant.
        &self.bytes[HEADER_BYTES..HEADER_BYTES + self.header.payload_len as usize]
    }

    /// Payload as a zero-copy `Bytes` slice of the frame buffer.
    pub fn payload_bytes(&self) -> Bytes {
        self.bytes
            .slice(HEADER_BYTES..HEADER_BYTES + self.header.payload_len as usize)
    }

    /// The relay trailer, if the frame carries one.
    pub const fn relay_trailer(&self) -> Option<&RelayTrailer> {
        self.trailer.as_ref()
    }

    /// Complete wire encoding of the frame.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Complete wire encoding as a cheaply clonable `Bytes`.
    pub fn to_bytes(&self) -> Bytes {
        self.bytes.clone()
    }

    /// Consumes the frame and returns its wire encoding.
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    /// Copies the frame into its own exact-size allocation.
    ///
    /// A frame produced by [`crate::codec::FrameDecoder`] shares the read buffer's allocation, which
    /// may have grown to [`MAX_FRAME_BYTES`]. Keeping such a frame alive keeps that whole allocation
    /// alive. Call this for any frame stored beyond the current dispatch (relay join cache,
    /// latest-wins slots, reorder buffers).
    pub fn detach(&self) -> Frame {
        Frame {
            header: self.header,
            trailer: self.trailer,
            bytes: Bytes::copy_from_slice(&self.bytes),
        }
    }

    /// Length of the frame on the wire.
    pub fn wire_len(&self) -> usize {
        self.bytes.len()
    }

    /// The CRC-32C stored at the end of the frame.
    pub fn crc(&self) -> u32 {
        self.bytes
            .last_chunk::<CRC_BYTES>()
            .map_or(0, |c| u32::from_le_bytes(*c))
    }

    /// Checks the per-type header rules; see [`FrameHeader::check_rules`].
    pub fn check_rules(&self) -> Result<MsgType, HeaderRuleError> {
        self.header.check_rules()
    }

    /// Relay fast path: returns this frame with `trailer` appended. An existing trailer is
    /// replaced. The RELAY_TRAILER flag is set and the CRC recomputed. The payload is copied verbatim
    /// and never decoded. To stamp one frame for many receivers, use [`TrailerStamper`].
    pub fn with_relay_trailer(&self, trailer: &RelayTrailer) -> Frame {
        let mut header = self.header;
        header.flags = header.flags.with(Flags::RELAY_TRAILER);
        let mut buf = Vec::with_capacity(header.frame_len());
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(self.payload());
        buf.extend_from_slice(&trailer.to_bytes());
        let crc = crc32c::crc32c(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Frame {
            header,
            trailer: Some(*trailer),
            bytes: Bytes::from(buf),
        }
    }

    /// Returns this frame without a relay trailer and with the CRC recomputed. If there is no
    /// trailer, the frame is returned unchanged and cheaply.
    pub fn without_relay_trailer(&self) -> Frame {
        if self.trailer.is_none() {
            return self.clone();
        }
        let mut header = self.header;
        header.flags = header.flags.without(Flags::RELAY_TRAILER);
        let mut buf = Vec::with_capacity(header.frame_len());
        buf.extend_from_slice(&header.to_bytes());
        buf.extend_from_slice(self.payload());
        let crc = crc32c::crc32c(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Frame {
            header,
            trailer: None,
            bytes: Bytes::from(buf),
        }
    }
}

/// Relay fan-out helper. It prepares one received frame once, then stamps a per-receiver relay
/// trailer onto it for the cost of hashing 24 bytes. It never decodes the payload.
///
/// [`TrailerStamper::body`] (header with RELAY_TRAILER set, then the payload) is shared by every
/// receiver. [`TrailerStamper::tail`] returns the 28 receiver-specific bytes (trailer and CRC), so a
/// vectored write sends `body ++ tail` without copying the payload.
#[derive(Debug, Clone)]
pub struct TrailerStamper {
    header: FrameHeader,
    body: Bytes,
    body_crc: u32,
}

impl TrailerStamper {
    /// Prepares `frame` for stamping. An existing trailer is dropped and replaced by each stamp.
    pub fn new(frame: &Frame) -> Self {
        let mut header = frame.header;
        header.flags = header.flags.with(Flags::RELAY_TRAILER);
        let mut body = Vec::with_capacity(HEADER_BYTES + frame.payload().len());
        body.extend_from_slice(&header.to_bytes());
        body.extend_from_slice(frame.payload());
        let body_crc = crc32c::crc32c(&body);
        Self {
            header,
            body: Bytes::from(body),
            body_crc,
        }
    }

    /// Header (with RELAY_TRAILER set) and payload, shared by every receiver.
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// The receiver-specific end of the frame: the trailer, then the CRC-32C over `body ++ trailer`.
    pub fn tail(&self, trailer: &RelayTrailer) -> [u8; RELAY_TRAILER_BYTES + CRC_BYTES] {
        let trailer_bytes = trailer.to_bytes();
        let crc = crc32c::crc32c_append(self.body_crc, &trailer_bytes);
        let mut tail = [0u8; RELAY_TRAILER_BYTES + CRC_BYTES];
        tail[..RELAY_TRAILER_BYTES].copy_from_slice(&trailer_bytes);
        tail[RELAY_TRAILER_BYTES..].copy_from_slice(&crc.to_le_bytes());
        tail
    }

    /// A contiguous frame for one receiver. It is byte-identical to [`Frame::with_relay_trailer`].
    pub fn stamp(&self, trailer: &RelayTrailer) -> Frame {
        let mut buf = Vec::with_capacity(self.body.len() + RELAY_TRAILER_BYTES + CRC_BYTES);
        buf.extend_from_slice(&self.body);
        buf.extend_from_slice(&self.tail(trailer));
        Frame {
            header: self.header,
            trailer: Some(*trailer),
            bytes: Bytes::from(buf),
        }
    }
}

/// Little-endian `u32` at a constant offset of a fixed-size block. Every call site uses offsets
/// from the layout tables above, which are in bounds for `N`.
fn read_u32<const N: usize>(b: &[u8; N], at: usize) -> u32 {
    let mut v = [0u8; 4];
    v.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(v)
}

/// Little-endian `u64` at a constant offset of a fixed-size block; see [`read_u32`].
fn read_u64<const N: usize>(b: &[u8; N], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header() -> FrameHeader {
        FrameHeader {
            minor: PROTOCOL_MINOR,
            msg_type: MsgType::TelemetryDelta.id(),
            flags: Flags::empty(),
            priority: FramePriority::P1,
            payload_len: 3,
            session_id: 0x1122_3344_5566_7788,
            sequence: 0x0102_0304_0506_0708,
            source_ts_ns: 0xA1A2_A3A4_A5A6_A7A8,
        }
    }

    fn delta_frame(payload: &[u8]) -> Frame {
        let meta = FrameMeta::stream(FramePriority::P1, 0x1122_3344_5566_7788, 42, 7_000);
        Frame::build(MsgType::TelemetryDelta, &meta, payload).expect("build")
    }

    /// Recomputes the CRC of a raw frame after a test modified it (to reach step 5).
    fn refresh_crc(raw: &mut [u8]) {
        let n = raw.len() - CRC_BYTES;
        let crc = crc32c::crc32c(&raw[..n]);
        raw[n..].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn header_layout_matches_protocol_md() {
        let b = sample_header().to_bytes();
        assert_eq!(&b[0..2], &[0x4C, 0x52]);
        assert_eq!(b[2], 1);
        assert_eq!(b[3], PROTOCOL_MINOR);
        assert_eq!(b[4], 0x21);
        assert_eq!(b[5], 0);
        assert_eq!(b[6], 1);
        assert_eq!(b[7], 0);
        assert_eq!(&b[8..12], &3u32.to_le_bytes());
        assert_eq!(
            &b[12..20],
            &[0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(&b[20..28], &0x0102_0304_0506_0708u64.to_le_bytes());
        assert_eq!(&b[28..36], &0xA1A2_A3A4_A5A6_A7A8u64.to_le_bytes());
        assert_eq!(&b[36..40], &[0, 0, 0, 0]);
    }

    #[test]
    fn header_roundtrip() {
        let h = sample_header();
        assert_eq!(FrameHeader::parse(&h.to_bytes()), Ok(h));
        assert_eq!(h.frame_len(), HEADER_BYTES + 3 + CRC_BYTES);
    }

    #[test]
    fn trailer_layout_matches_protocol_md() {
        let t = RelayTrailer {
            relay_rx_ns: 1,
            relay_tx_ns: 2,
            relay_dropped: 3,
            replayed: false,
        };
        let b = t.to_bytes();
        assert_eq!(&b[0..8], &1u64.to_le_bytes());
        assert_eq!(&b[8..16], &2u64.to_le_bytes());
        assert_eq!(&b[16..20], &3u32.to_le_bytes());
        assert_eq!(&b[20..24], &[0, 0, 0, 0]);
    }

    #[test]
    fn frame_roundtrip_and_crc_layout() {
        let f = delta_frame(b"abc");
        assert_eq!(f.wire_len(), MIN_FRAME_BYTES + 3);
        let raw = f.to_bytes();
        let crc = crc32c::crc32c(&raw[..raw.len() - CRC_BYTES]);
        assert_eq!(&raw[raw.len() - CRC_BYTES..], &crc.to_le_bytes());
        assert_eq!(f.crc(), crc);
        let d = Frame::decode(raw).expect("decode");
        assert_eq!(d, f);
        assert_eq!(d.payload(), b"abc");
        assert_eq!(&d.payload_bytes()[..], b"abc");
        assert_eq!(d.msg_type(), Some(MsgType::TelemetryDelta));
        assert_eq!(d.relay_trailer(), None);
    }

    #[test]
    fn empty_payload_is_the_minimum_frame() {
        let f = Frame::build(MsgType::Ping, &FrameMeta::control(0, 1), &[]).expect("build");
        assert_eq!(f.wire_len(), MIN_FRAME_BYTES);
        assert_eq!(Frame::decode(f.to_bytes()).expect("decode"), f);
    }

    #[test]
    fn validation_order_magic_first() {
        let mut raw = delta_frame(b"x").to_bytes().to_vec();
        raw[0] = b'X';
        raw[2] = 9; // also a bad major
        raw[8..12].copy_from_slice(&u32::MAX.to_le_bytes()); // and too large
        assert_eq!(
            Frame::decode(raw.into()),
            Err(FrameError::BadMagic { found: *b"XR" })
        );
    }

    #[test]
    fn validation_order_major_before_length_and_payload() {
        // Only the header is present: an incompatible major must be reported without the payload.
        let mut h = sample_header().to_bytes();
        h[2] = 2;
        h[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            peek_frame_len(&h, MAX_PAYLOAD_BYTES),
            Err(FrameError::IncompatibleMajor { found: 2 })
        );
        // Rejected as soon as the major byte is available.
        assert_eq!(
            peek_frame_len(&h[..3], MAX_PAYLOAD_BYTES),
            Err(FrameError::IncompatibleMajor { found: 2 })
        );
        assert_eq!(peek_frame_len(&h[..2], MAX_PAYLOAD_BYTES), Ok(None));
    }

    #[test]
    fn validation_order_length_before_crc() {
        let mut h = sample_header().to_bytes();
        h[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            peek_frame_len(&h, MAX_PAYLOAD_BYTES),
            Err(FrameError::PayloadTooLarge {
                len: u64::from(u32::MAX),
                limit: MAX_PAYLOAD_BYTES
            })
        );
        let max = u32::try_from(MAX_PAYLOAD_BYTES).expect("fits");
        h[8..12].copy_from_slice(&max.to_le_bytes());
        assert_eq!(
            peek_frame_len(&h, MAX_PAYLOAD_BYTES),
            Ok(Some(HEADER_BYTES + MAX_PAYLOAD_BYTES + CRC_BYTES))
        );
        h[8..12].copy_from_slice(&(max + 1).to_le_bytes());
        assert!(matches!(
            peek_frame_len(&h, MAX_PAYLOAD_BYTES),
            Err(FrameError::PayloadTooLarge { .. })
        ));
        // A tighter receiver limit applies; a looser one is clamped to the protocol maximum.
        h[8..12].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(
            peek_frame_len(&h, 99),
            Err(FrameError::PayloadTooLarge {
                len: 100,
                limit: 99
            })
        );
        h[8..12].copy_from_slice(&(max + 1).to_le_bytes());
        assert!(peek_frame_len(&h, usize::MAX).is_err());
    }

    #[test]
    fn validation_order_crc_before_reserved_bits() {
        let mut raw = delta_frame(b"abc").to_bytes().to_vec();
        raw[5] |= 0b1000_0000; // reserved flag bit, CRC not refreshed
        assert!(matches!(
            Frame::decode(raw.clone().into()),
            Err(FrameError::CrcMismatch { .. })
        ));
        refresh_crc(&mut raw);
        assert_eq!(
            Frame::decode(raw.into()),
            Err(FrameError::ReservedFlags { flags: 0b1000_0000 })
        );
    }

    #[test]
    fn reserved_fields_and_priority_are_rejected_after_crc() {
        let base = delta_frame(b"abc").to_bytes().to_vec();
        let mut r7 = base.clone();
        r7[7] = 1;
        refresh_crc(&mut r7);
        assert_eq!(
            Frame::decode(r7.into()),
            Err(FrameError::ReservedField { offset: 7 })
        );
        let mut r36 = base.clone();
        r36[39] = 0x80;
        refresh_crc(&mut r36);
        assert_eq!(
            Frame::decode(r36.into()),
            Err(FrameError::ReservedField { offset: 36 })
        );
        let mut prio = base;
        prio[6] = 3;
        refresh_crc(&mut prio);
        assert_eq!(
            Frame::decode(prio.into()),
            Err(FrameError::InvalidPriority { value: 3 })
        );
    }

    #[test]
    fn every_single_bit_flip_is_detected() {
        let raw = delta_frame(b"telemetry payload").to_bytes().to_vec();
        for byte in 0..raw.len() {
            for bit in 0..8 {
                let mut m = raw.clone();
                m[byte] ^= 1 << bit;
                assert!(
                    Frame::decode(m.into()).is_err(),
                    "flip of byte {byte} bit {bit} undetected"
                );
            }
        }
    }

    #[test]
    fn length_mismatch_and_truncation() {
        let raw = delta_frame(b"abc").to_bytes().to_vec();
        let mut longer = raw.clone();
        longer.push(0);
        assert_eq!(
            Frame::decode(longer.into()),
            Err(FrameError::LengthMismatch {
                declared: raw.len(),
                actual: raw.len() + 1
            })
        );
        for len in 0..raw.len() {
            assert!(
                Frame::decode(raw[..len].to_vec().into()).is_err(),
                "prefix {len} decoded"
            );
        }
    }

    #[test]
    fn relay_trailer_append_replace_remove() {
        let f = delta_frame(b"payload");
        let t1 = RelayTrailer {
            relay_rx_ns: 10,
            relay_tx_ns: 20,
            relay_dropped: 0,
            replayed: false,
        };
        let t2 = RelayTrailer {
            relay_rx_ns: 11,
            relay_tx_ns: 22,
            relay_dropped: 5,
            replayed: false,
        };
        let with = f.with_relay_trailer(&t1);
        assert_eq!(with.wire_len(), f.wire_len() + RELAY_TRAILER_BYTES);
        assert!(with.header().flags.contains(Flags::RELAY_TRAILER));
        assert_eq!(with.payload(), f.payload());
        let decoded = Frame::decode(with.to_bytes()).expect("decode with trailer");
        assert_eq!(decoded.relay_trailer(), Some(&t1));
        // Replace.
        let replaced = decoded.with_relay_trailer(&t2);
        assert_eq!(replaced.wire_len(), with.wire_len());
        assert_eq!(
            Frame::decode(replaced.to_bytes())
                .expect("decode")
                .relay_trailer(),
            Some(&t2)
        );
        // Remove restores the original bytes exactly.
        assert_eq!(replaced.without_relay_trailer(), f);
        assert_eq!(f.without_relay_trailer(), f);
    }

    #[test]
    fn trailer_stamper_matches_contiguous_stamp() {
        let f = delta_frame(&[7u8; 300]);
        let stamper = TrailerStamper::new(&f);
        for dropped in 0..4u32 {
            let t = RelayTrailer {
                relay_rx_ns: 100,
                relay_tx_ns: 200 + u64::from(dropped),
                relay_dropped: dropped,
                replayed: false,
            };
            let stamped = stamper.stamp(&t);
            assert_eq!(stamped, f.with_relay_trailer(&t));
            let mut vectored = stamper.body().to_vec();
            vectored.extend_from_slice(&stamper.tail(&t));
            assert_eq!(&vectored[..], stamped.as_bytes());
            assert_eq!(
                Frame::decode(vectored.into())
                    .expect("decode")
                    .relay_trailer(),
                Some(&t)
            );
        }
    }

    #[test]
    fn trailer_reserved_field_is_rejected() {
        let f = delta_frame(b"abc").with_relay_trailer(&RelayTrailer::default());
        let at = HEADER_BYTES + 3 + 20;
        for (byte, value) in [(at, 0b10u8), (at + 3, 0x80)] {
            let mut raw = f.to_bytes().to_vec();
            raw[byte] = value;
            refresh_crc(&mut raw);
            assert_eq!(
                Frame::decode(raw.into()),
                Err(FrameError::ReservedField { offset: at })
            );
        }
    }

    #[test]
    fn trailer_replay_flag_roundtrips() {
        let t = RelayTrailer {
            replayed: true,
            ..RelayTrailer::default()
        };
        assert_eq!(&t.to_bytes()[20..24], &TRAILER_FLAG_REPLAYED.to_le_bytes());
        let f = delta_frame(b"abc").with_relay_trailer(&t);
        let d = Frame::decode(f.to_bytes()).expect("replay bit is not reserved");
        assert_eq!(d.relay_trailer().map(|t| t.replayed), Some(true));
        let stamped = TrailerStamper::new(&f).stamp(&RelayTrailer::default());
        assert_eq!(
            Frame::decode(stamped.to_bytes())
                .expect("decode")
                .relay_trailer()
                .map(|t| t.replayed),
            Some(false)
        );
    }

    fn header_of(msg_type: MsgType, session_id: u64) -> FrameHeader {
        let meta = if msg_type.is_stream() {
            FrameMeta::stream(msg_type.default_priority(), session_id, 1, 0)
        } else {
            FrameMeta::control(session_id, 0)
        };
        *Frame::build(msg_type, &meta, &[]).expect("build").header()
    }

    #[test]
    fn session_binding_isolates_sessions() {
        use HeaderRuleError::*;
        let delta7 = header_of(MsgType::TelemetryDelta, 7);
        let delta8 = header_of(MsgType::TelemetryDelta, 8);
        let ping0 = header_of(MsgType::Ping, 0);
        let ping7 = header_of(MsgType::Ping, 7);
        let ping8 = header_of(MsgType::Ping, 8);
        // Before binding: only session 0.
        assert_eq!(check_session_binding(&ping0, None), Ok(()));
        assert_eq!(
            check_session_binding(&ping7, None),
            Err(SessionNotBound { found: 7 })
        );
        assert_eq!(
            check_session_binding(&delta7, None),
            Err(SessionNotBound { found: 7 })
        );
        // Bound to 7: driver A cannot inject into session 8.
        assert_eq!(check_session_binding(&delta7, Some(7)), Ok(()));
        assert_eq!(
            check_session_binding(&delta8, Some(7)),
            Err(SessionMismatch { bound: 7, found: 8 })
        );
        assert_eq!(
            check_session_binding(&ping8, Some(7)),
            Err(SessionMismatch { bound: 7, found: 8 })
        );
        // Control frames may still carry 0 after binding.
        assert_eq!(check_session_binding(&ping0, Some(7)), Ok(()));
        assert_eq!(check_session_binding(&ping7, Some(7)), Ok(()));
        // An engineer cannot target another session's driver with SNAPSHOT_REQUEST.
        let req8 = header_of(MsgType::SnapshotRequest, 8);
        assert_eq!(
            check_client_frame(&req8, Endpoint::Engineer, Some(7)),
            Err(SessionMismatch { bound: 7, found: 8 })
        );
        let req7 = header_of(MsgType::SnapshotRequest, 7);
        assert_eq!(
            check_client_frame(&req7, Endpoint::Engineer, Some(7)),
            Ok(MsgType::SnapshotRequest)
        );
    }

    #[test]
    fn client_frames_must_not_set_flags() {
        let f = delta_frame(b"abc");
        let sid = f.header().session_id;
        assert_eq!(
            check_client_frame(f.header(), Endpoint::Driver, Some(sid)),
            Ok(MsgType::TelemetryDelta)
        );
        // A driver forging a relay trailer (e.g. a fake REPLAYED flag or relay timestamps).
        let forged = f.with_relay_trailer(&RelayTrailer::default());
        assert_eq!(
            check_client_frame(forged.header(), Endpoint::Driver, Some(sid)),
            Err(HeaderRuleError::FlagsFromClient {
                flags: Flags::RELAY_TRAILER.bits()
            })
        );
        // A compressed payload would be refused by every 1.0 engineer: never forwarded.
        let mut raw = f.to_bytes().to_vec();
        raw[hdr::FLAGS] = Flags::PAYLOAD_ZSTD.bits();
        refresh_crc(&mut raw);
        let zstd = Frame::decode(raw.into()).expect("valid framing");
        assert_eq!(
            check_client_frame(zstd.header(), Endpoint::Driver, Some(sid)),
            Err(HeaderRuleError::FlagsFromClient {
                flags: Flags::PAYLOAD_ZSTD.bits()
            })
        );
    }

    #[test]
    fn client_frames_must_respect_the_direction_table() {
        use Endpoint::*;
        let sid = 7;
        // An engineer can neither inject telemetry (e.g. into the relay's join cache) nor use
        // the driver's session controls.
        for t in [
            MsgType::TelemetryDelta,
            MsgType::TelemetrySnapshot,
            MsgType::SchemaNegotiate,
            MsgType::SessionCreate,
            MsgType::ConnectCodeRequest,
            MsgType::EngineerKick,
        ] {
            assert_eq!(
                check_client_frame(&header_of(t, sid), Engineer, Some(sid)),
                Err(HeaderRuleError::DirectionNotAllowed {
                    msg_type: t,
                    from: Engineer
                }),
                "{t}"
            );
        }
        // A driver cannot redeem connection codes or ask itself for snapshots.
        for t in [MsgType::SessionJoin, MsgType::SnapshotRequest] {
            assert_eq!(
                check_client_frame(&header_of(t, sid), Driver, Some(sid)),
                Err(HeaderRuleError::DirectionNotAllowed {
                    msg_type: t,
                    from: Driver
                }),
                "{t}"
            );
        }
        // Relay-originated types are never accepted from a client.
        for t in [
            MsgType::SessionInfo,
            MsgType::ConnectCode,
            MsgType::EngineerList,
            MsgType::AuthOk,
            MsgType::Pong,
        ] {
            for from in [Driver, Engineer] {
                assert!(
                    check_client_frame(&header_of(t, sid), from, Some(sid)).is_err(),
                    "{t} from {from:?}"
                );
            }
        }
        // `Relay` is not a client role: it passes nothing.
        for t in MsgType::ALL {
            assert!(check_client_frame(&header_of(t, sid), Relay, Some(sid)).is_err());
        }
        assert_eq!(
            check_client_frame(&header_of(MsgType::Hello, 0), Engineer, None),
            Ok(MsgType::Hello)
        );
        assert_eq!(
            check_client_frame(&header_of(MsgType::EngineerKick, sid), Driver, Some(sid)),
            Ok(MsgType::EngineerKick)
        );
    }

    #[test]
    fn detach_copies_into_an_exact_allocation() {
        let a = delta_frame(b"first");
        let b = delta_frame(b"second");
        let mut shared = a.as_bytes().to_vec();
        shared.extend_from_slice(b.as_bytes());
        let shared = Bytes::from(shared);
        let first = Frame::decode(shared.slice(..a.wire_len())).expect("decode");
        let detached = first.detach();
        assert_eq!(detached, first);
        assert_eq!(detached.relay_trailer(), first.relay_trailer());
        assert_ne!(
            detached.as_bytes().as_ptr(),
            shared.as_ptr(),
            "detached frame must not share the read buffer"
        );
        assert_eq!(first.as_bytes().as_ptr(), shared.as_ptr());
    }

    #[test]
    fn msg_type_ids_are_exactly_the_documented_ones() {
        let documented: [(u8, &str); 27] = [
            (0x01, "HELLO"),
            (0x02, "AUTH"),
            (0x03, "AUTH_OK"),
            (0x04, "ACCOUNT_REGISTER"),
            (0x10, "SESSION_CREATE"),
            (0x11, "SESSION_JOIN"),
            (0x12, "SESSION_INFO"),
            (0x13, "SESSION_RESUME"),
            (0x14, "SESSION_CLOSE"),
            (0x15, "CONNECT_CODE_REQUEST"),
            (0x16, "CONNECT_CODE"),
            (0x17, "ENGINEER_LIST"),
            (0x18, "ENGINEER_KICK"),
            (0x19, "DRIVER_PRESENCE"),
            (0x20, "SCHEMA_NEGOTIATE"),
            (0x21, "TELEMETRY_DELTA"),
            (0x22, "TELEMETRY_SNAPSHOT"),
            (0x23, "SCORING_UPDATE"),
            (0x24, "WEATHER_UPDATE"),
            (0x25, "SESSION_EVENT"),
            (0x26, "SNAPSHOT_REQUEST"),
            (0x27, "DRIVER_STATUS"),
            (0x28, "RECORDING_STATUS"),
            (0x30, "PING"),
            (0x31, "PONG"),
            (0x3E, "DISCONNECT"),
            (0x3F, "ERROR"),
        ];
        for (t, (id, name)) in MsgType::ALL.iter().zip(documented) {
            assert_eq!((t.id(), t.name()), (id, name));
        }
        let known = (0..=u8::MAX)
            .filter(|id| MsgType::from_id(*id).is_some())
            .count();
        assert_eq!(known, documented.len(), "no undocumented ids may exist");
        for t in MsgType::ALL {
            assert_eq!(MsgType::from_id(t.id()), Some(t));
            assert_eq!(MsgType::try_from(t.id()), Ok(t));
        }
        assert_eq!(
            MsgType::try_from(0x29),
            Err(HeaderRuleError::UnknownMsgType(0x29))
        );
    }

    #[test]
    fn stream_range_and_delivery_classes() {
        for t in MsgType::ALL {
            assert_eq!(t.is_stream(), (0x20..=0x28).contains(&t.id()), "{t}");
            assert!(
                t.allows_priority(t.default_priority()),
                "{t} default priority must be allowed"
            );
        }
        for t in [
            MsgType::SchemaNegotiate,
            MsgType::SessionEvent,
            MsgType::SnapshotRequest,
        ] {
            assert_eq!(
                t.delivery(),
                Delivery::Reliable,
                "{t} must never be dropped"
            );
        }
        assert!(!MsgType::TelemetryDelta.allows_priority(FramePriority::Control));
        assert!(MsgType::TelemetryDelta.allows_priority(FramePriority::P2));
        assert!(!MsgType::Hello.allows_priority(FramePriority::P0));
    }

    #[test]
    fn driver_whitelist_is_snapshot_request_and_session_control_only() {
        let accepted: Vec<MsgType> = MsgType::ALL
            .into_iter()
            .filter(|t| driver_accepts(*t))
            .collect();
        assert_eq!(
            accepted,
            vec![
                MsgType::Hello,
                MsgType::AuthOk,
                MsgType::SessionInfo,
                MsgType::ConnectCode,
                MsgType::EngineerList,
                MsgType::SnapshotRequest,
                MsgType::Pong,
                MsgType::Disconnect,
                MsgType::Error,
            ]
        );
        // Only SNAPSHOT_REQUEST is forwarded from an engineer to the driver.
        let forwarded: Vec<MsgType> = MsgType::ALL
            .into_iter()
            .filter(|t| t.forward_target(Endpoint::Engineer) == Some(Endpoint::Driver))
            .collect();
        assert_eq!(forwarded, vec![MsgType::SnapshotRequest]);
    }

    #[test]
    fn engineers_originate_only_session_control_and_snapshot_requests() {
        // D-022: no engineer-originated type beyond joining by code, resuming with a ticket,
        // snapshot requests and connection housekeeping. Nothing lists or discovers drivers.
        let originated: Vec<MsgType> = MsgType::ALL
            .into_iter()
            .filter(|t| t.may_send(Endpoint::Engineer, Endpoint::Relay))
            .collect();
        assert_eq!(
            originated,
            vec![
                MsgType::Hello,
                MsgType::Auth,
                MsgType::AccountRegister,
                MsgType::SessionJoin,
                MsgType::SessionResume,
                MsgType::SnapshotRequest,
                MsgType::Ping,
                MsgType::Disconnect,
                MsgType::Error,
            ]
        );
        // Engineers hear nothing about connection codes or other engineers.
        for t in [
            MsgType::ConnectCode,
            MsgType::EngineerList,
            MsgType::ConnectCodeRequest,
            MsgType::EngineerKick,
        ] {
            assert!(!t.may_send(Endpoint::Relay, Endpoint::Engineer), "{t}");
            assert!(!t.may_send(Endpoint::Engineer, Endpoint::Relay), "{t}");
            assert_eq!(t.forward_target(Endpoint::Driver), None, "{t}");
        }
    }

    #[test]
    fn direction_table() {
        use Endpoint::*;
        assert!(MsgType::TelemetryDelta.may_send(Driver, Relay));
        assert!(MsgType::TelemetryDelta.may_send(Relay, Engineer));
        assert!(!MsgType::TelemetryDelta.may_send(Engineer, Relay));
        assert!(!MsgType::TelemetryDelta.may_send(Relay, Driver));
        assert_eq!(
            MsgType::TelemetryDelta.forward_target(Driver),
            Some(Engineer)
        );
        assert_eq!(MsgType::TelemetryDelta.forward_target(Engineer), None);
        assert_eq!(MsgType::SessionJoin.forward_target(Engineer), None);
        assert!(MsgType::SessionJoin.may_send(Engineer, Relay));
        assert!(!MsgType::SessionJoin.may_send(Driver, Relay));
        assert!(MsgType::SessionCreate.may_send(Driver, Relay));
        assert!(!MsgType::SessionCreate.may_send(Engineer, Relay));
        assert!(MsgType::SessionResume.may_send(Driver, Relay));
        assert!(MsgType::SessionResume.may_send(Engineer, Relay));
        assert!(MsgType::ConnectCodeRequest.may_send(Driver, Relay));
        assert!(MsgType::ConnectCode.may_send(Relay, Driver));
        assert!(MsgType::EngineerList.may_send(Relay, Driver));
        assert!(MsgType::EngineerKick.may_send(Driver, Relay));
        assert!(MsgType::SessionClose.may_send(Relay, Engineer));
        // 1.1: driver presence goes from the relay to engineers only, never to or from a driver.
        assert!(MsgType::DriverPresence.may_send(Relay, Engineer));
        assert!(!MsgType::DriverPresence.may_send(Relay, Driver));
        assert!(!MsgType::DriverPresence.may_send(Driver, Relay));
        assert!(!MsgType::DriverPresence.may_send(Engineer, Relay));
        assert!(!MsgType::DriverPresence.is_stream());
        assert_eq!(MsgType::DriverPresence.delivery(), Delivery::Reliable);
        for t in MsgType::ALL {
            assert!(
                !t.may_send(Driver, Engineer) && !t.may_send(Engineer, Driver),
                "{t}"
            );
            assert!(!t.may_send(Relay, Relay), "{t}");
        }
    }

    #[test]
    fn header_rules() {
        use HeaderRuleError::*;
        let ok = check_header_rules(MsgType::TelemetryDelta, FramePriority::P0, 9, 1);
        assert_eq!(ok, Ok(()));
        assert_eq!(
            check_header_rules(MsgType::TelemetryDelta, FramePriority::Control, 9, 1),
            Err(PriorityNotAllowed {
                msg_type: MsgType::TelemetryDelta,
                priority: FramePriority::Control
            })
        );
        assert_eq!(
            check_header_rules(MsgType::TelemetryDelta, FramePriority::P0, 9, 0),
            Err(StreamSequenceZero {
                msg_type: MsgType::TelemetryDelta
            })
        );
        assert_eq!(
            check_header_rules(MsgType::TelemetryDelta, FramePriority::P0, 0, 1),
            Err(StreamWithoutSession {
                msg_type: MsgType::TelemetryDelta
            })
        );
        assert_eq!(
            check_header_rules(MsgType::Ping, FramePriority::Control, 0, 5),
            Err(ControlSequence {
                msg_type: MsgType::Ping,
                sequence: 5
            })
        );
        assert_eq!(
            check_header_rules(MsgType::Ping, FramePriority::Control, 0, 0),
            Ok(())
        );
    }

    #[test]
    fn encoder_refuses_invalid_frames() {
        let bad_prio = FrameMeta::stream(FramePriority::Control, 1, 1, 0);
        assert!(matches!(
            Frame::build(MsgType::TelemetryDelta, &bad_prio, b""),
            Err(FrameError::HeaderRule(
                HeaderRuleError::PriorityNotAllowed { .. }
            ))
        ));
        let meta = FrameMeta::control(0, 0);
        let too_big = vec![0u8; MAX_PAYLOAD_BYTES + 1];
        assert!(matches!(
            Frame::build(MsgType::Ping, &meta, &too_big),
            Err(FrameError::PayloadTooLarge { .. })
        ));
        let exact = vec![0u8; MAX_PAYLOAD_BYTES];
        let f = Frame::build(MsgType::Ping, &meta, &exact).expect("max payload is allowed");
        assert_eq!(f.wire_len(), HEADER_BYTES + MAX_PAYLOAD_BYTES + CRC_BYTES);
        for flags in [
            Flags::RELAY_TRAILER,
            Flags::PAYLOAD_ZSTD,
            Flags::from_bits(0b100),
        ] {
            let r: Result<Frame, FrameError> =
                Frame::encode_with(MsgType::Ping, &meta, flags, 0, |_| Ok(()));
            assert_eq!(
                r,
                Err(FrameError::InvalidSendFlags {
                    flags: flags.bits()
                })
            );
        }
        let r: Result<Frame, FrameError> =
            Frame::encode_with(MsgType::Ping, &meta, Flags::empty(), 0, |b| {
                b.clear();
                Ok(())
            });
        assert_eq!(r, Err(FrameError::HeaderOverwritten));
    }

    #[test]
    fn error_codes() {
        assert_eq!(
            FrameError::BadMagic { found: [0, 0] }.error_code(),
            ErrorCode::BadMagic
        );
        assert_eq!(
            FrameError::IncompatibleMajor { found: 2 }.error_code(),
            ErrorCode::Incompatible
        );
        assert_eq!(
            FrameError::PayloadTooLarge { len: 1, limit: 0 }.error_code(),
            ErrorCode::TooLarge
        );
        assert_eq!(
            FrameError::CrcMismatch {
                received: 0,
                computed: 1
            }
            .error_code(),
            ErrorCode::CrcMismatch
        );
        assert_eq!(
            FrameError::ReservedFlags { flags: 4 }.error_code(),
            ErrorCode::Malformed
        );
    }
}

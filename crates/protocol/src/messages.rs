//! Message payloads (docs/PROTOCOL.md §3, §6, §9) and their typed encoding.
//!
//! * Control and metadata messages are `postcard` structs (D-005). postcard encodes struct fields
//!   in declaration order and enum variants by declaration index, so **field and variant order is
//!   wire format**: new fields/variants are appended only, and only with a `PROTOCOL_MINOR` bump.
//! * TELEMETRY_DELTA, TELEMETRY_SNAPSHOT and WEATHER_UPDATE carry a `telemetry_model::codec` TLV
//!   block (§4), kept as raw bytes ([`TelemetryPayload`]) so the receiver can apply it to its frame
//!   state with `TelemetryFrame::apply_tlv` without decoding twice.
//! * Decoding is strict for a peer of the same minor: every payload byte must be consumed
//!   ([`MessageError::TrailingBytes`]). Bytes appended by a newer minor are ignored (§10,
//!   [`Message::decode_from_minor`]).
//! * Engineer access follows D-022: an engineer reaches a driver only by redeeming a one-time
//!   connection code ([`SessionJoin`], [`normalize_connect_code`]) and resumes after a network blip
//!   with an in-memory ticket ([`SessionResume::Engineer`]); the driver manages codes and engineers
//!   with [`ConnectCodeRequest`], [`ConnectCode`], [`EngineerList`] and [`EngineerKick`].
//!
//! Secrets (passwords, tokens, resume tokens) are wrapped in [`Secret`], whose `Debug` output is
//! redacted so they can never leak into logs.

use std::fmt;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use telemetry_model::codec::{self, TLV_HEADER_BYTES, TlvHeader, TlvRecord};
use telemetry_model::enums::SessionType;
use telemetry_model::events::EventRecord;
use telemetry_model::scoring::{CapabilityMap, ScoringState, SessionIdentity, VehicleCapabilities};
use thiserror::Error;

use crate::frame::{Endpoint, Flags, Frame, FrameError, FrameMeta, MsgType};
use crate::version::{Environment, PROTOCOL_MINOR, ProtocolVersion};

/// Initial capacity hint for postcard-encoded control payloads (most are far smaller).
const CONTROL_PAYLOAD_CAPACITY_HINT: usize = 256;

// ---------------------------------------------------------------------------------------------
// Shared building blocks
// ---------------------------------------------------------------------------------------------

/// A secret string (password, access/refresh/resume token). Serialized as a plain string;
/// `Debug` never prints the content.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret value. Call sites must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Account identifier assigned by the relay (UUID bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountId(pub [u8; 16]);

/// Role a client connects as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClientRole {
    Driver,
    Engineer,
}

impl From<ClientRole> for Endpoint {
    fn from(role: ClientRole) -> Self {
        match role {
            ClientRole::Driver => Endpoint::Driver,
            ClientRole::Engineer => Endpoint::Engineer,
        }
    }
}

/// Error codes of `ErrorMsg` (§9). The declaration order is the postcard variant index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    BadMagic,
    Incompatible,
    Malformed,
    TooLarge,
    CrcMismatch,
    AuthFailed,
    TokenExpired,
    NotAuthorized,
    SessionNotFound,
    SessionClosed,
    RateLimited,
    ServerBusy,
    Internal,
    /// SESSION_JOIN: the connection code is unknown, malformed, already redeemed or replaced by a
    /// newer code (D-022).
    CodeInvalid,
    /// SESSION_JOIN: the connection code expired unused (D-022).
    CodeExpired,
    /// SESSION_JOIN: the driver already has the maximum number of engineers connected (D-022).
    SessionFull,
}

// ---------------------------------------------------------------------------------------------
// Handshake and accounts (0x01–0x04)
// ---------------------------------------------------------------------------------------------

/// Client HELLO (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    /// Application version string (semver of the desktop app).
    pub app_version: String,
    pub protocol: ProtocolVersion,
    pub role: ClientRole,
    /// Telemetry schema versions this client can produce/consume, any order.
    pub schema_versions: Vec<u16>,
    /// Stable per-installation identifier (opaque to the relay except for diagnostics).
    pub client_id: String,
}

/// Limits the relay announces in its HELLO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLimits {
    /// Largest payload the relay accepts (≤ `MAX_PAYLOAD_BYTES`).
    pub max_payload_bytes: u32,
    /// Expected client PING period.
    pub heartbeat_interval_ms: u32,
    /// The relay closes a connection without inbound bytes for this long.
    pub heartbeat_timeout_ms: u32,
    /// Engineers that may watch one session at the same time.
    pub max_engineers_per_session: u16,
    /// How long a session survives a driver disconnect.
    pub session_driver_grace_s: u32,
}

/// Relay HELLO (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerHello {
    pub relay_version: String,
    pub protocol: ProtocolVersion,
    /// Relay wall clock, ns since the Unix epoch (coarse; precise sync uses PING/PONG).
    pub server_time_ns: u64,
    pub limits: ServerLimits,
    pub env: Environment,
}

/// Wire payload of a HELLO frame. HELLO travels in both directions with one message type, so the
/// payload is self-describing (postcard variant index 0 = client, 1 = server).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum HelloPayload {
    Client(Hello),
    Server(ServerHello),
}

/// Borrowing twin of [`HelloPayload`] for encoding without a clone (identical serialization).
#[derive(Serialize)]
enum HelloPayloadRef<'a> {
    Client(&'a Hello),
    Server(&'a ServerHello),
}

/// AUTH (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Auth {
    Password { username: String, password: Secret },
    AccessToken { token: Secret },
    RefreshToken { token: Secret },
}

/// AUTH_OK (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthOk {
    pub account: AccountId,
    pub username: String,
    pub display_name: String,
    pub role: ClientRole,
    pub access_token: Secret,
    pub refresh_token: Secret,
    /// Access token expiry, relay wall clock ns since the Unix epoch.
    pub access_expires_at_ns: u64,
    /// Refresh token expiry, relay wall clock ns since the Unix epoch.
    pub refresh_expires_at_ns: u64,
}

/// ACCOUNT_REGISTER (only if the relay allows open registration).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountRegister {
    pub username: String,
    pub password: Secret,
    pub display_name: String,
}

// ---------------------------------------------------------------------------------------------
// Sessions and engineer access (0x10–0x18, D-022)
// ---------------------------------------------------------------------------------------------

/// SESSION_CREATE (driver → relay).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCreate {
    /// Vehicle, class, track and session type as read from LMU (`None` = not known yet).
    pub identity: SessionIdentity,
    /// LMU build/version string as reported by the game, if known.
    pub lmu_version: Option<String>,
    pub app_version: String,
    /// Telemetry schema version the driver will stream.
    pub schema_version: u16,
    /// Coarse car feature flags (`None` entries = not determined yet).
    pub capabilities: VehicleCapabilities,
}

/// SESSION_JOIN (engineer → relay): redeem a one-time connection code (D-022). A code is single
/// use, so every new engineer connection needs a fresh code from the driver; there is no other
/// way for an engineer to reach a driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionJoin {
    /// The connection code in canonical form ([`normalize_connect_code`]).
    pub code: Secret,
}

/// Credential that lets a client resume its session after its connection dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResumeCredential {
    /// Driver: the session resume token. The driver app keeps it in Windows Credential Manager,
    /// so a restart within the grace window resumes the session (D-020).
    DriverToken(Secret),
    /// Engineer: the connection-bound resume ticket (D-022). Kept in memory only, never written
    /// to disk. It dies when the engineer app closes or disconnects, when the driver removes the
    /// engineer, when the session ends, or [`crate::timing::ENGINEER_TICKET_TTL_S`] after a drop;
    /// after that only a new connection code connects.
    EngineerTicket(Secret),
}

/// SESSION_INFO (relay → client): the session this connection is now bound to. Sent after an
/// accepted SESSION_CREATE / SESSION_JOIN / SESSION_RESUME, and again to every engineer when the
/// driver restarts its sequence space (§6); receivers reset their stream state on it (§5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: u64,
    /// Identity confirmation (D-022): after a SESSION_JOIN the engineer app shows the driver,
    /// car, class, track, session type and start time, and shows the stream only once the user
    /// confirmed them.
    pub driver_display_name: String,
    /// The driver's account name. Unlike the display name it is unique.
    pub driver_username: String,
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub track: Option<String>,
    pub session_type: Option<SessionType>,
    /// Session start, relay wall clock ns since the Unix epoch.
    pub started_at_ns: u64,
    pub schema_version: u16,
    /// The credential for SESSION_RESUME: [`ResumeCredential::DriverToken`] in the driver's copy,
    /// [`ResumeCredential::EngineerTicket`] in an engineer's. Clients keep the one from the newest
    /// SESSION_INFO (the relay may rotate it).
    pub resume: ResumeCredential,
}

/// SESSION_RESUME (client → relay): reconnect to the session after the connection dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionResume {
    /// The driver app reconnects, or restarted within the session grace window (D-020).
    Driver {
        session_id: u64,
        resume_token: Secret,
        /// The driver app restarted and begins a new sequence space (§5). The relay then resets
        /// the stream for every engineer before forwarding its frames (§6).
        new_sequence_space: bool,
    },
    /// The engineer app (still running) reconnects after a network blip with the ticket from its
    /// newest SESSION_INFO (D-022).
    Engineer { session_id: u64, ticket: Secret },
}

/// Why a session ended for the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionCloseReason {
    /// The driver stopped streaming.
    DriverStopped,
    /// The driver grace period elapsed without a resume.
    Expired,
    /// The driver disconnected this engineer (ENGINEER_KICK). Its resume ticket is dead; only a
    /// new connection code connects again.
    RemovedByDriver,
    /// The relay is shutting down.
    RelayShutdown,
}

/// SESSION_CLOSE.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionClose {
    pub session_id: u64,
    pub reason: SessionCloseReason,
}

/// CONNECT_CODE_REQUEST (driver → relay): issue a connection code, or rotate it when the driver
/// presses **New code** (the unused code is invalidated at once). Empty payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectCodeRequest;

/// A connection code issued by the relay (D-022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuedCode {
    /// Canonical form ([`normalize_connect_code`]); the driver app shows it as `XXXX-XXXX`.
    pub code: Secret,
    /// When the code expires if unused, relay wall clock ns since the Unix epoch.
    pub expires_at_ns: u64,
}

/// CONNECT_CODE (relay → driver): the session's current connection code and engineer slot usage.
/// Sent in reply to CONNECT_CODE_REQUEST and whenever the code or the slot usage changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectCode {
    /// The unused code, `None` when there is none: every engineer slot is taken, or the code
    /// expired and the driver has not asked for a new one.
    pub code: Option<IssuedCode>,
    /// Engineer slots in use: connected engineers plus dropped ones whose resume ticket is alive.
    pub engineers_connected: u16,
    /// Maximum engineers per session (relay configuration,
    /// [`crate::limits::MAX_ENGINEERS_PER_SESSION`] by default).
    pub engineers_max: u16,
}

/// Link state of one engineer as the relay sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EngineerLink {
    /// Connected; heartbeats on time and no live frames dropped for this engineer recently.
    Healthy,
    /// Connected, but heartbeats are late or the relay drops live frames for this engineer.
    Degraded,
    /// The connection dropped. The slot is held for the resume ticket until it expires or the
    /// driver removes the engineer.
    Reconnecting,
}

/// One engineer holding a slot in the driver's session (D-022 driver controls).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineerEntry {
    /// Relay-assigned id of this engineer connection, unique within the session. A resume keeps
    /// it; every new join gets a new one. Target of ENGINEER_KICK.
    pub connection_id: u64,
    pub username: String,
    pub display_name: String,
    /// When the engineer joined, relay wall clock ns since the Unix epoch.
    pub connected_since_ns: u64,
    pub link: EngineerLink,
}

/// ENGINEER_LIST (relay → driver): every engineer holding a slot in the driver's session. Sent
/// whenever it changes (join, resume, drop, disconnect, removal, link state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineerList {
    pub engineers: Vec<EngineerEntry>,
}

/// ENGINEER_KICK (driver → relay): disconnect one engineer. The relay sends that engineer
/// SESSION_CLOSE{RemovedByDriver}, closes its connection, kills its resume ticket and frees the
/// slot; the engineer needs a new code to connect again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineerKick {
    /// [`EngineerEntry::connection_id`] from the newest ENGINEER_LIST. Unknown ids are ignored.
    pub connection_id: u64,
}

/// Characters in a connection code: 8 Crockford base32 characters = 40 random bits (D-022).
pub const CONNECT_CODE_LEN: usize = 8;
/// Crockford base32 alphabet of connection codes (no I, L, O, U).
pub const CONNECT_CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Separator of the display form (`7KQ4-MZ9T`); ignored on input.
const CONNECT_CODE_SEPARATOR: char = '-';

/// Why user input is not a connection code. Carries no code characters: codes are never logged.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConnectCodeError {
    #[error("a connection code has {CONNECT_CODE_LEN} characters, the input has {len}")]
    WrongLength { len: usize },
    #[error("the input contains a character that cannot appear in a connection code")]
    InvalidCharacter,
}

/// Canonical (wire) form of a connection code typed by a user: separators and whitespace are
/// removed, letters upper-cased, and the look-alikes I/L read as `1` and O as `0` (Crockford
/// decoding). The engineer app sends this form in SESSION_JOIN; the relay normalizes again and
/// treats a failure as `ERROR CodeInvalid`.
pub fn normalize_connect_code(input: &str) -> Result<Secret, ConnectCodeError> {
    let mut code = String::with_capacity(CONNECT_CODE_LEN);
    let mut len = 0usize;
    for c in input.chars() {
        if c == CONNECT_CODE_SEPARATOR || c.is_whitespace() {
            continue;
        }
        let c = match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        };
        if !u8::try_from(c).is_ok_and(|b| CONNECT_CODE_ALPHABET.contains(&b)) {
            return Err(ConnectCodeError::InvalidCharacter);
        }
        len = len.saturating_add(1);
        if len <= CONNECT_CODE_LEN {
            code.push(c);
        }
    }
    if len != CONNECT_CODE_LEN {
        return Err(ConnectCodeError::WrongLength { len });
    }
    Ok(Secret::new(code))
}

// ---------------------------------------------------------------------------------------------
// Stream class (0x20–0x28)
// ---------------------------------------------------------------------------------------------

/// SCHEMA_NEGOTIATE (driver → relay → engineer): schema version and per-channel capability map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaNegotiate {
    pub schema_version: u16,
    pub capabilities: CapabilityMap,
    pub vehicle_capabilities: VehicleCapabilities,
}

/// Capture timestamps attached to postcard stream messages (same meaning as the TLV header, §4/§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureTime {
    /// Driver monotonic ns at the LMU read.
    pub mono_ns: u64,
    /// Driver estimate of the relay clock at capture; `None` while clock sync has no sample.
    pub relay_ns_est: Option<i64>,
}

/// SCORING_UPDATE: session scoring and the competitor list at LMU's native scoring rate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoringUpdate {
    pub capture: CaptureTime,
    pub scoring: ScoringState,
}

/// SESSION_EVENT: one session event (lap, sector, pit, flag, LMU lost, recording ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEventMsg {
    pub capture: CaptureTime,
    pub record: EventRecord,
}

/// Why a fresh snapshot is wanted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotReason {
    /// An engineer joined (sent by the relay on the engineer's behalf).
    EngineerJoined,
    /// The receiver detected a sequence gap it cannot repair from deltas.
    SequenceGap,
    /// The receiver reconnected.
    Reconnect,
    /// Requested by the engineer user.
    Manual,
}

/// SNAPSHOT_REQUEST (engineer → relay → driver). The only stream message an engineer may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    pub reason: SnapshotReason,
}

/// Coarse health of one layer of the driver pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HealthLevel {
    /// Working as intended.
    Ok,
    /// Working with reduced quality (e.g. stale data, backpressure).
    Degraded,
    /// Not working.
    Failed,
    /// Intentionally off (e.g. recording disabled).
    Inactive,
}

/// Health of one layer, with an optional human explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub level: HealthLevel,
    pub detail: Option<String>,
}

/// State of the LMU shared-memory source as seen by the driver app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LmuSourceState {
    /// LMU is not running or its shared memory does not exist.
    NotRunning,
    /// Shared memory found, waiting for the first consistent read.
    Connecting,
    /// Data is updating.
    Live,
    /// Mapped but not updating (menus, pause, replay).
    Paused,
    /// Expected updates stopped arriving.
    Stale,
    /// The reader failed (see `detail`).
    Error,
}

/// Measured rates. `None` means "not measured yet", never zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DriverRates {
    pub telemetry_read_hz: Option<f32>,
    pub scoring_read_hz: Option<f32>,
    pub frames_sent_hz: Option<f32>,
    pub bytes_sent_per_s: Option<u64>,
    /// Live frames dropped by the driver's latest-wins queues since the stream started.
    pub frames_dropped_total: u64,
}

/// Driver clock-sync quality, so the engineer can combine uncertainties (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSummary {
    /// θ: relay clock minus driver clock, ns.
    pub offset_ns: i64,
    /// δ_min / 2, ns.
    pub uncertainty_ns: u64,
}

/// DRIVER_STATUS (1 Hz): layered health of the driver pipeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriverStatus {
    pub lmu_state: LmuSourceState,
    pub lmu_source: ComponentHealth,
    pub processing: ComponentHealth,
    pub recording: ComponentHealth,
    pub network: ComponentHealth,
    pub rates: DriverRates,
    /// `None` until the driver has a clock-sync sample.
    pub clock: Option<ClockSummary>,
}

/// Recorder state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecordingState {
    Off,
    Active,
    Failed,
}

/// RECORDING_STATUS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingStatus {
    pub state: RecordingState,
    pub recording_id: Option<String>,
    pub bytes_written: u64,
    pub frames_written: u64,
    /// Frames the recorder could not persist (must stay 0 in a healthy recording).
    pub frames_lost: u64,
    /// `None` when the free space could not be determined.
    pub disk_free_bytes: Option<u64>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Connection housekeeping (0x30–0x3F)
// ---------------------------------------------------------------------------------------------

/// PING (client → relay): heartbeat and clock-sync probe (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub id: u64,
    /// Client monotonic clock at send, ns.
    pub t0: u64,
}

/// PONG (relay → client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    pub id: u64,
    /// Echo of the PING's `t0`.
    pub t0: u64,
    /// Relay wall clock at PING receipt, ns since the Unix epoch.
    pub t1: u64,
    /// Relay wall clock at PONG send, ns since the Unix epoch.
    pub t2: u64,
}

/// Why a connection is closed gracefully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisconnectReason {
    /// The user or app closed the connection.
    ClientShutdown,
    /// The relay is shutting down or restarting.
    ServerShutdown,
    /// The same account connected again with the same role; this connection is replaced.
    Replaced,
    /// No traffic within the heartbeat timeout.
    IdleTimeout,
    /// The session this connection served has ended.
    SessionEnded,
}

/// DISCONNECT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Disconnect {
    pub reason: DisconnectReason,
    pub message: Option<String>,
}

/// ERROR (§9). `fatal = true`: the sender closes the connection after sending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: ErrorCode,
    pub message: String,
    pub fatal: bool,
}

// ---------------------------------------------------------------------------------------------
// Telemetry TLV payload
// ---------------------------------------------------------------------------------------------

/// A TLV telemetry block (§4) kept as raw bytes. The relay never decodes it; receivers apply it
/// with `TelemetryFrame::apply_tlv` or decode it with [`TelemetryPayload::decode`].
///
/// A payload decoded by [`Message::from_frame`] is a zero-copy view of the codec's read buffer,
/// exactly like the frame it came from (§1): it pins the whole read-buffer allocation (up to
/// ~512 KiB) while it lives. Apply-and-drop is fine; a payload kept beyond the current dispatch
/// (last snapshot, recorder queue) MUST be copied with [`TelemetryPayload::detach`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryPayload(Bytes);

impl TelemetryPayload {
    /// Wraps an encoded TLV block. Fails if it is shorter than the TLV header.
    pub fn new(bytes: Bytes) -> Result<Self, MessageError> {
        if bytes.len() < TLV_HEADER_BYTES {
            return Err(MessageError::TelemetryTooShort { len: bytes.len() });
        }
        Ok(Self(bytes))
    }

    /// Raw TLV bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Raw TLV bytes as a cheaply clonable `Bytes`. Like the payload itself, a clone shares (and
    /// pins) the underlying buffer; see [`TelemetryPayload::detach`].
    pub fn bytes(&self) -> &Bytes {
        &self.0
    }

    /// A copy of this payload in its own allocation of exactly its length, so keeping it does not
    /// pin the read buffer it was decoded from (§1).
    pub fn detach(&self) -> TelemetryPayload {
        TelemetryPayload(Bytes::copy_from_slice(&self.0))
    }

    /// Decodes the block (header plus records; unknown channel ids are returned for the caller to
    /// skip).
    pub fn decode(&self) -> Result<(TlvHeader, Vec<TlvRecord>), codec::CodecError> {
        codec::decode(&self.0)
    }
}

// ---------------------------------------------------------------------------------------------
// Message enum
// ---------------------------------------------------------------------------------------------

/// Every protocol message, typed. One variant per message type, except HELLO which has a client
/// and a server form.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Hello(Hello),
    ServerHello(ServerHello),
    Auth(Auth),
    AuthOk(AuthOk),
    AccountRegister(AccountRegister),
    SessionCreate(SessionCreate),
    SessionJoin(SessionJoin),
    SessionInfo(SessionInfo),
    SessionResume(SessionResume),
    SessionClose(SessionClose),
    ConnectCodeRequest(ConnectCodeRequest),
    ConnectCode(ConnectCode),
    EngineerList(EngineerList),
    EngineerKick(EngineerKick),
    SchemaNegotiate(SchemaNegotiate),
    TelemetryDelta(TelemetryPayload),
    TelemetrySnapshot(TelemetryPayload),
    ScoringUpdate(ScoringUpdate),
    WeatherUpdate(TelemetryPayload),
    SessionEvent(SessionEventMsg),
    SnapshotRequest(SnapshotRequest),
    DriverStatus(DriverStatus),
    RecordingStatus(RecordingStatus),
    Ping(Ping),
    Pong(Pong),
    Disconnect(Disconnect),
    Error(ErrorMsg),
}

/// Errors encoding or decoding message payloads.
#[derive(Debug, Error)]
pub enum MessageError {
    #[error("framing error")]
    Frame(#[from] FrameError),
    #[error("unknown message type {0:#04x}")]
    UnknownMsgType(u8),
    #[error("{msg_type} payload is zstd-compressed; protocol 1.0 does not use compression")]
    CompressedPayload { msg_type: MsgType },
    #[error("{msg_type} payload is not valid postcard")]
    Postcard {
        msg_type: MsgType,
        #[source]
        source: postcard::Error,
    },
    #[error("{msg_type} payload has {count} trailing bytes")]
    TrailingBytes { msg_type: MsgType, count: usize },
    #[error("telemetry payload of {len} bytes is shorter than the TLV header")]
    TelemetryTooShort { len: usize },
}

impl MessageError {
    /// Wire error code to report to the peer.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            MessageError::Frame(e) => e.error_code(),
            MessageError::UnknownMsgType(_)
            | MessageError::CompressedPayload { .. }
            | MessageError::Postcard { .. }
            | MessageError::TrailingBytes { .. }
            | MessageError::TelemetryTooShort { .. } => ErrorCode::Malformed,
        }
    }
}

/// Encodes a postcard value by appending it to `buf`.
fn put<T: Serialize>(msg_type: MsgType, value: &T, buf: &mut Vec<u8>) -> Result<(), MessageError> {
    postcard::to_io(value, buf)
        .map(|_| ())
        .map_err(|source| MessageError::Postcard { msg_type, source })
}

/// How a payload's end is treated, from the sender's protocol minor (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tail {
    /// Same minor: every byte must be consumed.
    Strict,
    /// The sender speaks a newer minor, which may only append bytes at the end of a payload:
    /// bytes after the fields this build knows are ignored.
    IgnoreAppended,
}

/// Decodes a postcard value. With [`Tail::Strict`] it must consume the whole payload.
fn take<'a, T: Deserialize<'a>>(
    msg_type: MsgType,
    payload: &'a [u8],
    tail: Tail,
) -> Result<T, MessageError> {
    let (value, rest) = postcard::take_from_bytes::<T>(payload)
        .map_err(|source| MessageError::Postcard { msg_type, source })?;
    if !rest.is_empty() && tail == Tail::Strict {
        return Err(MessageError::TrailingBytes {
            msg_type,
            count: rest.len(),
        });
    }
    Ok(value)
}

impl Message {
    /// Message type id of this message.
    pub const fn msg_type(&self) -> MsgType {
        match self {
            Message::Hello(_) | Message::ServerHello(_) => MsgType::Hello,
            Message::Auth(_) => MsgType::Auth,
            Message::AuthOk(_) => MsgType::AuthOk,
            Message::AccountRegister(_) => MsgType::AccountRegister,
            Message::SessionCreate(_) => MsgType::SessionCreate,
            Message::SessionJoin(_) => MsgType::SessionJoin,
            Message::SessionInfo(_) => MsgType::SessionInfo,
            Message::SessionResume(_) => MsgType::SessionResume,
            Message::SessionClose(_) => MsgType::SessionClose,
            Message::ConnectCodeRequest(_) => MsgType::ConnectCodeRequest,
            Message::ConnectCode(_) => MsgType::ConnectCode,
            Message::EngineerList(_) => MsgType::EngineerList,
            Message::EngineerKick(_) => MsgType::EngineerKick,
            Message::SchemaNegotiate(_) => MsgType::SchemaNegotiate,
            Message::TelemetryDelta(_) => MsgType::TelemetryDelta,
            Message::TelemetrySnapshot(_) => MsgType::TelemetrySnapshot,
            Message::ScoringUpdate(_) => MsgType::ScoringUpdate,
            Message::WeatherUpdate(_) => MsgType::WeatherUpdate,
            Message::SessionEvent(_) => MsgType::SessionEvent,
            Message::SnapshotRequest(_) => MsgType::SnapshotRequest,
            Message::DriverStatus(_) => MsgType::DriverStatus,
            Message::RecordingStatus(_) => MsgType::RecordingStatus,
            Message::Ping(_) => MsgType::Ping,
            Message::Pong(_) => MsgType::Pong,
            Message::Disconnect(_) => MsgType::Disconnect,
            Message::Error(_) => MsgType::Error,
        }
    }

    /// Whether a driver app processes this message when it arrives from the relay (engineer-direction
    /// whitelist, §3). See [`crate::frame::driver_accepts`].
    pub const fn driver_accepts(&self) -> bool {
        match self {
            // Same msg type id as a client HELLO, but only the relay's form is meaningful to a driver.
            Message::Hello(_) => false,
            other => crate::frame::driver_accepts(other.msg_type()),
        }
    }

    /// Appends the payload encoding of this message to `buf`.
    pub fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), MessageError> {
        let t = self.msg_type();
        match self {
            Message::Hello(m) => put(t, &HelloPayloadRef::Client(m), buf),
            Message::ServerHello(m) => put(t, &HelloPayloadRef::Server(m), buf),
            Message::Auth(m) => put(t, m, buf),
            Message::AuthOk(m) => put(t, m, buf),
            Message::AccountRegister(m) => put(t, m, buf),
            Message::SessionCreate(m) => put(t, m, buf),
            Message::SessionJoin(m) => put(t, m, buf),
            Message::SessionInfo(m) => put(t, m, buf),
            Message::SessionResume(m) => put(t, m, buf),
            Message::SessionClose(m) => put(t, m, buf),
            Message::ConnectCodeRequest(m) => put(t, m, buf),
            Message::ConnectCode(m) => put(t, m, buf),
            Message::EngineerList(m) => put(t, m, buf),
            Message::EngineerKick(m) => put(t, m, buf),
            Message::SchemaNegotiate(m) => put(t, m, buf),
            Message::TelemetryDelta(p)
            | Message::TelemetrySnapshot(p)
            | Message::WeatherUpdate(p) => {
                buf.extend_from_slice(p.as_bytes());
                Ok(())
            }
            Message::ScoringUpdate(m) => put(t, m, buf),
            Message::SessionEvent(m) => put(t, m, buf),
            Message::SnapshotRequest(m) => put(t, m, buf),
            Message::DriverStatus(m) => put(t, m, buf),
            Message::RecordingStatus(m) => put(t, m, buf),
            Message::Ping(m) => put(t, m, buf),
            Message::Pong(m) => put(t, m, buf),
            Message::Disconnect(m) => put(t, m, buf),
            Message::Error(m) => put(t, m, buf),
        }
    }

    /// Encodes this message into a complete frame. `meta` must suit the type (control frames:
    /// [`FrameMeta::control`]; stream frames: [`FrameMeta::stream`]); violations are refused.
    pub fn to_frame(&self, meta: &FrameMeta) -> Result<Frame, MessageError> {
        let hint = match self {
            Message::TelemetryDelta(p)
            | Message::TelemetrySnapshot(p)
            | Message::WeatherUpdate(p) => p.as_bytes().len(),
            _ => CONTROL_PAYLOAD_CAPACITY_HINT,
        };
        Frame::encode_with(self.msg_type(), meta, Flags::empty(), hint, |buf| {
            self.encode_payload(buf)
        })
    }

    /// Decodes the payload of a message of type `msg_type` sent by a peer of this build's minor
    /// ([`PROTOCOL_MINOR`]): strict, every byte must be consumed. Use [`Message::from_frame`] or
    /// [`Message::decode_from_minor`] for payloads of a peer whose minor may differ.
    pub fn decode(msg_type: MsgType, payload: Bytes) -> Result<Message, MessageError> {
        Message::decode_from_minor(msg_type, payload, PROTOCOL_MINOR)
    }

    /// Decodes the payload of a message of type `msg_type` sent by a peer speaking protocol minor
    /// `sender_minor` (the frame header's `minor`, §10):
    /// * `sender_minor == PROTOCOL_MINOR`: strict, trailing bytes are [`MessageError::TrailingBytes`];
    /// * `sender_minor > PROTOCOL_MINOR`: a newer minor may only append bytes at the end of a payload,
    ///   so bytes after the fields this build knows are ignored;
    /// * `sender_minor < PROTOCOL_MINOR`: decoded with that minor's layout. 1.0 is the first minor,
    ///   so no older layout exists yet; every minor that appends fields must add its per-minor path
    ///   here.
    ///
    /// TLV payloads are only checked for a complete TLV header here (zero-copy); decode them with
    /// [`TelemetryPayload::decode`].
    pub fn decode_from_minor(
        msg_type: MsgType,
        payload: Bytes,
        sender_minor: u8,
    ) -> Result<Message, MessageError> {
        let tail = if sender_minor > PROTOCOL_MINOR {
            Tail::IgnoreAppended
        } else {
            Tail::Strict
        };
        let t = msg_type;
        let p = &payload[..];
        Ok(match msg_type {
            MsgType::Hello => match take::<HelloPayload>(t, p, tail)? {
                HelloPayload::Client(m) => Message::Hello(m),
                HelloPayload::Server(m) => Message::ServerHello(m),
            },
            MsgType::Auth => Message::Auth(take(t, p, tail)?),
            MsgType::AuthOk => Message::AuthOk(take(t, p, tail)?),
            MsgType::AccountRegister => Message::AccountRegister(take(t, p, tail)?),
            MsgType::SessionCreate => Message::SessionCreate(take(t, p, tail)?),
            MsgType::SessionJoin => Message::SessionJoin(take(t, p, tail)?),
            MsgType::SessionInfo => Message::SessionInfo(take(t, p, tail)?),
            MsgType::SessionResume => Message::SessionResume(take(t, p, tail)?),
            MsgType::SessionClose => Message::SessionClose(take(t, p, tail)?),
            MsgType::ConnectCodeRequest => Message::ConnectCodeRequest(take(t, p, tail)?),
            MsgType::ConnectCode => Message::ConnectCode(take(t, p, tail)?),
            MsgType::EngineerList => Message::EngineerList(take(t, p, tail)?),
            MsgType::EngineerKick => Message::EngineerKick(take(t, p, tail)?),
            MsgType::SchemaNegotiate => Message::SchemaNegotiate(take(t, p, tail)?),
            MsgType::TelemetryDelta => Message::TelemetryDelta(TelemetryPayload::new(payload)?),
            MsgType::TelemetrySnapshot => {
                Message::TelemetrySnapshot(TelemetryPayload::new(payload)?)
            }
            MsgType::ScoringUpdate => Message::ScoringUpdate(take(t, p, tail)?),
            MsgType::WeatherUpdate => Message::WeatherUpdate(TelemetryPayload::new(payload)?),
            MsgType::SessionEvent => Message::SessionEvent(take(t, p, tail)?),
            MsgType::SnapshotRequest => Message::SnapshotRequest(take(t, p, tail)?),
            MsgType::DriverStatus => Message::DriverStatus(take(t, p, tail)?),
            MsgType::RecordingStatus => Message::RecordingStatus(take(t, p, tail)?),
            MsgType::Ping => Message::Ping(take(t, p, tail)?),
            MsgType::Pong => Message::Pong(take(t, p, tail)?),
            MsgType::Disconnect => Message::Disconnect(take(t, p, tail)?),
            MsgType::Error => Message::Error(take(t, p, tail)?),
        })
    }

    /// Decodes the message carried by a validated frame. Unknown message types and compressed
    /// payloads are refused. The payload is decoded for the sender's minor carried in the header
    /// ([`Message::decode_from_minor`]). Header rules ([`Frame::check_rules`]) are the caller's
    /// responsibility (a rule violation is not fatal, see `frame`).
    pub fn from_frame(frame: &Frame) -> Result<Message, MessageError> {
        let id = frame.header().msg_type;
        let msg_type = MsgType::from_id(id).ok_or(MessageError::UnknownMsgType(id))?;
        if frame.header().flags.contains(Flags::PAYLOAD_ZSTD) {
            return Err(MessageError::CompressedPayload { msg_type });
        }
        Message::decode_from_minor(msg_type, frame.payload_bytes(), frame.header().minor)
    }
}

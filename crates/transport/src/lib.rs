//! Client-side relay networking for the driver and engineer modes (docs/ARCHITECTURE.md §5.1, §7;
//! docs/PROTOCOL.md; D-004, D-008, D-013, D-016, D-019, D-020, D-022).
//!
//! * [`transport`]: the [`TelemetryTransport`] abstraction (connect / send / recv / close over
//!   whole frames) and [`TcpTlsTelemetryTransport`] (TCP + TLS 1.3, ALPN `lmure/1`, webpki roots,
//!   development-only extra CA, no way to disable certificate validation). Outbound only: no
//!   listening socket, admin rights or firewall rule (D-019).
//! * [`state`]: [`ConnectionState`], the single definition of the connection state machine,
//!   published with details as [`ConnectionStatus`] through `tokio::sync::watch`.
//! * [`driver`] / [`engineer`]: the session clients. HELLO / AUTH / SESSION_* handshake, heartbeat
//!   PINGs with clock sync, DEGRADED / STALE / timeout detection, reconnect with exponential
//!   backoff and jitter, silent re-authentication, SESSION_RESUME (driver token or the engineer's
//!   in-memory ticket), snapshot hooks after reconnects, and the D-022 connection-code API.
//! * [`queue`]: bounded outbound queues (reliable FIFO + latest-wins slots per
//!   `(msg_type, priority)`, snapshots never superseded by deltas).
//! * [`auth`] / [`credentials`]: login once, tokens in Windows Credential Manager
//!   ([`KeyringCredentialStore`]), silent re-authentication, clear re-login state; [`Redacted`]
//!   keeps secrets out of logs.
//! * [`connectivity`]: the staged DNS → TCP → TLS → HELLO → AUTH → session service → RTT test.
//! * [`metrics`]: bytes/s, frames/s, RTT, jitter, clock offset, reconnects, dropped_obsolete and
//!   sequence statistics.
//! * [`endpoint`]: [`RelayEndpoint`] (host, port, region label, environment); nothing is
//!   hard-coded, and a relay announcing another environment is refused before AUTH.

pub mod auth;
pub mod backoff;
pub mod clock;
pub mod config;
pub mod connectivity;
pub mod credentials;
pub mod driver;
pub mod endpoint;
pub mod engineer;
pub mod error;
pub mod handshake;
pub mod liveness;
pub mod metrics;
pub mod queue;
pub mod redact;
mod session;
pub mod state;
pub mod tls;
pub mod transport;

pub use auth::{AccountInfo, AuthManager, LoginError, login, register};
pub use config::{ClientConfig, ConfigError, TimingConfig};
pub use connectivity::{
    ConnectivityReport, RttSummary, Stage, StageOutcome, StageResult, test_relay_connection,
};
pub use credentials::{
    CredentialError, CredentialKind, CredentialStore, KeyringCredentialStore, MemoryCredentialStore,
};
pub use driver::{
    ClientClosed, DriverClient, DriverEvent, SessionEndOutcome, SessionReadyKind, SnapshotTrigger,
    code_expires_in, display_connect_code,
};
pub use endpoint::{EndpointError, Environment, ExtraCa, RelayEndpoint};
pub use engineer::{
    EngineerClient, EngineerError, EngineerEvent, EngineerPhase, IdentityCard, InboundFrame,
    JoinError, StreamGap,
};
pub use error::{ConnectError, ConnectStage, TlsConfigError, TransportError};
pub use handshake::HandshakeError;
pub use liveness::{LinkHealth, LivenessConfig};
pub use metrics::{LinkMetrics, SequenceTotals};
pub use queue::{LiveOutcome, Outbound, OutboundQueue, QueueError};
pub use redact::Redacted;
pub use state::{
    ConnectionState, ConnectionStatus, DegradedReason, EngineerEndReason, FailureReason,
    ReloginCause,
};
pub use transport::{
    ConnectTimeouts, ConnectionInfo, FrameReader, FrameWriter, StreamFrameReader,
    StreamFrameWriter, TcpTlsTelemetryTransport, TelemetryTransport, TransportConnection,
    TransportStats, framed_split,
};

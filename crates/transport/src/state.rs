//! The connection state machine (docs/ARCHITECTURE.md §5.1). [`ConnectionState`] is the **single
//! definition** used by the transport, app-core and the UI bridge; it is published through a
//! `tokio::sync::watch` channel as part of [`ConnectionStatus`].
//!
//! | State          | Entered when                                                                 |
//! |----------------|------------------------------------------------------------------------------|
//! | DISCONNECTED   | initial; user stopped; or FAILED acknowledged                                 |
//! | CONNECTING     | DNS/TCP/TLS connect in progress                                               |
//! | AUTHENTICATING | TLS up, HELLO/AUTH (and the SESSION_* binding) exchange in progress           |
//! | CONNECTED      | AUTH_OK + session joined/created/resumed; heartbeats healthy                  |
//! | DEGRADED       | CONNECTED but RTT above threshold, loss above threshold or a missed heartbeat |
//! | STALE          | no inbound frame for longer than the stale threshold while the socket is open |
//! | RECONNECTING   | socket error, heartbeat timeout or relay close; backoff running               |
//! | FAILED         | non-retryable: re-login required, incompatible, wrong environment, session ended |
//!
//! Allowed transitions ([`ConnectionState::can_transition_to`]):
//!
//! ```text
//! DISCONNECTED   → CONNECTING
//! CONNECTING     → AUTHENTICATING | RECONNECTING | FAILED | DISCONNECTED
//! AUTHENTICATING → CONNECTED | RECONNECTING | FAILED | DISCONNECTED
//! CONNECTED      → DEGRADED | STALE | RECONNECTING | FAILED | DISCONNECTED
//! DEGRADED       → CONNECTED | STALE | RECONNECTING | FAILED | DISCONNECTED
//! STALE          → CONNECTED | DEGRADED | RECONNECTING | FAILED | DISCONNECTED
//! RECONNECTING   → CONNECTING | FAILED | DISCONNECTED
//! FAILED         → DISCONNECTED   (acknowledged, or a new user action such as a new code/login)
//! ```
//!
//! Telemetry *freshness* on the engineer UI (LIVE/DEGRADED/STALE/NO DATA) is a separate concept,
//! computed from sample age, not from this socket state (spec §248).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use protocol::messages::ClientRole;
use protocol::version::Environment;
use tokio::sync::watch;

/// Connection state (ARCHITECTURE.md §5.1). The `u8` values are stable (they are the value of the
/// `net.connection_state` channel, 0x0D20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum ConnectionState {
    #[default]
    Disconnected = 0,
    Connecting = 1,
    Authenticating = 2,
    Connected = 3,
    Degraded = 4,
    Stale = 5,
    Reconnecting = 6,
    Failed = 7,
}

impl ConnectionState {
    /// Every state, in `u8` order.
    pub const ALL: [ConnectionState; 8] = [
        ConnectionState::Disconnected,
        ConnectionState::Connecting,
        ConnectionState::Authenticating,
        ConnectionState::Connected,
        ConnectionState::Degraded,
        ConnectionState::Stale,
        ConnectionState::Reconnecting,
        ConnectionState::Failed,
    ];

    /// Stable numeric value.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// State from its numeric value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ConnectionState::Disconnected),
            1 => Some(ConnectionState::Connecting),
            2 => Some(ConnectionState::Authenticating),
            3 => Some(ConnectionState::Connected),
            4 => Some(ConnectionState::Degraded),
            5 => Some(ConnectionState::Stale),
            6 => Some(ConnectionState::Reconnecting),
            7 => Some(ConnectionState::Failed),
            _ => None,
        }
    }

    /// Upper-case name as written in ARCHITECTURE.md.
    pub const fn name(self) -> &'static str {
        match self {
            ConnectionState::Disconnected => "DISCONNECTED",
            ConnectionState::Connecting => "CONNECTING",
            ConnectionState::Authenticating => "AUTHENTICATING",
            ConnectionState::Connected => "CONNECTED",
            ConnectionState::Degraded => "DEGRADED",
            ConnectionState::Stale => "STALE",
            ConnectionState::Reconnecting => "RECONNECTING",
            ConnectionState::Failed => "FAILED",
        }
    }

    /// Whether a session is bound and the socket is open (CONNECTED, DEGRADED or STALE).
    pub const fn is_session_up(self) -> bool {
        matches!(
            self,
            ConnectionState::Connected | ConnectionState::Degraded | ConnectionState::Stale
        )
    }

    /// The transition table of the module documentation.
    pub const fn can_transition_to(self, next: ConnectionState) -> bool {
        use ConnectionState as S;
        match self {
            S::Disconnected => matches!(next, S::Connecting),
            S::Connecting => matches!(
                next,
                S::Authenticating | S::Reconnecting | S::Failed | S::Disconnected
            ),
            S::Authenticating => matches!(
                next,
                S::Connected | S::Reconnecting | S::Failed | S::Disconnected
            ),
            S::Connected => matches!(
                next,
                S::Degraded | S::Stale | S::Reconnecting | S::Failed | S::Disconnected
            ),
            S::Degraded => matches!(
                next,
                S::Connected | S::Stale | S::Reconnecting | S::Failed | S::Disconnected
            ),
            S::Stale => matches!(
                next,
                S::Connected | S::Degraded | S::Reconnecting | S::Failed | S::Disconnected
            ),
            S::Reconnecting => matches!(next, S::Connecting | S::Failed | S::Disconnected),
            S::Failed => matches!(next, S::Disconnected),
        }
    }
}

impl fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why the link is DEGRADED.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradedReason {
    /// No PONG for longer than the degraded threshold (a heartbeat was missed).
    MissedHeartbeat,
    /// Smoothed round-trip time above the threshold.
    HighRtt,
    /// Sequence loss above the threshold (engineer stream).
    PacketLoss,
    /// The driver's connection dropped and the session is in grace (H1).
    DriverAway,
    /// No telemetry stream frame received for longer than the stale threshold while connected (H1).
    NoStreamData,
    /// Stream frames are held awaiting SCHEMA_NEGOTIATE (H2).
    AwaitingSchema,
    /// The relay reported non-increasing stream sequence numbers (M8).
    SequenceDesync,
}

/// Why the user has to log in again. Shown as a clear re-login state, never as a crash or a silent
/// retry loop (D-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReloginCause {
    /// No refresh token is stored (first start, or after logout).
    NotLoggedIn,
    /// The relay rejected the stored refresh token (revoked or unknown).
    RefreshTokenRejected,
    /// The relay reported the refresh token as expired.
    RefreshTokenExpired,
}

/// Why an engineer's connection to a driver ended for good. After any of these only a new
/// connection code from the driver connects again (D-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineerEndReason {
    /// The in-memory resume ticket was rejected or expired after a network drop.
    TicketExpired,
    /// The driver disconnected this engineer.
    Kicked,
    /// The driver ended the session.
    DriverEndedSession,
    /// The driver did not come back within the session grace period.
    SessionExpired,
    /// The same engineer account joined this session from another connection.
    Replaced,
    /// The user disconnected (or cancelled the identity confirmation).
    UserDisconnected,
    /// The login is no longer valid; after logging in again a new code is needed.
    ReloginRequired,
    /// The relay refused this client's protocol version (`ERROR(INCOMPATIBLE)`): an update is
    /// required; retrying cannot succeed.
    Incompatible,
    /// Local configuration error (e.g. TLS configuration cannot be built).
    Configuration,
    /// The relay environment does not match this client.
    EnvironmentMismatch,
    /// The account is not authorized as an engineer.
    NotAnEngineer,
}

impl EngineerEndReason {
    /// User-facing explanation.
    pub const fn message(self) -> &'static str {
        match self {
            EngineerEndReason::TicketExpired => {
                "Connection ended. Enter a new code from the driver."
            }
            EngineerEndReason::Kicked => "The driver disconnected you.",
            EngineerEndReason::DriverEndedSession => "The driver ended the session.",
            EngineerEndReason::SessionExpired => "The driver's session expired.",
            EngineerEndReason::Replaced => "Your account connected to this driver from elsewhere.",
            EngineerEndReason::UserDisconnected => "Disconnected.",
            EngineerEndReason::ReloginRequired => {
                "Your login is no longer valid. Log in, then enter a new code from the driver."
            }
            EngineerEndReason::Incompatible => {
                "This app version is not compatible with the relay. Update the app, then enter a new code from the driver."
            }
            EngineerEndReason::Configuration => {
                "Configuration error. Please check your network and TLS settings."
            }
            EngineerEndReason::EnvironmentMismatch => {
                "The relay environment does not match this app version."
            }
            EngineerEndReason::NotAnEngineer => "This account is not authorized as an engineer.",
        }
    }
}

/// Why the state is FAILED (non-retryable without a user action).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureReason {
    /// Log in again (refresh token missing, revoked or expired).
    ReloginRequired(ReloginCause),
    /// The relay speaks an incompatible protocol version.
    Incompatible { detail: String },
    /// The relay announced another environment than this client is configured for.
    EnvironmentMismatch {
        expected: Environment,
        announced: Environment,
    },
    /// The account is not allowed to connect in this role (e.g. "not an engineer account").
    RoleMismatch {
        requested: ClientRole,
        granted: ClientRole,
    },
    /// Another app instance with the same account replaced this connection.
    Replaced,
    /// The engineer's connection to the driver ended (D-022).
    SessionEnded(EngineerEndReason),
    /// Local configuration error (e.g. the TLS configuration cannot be built).
    Configuration { detail: String },
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureReason::ReloginRequired(ReloginCause::NotLoggedIn) => {
                f.write_str("Not logged in. Please log in.")
            }
            FailureReason::ReloginRequired(ReloginCause::RefreshTokenRejected) => {
                f.write_str("Your login is no longer valid. Please log in again.")
            }
            FailureReason::ReloginRequired(ReloginCause::RefreshTokenExpired) => {
                f.write_str("Your login expired. Please log in again.")
            }
            FailureReason::Incompatible { detail } => {
                write!(f, "Relay is incompatible with this app version: {detail}")
            }
            FailureReason::EnvironmentMismatch {
                expected,
                announced,
            } => write!(
                f,
                "Relay environment {announced:?} does not match this app ({expected:?})"
            ),
            FailureReason::RoleMismatch {
                requested: ClientRole::Engineer,
                ..
            } => f.write_str("This account is not an engineer account."),
            FailureReason::RoleMismatch {
                requested: ClientRole::Driver,
                ..
            } => f.write_str("This account is not a driver account."),
            FailureReason::Replaced => {
                f.write_str("This account connected from another app instance.")
            }
            FailureReason::SessionEnded(reason) => f.write_str(reason.message()),
            FailureReason::Configuration { detail } => write!(f, "Configuration error: {detail}"),
        }
    }
}

/// Everything the UI needs to show about the connection. Published through a `watch` channel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    /// Set in FAILED.
    pub failure: Option<FailureReason>,
    /// Set in DEGRADED.
    pub degraded: Option<DegradedReason>,
    /// Whether the driver's connection is detached/away from the session (H1).
    pub driver_away: bool,
    /// Set when a credential or session token failed to be persisted (H4).
    pub persist_error: Option<String>,
    /// Whether a security issue (e.g. untrusted TLS certificate or ALPN failure) was encountered (M1).
    pub security_issue: bool,
    /// The most recent transient error (shown while RECONNECTING), never containing secrets.
    pub last_error: Option<String>,
    /// Reconnect attempts since the connection was last healthy.
    pub reconnect_attempt: u32,
    /// Backoff before the next attempt (RECONNECTING).
    pub retry_in: Option<Duration>,
    /// The bound session, if any.
    pub session_id: Option<u64>,
    /// Number of state transitions so far (lets observers detect missed intermediate states).
    pub transitions: u64,
}

/// Owner side of the published status. Enforces the transition table: an invalid transition is a
/// bug, logged and refused.
#[derive(Clone, Debug)]
pub struct StatePublisher {
    tx: Arc<watch::Sender<ConnectionStatus>>,
    component: &'static str,
}

impl StatePublisher {
    /// New publisher in DISCONNECTED and its first receiver.
    pub fn new(component: &'static str) -> (Self, watch::Receiver<ConnectionStatus>) {
        let (tx, rx) = watch::channel(ConnectionStatus::default());
        (
            Self {
                tx: Arc::new(tx),
                component,
            },
            rx,
        )
    }

    /// Another receiver.
    pub fn subscribe(&self) -> watch::Receiver<ConnectionStatus> {
        self.tx.subscribe()
    }

    /// Current state.
    pub fn state(&self) -> ConnectionState {
        self.tx.borrow().state
    }

    /// Current status.
    pub fn status(&self) -> ConnectionStatus {
        self.tx.borrow().clone()
    }

    /// Moves to `next` (if allowed), letting `update` adjust the other fields. Returns whether
    /// the transition happened. Re-entering the current state only applies `update`.
    pub fn transition(
        &self,
        next: ConnectionState,
        update: impl FnOnce(&mut ConnectionStatus),
    ) -> bool {
        let component = self.component;
        let mut changed = false;
        self.tx.send_if_modified(|status| {
            let current = status.state;
            if current != next && !current.can_transition_to(next) {
                tracing::error!(
                    component,
                    from = current.name(),
                    to = next.name(),
                    "invalid connection state transition refused"
                );
                return false;
            }
            let before = status.clone();
            if current != next {
                status.state = next;
                status.transitions = status.transitions.saturating_add(1);
                tracing::info!(
                    component,
                    from = current.name(),
                    to = next.name(),
                    "connection state"
                );
                changed = true;
            }
            if next != ConnectionState::Failed {
                status.failure = None;
            }
            if next != ConnectionState::Degraded {
                status.degraded = None;
            }
            if next != ConnectionState::Reconnecting {
                status.retry_in = None;
            }
            update(status);
            *status != before
        });
        changed
    }

    /// Changes fields without a state transition.
    pub fn update(&self, update: impl FnOnce(&mut ConnectionStatus)) {
        self.tx.send_if_modified(|status| {
            let before = status.clone();
            update(status);
            *status != before
        });
    }

    /// Moves to FAILED with `reason`, going through the states the table requires.
    pub fn fail(&self, reason: FailureReason) {
        let reason_text = reason.to_string();
        if !self.transition(ConnectionState::Failed, |s| {
            s.failure = Some(reason.clone());
            s.session_id = None;
        }) && self.state() != ConnectionState::Failed
        {
            // Only DISCONNECTED cannot go to FAILED directly; pass through CONNECTING (the
            // failure was found while preparing a connection).
            self.transition(ConnectionState::Connecting, |_| {});
            self.transition(ConnectionState::Failed, |s| {
                s.failure = Some(reason);
                s.session_id = None;
            });
        }
        tracing::warn!(component = self.component, reason = %reason_text, "connection failed");
    }

    /// Moves to DISCONNECTED (from any state).
    pub fn disconnected(&self) {
        self.transition(ConnectionState::Disconnected, |s| {
            s.session_id = None;
            s.reconnect_attempt = 0;
            s.driver_away = false;
            s.security_issue = false;
        });
    }

    /// A new user action (start streaming, join with a new code, after a login) acknowledges
    /// FAILED: FAILED → DISCONNECTED, so the next connection can go DISCONNECTED → CONNECTING. No
    /// effect in any other state.
    pub fn acknowledge_failure(&self) {
        if self.state() == ConnectionState::Failed {
            self.disconnected();
        }
    }

    /// Updates whether the driver is detached/away (H1).
    pub fn driver_away(&self, away: bool) {
        self.update(|s| s.driver_away = away);
    }

    /// Sets whether a security issue occurred (M1).
    pub fn security_issue(&self) {
        self.update(|s| s.security_issue = true);
    }

    /// Sets or clears a credential/token persist error (H4).
    pub fn persist_error(&self, error: Option<String>) {
        self.update(|s| s.persist_error = error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ConnectionState as S;

    #[test]
    fn engineer_end_messages_have_no_runs_of_spaces() {
        use EngineerEndReason as E;
        for reason in [
            E::TicketExpired,
            E::Kicked,
            E::DriverEndedSession,
            E::SessionExpired,
            E::Replaced,
            E::UserDisconnected,
            E::ReloginRequired,
            E::Incompatible,
            E::Configuration,
            E::EnvironmentMismatch,
            E::NotAnEngineer,
        ] {
            let text = reason.message();
            assert!(!text.contains("  "), "{reason:?}: {text:?}");
            assert_eq!(text.trim(), text, "{reason:?}");
        }
    }

    #[test]
    fn numeric_values_round_trip() {
        for s in S::ALL {
            assert_eq!(S::from_u8(s.as_u8()), Some(s));
        }
        assert_eq!(S::from_u8(8), None);
        assert_eq!(S::Failed.as_u8(), 7);
    }

    #[test]
    fn transition_table_matches_the_architecture() {
        // Every state can be stopped by the user, except DISCONNECTED itself.
        for s in S::ALL {
            if s != S::Disconnected {
                assert!(s.can_transition_to(S::Disconnected), "{s} → DISCONNECTED");
            }
        }
        assert!(S::Disconnected.can_transition_to(S::Connecting));
        assert!(!S::Disconnected.can_transition_to(S::Connected));
        assert!(S::Connecting.can_transition_to(S::Authenticating));
        assert!(!S::Connecting.can_transition_to(S::Connected));
        assert!(S::Authenticating.can_transition_to(S::Connected));
        assert!(S::Connected.can_transition_to(S::Degraded));
        assert!(S::Degraded.can_transition_to(S::Stale));
        assert!(S::Stale.can_transition_to(S::Reconnecting));
        assert!(S::Reconnecting.can_transition_to(S::Connecting));
        assert!(!S::Reconnecting.can_transition_to(S::Connected));
        assert!(!S::Failed.can_transition_to(S::Connecting));
        assert!(S::Failed.can_transition_to(S::Disconnected));
    }

    #[test]
    fn publisher_refuses_invalid_transitions_and_counts_valid_ones() {
        let (p, rx) = StatePublisher::new("test");
        assert!(!p.transition(S::Connected, |_| {}));
        assert_eq!(rx.borrow().state, S::Disconnected);
        assert!(p.transition(S::Connecting, |_| {}));
        assert!(p.transition(S::Authenticating, |_| {}));
        assert!(p.transition(S::Connected, |s| s.session_id = Some(5)));
        assert!(p.transition(S::Degraded, |s| s.degraded = Some(DegradedReason::HighRtt)));
        assert_eq!(rx.borrow().degraded, Some(DegradedReason::HighRtt));
        assert!(p.transition(S::Connected, |_| {}));
        assert_eq!(rx.borrow().degraded, None, "cleared when leaving DEGRADED");
        assert_eq!(rx.borrow().transitions, 5);
        p.fail(FailureReason::Replaced);
        assert_eq!(rx.borrow().state, S::Failed);
        assert_eq!(rx.borrow().failure, Some(FailureReason::Replaced));
        assert_eq!(rx.borrow().session_id, None);
        p.disconnected();
        assert_eq!(rx.borrow().state, S::Disconnected);
        assert_eq!(rx.borrow().failure, None);
    }

    #[test]
    fn a_new_user_action_acknowledges_failed() {
        let (p, rx) = StatePublisher::new("test");
        p.fail(FailureReason::Replaced);
        assert_eq!(rx.borrow().state, S::Failed);
        // FAILED cannot go to CONNECTING directly ...
        assert!(!p.transition(S::Connecting, |_| {}));
        // ... a new user action acknowledges it first.
        p.acknowledge_failure();
        assert_eq!(rx.borrow().state, S::Disconnected);
        assert!(p.transition(S::Connecting, |_| {}));
        // No effect outside FAILED.
        p.acknowledge_failure();
        assert_eq!(rx.borrow().state, S::Connecting);
    }

    #[test]
    fn fail_from_disconnected_passes_through_connecting() {
        let (p, rx) = StatePublisher::new("test");
        p.fail(FailureReason::ReloginRequired(ReloginCause::NotLoggedIn));
        assert_eq!(rx.borrow().state, S::Failed);
        assert_eq!(rx.borrow().transitions, 2);
    }

    #[test]
    fn failure_messages_are_user_facing() {
        let msg = FailureReason::RoleMismatch {
            requested: ClientRole::Engineer,
            granted: ClientRole::Driver,
        }
        .to_string();
        assert_eq!(msg, "This account is not an engineer account.");
        assert!(
            FailureReason::SessionEnded(EngineerEndReason::TicketExpired)
                .to_string()
                .contains("new code")
        );
    }
}

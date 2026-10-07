//! The sequential part of a connection (PROTOCOL.md §6): HELLO exchange with version and
//! environment checks, then AUTH. Runs on the freshly connected frame stream before the writer task
//! and the receive loop start, with a timeout on every reply.
//!
//! Order matters for security: the relay's environment is checked on its HELLO **before** any
//! credential is sent, so a client configured for production never offers its refresh token to a
//! development or staging relay.

use std::sync::Arc;
use std::time::Duration;

use protocol::frame::{FrameMeta, MAX_PAYLOAD_BYTES, MsgType, NO_SESSION};
use protocol::messages::{
    ClientRole, Disconnect, DisconnectReason, ErrorCode, ErrorMsg, Hello, Message, MessageError,
    ServerHello, ServerLimits,
};
use protocol::version::{Environment, Incompatible, ProtocolVersion, negotiate_protocol};
use protocol::{Frame, HeaderRuleError};
use thiserror::Error;

use crate::auth::{AccountInfo, AuthManager, AuthPlan, ReloginNeeded};
use crate::clock::LinkClock;
use crate::config::ClientConfig;
use crate::credentials::CredentialError;
use crate::error::TransportError;
use crate::state::{FailureReason, ReloginCause};
use crate::transport::{ConnectionInfo, FrameReader, FrameWriter, TransportStats};

/// Oldest relay protocol minor this client accepts (1.0 is the first).
const MIN_RELAY_MINOR: u8 = 0;
/// Frames received during the handshake that belong to the session phase and are kept for the
/// receive loop (e.g. CONNECT_CODE right after SESSION_INFO). More is a protocol violation.
pub(crate) const MAX_EARLY_FRAMES: usize = 32;
/// Smallest payload limit a relay may announce: a SESSION_CREATE or SCHEMA_NEGOTIATE must fit (the
/// relay refuses to start with less, `relay::config::MIN_PAYLOAD_LIMIT`). Below it nothing useful
/// could be streamed, so the relay is treated as incompatible instead of silently dropping frames.
pub const MIN_RELAY_PAYLOAD_LIMIT: usize = 16 * 1024;

/// Failure of the handshake or of the session binding that follows it.
#[derive(Debug, Error)]
pub enum HandshakeError {
    #[error("connection error during the handshake")]
    Transport(#[from] TransportError),
    #[error("the relay closed the connection during the handshake")]
    Closed,
    #[error("no reply to {stage} within {timeout:?}")]
    Timeout {
        stage: &'static str,
        timeout: Duration,
    },
    #[error("invalid {msg_type} from the relay")]
    Decode {
        msg_type: MsgType,
        #[source]
        source: MessageError,
    },
    #[error("cannot encode {msg_type}")]
    Encode {
        msg_type: MsgType,
        #[source]
        source: MessageError,
    },
    #[error("the relay sent {got} while {stage} was expected")]
    Unexpected { stage: &'static str, got: MsgType },
    #[error("the relay refused {stage}: {code:?} ({message})")]
    Refused {
        stage: &'static str,
        code: ErrorCode,
        message: String,
        fatal: bool,
    },
    #[error("the relay disconnected during {stage}: {reason:?}")]
    Disconnected {
        stage: &'static str,
        reason: DisconnectReason,
    },
    #[error("protocol versions are incompatible")]
    Incompatible(#[from] Incompatible),
    #[error(
        "the relay accepts payloads of at most {announced} bytes; this client needs at least {minimum}"
    )]
    PayloadLimitTooSmall { announced: u32, minimum: usize },
    #[error(
        "the relay announced environment {announced:?}, this client is configured for {expected:?}"
    )]
    EnvironmentMismatch {
        expected: Environment,
        announced: Environment,
    },
    #[error("log in required")]
    Relogin(ReloginCause),
    #[error("the relay granted role {granted:?}, {requested:?} was requested")]
    RoleMismatch {
        requested: ClientRole,
        granted: ClientRole,
    },
    /// The in-memory access token was refused; the next attempt uses the refresh token.
    #[error("the access token was refused")]
    AccessTokenRefused,
    #[error("credential store error")]
    Credential(#[from] CredentialError),
    /// The task running AUTH (detached from the connection attempt) panicked or was cancelled.
    #[error("the authentication task failed")]
    AuthTask {
        #[source]
        source: tokio::task::JoinError,
    },
}

/// What the supervisor does after a failed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// Retry after the backoff.
    Retry,
    /// Stop: FAILED with this reason.
    Fail(FailureReason),
}

impl HandshakeError {
    /// Retry or fail.
    pub(crate) fn disposition(&self) -> Disposition {
        match self {
            HandshakeError::Incompatible(e) => Disposition::Fail(FailureReason::Incompatible {
                detail: e.to_string(),
            }),
            HandshakeError::PayloadLimitTooSmall { .. } => {
                Disposition::Fail(FailureReason::Incompatible {
                    detail: self.to_string(),
                })
            }
            HandshakeError::Refused {
                code: ErrorCode::Incompatible,
                message,
                ..
            } => Disposition::Fail(FailureReason::Incompatible {
                detail: message.clone(),
            }),
            HandshakeError::EnvironmentMismatch {
                expected,
                announced,
            } => Disposition::Fail(FailureReason::EnvironmentMismatch {
                expected: *expected,
                announced: *announced,
            }),
            HandshakeError::Relogin(cause) => {
                Disposition::Fail(FailureReason::ReloginRequired(*cause))
            }
            HandshakeError::RoleMismatch { requested, granted } => {
                Disposition::Fail(FailureReason::RoleMismatch {
                    requested: *requested,
                    granted: *granted,
                })
            }
            HandshakeError::Disconnected {
                reason: DisconnectReason::Replaced,
                ..
            } => Disposition::Fail(FailureReason::Replaced),
            _ => Disposition::Retry,
        }
    }
}

impl From<ReloginNeeded> for HandshakeError {
    fn from(e: ReloginNeeded) -> Self {
        HandshakeError::Relogin(e.0)
    }
}

/// A connected frame stream during the sequential handshake.
pub(crate) struct Link<R, W> {
    pub reader: R,
    pub writer: W,
    pub stats: Arc<TransportStats>,
    pub info: ConnectionInfo,
    pub clock: LinkClock,
    pub reply_timeout: Duration,
}

impl<R: FrameReader, W: FrameWriter> Link<R, W> {
    /// Sends one control message (session id 0: no session is bound yet).
    pub async fn send(&mut self, message: &Message) -> Result<(), HandshakeError> {
        let frame = message
            .to_frame(&FrameMeta::control(NO_SESSION, self.clock.now_ns()))
            .map_err(|source| HandshakeError::Encode {
                msg_type: message.msg_type(),
                source,
            })?;
        self.writer.send(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }

    /// Receives the next frame that passes the header rules, with the reply timeout. Frames that
    /// violate header rules are skipped (not fatal, §1).
    pub async fn recv_frame(
        &mut self,
        stage: &'static str,
    ) -> Result<(Frame, MsgType), HandshakeError> {
        let deadline = tokio::time::Instant::now() + self.reply_timeout;
        loop {
            let next = tokio::time::timeout_at(deadline, self.reader.recv())
                .await
                .map_err(|_| HandshakeError::Timeout {
                    stage,
                    timeout: self.reply_timeout,
                })??;
            let Some(frame) = next else {
                return Err(HandshakeError::Closed);
            };
            match frame.check_rules() {
                Ok(msg_type) => return Ok((frame, msg_type)),
                Err(HeaderRuleError::UnknownMsgType(id)) => {
                    tracing::debug!(
                        component = "transport",
                        msg_type = id,
                        "unknown message type skipped"
                    );
                }
                Err(e) => {
                    tracing::warn!(component = "transport", error = %e, "frame violating header rules skipped");
                }
            }
        }
    }

    /// Receives and decodes the next message.
    pub async fn recv_message(
        &mut self,
        stage: &'static str,
    ) -> Result<(Frame, Message), HandshakeError> {
        let (frame, msg_type) = self.recv_frame(stage).await?;
        let message = Message::from_frame(&frame)
            .map_err(|source| HandshakeError::Decode { msg_type, source })?;
        Ok((frame, message))
    }

    /// Best-effort graceful close within `limit`: DISCONNECT{ClientShutdown}, then close the
    /// sending direction. Session id 0 is valid for a control frame also on a connection the relay
    /// has bound meanwhile (§1 session binding rule), so this also releases a slot the relay
    /// assigned to a join or resume whose reply was never processed.
    pub async fn goodbye(&mut self, limit: Duration) {
        let bye = Message::Disconnect(Disconnect {
            reason: DisconnectReason::ClientShutdown,
            message: None,
        });
        let closed = tokio::time::timeout(limit, async {
            self.send(&bye).await?;
            self.writer.close().await?;
            Ok::<(), HandshakeError>(())
        })
        .await;
        if !matches!(closed, Ok(Ok(()))) {
            tracing::debug!(
                component = "transport",
                "DISCONNECT not delivered (connection already unusable)"
            );
        }
    }
}

/// The payload limit to apply to a relay's [`ServerLimits`]: `0` means "not announced" (the
/// protocol maximum applies), larger values are clamped to [`MAX_PAYLOAD_BYTES`], and a limit
/// below [`MIN_RELAY_PAYLOAD_LIMIT`] is refused.
pub(crate) fn effective_payload_limit(limits: &ServerLimits) -> Result<usize, HandshakeError> {
    let announced = limits.max_payload_bytes;
    if announced == 0 {
        return Ok(MAX_PAYLOAD_BYTES);
    }
    let limit = usize::try_from(announced)
        .unwrap_or(MAX_PAYLOAD_BYTES)
        .min(MAX_PAYLOAD_BYTES);
    if limit < MIN_RELAY_PAYLOAD_LIMIT {
        return Err(HandshakeError::PayloadLimitTooSmall {
            announced,
            minimum: MIN_RELAY_PAYLOAD_LIMIT,
        });
    }
    Ok(limit)
}

/// Maps a relay ERROR or DISCONNECT received during `stage`, or reports an unexpected message.
pub(crate) fn unexpected(stage: &'static str, message: Message) -> HandshakeError {
    match message {
        Message::Error(ErrorMsg {
            code,
            message,
            fatal,
        }) => HandshakeError::Refused {
            stage,
            code,
            message,
            fatal,
        },
        Message::Disconnect(d) => HandshakeError::Disconnected {
            stage,
            reason: d.reason,
        },
        other => HandshakeError::Unexpected {
            stage,
            got: other.msg_type(),
        },
    }
}

/// HELLO exchange: version negotiation, the environment check (strict: the relay's environment
/// must equal the configured one, PROTOCOL.md §6) and the relay's limits.
pub(crate) async fn hello<R: FrameReader, W: FrameWriter>(
    link: &mut Link<R, W>,
    config: &ClientConfig,
    role: ClientRole,
) -> Result<ServerHello, HandshakeError> {
    const STAGE: &str = "HELLO";
    link.send(&Message::Hello(Hello {
        app_version: config.app_version.clone(),
        protocol: ProtocolVersion::CURRENT,
        role,
        schema_versions: config.schema_versions.clone(),
        client_id: config.client_id.clone(),
    }))
    .await?;
    let (_, message) = link.recv_message(STAGE).await?;
    match message {
        Message::ServerHello(server) => {
            negotiate_protocol(ProtocolVersion::CURRENT, server.protocol, MIN_RELAY_MINOR)?;
            let expected = config.endpoint.environment();
            if server.env != expected {
                return Err(HandshakeError::EnvironmentMismatch {
                    expected,
                    announced: server.env,
                });
            }
            effective_payload_limit(&server.limits)?;
            Ok(server)
        }
        // Nothing else may precede the relay's HELLO (ERROR Incompatible is mapped here).
        other => Err(unexpected(STAGE, other)),
    }
}

/// AUTH. With `first = None` the [`AuthManager`] picks the best credential: the in-memory access
/// token, else the stored refresh token. A refused access token is dropped and the refresh token
/// is tried on the same connection; a refused refresh token means "log in again". `first =
/// Some(AuthPlan::Password { .. })` is the explicit login.
pub(crate) async fn authenticate<R: FrameReader, W: FrameWriter>(
    link: &mut Link<R, W>,
    auth: &AuthManager,
    role: ClientRole,
    server: &ServerHello,
    first: Option<AuthPlan>,
) -> Result<AccountInfo, HandshakeError> {
    const STAGE: &str = "AUTH";
    let mut next = first;
    loop {
        let plan = match next.take() {
            Some(plan) => plan,
            None => auth.plan(link.clock.now()).await?,
        };
        link.send(&Message::Auth(plan.message())).await?;
        let (_, message) = link.recv_message(STAGE).await?;
        match message {
            Message::AuthOk(ok) => {
                if ok.role != role && matches!(plan, AuthPlan::Password { .. }) {
                    // An explicit login with an account of the other role is not kept (nothing is
                    // stored): the user sees "not a driver / engineer account" and any previous
                    // login stays as it was. A token AUTH must be accepted below even then, because
                    // the relay rotated the presented token.
                    return Err(HandshakeError::RoleMismatch {
                        requested: role,
                        granted: ok.role,
                    });
                }
                let account = auth.accepted(plan, &ok, server.server_time_ns, link.clock.now())?;
                if account.role != role {
                    return Err(HandshakeError::RoleMismatch {
                        requested: role,
                        granted: account.role,
                    });
                }
                return Ok(account);
            }
            Message::Error(e)
                if matches!(e.code, ErrorCode::AuthFailed | ErrorCode::TokenExpired) =>
            {
                match plan {
                    AuthPlan::Access { .. } => {
                        auth.access_rejected(&plan);
                        if e.fatal {
                            return Err(HandshakeError::AccessTokenRefused);
                        }
                        // Not fatal: try the refresh token on this connection.
                    }
                    AuthPlan::Refresh { .. } => {
                        let cause = if e.code == ErrorCode::TokenExpired {
                            ReloginCause::RefreshTokenExpired
                        } else {
                            ReloginCause::RefreshTokenRejected
                        };
                        auth.refresh_rejected(&plan);
                        return Err(HandshakeError::Relogin(cause));
                    }
                    AuthPlan::Password { .. } => {
                        return Err(HandshakeError::Refused {
                            stage: STAGE,
                            code: e.code,
                            message: e.message,
                            fatal: e.fatal,
                        });
                    }
                }
            }
            other => return Err(unexpected(STAGE, other)),
        }
    }
}

/// [`authenticate`] (silent: access token, else refresh token) in a task of its own, returning the
/// link afterwards.
///
/// Abandoning a connection attempt (stop, cancel, shutdown) while AUTH is in flight must not
/// strand a rotated refresh token: once AUTH{RefreshToken} was sent the relay may already have
/// rotated it, and only processing its AUTH_OK persists the successor. Without that, the next start
/// would present the rotated token and, after the relay's reuse grace, the login would be revoked
/// (D-020: only logout or revocation may ask for the password again). Dropping the returned future
/// therefore only detaches the task; it still finishes the exchange (bounded by the reply timeout)
/// and then closes its connection.
pub(crate) async fn authenticate_detached<R: FrameReader, W: FrameWriter>(
    mut link: Link<R, W>,
    auth: Arc<AuthManager>,
    role: ClientRole,
    server: ServerHello,
) -> Result<(Link<R, W>, AccountInfo), HandshakeError> {
    tokio::spawn(async move {
        let account = authenticate(&mut link, &auth, role, &server, None).await?;
        Ok((link, account))
    })
    .await
    .map_err(|source| HandshakeError::AuthTask { source })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(max_payload_bytes: u32) -> ServerLimits {
        ServerLimits {
            max_payload_bytes,
            heartbeat_interval_ms: 1_000,
            heartbeat_timeout_ms: 5_000,
            max_engineers_per_session: 2,
            session_driver_grace_s: 600,
        }
    }

    #[test]
    fn relay_payload_limits_are_validated() {
        assert_eq!(
            effective_payload_limit(&limits(0)).expect("not announced"),
            MAX_PAYLOAD_BYTES
        );
        assert_eq!(
            effective_payload_limit(&limits(u32::MAX)).expect("clamped"),
            MAX_PAYLOAD_BYTES
        );
        let minimum = u32::try_from(MIN_RELAY_PAYLOAD_LIMIT).expect("fits");
        assert_eq!(
            effective_payload_limit(&limits(minimum)).expect("smallest usable"),
            MIN_RELAY_PAYLOAD_LIMIT
        );
        let too_small = effective_payload_limit(&limits(minimum - 1)).expect_err("too small");
        assert!(matches!(
            too_small.disposition(),
            Disposition::Fail(FailureReason::Incompatible { .. })
        ));
        assert!(
            effective_payload_limit(&limits(1)).is_err(),
            "a bogus tiny limit is refused, not applied to every frame"
        );
    }

    #[test]
    fn dispositions() {
        assert_eq!(HandshakeError::Closed.disposition(), Disposition::Retry);
        assert_eq!(
            HandshakeError::Refused {
                stage: "AUTH",
                code: ErrorCode::ServerBusy,
                message: String::new(),
                fatal: true
            }
            .disposition(),
            Disposition::Retry
        );
        assert!(matches!(
            HandshakeError::Refused {
                stage: "HELLO",
                code: ErrorCode::Incompatible,
                message: "major".into(),
                fatal: true
            }
            .disposition(),
            Disposition::Fail(FailureReason::Incompatible { .. })
        ));
        assert_eq!(
            HandshakeError::Relogin(ReloginCause::RefreshTokenRejected).disposition(),
            Disposition::Fail(FailureReason::ReloginRequired(
                ReloginCause::RefreshTokenRejected
            ))
        );
        assert_eq!(
            HandshakeError::EnvironmentMismatch {
                expected: Environment::Production,
                announced: Environment::Staging
            }
            .disposition(),
            Disposition::Fail(FailureReason::EnvironmentMismatch {
                expected: Environment::Production,
                announced: Environment::Staging
            })
        );
        assert_eq!(
            unexpected(
                "AUTH",
                Message::Disconnect(Disconnect {
                    reason: DisconnectReason::Replaced,
                    message: None
                })
            )
            .disposition(),
            Disposition::Fail(FailureReason::Replaced)
        );
    }
}

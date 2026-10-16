//! Protocol and telemetry-schema version negotiation (docs/PROTOCOL.md §6).
//!
//! * The protocol **major** must match exactly. A mismatch is fatal (`ERROR Incompatible`). The frame
//!   decoder also rejects every frame whose header carries another major before it reads the payload.
//! * Each minor within a major is a compatible addition, so every side supports all minors up to its
//!   own. The connection runs at the highest common minor, `min(local.minor, remote.minor)`. A relay
//!   may refuse peers below a minimum minor (for example to force a security upgrade).
//! * The telemetry schema version (`TELEMETRY_SCHEMA_VERSION`) is negotiated separately: the highest
//!   version present in both lists.
//! * Environment separation: a client configured for production refuses a relay that announces any
//!   other environment.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::messages::{ErrorCode, ErrorMsg};

/// Protocol major version implemented by this crate. Breaking changes bump it.
pub const PROTOCOL_MAJOR: u8 = 1;
/// Protocol minor version implemented by this crate. Compatible additions bump it.
///
/// * 1.0: the first release.
/// * 1.1 (2026-10-16): DRIVER_PRESENCE (0x19, relay → engineer, PROTOCOL.md §6 "Driver presence").
pub const PROTOCOL_MINOR: u8 = 1;

/// A protocol version (`major.minor`) as carried in HELLO messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u8,
    pub minor: u8,
}

impl ProtocolVersion {
    /// The version implemented by this crate.
    pub const CURRENT: ProtocolVersion = ProtocolVersion {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };

    /// Creates a version.
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Deployment environment announced by the relay in `ServerHello.env`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Environment {
    Development,
    Staging,
    Production,
}

/// Why two peers cannot talk to each other. Every variant maps to `ERROR Incompatible` (fatal).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Incompatible {
    #[error("protocol major version mismatch: local {local}, remote {remote}")]
    MajorMismatch {
        local: ProtocolVersion,
        remote: ProtocolVersion,
    },
    #[error("remote protocol {remote} is older than the oldest accepted minor {min_minor}")]
    MinorTooOld {
        remote: ProtocolVersion,
        min_minor: u8,
    },
    #[error("no common telemetry schema version (local supports {local:?}, remote {remote:?})")]
    NoCommonSchema { local: Vec<u16>, remote: Vec<u16> },
    /// An engineer cannot decode the driver's stream: its negotiated minor is below the minor the
    /// driver's session runs at (§10, [`check_engineer_join_minor`]).
    #[error(
        "engineer protocol {engineer} is older than the driver session's protocol minor {driver_minor}; update the app"
    )]
    EngineerBehindDriver {
        engineer: ProtocolVersion,
        driver_minor: u8,
    },
    #[error(
        "relay announced environment {announced:?}, this client is configured for {expected:?}"
    )]
    EnvironmentMismatch {
        expected: Environment,
        announced: Environment,
    },
}

impl Incompatible {
    /// Wire error code for this failure (always [`ErrorCode::Incompatible`]).
    pub const fn error_code(&self) -> ErrorCode {
        ErrorCode::Incompatible
    }

    /// The fatal `ERROR` message to send before closing the connection.
    pub fn to_error_msg(&self) -> ErrorMsg {
        ErrorMsg {
            code: self.error_code(),
            message: self.to_string(),
            fatal: true,
        }
    }
}

/// Negotiates the protocol version for one connection.
///
/// The majors must be equal and `remote.minor` must be at least `min_remote_minor`. The result is
/// the highest common minor, `min(local.minor, remote.minor)`. A sender must not use any message,
/// field or enum variant introduced after that minor.
pub fn negotiate_protocol(
    local: ProtocolVersion,
    remote: ProtocolVersion,
    min_remote_minor: u8,
) -> Result<ProtocolVersion, Incompatible> {
    if local.major != remote.major {
        return Err(Incompatible::MajorMismatch { local, remote });
    }
    if remote.minor < min_remote_minor {
        return Err(Incompatible::MinorTooOld {
            remote,
            min_minor: min_remote_minor,
        });
    }
    Ok(ProtocolVersion {
        major: local.major,
        minor: local.minor.min(remote.minor),
    })
}

/// Selects the telemetry schema version: the highest version present in both lists.
pub fn select_schema_version(local: &[u16], remote: &[u16]) -> Result<u16, Incompatible> {
    local
        .iter()
        .copied()
        .filter(|v| remote.contains(v))
        .max()
        .ok_or_else(|| Incompatible::NoCommonSchema {
            local: local.to_vec(),
            remote: remote.to_vec(),
        })
}

/// Relay-side check before accepting an engineer's SESSION_JOIN or `SESSION_RESUME::Engineer`
/// (§10). Driver→relay→engineer payloads are forwarded unchanged, so the driver encodes them at the
/// minor it negotiated with the relay (`driver_session_minor`) without knowing the engineers'
/// minors. An engineer whose negotiated minor (`engineer`, the result of [`negotiate_protocol`] on
/// its connection) is older could not decode new message types, fields or enum variants of that
/// minor, so the relay refuses it with this error (the engineer app asks the user to update).
pub fn check_engineer_join_minor(
    driver_session_minor: u8,
    engineer: ProtocolVersion,
) -> Result<(), Incompatible> {
    if engineer.minor < driver_session_minor {
        return Err(Incompatible::EngineerBehindDriver {
            engineer,
            driver_minor: driver_session_minor,
        });
    }
    Ok(())
}

/// Environment separation (PROTOCOL.md §6). A client configured for production accepts only a
/// production relay. Development and staging clients accept any relay; the relay's own
/// authentication still applies.
pub fn check_relay_environment(
    client: Environment,
    relay: Environment,
) -> Result<(), Incompatible> {
    if client == Environment::Production && relay != Environment::Production {
        return Err(Incompatible::EnvironmentMismatch {
            expected: client,
            announced: relay,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_major_negotiates_highest_common_minor() {
        let local = ProtocolVersion::new(1, 3);
        assert_eq!(
            negotiate_protocol(local, ProtocolVersion::new(1, 1), 0),
            Ok(ProtocolVersion::new(1, 1))
        );
        assert_eq!(
            negotiate_protocol(local, ProtocolVersion::new(1, 7), 0),
            Ok(ProtocolVersion::new(1, 3))
        );
        assert_eq!(negotiate_protocol(local, local, 0), Ok(local));
    }

    #[test]
    fn major_mismatch_is_incompatible_in_both_directions() {
        let local = ProtocolVersion::new(1, 0);
        for remote in [ProtocolVersion::new(0, 9), ProtocolVersion::new(2, 0)] {
            let err = negotiate_protocol(local, remote, 0).expect_err("must be incompatible");
            assert_eq!(err, Incompatible::MajorMismatch { local, remote });
            let msg = err.to_error_msg();
            assert_eq!(msg.code, ErrorCode::Incompatible);
            assert!(msg.fatal);
        }
    }

    #[test]
    fn minimum_minor_is_enforced() {
        let local = ProtocolVersion::new(1, 4);
        let remote = ProtocolVersion::new(1, 1);
        assert_eq!(
            negotiate_protocol(local, remote, 2),
            Err(Incompatible::MinorTooOld {
                remote,
                min_minor: 2
            })
        );
        assert_eq!(negotiate_protocol(local, remote, 1), Ok(remote));
    }

    #[test]
    fn schema_selection_picks_highest_common() {
        assert_eq!(select_schema_version(&[1, 2, 3], &[2, 3, 4]), Ok(3));
        assert_eq!(select_schema_version(&[3, 1], &[1]), Ok(1));
        assert_eq!(
            select_schema_version(&[1, 2], &[3]),
            Err(Incompatible::NoCommonSchema {
                local: vec![1, 2],
                remote: vec![3]
            })
        );
        assert!(select_schema_version(&[], &[1]).is_err());
    }

    #[test]
    fn production_client_refuses_other_environments() {
        use Environment::*;
        assert_eq!(check_relay_environment(Production, Production), Ok(()));
        for relay in [Development, Staging] {
            assert_eq!(
                check_relay_environment(Production, relay),
                Err(Incompatible::EnvironmentMismatch {
                    expected: Production,
                    announced: relay
                })
            );
        }
        assert_eq!(check_relay_environment(Development, Staging), Ok(()));
        assert_eq!(check_relay_environment(Staging, Production), Ok(()));
    }

    #[test]
    fn engineer_older_than_the_driver_session_is_refused() {
        // Review finding: the driver negotiates only with the relay, so a 1.1 driver's frames
        // reached a 1.0 engineer that could not decode them (Malformed).
        let engineer = ProtocolVersion::new(1, 0);
        let err = check_engineer_join_minor(1, engineer).expect_err("refused");
        assert_eq!(
            err,
            Incompatible::EngineerBehindDriver {
                engineer,
                driver_minor: 1
            }
        );
        assert_eq!(err.to_error_msg().code, ErrorCode::Incompatible);
        assert_eq!(check_engineer_join_minor(0, engineer), Ok(()));
        assert_eq!(
            check_engineer_join_minor(1, ProtocolVersion::new(1, 2)),
            Ok(()),
            "a newer engineer decodes older payloads"
        );
    }

    #[test]
    fn current_version_matches_constants() {
        assert_eq!(
            ProtocolVersion::CURRENT.to_string(),
            format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}")
        );
    }
}

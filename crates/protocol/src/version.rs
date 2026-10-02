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
pub const PROTOCOL_MINOR: u8 = 0;

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
    fn current_version_matches_constants() {
        assert_eq!(
            ProtocolVersion::CURRENT.to_string(),
            format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}")
        );
    }
}

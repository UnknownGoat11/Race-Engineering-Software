//! Credential storage (ARCHITECTURE.md §7, D-008, D-020, D-022).
//!
//! What is stored, per relay endpoint and environment ([`crate::RelayEndpoint::credential_namespace`]):
//! * the rotating **refresh token** (login once; every start re-authenticates silently);
//! * the **driver's session resume token** (a driver app restart resumes its session).
//!
//! What is **never** stored: passwords, access tokens (kept in memory), connection codes and
//! engineer resume tickets (memory only, D-022), any driver or session an engineer was connected
//! to. Only the driver client writes [`CredentialKind::DriverSession`]; the engineer client has no
//! code path that writes anything but the refresh token.
//!
//! [`KeyringCredentialStore`] uses Windows Credential Manager (per-user, no admin rights, D-019);
//! [`MemoryCredentialStore`] is for tests. Values are [`Secret`]s; errors never contain them.

use std::collections::BTreeMap;
use std::fmt;

use parking_lot::Mutex;
use protocol::messages::Secret;
use thiserror::Error;

/// Default Windows Credential Manager service name of the app.
pub const DEFAULT_KEYRING_SERVICE: &str = "Race Engineering Tool";
/// Format tag of a stored driver session (`v1:<session_id>:<resume token>`).
const DRIVER_SESSION_FORMAT: &str = "v1";
/// Separator of the stored driver session fields.
const FIELD_SEPARATOR: char = ':';
/// Separator between namespace and credential kind in a store key.
const KEY_SEPARATOR: char = '#';

/// The kinds of credentials the transport stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CredentialKind {
    /// Refresh token stored before tokens were kept per role (role unknown). Read only as a
    /// fallback; the first successful AUTH with it moves it to the slot of its account's role.
    RefreshToken,
    /// The rotating refresh token of the driver account. Relay accounts have one role, so the
    /// driver and the engineer login are kept apart: switching modes never presents the other
    /// role's token (D-020: log in once per mode).
    DriverRefreshToken,
    /// The rotating refresh token of the engineer account.
    EngineerRefreshToken,
    /// The driver's session id and resume token ([`StoredDriverSession`]).
    DriverSession,
}

impl CredentialKind {
    /// Stable key suffix.
    pub const fn key(self) -> &'static str {
        match self {
            CredentialKind::RefreshToken => "refresh_token",
            CredentialKind::DriverRefreshToken => "driver_refresh_token",
            CredentialKind::EngineerRefreshToken => "engineer_refresh_token",
            CredentialKind::DriverSession => "driver_session",
        }
    }

    /// The refresh-token slot of `role`.
    pub const fn refresh_token(role: protocol::messages::ClientRole) -> Self {
        match role {
            protocol::messages::ClientRole::Driver => CredentialKind::DriverRefreshToken,
            protocol::messages::ClientRole::Engineer => CredentialKind::EngineerRefreshToken,
        }
    }
}

impl fmt::Display for CredentialKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// Credential store failures. Never carries a stored value.
#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("the credential store is not accessible")]
    NoAccess {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("the credential store failed")]
    Platform {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The stored blob is not UTF-8 (its bytes are deliberately not kept: they may be a secret).
    #[error("the stored credential is not valid UTF-8")]
    BadEncoding,
    #[error("credential attribute {attribute} exceeds the store limit of {limit}")]
    TooLong { attribute: String, limit: u32 },
    #[error("credential attribute {attribute} is invalid: {reason}")]
    Invalid { attribute: String, reason: String },
    #[error("{count} stored credentials match one key")]
    Ambiguous { count: usize },
    #[error("the stored driver session is malformed")]
    MalformedDriverSession,
    #[error("unexpected credential store error: {detail}")]
    Other { detail: String },
}

impl CredentialError {
    /// Whether the error indicates malformed or corrupted stored credential data (M10).
    pub fn is_malformed(&self) -> bool {
        matches!(
            self,
            CredentialError::BadEncoding | CredentialError::MalformedDriverSession
        )
    }
}

/// Persistent storage of secrets.
pub trait CredentialStore: Send + Sync + 'static {
    /// Loads a credential, `Ok(None)` if there is none.
    fn load(
        &self,
        namespace: &str,
        kind: CredentialKind,
    ) -> Result<Option<Secret>, CredentialError>;
    /// Stores (replaces) a credential.
    fn store(
        &self,
        namespace: &str,
        kind: CredentialKind,
        value: &Secret,
    ) -> Result<(), CredentialError>;
    /// Deletes a credential; deleting a missing one succeeds.
    fn delete(&self, namespace: &str, kind: CredentialKind) -> Result<(), CredentialError>;
}

fn store_key(namespace: &str, kind: CredentialKind) -> String {
    format!("{namespace}{KEY_SEPARATOR}{}", kind.key())
}

/// Windows Credential Manager store (via the `keyring` crate). Per-user, no elevation.
#[derive(Debug, Clone)]
pub struct KeyringCredentialStore {
    service: String,
}

impl Default for KeyringCredentialStore {
    fn default() -> Self {
        Self::new(DEFAULT_KEYRING_SERVICE)
    }
}

impl KeyringCredentialStore {
    /// Store under the given service name.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(
        &self,
        namespace: &str,
        kind: CredentialKind,
    ) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(&self.service, &store_key(namespace, kind)).map_err(map_keyring)
    }
}

impl CredentialStore for KeyringCredentialStore {
    fn load(
        &self,
        namespace: &str,
        kind: CredentialKind,
    ) -> Result<Option<Secret>, CredentialError> {
        match self.entry(namespace, kind)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring(e)),
        }
    }

    fn store(
        &self,
        namespace: &str,
        kind: CredentialKind,
        value: &Secret,
    ) -> Result<(), CredentialError> {
        self.entry(namespace, kind)?
            .set_password(value.expose())
            .map_err(map_keyring)
    }

    fn delete(&self, namespace: &str, kind: CredentialKind) -> Result<(), CredentialError> {
        match self.entry(namespace, kind)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring(e)),
        }
    }
}

/// Maps keyring errors without keeping any credential bytes.
fn map_keyring(e: keyring::Error) -> CredentialError {
    match e {
        keyring::Error::NoStorageAccess(source) => CredentialError::NoAccess { source },
        keyring::Error::PlatformFailure(source) => CredentialError::Platform { source },
        keyring::Error::BadEncoding(_) => CredentialError::BadEncoding,
        keyring::Error::TooLong(attribute, limit) => CredentialError::TooLong { attribute, limit },
        keyring::Error::Invalid(attribute, reason) => {
            CredentialError::Invalid { attribute, reason }
        }
        keyring::Error::Ambiguous(matches) => CredentialError::Ambiguous {
            count: matches.len(),
        },
        keyring::Error::NoEntry => CredentialError::Other {
            detail: "no entry".to_owned(),
        },
        // keyring::Error is non-exhaustive; its Display of future variants carries no secret
        // value in the known variants' style, but stay conservative and keep only the kind.
        other => CredentialError::Other {
            detail: format!("{:?}", std::mem::discriminant(&other)),
        },
    }
}

/// In-memory store for tests and for running without persistence.
#[derive(Debug, Default)]
pub struct MemoryCredentialStore {
    entries: Mutex<BTreeMap<String, Secret>>,
}

impl MemoryCredentialStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keys currently stored (`<namespace>#<kind>`), for tests that check what was persisted.
    pub fn keys(&self) -> Vec<String> {
        self.entries.lock().keys().cloned().collect()
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn load(
        &self,
        namespace: &str,
        kind: CredentialKind,
    ) -> Result<Option<Secret>, CredentialError> {
        Ok(self
            .entries
            .lock()
            .get(&store_key(namespace, kind))
            .cloned())
    }

    fn store(
        &self,
        namespace: &str,
        kind: CredentialKind,
        value: &Secret,
    ) -> Result<(), CredentialError> {
        self.entries
            .lock()
            .insert(store_key(namespace, kind), value.clone());
        Ok(())
    }

    fn delete(&self, namespace: &str, kind: CredentialKind) -> Result<(), CredentialError> {
        self.entries.lock().remove(&store_key(namespace, kind));
        Ok(())
    }
}

/// The driver's own session, persisted so that a driver app restart resumes it (D-020).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDriverSession {
    pub session_id: u64,
    pub resume_token: Secret,
}

impl StoredDriverSession {
    /// Storage form `v1:<session_id>:<token>`.
    pub fn encode(&self) -> Secret {
        Secret::new(format!(
            "{DRIVER_SESSION_FORMAT}{FIELD_SEPARATOR}{}{FIELD_SEPARATOR}{}",
            self.session_id,
            self.resume_token.expose()
        ))
    }

    /// Parses the storage form. Returns `Ok(None)` for an ended tombstone (`v1:ended`).
    pub fn decode(stored: &Secret) -> Result<Option<Self>, CredentialError> {
        let raw = stored.expose();
        let (format, rest) = raw
            .split_once(FIELD_SEPARATOR)
            .ok_or(CredentialError::MalformedDriverSession)?;
        if format != DRIVER_SESSION_FORMAT {
            return Err(CredentialError::MalformedDriverSession);
        }
        if rest == "ended" {
            return Ok(None);
        }
        let (id, token) = rest
            .split_once(FIELD_SEPARATOR)
            .ok_or(CredentialError::MalformedDriverSession)?;
        let session_id = id
            .parse::<u64>()
            .map_err(|_| CredentialError::MalformedDriverSession)?;
        if session_id == protocol::frame::NO_SESSION || token.is_empty() {
            return Err(CredentialError::MalformedDriverSession);
        }
        Ok(Some(Self {
            session_id,
            resume_token: Secret::new(token),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip_and_delete() {
        let store = MemoryCredentialStore::new();
        let ns = "production/relay.example.com:443";
        assert_eq!(
            store.load(ns, CredentialKind::RefreshToken).expect("load"),
            None
        );
        store
            .store(ns, CredentialKind::RefreshToken, &Secret::new("r1"))
            .expect("store");
        assert_eq!(
            store.load(ns, CredentialKind::RefreshToken).expect("load"),
            Some(Secret::new("r1"))
        );
        assert_eq!(store.keys(), vec![format!("{ns}#refresh_token")]);
        store
            .delete(ns, CredentialKind::RefreshToken)
            .expect("delete");
        store
            .delete(ns, CredentialKind::RefreshToken)
            .expect("idempotent");
        assert!(store.keys().is_empty());
    }

    #[test]
    fn driver_session_encoding_round_trips_tokens_with_separators() {
        let s = StoredDriverSession {
            session_id: 42,
            resume_token: Secret::new("abc:def/+="),
        };
        assert_eq!(
            StoredDriverSession::decode(&s.encode()).expect("decode"),
            Some(s)
        );
        assert_eq!(
            StoredDriverSession::decode(&Secret::new("v1:ended")).expect("tombstone"),
            None
        );
        for bad in ["", "v1", "v1:x:tok", "v2:1:tok", "v1:0:tok", "v1:5:"] {
            assert!(
                StoredDriverSession::decode(&Secret::new(bad)).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn errors_never_hold_credential_bytes() {
        let e = map_keyring(keyring::Error::BadEncoding(b"secret-token".to_vec()));
        assert!(!format!("{e:?} {e}").contains("secret"));
    }

    /// Writes, reads and deletes one entry in the real Windows Credential Manager under a test
    /// service name. Ignored by default because it touches the user's credential store; run with
    /// `cargo test -p transport -- --ignored keyring`.
    #[test]
    #[ignore = "touches the real Windows Credential Manager"]
    fn keyring_round_trip_in_windows_credential_manager() {
        let store = KeyringCredentialStore::new("Race Engineering Tool transport test");
        let ns = "development/keyring-test.invalid:1";
        store
            .delete(ns, CredentialKind::RefreshToken)
            .expect("clean start");
        assert_eq!(
            store.load(ns, CredentialKind::RefreshToken).expect("load"),
            None
        );
        store
            .store(
                ns,
                CredentialKind::RefreshToken,
                &Secret::new("test-refresh-token"),
            )
            .expect("store");
        assert_eq!(
            store.load(ns, CredentialKind::RefreshToken).expect("load"),
            Some(Secret::new("test-refresh-token"))
        );
        store
            .delete(ns, CredentialKind::RefreshToken)
            .expect("delete");
        assert_eq!(
            store.load(ns, CredentialKind::RefreshToken).expect("load"),
            None
        );
    }
}

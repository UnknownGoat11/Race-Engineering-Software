//! Persistent relay state (D-007): accounts, refresh tokens and session metadata.
//!
//! * [`AccountStore`] and [`SessionStore`] are implemented by [`PgStore`] (PostgreSQL, runtime
//!   `sqlx` queries, migrations in `crates/relay/migrations`, no compile-time `DATABASE_URL`) and by
//!   [`MemoryStore`] (tests and `RELAY_ENV=development` only). [`Store`] selects one at runtime.
//! * The store never holds telemetry samples, connection codes or engineer resume tickets (nor
//!   their digests). Codes and ticket digests live in relay memory only. Refresh tokens and driver
//!   resume tokens are stored as SHA-256 digests only ([`TokenHash`]).
//! * Engineer slots of open sessions ([`EngineerSlotRecord`]: who holds a slot, since when, and
//!   whether the connection dropped) are session metadata; they let engineers resume after a relay
//!   restart (D-022 network blips, [`crate::tickets`]).
//! * Refresh tokens form **families** (one per login). Using a token rotates it; a rotated token
//!   presented again after the reuse grace revokes its whole family (theft detection). Revoking an
//!   account revokes all of its families (logout everywhere).

mod memory;
mod postgres;

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use protocol::messages::ClientRole;
use thiserror::Error;
use uuid::Uuid;

use crate::secrets::TokenHash;

pub use memory::MemoryStore;
pub use postgres::PgStore;

/// Store failures. `Conflict` is a normal outcome (duplicate username); every other variant is
/// an infrastructure failure reported to clients as `ERROR Internal`.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{0} already exists")]
    Conflict(&'static str),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("stored value is invalid: {0}")]
    Corrupt(String),
    /// The call did not complete within [`STORE_CALL_TIMEOUT`] (database unreachable or stalled).
    #[error("database call timed out after {0:?}")]
    Timeout(Duration),
}

/// Upper bound of one PostgreSQL store call as the relay awaits it (pool acquire plus statement
/// time), so a stalled or black-holed database never blocks a connection task, the housekeeping
/// pass or a shutdown for longer.
pub const STORE_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Awaits `call` for at most `limit`; [`StoreError::Timeout`] afterwards (the call is dropped).
async fn bounded<T>(
    limit: Duration,
    call: impl Future<Output = Result<T, StoreError>>,
) -> Result<T, StoreError> {
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or_else(|_| Err(StoreError::Timeout(limit)))
}

/// Role of an account (D-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountRole {
    Driver,
    Engineer,
    /// May connect in either role (support, testing). Created only with the CLI.
    Admin,
}

impl AccountRole {
    /// Storage / CLI name.
    pub const fn as_str(self) -> &'static str {
        match self {
            AccountRole::Driver => "driver",
            AccountRole::Engineer => "engineer",
            AccountRole::Admin => "admin",
        }
    }

    /// Parses a storage / CLI name (case-insensitive).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "driver" => Some(AccountRole::Driver),
            "engineer" => Some(AccountRole::Engineer),
            "admin" => Some(AccountRole::Admin),
            _ => None,
        }
    }

    /// The role a connection of this account acts in when its HELLO asked for `requested`:
    /// driver and engineer accounts always act in their own role (a mismatch is visible to the
    /// client in AUTH_OK); admin accounts act in the requested role.
    pub const fn effective_role(self, requested: ClientRole) -> ClientRole {
        match self {
            AccountRole::Driver => ClientRole::Driver,
            AccountRole::Engineer => ClientRole::Engineer,
            AccountRole::Admin => requested,
        }
    }
}

/// A relay account.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub id: Uuid,
    /// Normalized (lower-case) unique login name.
    pub username: String,
    pub display_name: String,
    pub role: AccountRole,
    /// Argon2id PHC string. Never logged.
    pub password_hash: String,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("role", &self.role)
            .field("password_hash", &"<redacted>")
            .field("disabled", &self.disabled)
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// A refresh token as stored (the token itself is never stored, only its digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshTokenRecord {
    pub id: Uuid,
    pub family_id: Uuid,
    pub account_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Set when the token was used and replaced by a successor.
    pub rotated_at: Option<DateTime<Utc>>,
    /// Set when the token (or its family) was revoked.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A refresh token to insert.
#[derive(Debug, Clone)]
pub struct NewRefreshToken {
    pub id: Uuid,
    pub family_id: Uuid,
    pub account_id: Uuid,
    pub token_hash: TokenHash,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Outcome of [`AccountStore::rotate_refresh_token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateOutcome {
    /// The old token was marked rotated and the successor inserted.
    Rotated,
    /// The old token was rotated or revoked concurrently; nothing was inserted.
    Lost,
}

/// Lifecycle state of a session (D-020): `Live` while the driver is attached, `Grace` while the
/// relay waits for the driver to resume, `Closed` afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Live,
    Grace,
    Closed,
}

impl SessionStatus {
    /// Storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Live => "live",
            SessionStatus::Grace => "grace",
            SessionStatus::Closed => "closed",
        }
    }

    /// Parses a storage name.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "live" => Some(SessionStatus::Live),
            "grace" => Some(SessionStatus::Grace),
            "closed" => Some(SessionStatus::Closed),
            _ => None,
        }
    }
}

/// `close_reason` of a session whose driver grace period elapsed.
pub const CLOSE_REASON_EXPIRED: &str = "expired";
/// `close_reason` of a session the driver ended (SESSION_CLOSE).
pub const CLOSE_REASON_DRIVER_STOPPED: &str = "driver_stopped";
/// `close_reason` of a session replaced by a new session of the same driver account.
pub const CLOSE_REASON_REPLACED: &str = "replaced";
/// `close_reason` of a session stored but refused by the registry (relay session limit).
pub const CLOSE_REASON_RELAY_FULL: &str = "relay_full";

/// Session identity shown to engineers (SESSION_INFO) and kept as metadata. Never telemetry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMeta {
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub track: Option<String>,
    /// Raw LMU `mSession` value (`SessionType::raw`).
    pub session_type: Option<i32>,
    pub lmu_version: Option<String>,
    pub app_version: String,
    pub schema_version: u16,
}

/// An engineer slot of an open session as stored (D-022), so a running engineer app resumes with
/// its in-memory ticket after a relay restart. Session metadata only: no ticket, ticket digest or
/// connection code (tickets carry an HMAC tag, see [`crate::tickets`]). A row exists exactly while
/// the slot does; deleting it (DISCONNECT, ENGINEER_KICK, ticket expiry, session end) kills every
/// ticket of the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineerSlotRecord {
    pub session_id: u64,
    /// The engineer account holding the slot (at most one slot per account and session).
    pub account_id: Uuid,
    /// Relay-assigned connection id (ENGINEER_LIST, ENGINEER_KICK), unique within the session.
    pub connection_id: u64,
    /// When the engineer joined (ENGINEER_LIST `connected_since`).
    pub connected_since: DateTime<Utc>,
    /// When the engineer's connection dropped (start of the ticket window); `None` while
    /// connected.
    pub dropped_at: Option<DateTime<Utc>>,
}

/// Session metadata as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    /// Relay-assigned id (1..=i64::MAX).
    pub id: u64,
    pub driver_account: Uuid,
    /// Digest of the driver's resume token; `None` once the session is closed.
    pub resume_hash: Option<TokenHash>,
    pub meta: SessionMeta,
    pub started_at: DateTime<Utc>,
    pub status: SessionStatus,
    pub close_reason: Option<String>,
    pub last_activity: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// Accounts and refresh tokens.
pub trait AccountStore: Send + Sync {
    /// Inserts an account; [`StoreError::Conflict`] if the username exists.
    fn create_account(
        &self,
        account: &Account,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The account with this (normalized) username.
    fn account_by_username(
        &self,
        username: &str,
    ) -> impl Future<Output = Result<Option<Account>, StoreError>> + Send;

    /// The account with this id.
    fn account_by_id(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<Account>, StoreError>> + Send;

    /// Records a successful login.
    fn record_login(
        &self,
        id: Uuid,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Starts a refresh-token family (one per login).
    fn create_token_family(
        &self,
        family: Uuid,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// `Some(true)` if the family was revoked, `Some(false)` if active, `None` if unknown.
    fn family_revoked(
        &self,
        family: Uuid,
    ) -> impl Future<Output = Result<Option<bool>, StoreError>> + Send;

    /// The members of `families` whose login may no longer be used: revoked (logout everywhere,
    /// refresh-token reuse), unknown (account deleted) or belonging to a disabled account. The
    /// relay closes the open connections of such logins.
    fn inactive_families(
        &self,
        families: &[Uuid],
    ) -> impl Future<Output = Result<Vec<Uuid>, StoreError>> + Send;

    /// Inserts a refresh token and revokes the oldest active tokens of its family beyond
    /// `keep_active` (orphans a client no longer holds).
    fn insert_refresh_token(
        &self,
        token: &NewRefreshToken,
        keep_active: usize,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The refresh token with this digest.
    fn refresh_token_by_hash(
        &self,
        hash: &TokenHash,
    ) -> impl Future<Output = Result<Option<RefreshTokenRecord>, StoreError>> + Send;

    /// Atomically marks `old` rotated (only if neither rotated nor revoked) and inserts its
    /// successor.
    fn rotate_refresh_token(
        &self,
        old: Uuid,
        successor: &NewRefreshToken,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<RotateOutcome, StoreError>> + Send;

    /// Revokes a family and every token in it.
    fn revoke_family(
        &self,
        family: Uuid,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Revokes every family and token of an account. Returns the number of tokens revoked.
    fn revoke_account_tokens(
        &self,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Deletes refresh tokens that expired before `before`. Returns the number deleted.
    fn purge_refresh_tokens(
        &self,
        before: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}

/// Session metadata.
pub trait SessionStore: Send + Sync {
    /// Inserts a new session.
    fn insert_session(
        &self,
        record: &SessionRecord,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The session with this id.
    fn session(
        &self,
        id: u64,
    ) -> impl Future<Output = Result<Option<SessionRecord>, StoreError>> + Send;

    /// Sets `status` (live / grace) and `last_activity` of an open session.
    fn set_session_status(
        &self,
        id: u64,
        status: SessionStatus,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Closes a session (clears its resume digest, deletes its engineer slots).
    fn close_session(
        &self,
        id: u64,
        reason: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Updates `last_activity` of open sessions.
    fn touch_sessions(
        &self,
        ids: &[u64],
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Closes (reason `expired`) every open session whose `last_activity` is before
    /// `idle_before` and deletes the engineer slots of closed sessions. Returns the number of
    /// sessions closed.
    fn expire_sessions(
        &self,
        idle_before: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Deletes closed sessions that ended before `ended_before`, with their engineer slots.
    /// Returns the number of sessions deleted.
    fn purge_closed_sessions(
        &self,
        ended_before: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Inserts or replaces the slot of `slot.account_id` in `slot.session_id`. Does nothing if
    /// that session is closed or unknown.
    fn put_engineer_slot(
        &self,
        slot: &EngineerSlotRecord,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Deletes the slot of `account` in `session_id` if it still belongs to `connection_id` (a
    /// newer slot of the same account is kept).
    fn delete_engineer_slot(
        &self,
        session_id: u64,
        account: Uuid,
        connection_id: u64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The engineer slots of a session, by connection id.
    fn engineer_slots(
        &self,
        session_id: u64,
    ) -> impl Future<Output = Result<Vec<EngineerSlotRecord>, StoreError>> + Send;

    /// Checks that the store is reachable.
    fn ping(&self) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// The store selected at startup: PostgreSQL, or memory (development and tests).
#[derive(Debug, Clone)]
pub enum Store {
    Memory(MemoryStore),
    Postgres(PgStore),
}

impl Store {
    /// Whether this is the in-memory store.
    pub const fn is_memory(&self) -> bool {
        matches!(self, Store::Memory(_))
    }
}

macro_rules! delegate {
    ($self:ident, $method:ident ( $($arg:expr),* )) => {
        match $self {
            Store::Memory(s) => s.$method($($arg),*).await,
            Store::Postgres(s) => bounded(STORE_CALL_TIMEOUT, s.$method($($arg),*)).await,
        }
    };
}

impl AccountStore for Store {
    async fn create_account(&self, account: &Account) -> Result<(), StoreError> {
        delegate!(self, create_account(account))
    }
    async fn account_by_username(&self, username: &str) -> Result<Option<Account>, StoreError> {
        delegate!(self, account_by_username(username))
    }
    async fn account_by_id(&self, id: Uuid) -> Result<Option<Account>, StoreError> {
        delegate!(self, account_by_id(id))
    }
    async fn record_login(&self, id: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        delegate!(self, record_login(id, at))
    }
    async fn create_token_family(
        &self,
        family: Uuid,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        delegate!(self, create_token_family(family, account, at))
    }
    async fn family_revoked(&self, family: Uuid) -> Result<Option<bool>, StoreError> {
        delegate!(self, family_revoked(family))
    }
    async fn inactive_families(&self, families: &[Uuid]) -> Result<Vec<Uuid>, StoreError> {
        delegate!(self, inactive_families(families))
    }
    async fn insert_refresh_token(
        &self,
        token: &NewRefreshToken,
        keep_active: usize,
    ) -> Result<(), StoreError> {
        delegate!(self, insert_refresh_token(token, keep_active))
    }
    async fn refresh_token_by_hash(
        &self,
        hash: &TokenHash,
    ) -> Result<Option<RefreshTokenRecord>, StoreError> {
        delegate!(self, refresh_token_by_hash(hash))
    }
    async fn rotate_refresh_token(
        &self,
        old: Uuid,
        successor: &NewRefreshToken,
        at: DateTime<Utc>,
    ) -> Result<RotateOutcome, StoreError> {
        delegate!(self, rotate_refresh_token(old, successor, at))
    }
    async fn revoke_family(&self, family: Uuid, at: DateTime<Utc>) -> Result<(), StoreError> {
        delegate!(self, revoke_family(family, at))
    }
    async fn revoke_account_tokens(
        &self,
        account: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        delegate!(self, revoke_account_tokens(account, at))
    }
    async fn purge_refresh_tokens(&self, before: DateTime<Utc>) -> Result<u64, StoreError> {
        delegate!(self, purge_refresh_tokens(before))
    }
}

impl SessionStore for Store {
    async fn insert_session(&self, record: &SessionRecord) -> Result<(), StoreError> {
        delegate!(self, insert_session(record))
    }
    async fn session(&self, id: u64) -> Result<Option<SessionRecord>, StoreError> {
        delegate!(self, session(id))
    }
    async fn set_session_status(
        &self,
        id: u64,
        status: SessionStatus,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        delegate!(self, set_session_status(id, status, at))
    }
    async fn close_session(
        &self,
        id: u64,
        reason: &str,
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        delegate!(self, close_session(id, reason, at))
    }
    async fn touch_sessions(&self, ids: &[u64], at: DateTime<Utc>) -> Result<(), StoreError> {
        delegate!(self, touch_sessions(ids, at))
    }
    async fn expire_sessions(
        &self,
        idle_before: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<u64, StoreError> {
        delegate!(self, expire_sessions(idle_before, at))
    }
    async fn purge_closed_sessions(&self, ended_before: DateTime<Utc>) -> Result<u64, StoreError> {
        delegate!(self, purge_closed_sessions(ended_before))
    }
    async fn put_engineer_slot(&self, slot: &EngineerSlotRecord) -> Result<(), StoreError> {
        delegate!(self, put_engineer_slot(slot))
    }
    async fn delete_engineer_slot(
        &self,
        session_id: u64,
        account: Uuid,
        connection_id: u64,
    ) -> Result<(), StoreError> {
        delegate!(
            self,
            delete_engineer_slot(session_id, account, connection_id)
        )
    }
    async fn engineer_slots(&self, session_id: u64) -> Result<Vec<EngineerSlotRecord>, StoreError> {
        delegate!(self, engineer_slots(session_id))
    }
    async fn ping(&self) -> Result<(), StoreError> {
        delegate!(self, ping())
    }
}

#[cfg(test)]
pub(crate) mod conformance;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_roundtrip() {
        for role in [
            AccountRole::Driver,
            AccountRole::Engineer,
            AccountRole::Admin,
        ] {
            assert_eq!(AccountRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(
            AccountRole::parse(" Engineer "),
            Some(AccountRole::Engineer)
        );
        assert_eq!(AccountRole::parse("root"), None);
    }

    #[test]
    fn effective_roles() {
        assert_eq!(
            AccountRole::Driver.effective_role(ClientRole::Engineer),
            ClientRole::Driver
        );
        assert_eq!(
            AccountRole::Engineer.effective_role(ClientRole::Driver),
            ClientRole::Engineer
        );
        assert_eq!(
            AccountRole::Admin.effective_role(ClientRole::Engineer),
            ClientRole::Engineer
        );
    }

    #[test]
    fn account_debug_redacts_the_hash() {
        let a = Account {
            id: Uuid::nil(),
            username: "alice".into(),
            display_name: "Alice".into(),
            role: AccountRole::Engineer,
            password_hash: "$argon2id$v=19$secret".into(),
            disabled: false,
            created_at: DateTime::<Utc>::MIN_UTC,
        };
        let text = format!("{a:?}");
        assert!(!text.contains("argon2id"));
        assert!(text.contains("<redacted>"));
    }

    #[tokio::test(start_paused = true)]
    async fn store_calls_are_bounded() {
        let limit = Duration::from_millis(50);
        let stalled = bounded(limit, std::future::pending::<Result<u64, StoreError>>()).await;
        assert!(matches!(stalled, Err(StoreError::Timeout(d)) if d == limit));
        let prompt = bounded(limit, async { Ok::<u64, StoreError>(7) }).await;
        assert!(matches!(prompt, Ok(7)));
    }

    #[test]
    fn status_names_roundtrip() {
        for s in [
            SessionStatus::Live,
            SessionStatus::Grace,
            SessionStatus::Closed,
        ] {
            assert_eq!(SessionStatus::parse(s.as_str()), Some(s));
        }
    }
}

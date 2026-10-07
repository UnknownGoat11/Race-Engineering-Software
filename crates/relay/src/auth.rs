//! Accounts and authentication (D-008, D-020).
//!
//! * Passwords are hashed with Argon2id ([`PasswordHasher`]) on blocking threads, at most
//!   `max_concurrent_password_hashes` at a time. An unknown username is verified against a dummy
//!   hash so the response time does not reveal whether the account exists.
//! * Access tokens are EdDSA JWTs ([`crate::jwt`]), 15 minutes by default.
//! * Refresh tokens are random 256-bit values stored as SHA-256 digests. Every use rotates the
//!   token and its successor gets a fresh sliding expiry (default 90 days since last use, D-020),
//!   so a client that starts at least once per lifetime stays logged in indefinitely. A rotated
//!   token presented again within `refresh_reuse_grace` is accepted (the client may have missed the
//!   AUTH_OK with its successor); later reuse revokes the whole family (theft detection).
//! * An access-token AUTH also returns a refresh token: the relay knows refresh tokens only by
//!   digest and clients keep the refresh token of the newest AUTH_OK, so a successor in the same
//!   family is issued (older ones age out through the family cap).
//! * Credentials never reach a log: they are [`Secret`]s and only digests are stored.

use std::sync::Arc;

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::{DateTime, TimeDelta, Utc};
use protocol::messages::{AccountId, AccountRegister, AuthOk, ClientRole, ErrorCode, Secret};
use thiserror::Error;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::config::{AuthSettings, PasswordHashParams};
use crate::jwt::{AccessClaims, AccessTokenKeys, JWT_ISSUER, JwtError, new_jti};
use crate::secrets::{RandomError, new_token, presented_token_hash, random_bytes};
use crate::store::{
    Account, AccountRole, AccountStore, NewRefreshToken, RotateOutcome, Store, StoreError,
};
use crate::time::{NANOS_PER_SEC, WallClock, datetime_to_ns};

/// Log component of this module.
const COMPONENT: &str = "relay.auth";
/// Shortest username.
pub const MIN_USERNAME_CHARS: usize = 3;
/// Longest username.
pub const MAX_USERNAME_CHARS: usize = 32;
/// Longest display name.
pub const MAX_DISPLAY_NAME_CHARS: usize = 64;
/// Shortest accepted password (characters).
pub const MIN_PASSWORD_CHARS: usize = 10;
/// Longest accepted password (bytes); longer input is refused without hashing.
pub const MAX_PASSWORD_BYTES: usize = 512;
/// Random salt bytes per password hash.
const SALT_BYTES: usize = 16;

/// Invalid account input (registration, `create-user`).
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AccountInputError {
    #[error("a username has {MIN_USERNAME_CHARS} to {MAX_USERNAME_CHARS} characters")]
    UsernameLength,
    #[error("a username may contain only a-z, 0-9, '.', '_' and '-'")]
    UsernameCharacters,
    #[error("a display name has 1 to {MAX_DISPLAY_NAME_CHARS} printable characters")]
    DisplayName,
    #[error(
        "a password has at least {MIN_PASSWORD_CHARS} characters and at most {MAX_PASSWORD_BYTES} bytes"
    )]
    PasswordLength,
}

/// Normalized login name: trimmed, ASCII lower case, `[a-z0-9._-]{3,32}`. The length is checked
/// before anything is copied (the input may be as large as a whole frame payload).
pub fn normalize_username(input: &str) -> Result<String, AccountInputError> {
    let trimmed = input.trim();
    if !(MIN_USERNAME_CHARS..=MAX_USERNAME_CHARS).contains(&trimmed.chars().count()) {
        return Err(AccountInputError::UsernameLength);
    }
    let name = trimmed.to_ascii_lowercase();
    let allowed =
        |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-');
    if !name.bytes().all(allowed) {
        return Err(AccountInputError::UsernameCharacters);
    }
    Ok(name)
}

/// Trimmed display name of 1..=64 characters without control characters.
pub fn validate_display_name(input: &str) -> Result<String, AccountInputError> {
    let name = input.trim();
    let len = name.chars().count();
    if len == 0 || len > MAX_DISPLAY_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(AccountInputError::DisplayName);
    }
    Ok(name.to_owned())
}

/// Checks the password length bounds.
pub fn validate_password(password: &Secret) -> Result<(), AccountInputError> {
    let value = password.expose();
    if value.chars().count() < MIN_PASSWORD_CHARS || value.len() > MAX_PASSWORD_BYTES {
        return Err(AccountInputError::PasswordLength);
    }
    Ok(())
}

/// Authentication failures. [`AuthError::error_code`] maps them to the wire.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("token expired")]
    TokenExpired,
    #[error("account disabled")]
    AccountDisabled,
    #[error("a rotated refresh token was reused; its family was revoked")]
    RefreshReuse,
    #[error("open registration is disabled")]
    RegistrationClosed,
    #[error("username already taken")]
    UsernameTaken,
    #[error(transparent)]
    Input(#[from] AccountInputError),
    #[error("account store failure")]
    Store(#[from] StoreError),
    #[error("invalid Argon2 parameters")]
    HashParams(#[source] argon2::Error),
    #[error("password hashing failed")]
    Hash(#[source] argon2::password_hash::Error),
    #[error("password hashing task failed")]
    Task(#[source] tokio::task::JoinError),
    #[error("password hashing is shut down")]
    HasherClosed,
    #[error(transparent)]
    Random(#[from] RandomError),
    #[error("access token signing failed")]
    Jwt(#[from] JwtError),
}

impl AuthError {
    /// Wire error code.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            AuthError::InvalidCredentials
            | AuthError::AccountDisabled
            | AuthError::RefreshReuse
            | AuthError::UsernameTaken
            | AuthError::Input(_) => ErrorCode::AuthFailed,
            AuthError::TokenExpired => ErrorCode::TokenExpired,
            AuthError::RegistrationClosed => ErrorCode::NotAuthorized,
            AuthError::Store(_)
            | AuthError::HashParams(_)
            | AuthError::Hash(_)
            | AuthError::Task(_)
            | AuthError::HasherClosed
            | AuthError::Random(_)
            | AuthError::Jwt(_) => ErrorCode::Internal,
        }
    }

    /// Message for the client. It never says whether a username exists.
    pub fn client_message(&self) -> String {
        match self {
            AuthError::InvalidCredentials
            | AuthError::AccountDisabled
            | AuthError::RefreshReuse => "invalid credentials".to_owned(),
            AuthError::TokenExpired => "token expired".to_owned(),
            AuthError::RegistrationClosed => "this relay does not allow registration".to_owned(),
            AuthError::UsernameTaken => "username already taken".to_owned(),
            AuthError::Input(e) => e.to_string(),
            _ => "internal relay error".to_owned(),
        }
    }

    /// Whether this is an infrastructure failure (not the client's fault).
    pub fn is_internal(&self) -> bool {
        self.error_code() == ErrorCode::Internal
    }
}

/// Argon2id hashing with bounded concurrency.
pub struct PasswordHasher {
    params: Params,
    permits: Arc<Semaphore>,
    dummy_phc: String,
}

impl std::fmt::Debug for PasswordHasher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PasswordHasher")
            .field("m_cost_kib", &self.params.m_cost())
            .field("t_cost", &self.params.t_cost())
            .field("p_cost", &self.params.p_cost())
            .finish_non_exhaustive()
    }
}

impl PasswordHasher {
    /// A hasher for new hashes with `params`; at most `max_concurrent` hashes run at once.
    pub async fn new(params: PasswordHashParams, max_concurrent: usize) -> Result<Self, AuthError> {
        let params = Params::new(params.m_cost_kib, params.t_cost, params.p_cost, None)
            .map_err(AuthError::HashParams)?;
        let mut hasher = Self {
            params,
            permits: Arc::new(Semaphore::new(max_concurrent.max(1))),
            dummy_phc: String::new(),
        };
        // Timing equalization for unknown usernames: a hash of a random password.
        let dummy = Secret::new(crate::secrets::b64url(&random_bytes::<SALT_BYTES>()?));
        hasher.dummy_phc = hasher.hash(&dummy).await?;
        Ok(hasher)
    }

    /// Argon2id PHC string of `password`.
    pub async fn hash(&self, password: &Secret) -> Result<String, AuthError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| AuthError::HasherClosed)?;
        let salt = random_bytes::<SALT_BYTES>()?;
        let params = self.params.clone();
        let password = password.expose().to_owned();
        tokio::task::spawn_blocking(move || {
            let salt = SaltString::encode_b64(&salt)?;
            let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
            Ok::<String, argon2::password_hash::Error>(
                argon.hash_password(password.as_bytes(), &salt)?.to_string(),
            )
        })
        .await
        .map_err(AuthError::Task)?
        .map_err(AuthError::Hash)
    }

    /// Verifies `password` against `phc`. `None` (unknown account) verifies against a dummy hash
    /// and returns `false`, taking as long as a real verification.
    pub async fn verify(&self, password: &Secret, phc: Option<&str>) -> Result<bool, AuthError> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| AuthError::HasherClosed)?;
        let real = phc.is_some();
        let phc = phc.unwrap_or(&self.dummy_phc).to_owned();
        let password = password.expose().to_owned();
        let matches = tokio::task::spawn_blocking(move || {
            // The parameters are read from the PHC string itself.
            PasswordHash::new(&phc).map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
        })
        .await
        .map_err(AuthError::Task)?
        .map_err(AuthError::Hash)?;
        Ok(real && matches)
    }
}

/// How a connection authenticated (logs, metrics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    Password,
    AccessToken,
    RefreshToken,
    Registration,
}

impl AuthMethod {
    /// Log name.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthMethod::Password => "password",
            AuthMethod::AccessToken => "access_token",
            AuthMethod::RefreshToken => "refresh_token",
            AuthMethod::Registration => "registration",
        }
    }
}

/// A successful authentication: the account and its fresh tokens.
#[derive(Debug, Clone)]
pub struct LoginOk {
    pub account: Account,
    pub method: AuthMethod,
    /// Refresh-token family (login) the tokens belong to. Revoking it ends the login, including
    /// the connections it authenticated (`AccountStore::inactive_families`).
    pub family: Uuid,
    pub access_token: Secret,
    /// Access token expiry, wall clock ns since the Unix epoch.
    pub access_expires_at_ns: u64,
    pub refresh_token: Secret,
    /// Refresh token expiry, wall clock ns since the Unix epoch.
    pub refresh_expires_at_ns: u64,
}

impl LoginOk {
    /// AUTH_OK for a connection whose HELLO asked for `requested` (see
    /// [`AccountRole::effective_role`]).
    pub fn auth_ok(&self, requested: ClientRole) -> AuthOk {
        AuthOk {
            account: AccountId(*self.account.id.as_bytes()),
            username: self.account.username.clone(),
            display_name: self.account.display_name.clone(),
            role: self.account.role.effective_role(requested),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            access_expires_at_ns: self.access_expires_at_ns,
            refresh_expires_at_ns: self.refresh_expires_at_ns,
        }
    }
}

/// A refresh token about to be stored, with its plaintext for the client.
struct IssuedRefresh {
    token: Secret,
    record: NewRefreshToken,
}

/// Accounts, passwords and tokens.
#[derive(Debug)]
pub struct AuthService {
    store: Store,
    keys: AccessTokenKeys,
    hasher: PasswordHasher,
    settings: AuthSettings,
    open_registration: bool,
    clock: WallClock,
}

impl AuthService {
    /// A service over `store` signing access tokens with `keys`.
    pub async fn new(
        store: Store,
        keys: AccessTokenKeys,
        settings: AuthSettings,
        max_concurrent_hashes: usize,
        open_registration: bool,
        clock: WallClock,
    ) -> Result<Self, AuthError> {
        let hasher = PasswordHasher::new(settings.password_hash, max_concurrent_hashes).await?;
        Ok(Self {
            store,
            keys,
            hasher,
            settings,
            open_registration,
            clock,
        })
    }

    /// The store (shared with the session layer).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Whether ACCOUNT_REGISTER is allowed.
    pub fn open_registration(&self) -> bool {
        self.open_registration
    }

    /// AUTH with username and password.
    pub async fn login_password(
        &self,
        username: &str,
        password: &Secret,
    ) -> Result<LoginOk, AuthError> {
        if password.expose().len() > MAX_PASSWORD_BYTES {
            return Err(AuthError::InvalidCredentials);
        }
        let account = match normalize_username(username) {
            Ok(name) => self.store.account_by_username(&name).await?,
            Err(_) => None,
        };
        let verified = self
            .hasher
            .verify(password, account.as_ref().map(|a| a.password_hash.as_str()))
            .await?;
        let Some(account) = account.filter(|_| verified) else {
            return Err(AuthError::InvalidCredentials);
        };
        if account.disabled {
            return Err(AuthError::AccountDisabled);
        }
        self.new_login(account, AuthMethod::Password).await
    }

    /// AUTH with an access token.
    pub async fn login_access_token(&self, token: &Secret) -> Result<LoginOk, AuthError> {
        let now = self.clock.now_utc();
        let claims = match self.keys.verify(token.expose(), self.clock.now_s()) {
            Ok(claims) => claims,
            Err(JwtError::Expired) => return Err(AuthError::TokenExpired),
            Err(e) => {
                tracing::debug!(component = COMPONENT, error = %e, "access token refused");
                return Err(AuthError::InvalidCredentials);
            }
        };
        let (Ok(account_id), Ok(family)) =
            (Uuid::parse_str(&claims.sub), Uuid::parse_str(&claims.fam))
        else {
            return Err(AuthError::InvalidCredentials);
        };
        let account = self
            .store
            .account_by_id(account_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if account.disabled {
            return Err(AuthError::AccountDisabled);
        }
        // A role change after issue invalidates the token.
        if account.role.as_str() != claims.role {
            return Err(AuthError::InvalidCredentials);
        }
        // Logout / revocation of the login also ends its access tokens.
        if self.store.family_revoked(family).await? != Some(false) {
            return Err(AuthError::InvalidCredentials);
        }
        let successor = self.new_refresh_token(&account, family, now)?;
        self.store
            .insert_refresh_token(
                &successor.record,
                self.settings.max_active_refresh_per_family,
            )
            .await?;
        self.issue(account, AuthMethod::AccessToken, family, successor, now)
    }

    /// AUTH with a refresh token: rotates it (sliding expiry).
    pub async fn login_refresh_token(&self, token: &Secret) -> Result<LoginOk, AuthError> {
        let Some(hash) = presented_token_hash(token) else {
            return Err(AuthError::InvalidCredentials);
        };
        let Some(record) = self.store.refresh_token_by_hash(&hash).await? else {
            return Err(AuthError::InvalidCredentials);
        };
        let now = self.clock.now_utc();
        if record.revoked_at.is_some()
            || self.store.family_revoked(record.family_id).await? != Some(false)
        {
            return Err(AuthError::InvalidCredentials);
        }
        if record.expires_at <= now {
            return Err(AuthError::TokenExpired);
        }
        let account = self
            .store
            .account_by_id(record.account_id)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if account.disabled {
            return Err(AuthError::AccountDisabled);
        }
        let keep = self.settings.max_active_refresh_per_family;
        let successor = self.new_refresh_token(&account, record.family_id, now)?;
        let grace =
            TimeDelta::from_std(self.settings.refresh_reuse_grace).unwrap_or(TimeDelta::MAX);
        match record.rotated_at {
            None => match self
                .store
                .rotate_refresh_token(record.id, &successor.record, now)
                .await?
            {
                RotateOutcome::Rotated => {}
                RotateOutcome::Lost => {
                    // Rotated (or revoked) concurrently a moment ago.
                    let current = self.store.refresh_token_by_hash(&hash).await?;
                    if current.is_none_or(|c| c.revoked_at.is_some()) {
                        return Err(AuthError::InvalidCredentials);
                    }
                    self.store
                        .insert_refresh_token(&successor.record, keep)
                        .await?;
                }
            },
            Some(rotated_at) if now.signed_duration_since(rotated_at) <= grace => {
                self.store
                    .insert_refresh_token(&successor.record, keep)
                    .await?;
            }
            Some(_) => {
                self.store.revoke_family(record.family_id, now).await?;
                tracing::warn!(
                    component = COMPONENT,
                    account = %account.id,
                    username = %account.username,
                    family = %record.family_id,
                    "rotated refresh token reused after the grace period: login family revoked"
                );
                return Err(AuthError::RefreshReuse);
            }
        }
        self.issue(
            account,
            AuthMethod::RefreshToken,
            record.family_id,
            successor,
            now,
        )
    }

    /// ACCOUNT_REGISTER (open registration only): creates the account in `role` and logs in.
    pub async fn register(
        &self,
        request: &AccountRegister,
        role: AccountRole,
    ) -> Result<LoginOk, AuthError> {
        if !self.open_registration {
            return Err(AuthError::RegistrationClosed);
        }
        let account = self
            .create_account(
                &request.username,
                &request.display_name,
                role,
                &request.password,
            )
            .await?;
        self.new_login(account, AuthMethod::Registration).await
    }

    /// Creates an account (CLI `create-user`, registration).
    pub async fn create_account(
        &self,
        username: &str,
        display_name: &str,
        role: AccountRole,
        password: &Secret,
    ) -> Result<Account, AuthError> {
        let username = normalize_username(username)?;
        let display_name = validate_display_name(display_name)?;
        validate_password(password)?;
        let password_hash = self.hasher.hash(password).await?;
        let account = Account {
            id: Uuid::new_v4(),
            username,
            display_name,
            role,
            password_hash,
            disabled: false,
            created_at: self.clock.now_utc(),
        };
        match self.store.create_account(&account).await {
            Ok(()) => Ok(account),
            Err(StoreError::Conflict(_)) => Err(AuthError::UsernameTaken),
            Err(e) => Err(e.into()),
        }
    }

    /// Revokes every refresh token (and thereby every access token) of `username`: logout
    /// everywhere. Returns the number of tokens revoked.
    pub async fn revoke_tokens(&self, username: &str) -> Result<u64, AuthError> {
        let name = normalize_username(username)?;
        let account = self
            .store
            .account_by_username(&name)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        Ok(self
            .store
            .revoke_account_tokens(account.id, self.clock.now_utc())
            .await?)
    }

    /// Deletes refresh tokens that expired before now.
    pub async fn purge_expired_tokens(&self) -> Result<u64, AuthError> {
        Ok(self
            .store
            .purge_refresh_tokens(self.clock.now_utc())
            .await?)
    }

    async fn new_login(&self, account: Account, method: AuthMethod) -> Result<LoginOk, AuthError> {
        let now = self.clock.now_utc();
        self.store.record_login(account.id, now).await?;
        let family = Uuid::new_v4();
        self.store
            .create_token_family(family, account.id, now)
            .await?;
        let refresh = self.new_refresh_token(&account, family, now)?;
        self.store
            .insert_refresh_token(&refresh.record, self.settings.max_active_refresh_per_family)
            .await?;
        self.issue(account, method, family, refresh, now)
    }

    fn new_refresh_token(
        &self,
        account: &Account,
        family: Uuid,
        now: DateTime<Utc>,
    ) -> Result<IssuedRefresh, AuthError> {
        let (token, token_hash) = new_token()?;
        let ttl = TimeDelta::from_std(self.settings.refresh_token_ttl).unwrap_or(TimeDelta::MAX);
        let expires_at = now
            .checked_add_signed(ttl)
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        Ok(IssuedRefresh {
            token,
            record: NewRefreshToken {
                id: Uuid::new_v4(),
                family_id: family,
                account_id: account.id,
                token_hash,
                created_at: now,
                expires_at,
            },
        })
    }

    fn issue(
        &self,
        account: Account,
        method: AuthMethod,
        family: Uuid,
        refresh: IssuedRefresh,
        now: DateTime<Utc>,
    ) -> Result<LoginOk, AuthError> {
        let iat = u64::try_from(now.timestamp()).unwrap_or(0);
        let exp = iat.saturating_add(self.settings.access_token_ttl.as_secs().max(1));
        let claims = AccessClaims {
            iss: JWT_ISSUER.to_owned(),
            sub: account.id.to_string(),
            role: account.role.as_str().to_owned(),
            fam: family.to_string(),
            iat,
            exp,
            jti: new_jti()?,
        };
        let access_token = self.keys.sign(&claims)?;
        Ok(LoginOk {
            account,
            method,
            family,
            access_token,
            access_expires_at_ns: exp.saturating_mul(NANOS_PER_SEC),
            refresh_token: refresh.token,
            refresh_expires_at_ns: datetime_to_ns(refresh.record.expires_at),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::store::MemoryStore;

    const T0_NS: u64 = 1_790_000_000 * NANOS_PER_SEC;
    const PASSWORD: &str = "correct horse battery";

    async fn service(open_registration: bool) -> (AuthService, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(T0_NS));
        let clock_now = Arc::clone(&now);
        let settings = AuthSettings {
            password_hash: PasswordHashParams::TEST_ONLY_FAST,
            ..AuthSettings::default()
        };
        let svc = AuthService::new(
            Store::Memory(MemoryStore::new()),
            AccessTokenKeys::generate().expect("rng"),
            settings,
            2,
            open_registration,
            WallClock::from_fn(move || clock_now.load(Ordering::SeqCst)),
        )
        .await
        .expect("service");
        (svc, now)
    }

    fn advance(clock: &AtomicU64, d: Duration) {
        clock.fetch_add(crate::time::duration_ns(d), Ordering::SeqCst);
    }

    #[test]
    fn usernames_are_normalized_and_checked() {
        assert_eq!(normalize_username("  Alice.B "), Ok("alice.b".to_owned()));
        assert_eq!(
            normalize_username("ab"),
            Err(AccountInputError::UsernameLength)
        );
        assert_eq!(
            normalize_username("al ice"),
            Err(AccountInputError::UsernameCharacters)
        );
        assert_eq!(
            normalize_username("élodie"),
            Err(AccountInputError::UsernameCharacters)
        );
        assert!(validate_display_name("  Alice Driver ").is_ok());
        assert!(validate_display_name("bad\nname").is_err());
        assert!(validate_password(&Secret::new("short")).is_err());
        assert!(validate_password(&Secret::new(PASSWORD)).is_ok());
    }

    #[tokio::test]
    async fn password_login_issues_tokens_and_rejects_wrong_passwords() {
        let (svc, _) = service(false).await;
        svc.create_account(
            "Alice",
            "Alice",
            AccountRole::Driver,
            &Secret::new(PASSWORD),
        )
        .await
        .expect("create");
        assert!(matches!(
            svc.create_account(
                "alice",
                "Other",
                AccountRole::Engineer,
                &Secret::new(PASSWORD)
            )
            .await,
            Err(AuthError::UsernameTaken)
        ));
        let ok = svc
            .login_password("ALICE", &Secret::new(PASSWORD))
            .await
            .expect("login");
        assert_eq!(ok.account.username, "alice");
        assert_eq!(ok.access_expires_at_ns, T0_NS + 15 * 60 * NANOS_PER_SEC);
        assert_eq!(
            ok.refresh_expires_at_ns,
            T0_NS + 90 * 86_400 * NANOS_PER_SEC
        );
        assert!(matches!(
            svc.login_password("alice", &Secret::new("wrong password!"))
                .await,
            Err(AuthError::InvalidCredentials)
        ));
        assert!(matches!(
            svc.login_password("nobody", &Secret::new(PASSWORD)).await,
            Err(AuthError::InvalidCredentials)
        ));
        let auth_ok = ok.auth_ok(ClientRole::Engineer);
        assert_eq!(
            auth_ok.role,
            ClientRole::Driver,
            "a driver account stays a driver"
        );
        assert!(!format!("{auth_ok:?}").contains(ok.refresh_token.expose()));
    }

    #[tokio::test]
    async fn refresh_tokens_rotate_and_reuse_revokes_the_family() {
        let (svc, clock) = service(false).await;
        svc.create_account(
            "erin",
            "Erin",
            AccountRole::Engineer,
            &Secret::new(PASSWORD),
        )
        .await
        .expect("create");
        let first = svc
            .login_password("erin", &Secret::new(PASSWORD))
            .await
            .expect("login");
        advance(&clock, Duration::from_secs(3_600));
        let second = svc
            .login_refresh_token(&first.refresh_token)
            .await
            .expect("refresh");
        assert_ne!(second.refresh_token, first.refresh_token);
        assert_eq!(second.family, first.family, "a refresh continues the login");
        // Sliding expiry: 90 days from this use.
        assert_eq!(
            second.refresh_expires_at_ns,
            T0_NS + (3_600 + 90 * 86_400) * NANOS_PER_SEC
        );
        // Within the reuse grace the rotated token still works once the client missed AUTH_OK.
        advance(&clock, Duration::from_secs(10));
        let retry = svc
            .login_refresh_token(&first.refresh_token)
            .await
            .expect("grace reuse");
        // After the grace, reuse is theft: the family is revoked, successors die too.
        advance(&clock, Duration::from_secs(120));
        assert!(matches!(
            svc.login_refresh_token(&first.refresh_token).await,
            Err(AuthError::RefreshReuse)
        ));
        for token in [&second.refresh_token, &retry.refresh_token] {
            assert!(matches!(
                svc.login_refresh_token(token).await,
                Err(AuthError::InvalidCredentials)
            ));
        }
        assert!(matches!(
            svc.login_access_token(&second.access_token).await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn expired_tokens_are_reported_as_expired() {
        let (svc, clock) = service(false).await;
        svc.create_account("dave", "Dave", AccountRole::Driver, &Secret::new(PASSWORD))
            .await
            .expect("create");
        let ok = svc
            .login_password("dave", &Secret::new(PASSWORD))
            .await
            .expect("login");
        let via_access = svc
            .login_access_token(&ok.access_token)
            .await
            .expect("access token valid");
        assert_eq!(via_access.method, AuthMethod::AccessToken);
        advance(&clock, Duration::from_secs(15 * 60));
        assert!(matches!(
            svc.login_access_token(&ok.access_token).await,
            Err(AuthError::TokenExpired)
        ));
        advance(&clock, Duration::from_secs(91 * 86_400));
        assert!(matches!(
            svc.login_refresh_token(&via_access.refresh_token).await,
            Err(AuthError::TokenExpired)
        ));
        assert!(matches!(
            svc.login_refresh_token(&Secret::new("garbage")).await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn revocation_logs_out_everywhere() {
        let (svc, _) = service(false).await;
        svc.create_account(
            "erin",
            "Erin",
            AccountRole::Engineer,
            &Secret::new(PASSWORD),
        )
        .await
        .expect("create");
        let a = svc
            .login_password("erin", &Secret::new(PASSWORD))
            .await
            .expect("login");
        let b = svc
            .login_password("erin", &Secret::new(PASSWORD))
            .await
            .expect("login");
        assert_ne!(a.family, b.family, "every password login is its own login");
        assert_eq!(svc.revoke_tokens("erin").await.expect("revoke"), 2);
        for ok in [&a, &b] {
            assert!(svc.login_refresh_token(&ok.refresh_token).await.is_err());
            assert!(svc.login_access_token(&ok.access_token).await.is_err());
        }
    }

    #[tokio::test]
    async fn registration_requires_the_flag() {
        let request = AccountRegister {
            username: "newbie".into(),
            password: Secret::new(PASSWORD),
            display_name: "Newbie".into(),
        };
        let (closed, _) = service(false).await;
        assert!(matches!(
            closed.register(&request, AccountRole::Engineer).await,
            Err(AuthError::RegistrationClosed)
        ));
        let (open, _) = service(true).await;
        let ok = open
            .register(&request, AccountRole::Engineer)
            .await
            .expect("registered");
        assert_eq!(ok.account.role, AccountRole::Engineer);
        assert_eq!(ok.method, AuthMethod::Registration);
    }
}

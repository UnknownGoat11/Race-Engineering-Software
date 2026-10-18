//! Authentication state, login and logout (D-008, D-020).
//!
//! * The user logs in **once** ([`login`]). The relay returns a short-lived access token (kept in
//!   memory only) and a rotating refresh token, persisted through the [`CredentialStore`]
//!   (Windows Credential Manager). Every later connection, including every app start,
//!   authenticates silently: with the access token while it is valid (no rotation), else with the
//!   refresh token.
//! * Refresh-token use is serialized process-wide (one rotation at a time) and the newest token is
//!   always stored before the next use, so two connections never present the same rotated token.
//!   Share **one** `AuthManager` (in an `Arc`) between the client, login and the connectivity test.
//! * A refused refresh token is deleted and reported as a clear re-login state
//!   ([`crate::state::FailureReason::ReloginRequired`]), never retried in a loop. Only an explicit
//!   [`AuthManager::logout`] or a server-side revocation asks for the password again.
//! * Connections run AUTH in a detached task: stopping or cancelling while AUTH{RefreshToken} is in
//!   flight still persists the rotated successor when the reply arrives (otherwise the next start
//!   would present a rotated token, which the relay treats as reuse after its grace period).
//! * A logout starts a new login generation: an AUTH that completes afterwards is discarded and
//!   cannot log the user back in. An explicit login with an account of the other role (e.g. a
//!   driver account in engineer mode) stores nothing.
//! * Relay accounts have exactly one role, so tokens are kept **per role**
//!   ([`CredentialKind::DriverRefreshToken`] / [`CredentialKind::EngineerRefreshToken`]): each
//!   mode logs in once, and switching modes never presents the other role's token. A token stored
//!   before this split ([`CredentialKind::RefreshToken`]) is used as a fallback and moved to the
//!   slot of the role its AUTH_OK reports.
//!
//! Relay requirement (not enforceable here): if a connection drops after AUTH{RefreshToken} was
//! sent but before AUTH_OK arrived, the client never learns the rotated token. The relay should
//! accept the previous refresh token for a short grace period, otherwise that rare drop forces a
//! re-login.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use protocol::messages::{
    AccountId, AccountRegister, Auth, AuthOk, ClientRole, Disconnect, DisconnectReason, ErrorCode,
    Message, Secret,
};
use thiserror::Error;
use tokio::sync::OwnedMutexGuard;
use tokio::time::Instant;

use crate::clock::LinkClock;
use crate::config::ClientConfig;
use crate::credentials::{CredentialError, CredentialKind, CredentialStore, StoredDriverSession};
use crate::endpoint::RelayEndpoint;
use crate::error::ConnectError;
use crate::handshake::{self, HandshakeError, Link};
use crate::state::ReloginCause;
use crate::transport::{FrameWriter, TelemetryTransport};

/// An access token is not used within this margin before its expiry.
const ACCESS_TOKEN_MARGIN: Duration = Duration::from_secs(60);

/// The authenticated account, as reported by AUTH_OK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    pub account: AccountId,
    pub username: String,
    pub display_name: String,
    pub role: ClientRole,
}

/// A login is needed before connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReloginNeeded(pub ReloginCause);

/// The credential one AUTH attempt presents. Refresh and password plans hold the process-wide
/// rotation lock until the reply was processed. `generation` is the login generation the plan was
/// made in: a [`AuthManager::logout`] meanwhile makes its outcome irrelevant (nothing is stored).
/// `role` is the mode the plan was made for (the token slot it came from).
pub(crate) enum AuthPlan {
    Access {
        token: Secret,
        role: ClientRole,
        generation: u64,
    },
    Refresh {
        token: Secret,
        role: ClientRole,
        /// Loaded from the pre-role [`CredentialKind::RefreshToken`] key.
        legacy: bool,
        generation: u64,
        _rotation: OwnedMutexGuard<()>,
    },
    Password {
        username: String,
        password: Secret,
        role: ClientRole,
        generation: u64,
        _rotation: OwnedMutexGuard<()>,
    },
}

impl AuthPlan {
    /// Login generation the plan was made in.
    pub(crate) const fn generation(&self) -> u64 {
        match self {
            AuthPlan::Access { generation, .. }
            | AuthPlan::Refresh { generation, .. }
            | AuthPlan::Password { generation, .. } => *generation,
        }
    }

    /// The mode the plan was made for.
    pub(crate) const fn role(&self) -> ClientRole {
        match self {
            AuthPlan::Access { role, .. }
            | AuthPlan::Refresh { role, .. }
            | AuthPlan::Password { role, .. } => *role,
        }
    }

    /// The AUTH payload.
    pub(crate) fn message(&self) -> Auth {
        match self {
            AuthPlan::Access { token, .. } => Auth::AccessToken {
                token: token.clone(),
            },
            AuthPlan::Refresh { token, .. } => Auth::RefreshToken {
                token: token.clone(),
            },
            AuthPlan::Password {
                username, password, ..
            } => Auth::Password {
                username: username.clone(),
                password: password.clone(),
            },
        }
    }
}

/// The tokens of one role's account.
#[derive(Debug, Default)]
struct RoleTokens {
    /// Access token and the local instant until which it is used.
    access: Option<(Secret, Instant)>,
    /// Newest refresh token known to this process (also persisted).
    refresh: Option<Secret>,
}

/// Index of a role's [`RoleTokens`].
const fn role_index(role: ClientRole) -> usize {
    match role {
        ClientRole::Driver => 0,
        ClientRole::Engineer => 1,
    }
}

/// Number of roles (token slots).
const ROLE_COUNT: usize = 2;

#[derive(Debug, Default)]
struct AuthState {
    /// Per role: relay accounts have exactly one role, so the driver and the engineer login of
    /// one user are separate accounts with separate tokens.
    tokens: [RoleTokens; ROLE_COUNT],
    account: Option<AccountInfo>,
    /// Incremented by every logout. AUTH results of an older generation are discarded, so an AUTH
    /// that completes after a logout cannot log the user back in.
    generation: u64,
    last_persist_error: Option<String>,
    pending_persist: Option<(CredentialKind, Secret)>,
}

/// Process-wide authentication state for one relay endpoint.
pub struct AuthManager {
    store: Arc<dyn CredentialStore>,
    namespace: String,
    state: Mutex<AuthState>,
    rotation: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

impl AuthManager {
    /// Manager for `endpoint`, persisting through `store`.
    pub fn new(store: Arc<dyn CredentialStore>, endpoint: &RelayEndpoint) -> Self {
        Self {
            store,
            namespace: endpoint.credential_namespace(),
            state: Mutex::new(AuthState::default()),
            rotation: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Credential namespace of the endpoint.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Whether a refresh token of `role`'s account is available (the user logged in before in
    /// that mode). A token stored before tokens were kept per role counts for either mode until
    /// its first use reveals its role.
    pub fn is_logged_in(&self, role: ClientRole) -> Result<bool, CredentialError> {
        if self.state.lock().tokens[role_index(role)].refresh.is_some() {
            return Ok(true);
        }
        for kind in [
            CredentialKind::refresh_token(role),
            CredentialKind::RefreshToken,
        ] {
            if self.store.load(&self.namespace, kind)?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Sets a local authenticated user session without requiring an online relay server.
    /// The token is persisted into the credential store so the user remains logged in across restarts.
    pub fn set_local_login(
        &self,
        role: ClientRole,
        username: &str,
        display_name: &str,
    ) -> Result<AccountInfo, CredentialError> {
        let account = AccountInfo {
            account: protocol::messages::AccountId([0; 16]),
            username: username.to_owned(),
            display_name: display_name.to_owned(),
            role,
        };
        let token = Secret::new(format!("local:{username}:{display_name}"));
        let mut state = self.state.lock();
        state.account = Some(account.clone());
        state.tokens[role_index(role)].refresh = Some(token.clone());
        drop(state);
        self.store.store(
            &self.namespace,
            CredentialKind::refresh_token(role),
            &token,
        )?;
        Ok(account)
    }

    /// The account of the most recent successful AUTH in this process.
    pub fn account(&self) -> Option<AccountInfo> {
        let mut state = self.state.lock();
        if state.account.is_some() {
            return state.account.clone();
        }
        for role in [ClientRole::Driver, ClientRole::Engineer] {
            if let Ok(Some(token)) = self
                .store
                .load(&self.namespace, CredentialKind::refresh_token(role))
            {
                let secret_str = token.expose();
                if let Some(rest) = secret_str.strip_prefix("local:") {
                    let mut parts = rest.splitn(2, ':');
                    let username = parts.next().unwrap_or("user");
                    let display_name = parts.next().unwrap_or(username);
                    let acc = AccountInfo {
                        account: protocol::messages::AccountId([0; 16]),
                        username: username.to_string(),
                        display_name: display_name.to_string(),
                        role,
                    };
                    state.account = Some(acc.clone());
                    return Some(acc);
                }
            }
        }
        None
    }

    /// Logs out: forgets the tokens in memory and deletes the stored refresh token and the stored
    /// driver session. The next connection needs [`login`]. An AUTH still in flight cannot undo
    /// this: its result is discarded (it belongs to the previous login generation).
    pub fn logout(&self) -> Result<(), CredentialError> {
        // The state lock is held across the deletes so that no concurrent AUTH result can be
        // persisted in between (see `accepted`).
        let mut state = self.state.lock();
        let generation = state.generation.wrapping_add(1);
        *state = AuthState {
            generation,
            ..AuthState::default()
        };
        for kind in [
            CredentialKind::RefreshToken,
            CredentialKind::DriverRefreshToken,
            CredentialKind::EngineerRefreshToken,
            CredentialKind::DriverSession,
        ] {
            self.store.delete(&self.namespace, kind)?;
        }
        drop(state);
        tracing::info!(component = "transport", "logged out");
        Ok(())
    }

    /// `role`'s access token if it is usable at `now`, with the current login generation.
    fn valid_access(&self, now: Instant, role: ClientRole) -> Option<(Secret, u64)> {
        let state = self.state.lock();
        state.tokens[role_index(role)]
            .access
            .as_ref()
            .filter(|(_, until)| now < *until)
            .map(|(token, _)| (token.clone(), state.generation))
    }

    /// The credential for the next AUTH in `role`'s mode.
    pub(crate) async fn plan(
        &self,
        now: Instant,
        role: ClientRole,
    ) -> Result<AuthPlan, HandshakeError> {
        if let Some((token, generation)) = self.valid_access(now, role) {
            return Ok(AuthPlan::Access {
                token,
                role,
                generation,
            });
        }
        let wait_started = Instant::now();
        let rotation = Arc::clone(&self.rotation).lock_owned().await;
        // Another connection may have refreshed while this one waited for the lock. Re-check at
        // `now` plus the time spent waiting.
        if let Some((token, generation)) = self.valid_access(now + wait_started.elapsed(), role) {
            return Ok(AuthPlan::Access {
                token,
                role,
                generation,
            });
        }
        let (in_memory, generation) = {
            let state = self.state.lock();
            (
                state.tokens[role_index(role)].refresh.clone(),
                state.generation,
            )
        };
        let (token, legacy) = match in_memory {
            Some(token) => (token, false),
            None => {
                let (stored, legacy) = match self
                    .store
                    .load(&self.namespace, CredentialKind::refresh_token(role))?
                {
                    Some(token) => (Some(token), false),
                    None => (
                        self.store
                            .load(&self.namespace, CredentialKind::RefreshToken)?,
                        true,
                    ),
                };
                let Some(token) = stored else {
                    return Err(HandshakeError::Relogin(ReloginCause::NotLoggedIn));
                };
                // A pre-role token is not cached for `role`: its account may have the other role
                // (`accepted` files it under the role the relay reports).
                let mut state = self.state.lock();
                // A logout since the generation was read deleted this token: do not cache it.
                if state.generation == generation && !legacy {
                    state.tokens[role_index(role)].refresh = Some(token.clone());
                }
                (token, legacy)
            }
        };
        Ok(AuthPlan::Refresh {
            token,
            role,
            legacy,
            generation,
            _rotation: rotation,
        })
    }

    /// Plan for an explicit password login in `role`'s mode.
    pub(crate) async fn password_plan(
        &self,
        username: &str,
        password: Secret,
        role: ClientRole,
    ) -> AuthPlan {
        let rotation = Arc::clone(&self.rotation).lock_owned().await;
        AuthPlan::Password {
            username: username.to_owned(),
            password,
            role,
            generation: self.state.lock().generation,
            _rotation: rotation,
        }
    }

    /// AUTH_OK received for `plan`: keeps the access token in memory and persists the (possibly
    /// rotated) refresh token. A persistence failure is logged; the login then lasts until the app
    /// exits. `server_time_ns` is the relay clock of its HELLO, used to turn the relay-clock expiry
    /// into a local deadline without trusting the local wall clock. If the user logged out since
    /// `plan` was made, nothing is kept and the result is "log in again".
    pub(crate) fn accepted(
        &self,
        plan: AuthPlan,
        ok: &AuthOk,
        server_time_ns: u64,
        now: Instant,
    ) -> Result<AccountInfo, ReloginNeeded> {
        let account = AccountInfo {
            account: ok.account,
            username: ok.username.clone(),
            display_name: ok.display_name.clone(),
            role: ok.role,
        };
        let lifetime = Duration::from_nanos(ok.access_expires_at_ns.saturating_sub(server_time_ns));
        // An empty token means "none issued" (e.g. a relay that does not rotate on an access-token
        // AUTH): the refresh token we hold stays valid and must never be overwritten by "".
        let refresh = Some(&ok.refresh_token).filter(|t| !t.expose().is_empty());
        let access = Some(&ok.access_token).filter(|t| !t.expose().is_empty());
        // Held until the refresh token is persisted, so a concurrent logout is ordered strictly
        // before or after this (never "deleted, then re-stored by a stale AUTH").
        let mut state = self.state.lock();
        if plan.generation() != state.generation {
            drop(state);
            tracing::info!(
                component = "transport",
                "AUTH completed after a logout: its tokens are discarded"
            );
            return Err(ReloginNeeded(ReloginCause::NotLoggedIn));
        }
        // Tokens are filed under the role the relay reports for the account. A token presented
        // from the pre-role key (`legacy`) or from the other mode's slot (`misfiled`) moves there.
        let slot = role_index(ok.role);
        let (presented, legacy) = match &plan {
            AuthPlan::Refresh { token, legacy, .. } => (Some(token.clone()), *legacy),
            _ => (None, false),
        };
        let misfiled = !matches!(plan, AuthPlan::Password { .. }) && plan.role() != ok.role;
        let move_token = legacy || misfiled;
        let rotated = refresh.is_some_and(|r| state.tokens[slot].refresh.as_ref() != Some(r));
        if let Some(r) = refresh {
            state.tokens[slot].refresh = Some(r.clone());
        } else if let Some(p) = presented.filter(|_| move_token) {
            // Not rotated: the presented token stays valid in its proper slot.
            state.tokens[slot].refresh = Some(p);
        } else if state.tokens[slot].refresh.is_none() {
            tracing::warn!(
                component = "transport",
                "AUTH_OK carried no refresh token: the login lasts until the app exits"
            );
        }
        if misfiled {
            state.tokens[role_index(plan.role())] = RoleTokens::default();
        }
        state.tokens[slot].access = access.and_then(|token| {
            lifetime
                .checked_sub(ACCESS_TOKEN_MARGIN)
                .filter(|usable| !usable.is_zero())
                .map(|usable| (token.clone(), now + usable))
        });
        state.account = Some(account.clone());
        if (rotated || move_token)
            && let Some(token) = state.tokens[slot].refresh.clone()
        {
            let kind = CredentialKind::refresh_token(ok.role);
            match self.store.store(&self.namespace, kind, &token) {
                Ok(()) => {
                    state.pending_persist = None;
                    state.last_persist_error = None;
                    // Delete the old copy only once the new one is stored.
                    let old = if legacy {
                        Some(CredentialKind::RefreshToken)
                    } else if misfiled {
                        Some(CredentialKind::refresh_token(plan.role()))
                    } else {
                        None
                    };
                    if let Some(old) = old
                        && let Err(e) = self.store.delete(&self.namespace, old)
                    {
                        tracing::warn!(component = "transport", error = %e, "could not delete the moved refresh token");
                    }
                }
                Err(e) => {
                    tracing::error!(
                        component = "transport",
                        error = %e,
                        "could not persist the refresh token; the login lasts until the app exits"
                    );
                    state.last_persist_error = Some(e.to_string());
                    state.pending_persist = Some((kind, token));
                }
            }
        }
        drop(state);
        // Releases the rotation lock only after the newest refresh token is known.
        drop(plan);
        Ok(account)
    }

    /// The relay refused the access token of `plan`: it is forgotten (unless a logout already
    /// started a new login generation).
    pub(crate) fn access_rejected(&self, plan: &AuthPlan) {
        let mut state = self.state.lock();
        if state.generation == plan.generation() {
            state.tokens[role_index(plan.role())].access = None;
        }
    }

    /// The relay refused the refresh token of `plan` (revoked or expired): it is deleted. After a
    /// logout meanwhile there is nothing left to delete (and a newer login must not be touched).
    pub(crate) fn refresh_rejected(&self, plan: &AuthPlan) {
        let mut state = self.state.lock();
        if state.generation != plan.generation() {
            return;
        }
        state.tokens[role_index(plan.role())] = RoleTokens::default();
        let kind = match plan {
            AuthPlan::Refresh { legacy: true, .. } => CredentialKind::RefreshToken,
            _ => CredentialKind::refresh_token(plan.role()),
        };
        if let Err(e) = self.store.delete(&self.namespace, kind) {
            tracing::error!(component = "transport", error = %e, "could not delete the refused refresh token");
        }
        drop(state);
        tracing::warn!(
            component = "transport",
            "refresh token refused by the relay: log in again"
        );
    }

    /// The driver's stored session (driver role only).
    pub(crate) fn load_driver_session(
        &self,
    ) -> Result<Option<StoredDriverSession>, CredentialError> {
        match self
            .store
            .load(&self.namespace, CredentialKind::DriverSession)?
        {
            Some(s) => StoredDriverSession::decode(&s),
            None => Ok(None),
        }
    }

    /// Persists the driver's session (driver role only).
    pub(crate) fn store_driver_session(
        &self,
        session: &StoredDriverSession,
    ) -> Result<(), CredentialError> {
        self.store.store(
            &self.namespace,
            CredentialKind::DriverSession,
            &session.encode(),
        )
    }

    /// Persists an "ended" tombstone when deleting the session failed (M11).
    pub(crate) fn store_driver_session_tombstone(&self) -> Result<(), CredentialError> {
        self.store.store(
            &self.namespace,
            CredentialKind::DriverSession,
            &Secret::new("v1:ended".to_owned()),
        )
    }

    /// Forgets the driver's stored session.
    pub(crate) fn clear_driver_session(&self) -> Result<(), CredentialError> {
        self.store
            .delete(&self.namespace, CredentialKind::DriverSession)
    }

    /// The last credential store error, if any (H4).
    pub fn last_persist_error(&self) -> Option<String> {
        self.state.lock().last_persist_error.clone()
    }

    /// Retries storing any pending credential that previously failed (H4).
    pub fn retry_pending_persists(&self) -> Result<(), CredentialError> {
        let pending = self.state.lock().pending_persist.clone();
        if let Some((kind, token)) = pending {
            match self.store.store(&self.namespace, kind, &token) {
                Ok(()) => {
                    let mut state = self.state.lock();
                    state.pending_persist = None;
                    state.last_persist_error = None;
                    Ok(())
                }
                Err(e) => {
                    self.state.lock().last_persist_error = Some(e.to_string());
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }
}

/// Login or registration failed.
#[derive(Debug, Error)]
pub enum LoginError {
    #[error("cannot reach the relay")]
    Connect(#[from] ConnectError),
    #[error("username or password is wrong")]
    InvalidCredentials,
    #[error("the relay refused: {code:?} ({message})")]
    Refused { code: ErrorCode, message: String },
    /// The account has the other role (e.g. a driver account in engineer mode). Nothing was
    /// stored; a previous login is unchanged.
    #[error("{}", wrong_role_text(*requested))]
    RoleMismatch {
        requested: ClientRole,
        granted: ClientRole,
    },
    #[error("login failed: {0}")]
    Handshake(#[from] HandshakeError),
}

/// User-facing text of [`LoginError::RoleMismatch`].
const fn wrong_role_text(requested: ClientRole) -> &'static str {
    match requested {
        ClientRole::Driver => "this account is not a driver account",
        ClientRole::Engineer => "this account is not an engineer account",
    }
}

/// Maps a handshake failure of an explicit login.
fn login_error(e: HandshakeError) -> LoginError {
    match e {
        HandshakeError::Refused {
            code: ErrorCode::AuthFailed,
            ..
        } => LoginError::InvalidCredentials,
        HandshakeError::Refused { code, message, .. } => LoginError::Refused { code, message },
        HandshakeError::RoleMismatch { requested, granted } => {
            LoginError::RoleMismatch { requested, granted }
        }
        other => LoginError::Handshake(other),
    }
}

/// Logs in with username and password (the only time the password is used; it is never stored).
/// Afterwards every connection authenticates silently. An account of the other role (e.g. a driver
/// account in engineer mode) fails with [`LoginError::RoleMismatch`] and nothing is stored.
pub async fn login<T: TelemetryTransport>(
    transport: &T,
    config: &ClientConfig,
    auth: &AuthManager,
    role: ClientRole,
    username: &str,
    password: Secret,
) -> Result<AccountInfo, LoginError> {
    let mut link = open_link(transport, config).await?;
    let server = handshake::hello(&mut link, config, role).await?;
    let plan = auth.password_plan(username, password, role).await;
    let result = handshake::authenticate(&mut link, auth, role, &server, Some(plan)).await;
    goodbye(&mut link).await;
    let account = result.map_err(login_error)?;
    tracing::info!(component = "transport", "logged in");
    Ok(account)
}

/// Registers a new account (only if the relay allows open registration) and logs it in. The relay
/// is expected to answer ACCOUNT_REGISTER with AUTH_OK (or ERROR).
pub async fn register<T: TelemetryTransport>(
    transport: &T,
    config: &ClientConfig,
    auth: &AuthManager,
    role: ClientRole,
    username: &str,
    password: Secret,
    display_name: &str,
) -> Result<AccountInfo, LoginError> {
    const STAGE: &str = "ACCOUNT_REGISTER";
    let mut link = open_link(transport, config).await?;
    let server = handshake::hello(&mut link, config, role).await?;
    let plan = auth.password_plan(username, password.clone(), role).await;
    link.send(&Message::AccountRegister(AccountRegister {
        username: username.to_owned(),
        password,
        display_name: display_name.to_owned(),
    }))
    .await?;
    let reply = link.recv_message(STAGE).await;
    goodbye(&mut link).await;
    match reply?.1 {
        // Like an explicit login: an account of the other role is not kept.
        Message::AuthOk(ok) if ok.role != role => Err(LoginError::RoleMismatch {
            requested: role,
            granted: ok.role,
        }),
        Message::AuthOk(ok) => auth
            .accepted(plan, &ok, server.server_time_ns, link.clock.now())
            .map_err(|relogin| HandshakeError::from(relogin).into()),
        Message::Error(e) => Err(LoginError::Refused {
            code: e.code,
            message: e.message,
        }),
        other => Err(handshake::unexpected(STAGE, other).into()),
    }
}

async fn open_link<T: TelemetryTransport>(
    transport: &T,
    config: &ClientConfig,
) -> Result<Link<T::Reader, T::Writer>, ConnectError> {
    let conn = transport.connect(&config.endpoint).await?;
    Ok(Link {
        reader: conn.reader,
        writer: conn.writer,
        stats: conn.stats,
        info: conn.info,
        clock: LinkClock::with_epoch(config.mono_epoch),
        reply_timeout: config.timing.handshake_timeout,
    })
}

/// Best-effort graceful close of a short-lived connection.
async fn goodbye<R, W: FrameWriter>(link: &mut Link<R, W>)
where
    R: crate::transport::FrameReader,
{
    let bye = Message::Disconnect(Disconnect {
        reason: DisconnectReason::ClientShutdown,
        message: None,
    });
    if link.send(&bye).await.is_ok() {
        let _ = tokio::time::timeout(link.reply_timeout, link.writer.close()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryCredentialStore;
    use protocol::version::Environment;

    fn manager() -> (AuthManager, Arc<MemoryCredentialStore>) {
        let store = Arc::new(MemoryCredentialStore::new());
        let endpoint = RelayEndpoint::new("relay.example.com", 443, "EU", Environment::Production)
            .expect("endpoint");
        (AuthManager::new(store.clone(), &endpoint), store)
    }

    fn auth_ok(refresh: &str, access_expires_at_ns: u64) -> AuthOk {
        AuthOk {
            account: AccountId([7; 16]),
            username: "alice".into(),
            display_name: "Alice".into(),
            role: ClientRole::Driver,
            access_token: Secret::new("access-1"),
            refresh_token: Secret::new(refresh),
            access_expires_at_ns,
            refresh_expires_at_ns: u64::MAX,
        }
    }

    const SERVER_NOW: u64 = 1_790_000_000_000_000_000;

    #[tokio::test(start_paused = true)]
    async fn not_logged_in_without_a_refresh_token() {
        let (m, _) = manager();
        assert!(!m.is_logged_in(ClientRole::Driver).expect("store"));
        assert!(matches!(
            m.plan(Instant::now(), ClientRole::Driver).await,
            Err(HandshakeError::Relogin(ReloginCause::NotLoggedIn))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_then_access_token_and_rotation_is_persisted() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let now = Instant::now();
        let plan = m.plan(now, ClientRole::Driver).await.expect("plan");
        assert!(
            matches!(plan.message(), Auth::RefreshToken { ref token } if token.expose() == "r1")
        );
        // 15-minute access token.
        let ok = auth_ok("r2", SERVER_NOW + 15 * 60 * 1_000_000_000);
        let account = m.accepted(plan, &ok, SERVER_NOW, now).expect("accepted");
        assert_eq!(account.username, "alice");
        assert_eq!(
            store
                .load(m.namespace(), CredentialKind::DriverRefreshToken)
                .expect("load"),
            Some(Secret::new("r2")),
            "rotated refresh token persisted"
        );
        // Next connection uses the access token (no rotation) ...
        assert!(matches!(
            m.plan(now, ClientRole::Driver).await.expect("plan"),
            AuthPlan::Access { .. }
        ));
        // ... until 60 s before its expiry.
        let late = now + Duration::from_secs(15 * 60 - 59);
        assert!(matches!(
            m.plan(late, ClientRole::Driver).await.expect("plan"),
            AuthPlan::Refresh { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn refused_refresh_token_is_deleted() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let plan = m
            .plan(Instant::now(), ClientRole::Driver)
            .await
            .expect("plan");
        m.refresh_rejected(&plan);
        drop(plan);
        assert!(!m.is_logged_in(ClientRole::Driver).expect("store"));
        assert!(store.keys().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_auth_completing_after_a_logout_cannot_log_back_in() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let now = Instant::now();
        // AUTH{RefreshToken} is in flight when the user logs out.
        let plan = m.plan(now, ClientRole::Driver).await.expect("plan");
        m.logout().expect("logout");
        let ok = auth_ok("r2", SERVER_NOW + 15 * 60 * 1_000_000_000);
        assert_eq!(
            m.accepted(plan, &ok, SERVER_NOW, now),
            Err(ReloginNeeded(ReloginCause::NotLoggedIn))
        );
        assert!(store.keys().is_empty(), "nothing re-persisted");
        assert!(!m.is_logged_in(ClientRole::Driver).expect("store"));
        assert_eq!(m.account(), None);
        assert!(matches!(
            m.plan(now, ClientRole::Driver).await,
            Err(HandshakeError::Relogin(ReloginCause::NotLoggedIn))
        ));

        // A refusal of a pre-logout token never touches a newer login.
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r9"),
            )
            .expect("seed");
        let stale = m.plan(now, ClientRole::Driver).await.expect("plan");
        m.logout().expect("logout");
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r10"),
            )
            .expect("new login");
        m.refresh_rejected(&stale);
        assert_eq!(
            store
                .load(m.namespace(), CredentialKind::DriverRefreshToken)
                .expect("load"),
            Some(Secret::new("r10"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn logout_deletes_refresh_token_and_driver_session() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        m.store_driver_session(&StoredDriverSession {
            session_id: 3,
            resume_token: Secret::new("t"),
        })
        .expect("seed");
        assert_eq!(store.keys().len(), 2);
        m.logout().expect("logout");
        assert!(store.keys().is_empty());
        assert_eq!(m.account(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_use_is_serialized() {
        let (m, store) = manager();
        let m = Arc::new(m);
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let first = m
            .plan(Instant::now(), ClientRole::Driver)
            .await
            .expect("plan");
        let m2 = Arc::clone(&m);
        let second = tokio::spawn(async move {
            m2.plan(Instant::now(), ClientRole::Driver)
                .await
                .map(|p| p.message())
        });
        tokio::task::yield_now().await;
        assert!(!second.is_finished(), "second refresh waits for the first");
        let ok = auth_ok("r2", SERVER_NOW + 15 * 60 * 1_000_000_000);
        m.accepted(first, &ok, SERVER_NOW, Instant::now())
            .expect("accepted");
        let msg = second.await.expect("join").expect("plan");
        assert!(
            matches!(msg, Auth::AccessToken { .. }),
            "the waiter uses the fresh access token instead of the rotated refresh token"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn empty_tokens_in_auth_ok_never_replace_the_stored_ones() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::DriverRefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let now = Instant::now();
        let plan = m.plan(now, ClientRole::Driver).await.expect("plan");
        let mut ok = auth_ok("", SERVER_NOW + 15 * 60 * 1_000_000_000);
        ok.access_token = Secret::new("");
        m.accepted(plan, &ok, SERVER_NOW, now).expect("accepted");
        assert_eq!(
            store
                .load(m.namespace(), CredentialKind::DriverRefreshToken)
                .expect("load"),
            Some(Secret::new("r1")),
            "an empty refresh token is \"none issued\", not a new token"
        );
        // No (empty) access token is used: the next AUTH presents the refresh token again.
        match m.plan(now, ClientRole::Driver).await.expect("plan") {
            AuthPlan::Refresh { token, .. } => assert_eq!(token.expose(), "r1"),
            _ => panic!("expected the stored refresh token"),
        }
    }

    const FIFTEEN_MIN_NS: u64 = 15 * 60 * 1_000_000_000;

    #[tokio::test(start_paused = true)]
    async fn driver_and_engineer_logins_are_kept_apart() {
        // F7: switching modes never presents the other role's token.
        let (m, store) = manager();
        let now = Instant::now();
        let plan = m
            .password_plan("alice", Secret::new("pw"), ClientRole::Driver)
            .await;
        m.accepted(
            plan,
            &auth_ok("rd", SERVER_NOW + FIFTEEN_MIN_NS),
            SERVER_NOW,
            now,
        )
        .expect("driver login");
        assert!(m.is_logged_in(ClientRole::Driver).expect("store"));
        assert!(!m.is_logged_in(ClientRole::Engineer).expect("store"));
        assert!(matches!(
            m.plan(now, ClientRole::Engineer).await,
            Err(HandshakeError::Relogin(ReloginCause::NotLoggedIn))
        ));
        let plan = m
            .password_plan("bob", Secret::new("pw"), ClientRole::Engineer)
            .await;
        let mut ok = auth_ok("re", SERVER_NOW + FIFTEEN_MIN_NS);
        ok.role = ClientRole::Engineer;
        ok.access_token = Secret::new("access-e");
        m.accepted(plan, &ok, SERVER_NOW, now)
            .expect("engineer login");
        for (role, kind, refresh, access) in [
            (
                ClientRole::Driver,
                CredentialKind::DriverRefreshToken,
                "rd",
                "access-1",
            ),
            (
                ClientRole::Engineer,
                CredentialKind::EngineerRefreshToken,
                "re",
                "access-e",
            ),
        ] {
            assert_eq!(
                store.load(m.namespace(), kind).expect("load"),
                Some(Secret::new(refresh))
            );
            match m.plan(now, role).await.expect("plan") {
                AuthPlan::Access { token, .. } => assert_eq!(token.expose(), access),
                _ => panic!("expected {role:?}'s access token"),
            }
        }
        m.logout().expect("logout");
        assert!(store.keys().is_empty(), "logout forgets both logins");
    }

    #[tokio::test(start_paused = true)]
    async fn a_pre_role_token_moves_to_its_accounts_role() {
        let (m, store) = manager();
        store
            .store(
                m.namespace(),
                CredentialKind::RefreshToken,
                &Secret::new("r1"),
            )
            .expect("seed");
        let now = Instant::now();
        assert!(m.is_logged_in(ClientRole::Engineer).expect("store"));
        // Used in engineer mode, but the account is a driver account (not rotated).
        let plan = m.plan(now, ClientRole::Engineer).await.expect("plan");
        let mut ok = auth_ok("", SERVER_NOW + FIFTEEN_MIN_NS);
        ok.role = ClientRole::Driver;
        let account = m.accepted(plan, &ok, SERVER_NOW, now).expect("accepted");
        assert_eq!(account.role, ClientRole::Driver);
        assert_eq!(
            store
                .load(m.namespace(), CredentialKind::DriverRefreshToken)
                .expect("load"),
            Some(Secret::new("r1"))
        );
        assert_eq!(
            store
                .load(m.namespace(), CredentialKind::RefreshToken)
                .expect("load"),
            None
        );
        assert!(!m.is_logged_in(ClientRole::Engineer).expect("store"));
        assert!(m.is_logged_in(ClientRole::Driver).expect("store"));
    }
}

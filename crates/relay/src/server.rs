//! Relay startup, the accept loop, housekeeping and shutdown.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::TimeDelta;
use protocol::frame::NO_SESSION;
use protocol::messages::{ErrorCode, ErrorMsg, Message};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::task::TaskTracker;

use crate::admin;
use crate::auth::{AuthError, AuthService};
use crate::codes::{CodeHasher, CodeHasherError};
use crate::config::{ConfigError, RelayConfig, environment_name};
use crate::conn::{flush_slot_writes, serve_connection};
use crate::jwt::{AccessTokenKeys, KeyDerivationError, KeyFileError};
use crate::metrics::{Metrics, RegistryGauges};
use crate::registry::{Housekeeping, Registry, RegistryConfig, SessionCell, SessionDebug};
use crate::secrets::RandomError;
use crate::state::RelayState;
use crate::store::{AccountStore as _, CLOSE_REASON_EXPIRED, SessionStore as _, Store};
use crate::tickets::{TICKET_KEY_LABEL, TicketKeyError, TicketSealer};
use crate::time::{WallClock, now_utc};
use crate::tls::ReloadingTls;

/// Log component of this module.
const COMPONENT: &str = "relay.server";
/// Period of the rate-limiter sweep (forgets idle keys).
const LIMITER_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// Pause after a failed `accept` (e.g. out of file descriptors) before trying again.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);
/// Logins checked per store query of the revocation check.
const REVOCATION_CHECK_BATCH: usize = 1_000;
/// Expired sessions whose store close is owed while the store is slow, at most.
const MAX_OWED_SESSION_CLOSES: usize = 4_096;

/// Startup failures.
#[derive(Debug, Error)]
pub enum StartError {
    #[error("invalid configuration")]
    Config(#[from] ConfigError),
    #[error("cannot load the JWT signing key")]
    JwtKey(#[from] KeyFileError),
    #[error("random generator failed")]
    Random(#[from] RandomError),
    #[error("connection code hasher unavailable")]
    CodeHasher(#[from] CodeHasherError),
    #[error("cannot derive the engineer ticket key")]
    TicketKeyDerivation(#[from] KeyDerivationError),
    #[error("engineer ticket key unavailable")]
    TicketKey(#[from] TicketKeyError),
    #[error("authentication service unavailable")]
    Auth(#[from] AuthError),
    #[error("cannot listen on {addr}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// Builds and starts a relay.
#[derive(Debug)]
pub struct RelayBuilder {
    config: RelayConfig,
    store: Store,
    tls: Arc<ReloadingTls>,
    wall_clock: WallClock,
    jwt_keys: Option<AccessTokenKeys>,
}

impl RelayBuilder {
    /// A relay serving `config` with `store` and the TLS pair of `tls`.
    pub fn new(config: RelayConfig, store: Store, tls: Arc<ReloadingTls>) -> Self {
        Self {
            config,
            store,
            tls,
            wall_clock: WallClock::system(),
            jwt_keys: None,
        }
    }

    /// Wall clock for token expiry (tests).
    pub fn wall_clock(mut self, clock: WallClock) -> Self {
        self.wall_clock = clock;
        self
    }

    /// Access-token keys to use instead of `config.jwt_key_file` / an ephemeral key (tests that
    /// restart a relay and keep access tokens valid).
    pub fn jwt_keys(mut self, keys: AccessTokenKeys) -> Self {
        self.jwt_keys = Some(keys);
        self
    }

    /// Validates the configuration, binds the listeners and starts every task.
    pub async fn start(self) -> Result<RelayHandle, StartError> {
        let RelayBuilder {
            config,
            store,
            tls,
            wall_clock,
            jwt_keys,
        } = self;
        config.validate()?;
        let keys = match (jwt_keys, &config.jwt_key_file) {
            (Some(keys), _) => keys,
            (None, Some(path)) => AccessTokenKeys::load(path)?,
            (None, None) => {
                tracing::warn!(
                    component = COMPONENT,
                    "no JWT_KEY_FILE: access tokens are signed with an ephemeral key (development only)"
                );
                AccessTokenKeys::generate()?
            }
        };
        // Engineer tickets survive a relay restart exactly when the signing key does.
        let sealer = TicketSealer::new(&keys.derive_key(TICKET_KEY_LABEL)?)?;
        let auth = AuthService::new(
            store.clone(),
            keys,
            config.auth.clone(),
            config.limits.max_concurrent_password_hashes,
            config.open_registration,
            wall_clock,
        )
        .await?;
        let metrics = Arc::new(Metrics::default());
        let epoch = Instant::now();
        let registry = Registry::new(
            RegistryConfig::from_relay(&config),
            CodeHasher::new()?,
            sealer,
            Arc::clone(&metrics),
            epoch,
        );
        let listener = TcpListener::bind(config.bind)
            .await
            .map_err(|source| StartError::Bind {
                addr: config.bind,
                source,
            })?;
        let local_addr = listener.local_addr().map_err(|source| StartError::Bind {
            addr: config.bind,
            source,
        })?;
        let admin_listener = match config.admin_bind {
            Some(addr) => Some(
                TcpListener::bind(addr)
                    .await
                    .map_err(|source| StartError::Bind { addr, source })?,
            ),
            None => None,
        };
        let admin_addr = admin_listener.as_ref().and_then(|l| l.local_addr().ok());
        let state = Arc::new(RelayState::new(
            config, store, auth, registry, metrics, epoch,
        ));
        // Every relay task is tracked (shutdown waits for them), background store work included.
        let tracker = state.tasks.clone();
        tracker.spawn(accept_loop(
            Arc::clone(&state),
            listener,
            Arc::clone(&tls),
            tracker.clone(),
        ));
        let state_hk = Arc::clone(&state);
        tracker.spawn(async move {
            while !state_hk.shutdown.is_cancelled() {
                let s = Arc::clone(&state_hk);
                let handle = tokio::spawn(housekeeping_loop(s));
                match handle.await {
                    Ok(()) => break,
                    Err(e) if e.is_panic() => {
                        tracing::error!(
                            component = COMPONENT,
                            error = %e,
                            "housekeeping task panicked; restarting after backoff"
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                            _ = state_hk.shutdown.cancelled() => break,
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let state_tls = Arc::clone(&state);
        let tls_watch = Arc::clone(&tls);
        tracker.spawn(async move {
            while !state_tls.shutdown.is_cancelled() {
                let t = Arc::clone(&tls_watch);
                let interval = state_tls.config.timeouts.tls_reload_interval;
                let shutdown = state_tls.shutdown.clone();
                let metrics = Arc::clone(&state_tls.metrics);
                let handle = tokio::spawn(t.watch(interval, shutdown, metrics));
                match handle.await {
                    Ok(()) => break,
                    Err(e) if e.is_panic() => {
                        tracing::error!(
                            component = COMPONENT,
                            error = %e,
                            "tls watch task panicked; restarting after backoff"
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                            _ = state_tls.shutdown.cancelled() => break,
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        if let Some(listener) = admin_listener {
            tracker.spawn(admin::serve(Arc::clone(&state), listener));
        }
        tracing::info!(
            component = COMPONENT,
            addr = %local_addr,
            admin = admin_addr.map_or_else(|| "disabled".to_owned(), |a| a.to_string()),
            env = environment_name(state.config.env),
            store = if state.store.is_memory() { "memory" } else { "postgres" },
            version = %state.relay_version,
            "relay listening"
        );
        Ok(RelayHandle {
            state,
            tracker,
            local_addr,
            admin_addr,
        })
    }
}

/// A running relay.
#[derive(Debug)]
pub struct RelayHandle {
    state: Arc<RelayState>,
    tracker: TaskTracker,
    local_addr: SocketAddr,
    admin_addr: Option<SocketAddr>,
}

impl RelayHandle {
    /// Address of the telemetry listener.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Address of the admin listener, if enabled.
    pub fn admin_addr(&self) -> Option<SocketAddr> {
        self.admin_addr
    }

    /// Relay metrics.
    pub fn metrics(&self) -> &Metrics {
        &self.state.metrics
    }

    /// Registry gauges (sessions, engineers).
    pub fn gauges(&self) -> RegistryGauges {
        self.state.registry.gauges()
    }

    /// Diagnostics view of one session.
    pub fn session_debug(&self, id: u64) -> Option<SessionDebug> {
        self.state.registry.session_debug(id, Instant::now())
    }

    /// Accounts and tokens (tests, tooling).
    pub fn auth(&self) -> &AuthService {
        &self.state.auth
    }

    /// The store.
    pub fn store(&self) -> &Store {
        &self.state.store
    }

    /// Graceful shutdown: stop accepting; engineers get SESSION_CLOSE{RelayShutdown}, drivers and
    /// unbound clients DISCONNECT{ServerShutdown}; queued frames are flushed for up to
    /// `shutdown_grace`, then remaining writes are abandoned. Sessions stay open in the store
    /// (drivers resume after the restart).
    pub async fn shutdown(self) {
        tracing::info!(component = COMPONENT, "relay shutting down");
        self.state.registry.shutdown();
        self.state.shutdown.cancel();
        self.tracker.close();
        let grace = self.state.config.timeouts.shutdown_grace;
        if tokio::time::timeout(grace, self.tracker.wait())
            .await
            .is_err()
        {
            tracing::warn!(
                component = COMPONENT,
                "connections still open after the shutdown grace; abandoning them"
            );
            self.state.hard_stop.cancel();
            let _ = tokio::time::timeout(grace, self.tracker.wait()).await;
        }
        tracing::info!(component = COMPONENT, "relay stopped");
    }
}

async fn accept_loop(
    state: Arc<RelayState>,
    listener: TcpListener,
    tls: Arc<ReloadingTls>,
    tracker: TaskTracker,
) {
    loop {
        let accepted = tokio::select! {
            _ = state.shutdown.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (tcp, peer) = match accepted {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(component = COMPONENT, error = %e, "accept failed");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                continue;
            }
        };
        if state.metrics.active_connections.get() >= state.config.limits.max_connections as u64 {
            state.metrics.connections_rejected.inc();
            tracing::warn!(component = COMPONENT, %peer, "connection limit reached; connection refused");
            continue;
        }
        let Some(ip_guard) = state.limiters.connection_opened(peer.ip()) else {
            state.metrics.connections_rejected.inc();
            tracing::warn!(component = COMPONENT, %peer, "per-IP connection limit reached; connection refused");
            continue;
        };
        if let Err(e) = tcp.set_nodelay(true) {
            tracing::debug!(component = COMPONENT, %peer, error = %e, "TCP_NODELAY not set");
        }
        let acceptor = TlsAcceptor::from(tls.current());
        let state = Arc::clone(&state);
        state.metrics.active_connections.inc();
        tracker.spawn(async move {
            serve_connection(Arc::clone(&state), tcp, peer, acceptor).await;
            state.metrics.active_connections.dec();
            drop(ip_guard);
        });
    }
}

/// Store work owed by the housekeeping loop, carried until a background pass takes it.
#[derive(Debug, Default)]
struct StoreWork {
    /// Sessions with queued engineer-slot writes (deduplicated).
    slot_writes: Vec<Arc<SessionCell>>,
    /// Sessions closed by grace expiry, to close in the store (oldest first, bounded).
    expired: Vec<u64>,
    touch: bool,
    revocation_check: bool,
    cleanup: bool,
}

impl StoreWork {
    /// Adds the store side of one registry housekeeping pass. Expired ids beyond
    /// [`MAX_OWED_SESSION_CLOSES`] are dropped (oldest first): the periodic store cleanup closes
    /// such sessions by their idle time anyway.
    fn absorb(&mut self, report: Housekeeping) {
        for cell in report.slot_writes {
            if !self.slot_writes.iter().any(|c| c.id() == cell.id()) {
                self.slot_writes.push(cell);
            }
        }
        self.expired.extend(report.expired);
        if self.expired.len() > MAX_OWED_SESSION_CLOSES {
            let excess = self.expired.len() - MAX_OWED_SESSION_CLOSES;
            self.expired.drain(..excess);
            tracing::warn!(
                component = COMPONENT,
                dropped = excess,
                "the store is not keeping up; expired sessions are left to the store cleanup"
            );
        }
    }

    fn is_empty(&self) -> bool {
        self.slot_writes.is_empty()
            && self.expired.is_empty()
            && !self.touch
            && !self.revocation_check
            && !self.cleanup
    }
}

/// In-memory housekeeping (expiries, link health, limiter sweeps) on every tick; the store side
/// runs beside it in one background pass at a time, so a slow or unreachable database never
/// delays the in-memory work. Work owed while a pass runs is merged into the next one.
async fn housekeeping_loop(state: Arc<RelayState>) {
    let timeouts = &state.config.timeouts;
    let mut ticker = tokio::time::interval(timeouts.housekeeping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_touch = Instant::now();
    let mut last_sweep = Instant::now();
    let mut last_revocation_check = Instant::now();
    // The first pass cleans the store right away (sessions orphaned by a crash).
    let mut last_cleanup: Option<Instant> = None;
    let mut owed = StoreWork::default();
    let mut running: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let now = Instant::now();
        owed.absorb(state.registry.housekeeping(now));
        if now.saturating_duration_since(last_sweep) >= LIMITER_SWEEP_INTERVAL {
            last_sweep = now;
            state.limiters.sweep(now);
        }
        if now.saturating_duration_since(last_touch) >= timeouts.session_touch_interval {
            last_touch = now;
            owed.touch = true;
        }
        if now.saturating_duration_since(last_revocation_check)
            >= timeouts.revocation_check_interval
        {
            last_revocation_check = now;
            owed.revocation_check = true;
        }
        if last_cleanup
            .is_none_or(|t| now.saturating_duration_since(t) >= timeouts.store_cleanup_interval)
        {
            last_cleanup = Some(now);
            owed.cleanup = true;
        }
        if !owed.is_empty() && running.as_ref().is_none_or(|h| h.is_finished()) {
            let work = std::mem::take(&mut owed);
            running = Some(state.tasks.spawn(store_pass(Arc::clone(&state), work)));
        }
    }
}

/// One background pass of housekeeping store work. Every call is bounded
/// ([`crate::store::STORE_CALL_TIMEOUT`]); failures are logged and retried by later passes where
/// that matters (slot writes stay queued on their session, revocation and cleanup are periodic).
async fn store_pass(state: Arc<RelayState>, work: StoreWork) {
    for cell in &work.slot_writes {
        flush_slot_writes(&state, cell).await;
    }
    for id in work.expired {
        if let Err(e) = state
            .store
            .close_session(id, CLOSE_REASON_EXPIRED, now_utc())
            .await
        {
            state.metrics.owed_close_failures.inc();
            state.queue_owed_close(id, CLOSE_REASON_EXPIRED);
            tracing::warn!(component = COMPONENT, session_id = id, error = %e, "expired session not closed in the store; queued for retry");
        }
    }
    let owed = state.take_owed_closes();
    let mut still_owed = Vec::new();
    for (id, reason) in owed {
        if let Err(e) = state.store.close_session(id, reason, now_utc()).await {
            state.metrics.owed_close_failures.inc();
            tracing::warn!(component = COMPONENT, session_id = id, reason, error = %e, "owed session close not stored; will retry");
            still_owed.push((id, reason));
        }
    }
    if !still_owed.is_empty() {
        let mut list = state.owed_closes.lock();
        still_owed.append(&mut *list);
        *list = still_owed;
    }
    if work.touch {
        let ids = state.registry.attached_session_ids();
        if let Err(e) = state.store.touch_sessions(&ids, now_utc()).await {
            tracing::warn!(component = COMPONENT, error = %e, "session activity not stored");
        }
    }
    if work.revocation_check {
        end_revoked_logins(&state).await;
    }
    if work.cleanup {
        store_cleanup(&state).await;
    }
}

/// Closes the open connections of logins that may no longer be used: revoked (logout everywhere
/// with `lmu-relay revoke-tokens`, refresh-token reuse) or whose account was deleted or disabled.
/// Their clients get a fatal `ERROR AuthFailed` and must sign in again. A driver's session then
/// enters its grace period and an engineer's slot is held as after any drop; neither can be
/// resumed with the revoked login. On a store failure the check is retried next interval.
async fn end_revoked_logins(state: &RelayState) {
    let families = state.logins.families();
    for batch in families.chunks(REVOCATION_CHECK_BATCH) {
        let inactive = match state.store.inactive_families(batch).await {
            Ok(inactive) => inactive,
            Err(e) => {
                state.metrics.revocation_check_failures.inc();
                tracing::warn!(component = COMPONENT, error = %e, "revoked logins not checked");
                return;
            }
        };
        if inactive.is_empty() {
            continue;
        }
        let revoked = Message::Error(ErrorMsg {
            code: ErrorCode::AuthFailed,
            message: "this login was revoked; sign in again".into(),
            fatal: true,
        });
        for conn in state.logins.connections_of(&inactive) {
            state.metrics.revoked_disconnects.inc();
            tracing::info!(
                component = COMPONENT,
                conn = conn.id,
                peer = %conn.peer,
                "login revoked; closing its connection"
            );
            conn.send(&revoked, NO_SESSION);
            conn.close();
        }
    }
}

/// Closes store sessions idle for longer than the grace period (plus one touch interval, by
/// which a live session's `last_activity` may lag), deletes closed sessions past their retention
/// and deletes expired refresh tokens.
async fn store_cleanup(state: &RelayState) {
    let timeouts = &state.config.timeouts;
    let idle = timeouts
        .driver_grace
        .saturating_add(timeouts.session_touch_interval);
    let at = now_utc();
    let idle_before = at
        .checked_sub_signed(TimeDelta::from_std(idle).unwrap_or(TimeDelta::MAX))
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
    match state.store.expire_sessions(idle_before, at).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            component = COMPONENT,
            closed = n,
            "stale sessions closed in the store"
        ),
        Err(e) => tracing::warn!(component = COMPONENT, error = %e, "stale sessions not closed"),
    }
    let retention =
        TimeDelta::from_std(timeouts.closed_session_retention).unwrap_or(TimeDelta::MAX);
    let ended_before = at
        .checked_sub_signed(retention)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
    match state.store.purge_closed_sessions(ended_before).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            component = COMPONENT,
            deleted = n,
            "closed sessions past their retention deleted"
        ),
        Err(e) => tracing::warn!(component = COMPONENT, error = %e, "closed sessions not purged"),
    }
    match state.store.purge_refresh_tokens(at).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            component = COMPONENT,
            deleted = n,
            "expired refresh tokens deleted"
        ),
        Err(e) => {
            tracing::warn!(component = COMPONENT, error = %e, "expired refresh tokens not deleted")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owed_store_work_is_bounded_and_merged() {
        let mut owed = StoreWork::default();
        assert!(owed.is_empty());
        let total = MAX_OWED_SESSION_CLOSES as u64 + 10;
        for id in 1..=total {
            owed.absorb(Housekeeping {
                expired: vec![id],
                slot_writes: Vec::new(),
            });
        }
        assert_eq!(owed.expired.len(), MAX_OWED_SESSION_CLOSES);
        assert_eq!(
            owed.expired.first(),
            Some(&11),
            "the oldest are dropped first"
        );
        assert_eq!(owed.expired.last(), Some(&total));
        assert!(!owed.is_empty());
        let taken = std::mem::take(&mut owed);
        assert_eq!(taken.expired.len(), MAX_OWED_SESSION_CLOSES);
        assert!(owed.is_empty());
        owed.cleanup = true;
        assert!(!owed.is_empty());
    }
}

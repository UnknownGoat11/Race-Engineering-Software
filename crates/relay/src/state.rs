//! State shared by every task of a running relay.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use crate::auth::{AuthService, normalize_username};
use crate::config::RelayConfig;
use crate::metrics::Metrics;
use crate::outbox::ConnHandle;
use crate::ratelimit::{KeyedLimiter, Window};
use crate::registry::Registry;
use crate::store::Store;

/// Open authenticated connections by login (refresh-token family), so a login that may no longer
/// be used (revoked, account deleted or disabled) also loses its open connections. One entry per
/// authenticated connection, removed when the connection ends (bounded by the connection limit).
#[derive(Debug, Default)]
pub struct Logins {
    by_conn: Mutex<HashMap<u64, (Uuid, Weak<ConnHandle>)>>,
}

impl Logins {
    /// Records that `conn` authenticated with login `family`.
    pub fn register(&self, conn: &Arc<ConnHandle>, family: Uuid) {
        self.by_conn
            .lock()
            .insert(conn.id, (family, Arc::downgrade(conn)));
    }

    /// Forgets connection `conn_id` (it ended).
    pub fn remove(&self, conn_id: u64) {
        self.by_conn.lock().remove(&conn_id);
    }

    /// The distinct logins of the open connections.
    pub fn families(&self) -> Vec<Uuid> {
        let distinct: HashSet<Uuid> = self.by_conn.lock().values().map(|(f, _)| *f).collect();
        distinct.into_iter().collect()
    }

    /// The open connections authenticated with one of `families`.
    pub fn connections_of(&self, families: &[Uuid]) -> Vec<Arc<ConnHandle>> {
        let wanted: HashSet<&Uuid> = families.iter().collect();
        self.by_conn
            .lock()
            .values()
            .filter(|(family, _)| wanted.contains(family))
            .filter_map(|(_, conn)| conn.upgrade())
            .collect()
    }

    /// Number of authenticated connections tracked.
    pub fn len(&self) -> usize {
        self.by_conn.lock().len()
    }

    /// Whether no authenticated connection is tracked.
    pub fn is_empty(&self) -> bool {
        self.by_conn.lock().is_empty()
    }
}

/// Outcome of [`Limiters::admit_password_attempt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordAttempt {
    /// Within the per-account limit.
    Admitted,
    /// Too many attempts for this account.
    RateLimited,
    /// Not a valid login name, so no account can match: refused without a password check.
    InvalidUsername,
}

/// Keyed rate limiters (token buckets, bounded maps).
#[derive(Debug)]
pub struct Limiters {
    auth_ip: Mutex<KeyedLimiter<IpAddr>>,
    /// Keyed by normalized username (at most `MAX_USERNAME_CHARS` bytes).
    auth_account: Mutex<KeyedLimiter<String>>,
    join_account: Mutex<KeyedLimiter<Uuid>>,
    join_ip: Mutex<KeyedLimiter<IpAddr>>,
    resume_account: Mutex<KeyedLimiter<Uuid>>,
    resume_ip: Mutex<KeyedLimiter<IpAddr>>,
    session_create: Mutex<KeyedLimiter<Uuid>>,
    ip_connections: Mutex<HashMap<IpAddr, usize>>,
    max_connections_per_ip: usize,
}

impl Limiters {
    /// Limiters configured from `config.limits`.
    pub fn new(config: &RelayConfig) -> Self {
        let l = &config.limits;
        let max_keys = l.rate_limiter_max_keys;
        let joins = [
            Window::per_minute(u64::from(l.code_redemptions_per_minute)),
            Window::per_hour(u64::from(l.code_redemptions_per_hour)),
        ];
        Self {
            auth_ip: Mutex::new(KeyedLimiter::new(
                &[Window::per_minute(u64::from(
                    l.auth_attempts_per_ip_per_minute,
                ))],
                max_keys,
            )),
            auth_account: Mutex::new(KeyedLimiter::new(
                &[Window::per_minute(u64::from(
                    l.auth_attempts_per_account_per_minute,
                ))],
                max_keys,
            )),
            join_account: Mutex::new(KeyedLimiter::new(&joins, max_keys)),
            join_ip: Mutex::new(KeyedLimiter::new(&joins, max_keys)),
            resume_account: Mutex::new(KeyedLimiter::new(
                &[Window::per_minute(u64::from(
                    l.resume_lookups_per_account_per_minute,
                ))],
                max_keys,
            )),
            resume_ip: Mutex::new(KeyedLimiter::new(
                &[Window::per_minute(u64::from(
                    l.resume_lookups_per_ip_per_minute,
                ))],
                max_keys,
            )),
            session_create: Mutex::new(KeyedLimiter::new(
                &[Window::per_minute(u64::from(
                    l.session_creates_per_account_per_minute,
                ))],
                max_keys,
            )),
            ip_connections: Mutex::new(HashMap::new()),
            max_connections_per_ip: l.max_connections_per_ip,
        }
    }

    /// One AUTH / ACCOUNT_REGISTER attempt from `ip`.
    pub fn admit_auth_ip(&self, ip: IpAddr, now: Instant) -> bool {
        self.auth_ip.lock().try_admit(&ip, now)
    }

    /// One password attempt for `username` as the client sent it. Only a valid login name
    /// ([`normalize_username`]) is tracked, under its normalized form, so no key exceeds
    /// [`crate::auth::MAX_USERNAME_CHARS`] bytes whatever the client sends; an invalid name cannot be an
    /// account and never becomes a key ([`PasswordAttempt::InvalidUsername`]).
    pub fn admit_password_attempt(&self, username: &str, now: Instant) -> PasswordAttempt {
        let Ok(key) = normalize_username(username) else {
            return PasswordAttempt::InvalidUsername;
        };
        if self.auth_account.lock().try_admit(&key, now) {
            PasswordAttempt::Admitted
        } else {
            PasswordAttempt::RateLimited
        }
    }

    /// One store-backed SESSION_RESUME lookup (the session is not in relay memory) by `account`
    /// from `ip`. Both limits are charged.
    pub fn admit_resume_lookup(&self, account: Uuid, ip: IpAddr, now: Instant) -> bool {
        let by_account = self.resume_account.lock().try_admit(&account, now);
        let by_ip = self.resume_ip.lock().try_admit(&ip, now);
        by_account && by_ip
    }

    /// One SESSION_CREATE by `account`.
    pub fn admit_session_create(&self, account: Uuid, now: Instant) -> bool {
        self.session_create.lock().try_admit(&account, now)
    }

    /// One SESSION_JOIN attempt by `account` from `ip` (D-022: 5/min and 20/h each). Both limits
    /// are charged, so neither can be probed separately.
    pub fn admit_join(&self, account: Uuid, ip: IpAddr, now: Instant) -> bool {
        let by_account = self.join_account.lock().try_admit(&account, now);
        let by_ip = self.join_ip.lock().try_admit(&ip, now);
        by_account && by_ip
    }

    /// Registers a new connection from `ip`; `None` if the per-IP limit is reached.
    pub fn connection_opened(self: &Arc<Self>, ip: IpAddr) -> Option<IpConnectionGuard> {
        let mut map = self.ip_connections.lock();
        let count = map.entry(ip).or_insert(0);
        if *count >= self.max_connections_per_ip {
            return None;
        }
        *count += 1;
        Some(IpConnectionGuard {
            limiters: Arc::clone(self),
            ip,
        })
    }

    /// Forgets idle keys of every limiter.
    pub fn sweep(&self, now: Instant) {
        self.auth_ip.lock().sweep(now);
        self.auth_account.lock().sweep(now);
        self.join_account.lock().sweep(now);
        self.join_ip.lock().sweep(now);
        self.resume_account.lock().sweep(now);
        self.resume_ip.lock().sweep(now);
        self.session_create.lock().sweep(now);
    }
}

/// Decrements the per-IP connection count when dropped.
#[derive(Debug)]
pub struct IpConnectionGuard {
    limiters: Arc<Limiters>,
    ip: IpAddr,
}

impl Drop for IpConnectionGuard {
    fn drop(&mut self) {
        let mut map = self.limiters.ip_connections.lock();
        if let Some(count) = map.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

/// Everything a relay task needs.
#[derive(Debug)]
pub struct RelayState {
    pub config: RelayConfig,
    pub store: Store,
    pub auth: AuthService,
    pub registry: Registry,
    pub metrics: Arc<Metrics>,
    pub limiters: Arc<Limiters>,
    /// Authenticated connections by login (revocation checks).
    pub logins: Logins,
    /// Monotonic epoch of this relay process (frame `source_ts_ns`, outbox clocks).
    pub epoch: Instant,
    /// Version announced in HELLO and on `/health`.
    pub relay_version: String,
    /// Soft shutdown: stop accepting, notify and close connections.
    pub shutdown: CancellationToken,
    /// Hard stop after the shutdown grace: abandon remaining writes.
    pub hard_stop: CancellationToken,
    /// Every task of the relay (connections, housekeeping, background store writes); shutdown
    /// waits for them.
    pub tasks: TaskTracker,
    /// Sessions whose closure write to the store failed, queued for retry during housekeeping (L8).
    pub owed_closes: Mutex<Vec<(u64, &'static str)>>,
    next_conn_id: AtomicU64,
}

impl RelayState {
    /// Assembles the state.
    pub fn new(
        config: RelayConfig,
        store: Store,
        auth: AuthService,
        registry: Registry,
        metrics: Arc<Metrics>,
        epoch: Instant,
    ) -> Self {
        let limiters = Arc::new(Limiters::new(&config));
        Self {
            config,
            store,
            auth,
            registry,
            metrics,
            limiters,
            logins: Logins::default(),
            epoch,
            relay_version: format!("lmu-relay/{}", env!("CARGO_PKG_VERSION")),
            shutdown: CancellationToken::new(),
            hard_stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
            owed_closes: Mutex::new(Vec::new()),
            next_conn_id: AtomicU64::new(1),
        }
    }

    /// Queues a session whose close write failed for retry during housekeeping (L8).
    pub fn queue_owed_close(&self, session_id: u64, reason: &'static str) {
        let mut list = self.owed_closes.lock();
        if !list.iter().any(|(id, _)| *id == session_id) {
            list.push((session_id, reason));
        }
    }

    /// Takes the currently owed session closes to retry them against the store (L8).
    pub fn take_owed_closes(&self) -> Vec<(u64, &'static str)> {
        std::mem::take(&mut *self.owed_closes.lock())
    }

    /// A relay-wide unique connection id.
    pub fn next_conn_id(&self) -> u64 {
        self.next_conn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Time since the relay started.
    pub fn uptime(&self) -> Duration {
        self.epoch.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;

    fn conn(id: u64) -> Arc<ConnHandle> {
        Arc::new(ConnHandle::new(
            id,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40_000),
            Instant::now(),
            16,
            1 << 16,
        ))
    }

    #[test]
    fn oversized_or_invalid_usernames_never_become_limiter_keys() {
        let mut config = RelayConfig::development();
        config.limits.auth_attempts_per_account_per_minute = 2;
        let limiters = Limiters::new(&config);
        let now = Instant::now();
        let huge = "a".repeat(250 * 1024);
        for name in [huge.as_str(), "x", "al ice", "élodie", "   "] {
            for _ in 0..5 {
                assert_eq!(
                    limiters.admit_password_attempt(name, now),
                    PasswordAttempt::InvalidUsername
                );
            }
        }
        assert!(limiters.auth_account.lock().is_empty(), "no key was stored");
        // Valid names are tracked under their normalized form: case and padding share a bucket.
        assert_eq!(
            limiters.admit_password_attempt("Alice", now),
            PasswordAttempt::Admitted
        );
        assert_eq!(
            limiters.admit_password_attempt("  alice ", now),
            PasswordAttempt::Admitted
        );
        assert_eq!(
            limiters.admit_password_attempt("ALICE", now),
            PasswordAttempt::RateLimited
        );
        assert_eq!(limiters.auth_account.lock().len(), 1);
    }

    #[test]
    fn resume_lookups_and_session_creates_are_limited() {
        let mut config = RelayConfig::development();
        config.limits.resume_lookups_per_account_per_minute = 2;
        config.limits.resume_lookups_per_ip_per_minute = 3;
        config.limits.session_creates_per_account_per_minute = 1;
        let limiters = Limiters::new(&config);
        let now = Instant::now();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(limiters.admit_resume_lookup(a, ip, now));
        assert!(limiters.admit_resume_lookup(a, ip, now));
        assert!(!limiters.admit_resume_lookup(a, ip, now), "account limit");
        // The refused attempt above still charged the IP (both limits always are), so another
        // account from the same IP is refused too.
        assert!(!limiters.admit_resume_lookup(b, ip, now), "IP limit");
        assert!(limiters.admit_resume_lookup(b, ip, now + Duration::from_secs(60)));
        assert!(limiters.admit_session_create(a, now));
        assert!(!limiters.admit_session_create(a, now));
        assert!(limiters.admit_session_create(b, now));
    }

    #[test]
    fn logins_map_families_to_open_connections() {
        let logins = Logins::default();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (c1, c2, c3) = (conn(1), conn(2), conn(3));
        logins.register(&c1, a);
        logins.register(&c2, a);
        logins.register(&c3, b);
        let mut families = logins.families();
        families.sort();
        let mut expected = vec![a, b];
        expected.sort();
        assert_eq!(families, expected, "distinct logins");
        let mut ids: Vec<u64> = logins.connections_of(&[a]).iter().map(|c| c.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 2]);
        logins.remove(1);
        assert_eq!(logins.len(), 2);
        // A connection that is gone (handle dropped) is never returned.
        drop(c3);
        assert!(logins.connections_of(&[b]).is_empty());
        logins.remove(2);
        logins.remove(3);
        assert!(logins.is_empty());
    }
}

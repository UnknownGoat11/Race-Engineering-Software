//! Relay configuration (validated). The CLI ([`crate::cli`]) builds it from environment variables
//! and flags; tests build it with [`RelayConfig::development`] and override fields.
//!
//! Defaults follow docs/PROTOCOL.md §7 (`protocol::timing`, `protocol::limits`) and D-022. Rate
//! limits leave generous headroom over a 100 Hz driver stream (≈ 300–400 frames/s, a few hundred
//! KiB/s), so a conforming client never hits them.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use protocol::messages::Secret;
use protocol::version::Environment;
use protocol::{DEFAULT_PORT, MAX_PAYLOAD_BYTES, limits, timing};
use thiserror::Error;

use crate::ratelimit::RateSpec;

/// Default admin listener (loopback only).
pub const DEFAULT_ADMIN_PORT: u16 = 9090;
/// Shortest accepted admin bearer token.
pub const MIN_ADMIN_TOKEN_CHARS: usize = 32;
/// Upper bound for `max_engineers_per_session` (slots are scanned linearly).
pub const MAX_ENGINEERS_CAP: u16 = 16;
/// Smallest accepted `max_payload_bytes` (a SESSION_CREATE or SCHEMA_NEGOTIATE must fit).
pub const MIN_PAYLOAD_LIMIT: usize = 16 * 1024;
/// Default retention of closed sessions in the store.
pub const CLOSED_SESSION_RETENTION_DAYS: u64 = 30;
/// Seconds per day.
const DAY_S: u64 = 24 * 3_600;

/// Validated relay configuration.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Announced in `ServerHello.env`; development is the only environment allowed to run
    /// without PostgreSQL, a JWT key file or an admin token.
    pub env: Environment,
    /// Telemetry listener (TLS 1.3, ALPN `lmure/1`).
    pub bind: SocketAddr,
    /// Admin/health listener (`None` = disabled).
    pub admin_bind: Option<SocketAddr>,
    /// Bearer token of the admin listener.
    pub admin_token: Option<Secret>,
    /// PostgreSQL URL (`None` = in-memory store, development only).
    pub database_url: Option<Secret>,
    /// Maximum PostgreSQL pool connections.
    pub database_max_connections: u32,
    /// Ed25519 seed file for access tokens (`None` = ephemeral key, development only).
    pub jwt_key_file: Option<PathBuf>,
    /// Allow ACCOUNT_REGISTER (default off).
    pub open_registration: bool,
    pub limits: Limits,
    pub timeouts: Timeouts,
    pub auth: AuthSettings,
}

/// Capacity and abuse limits.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Concurrent TCP connections (TLS handshakes included).
    pub max_connections: usize,
    /// Concurrent connections from one IP address.
    pub max_connections_per_ip: usize,
    /// Live sessions (driver attached or in grace).
    pub max_sessions: usize,
    /// Engineer slots per driver session (D-022, default 2).
    pub max_engineers_per_session: u16,
    /// Largest accepted payload (≤ `protocol::MAX_PAYLOAD_BYTES`), announced in HELLO.
    pub max_payload_bytes: usize,
    /// Inbound rate of one connection.
    pub connection_rate: RateSpec,
    /// Inbound rate of one session's driver stream. Engineer SNAPSHOT_REQUESTs are metered by
    /// their own connection and request limiter only, so engineers cannot drain it.
    pub session_rate: RateSpec,
    /// Header-rule violations a connection may produce (refilled 1/s) before it is closed.
    pub malformed_budget: u32,
    /// Reliable (never dropped) frames queued for one receiver before it is disconnected as
    /// too slow.
    pub reliable_queue_frames: usize,
    /// Bytes of reliable frames queued for one receiver before it is disconnected as too slow.
    pub reliable_queue_bytes: usize,
    /// SESSION_JOIN attempts per account and per IP per minute (D-022: 5).
    pub code_redemptions_per_minute: u32,
    /// SESSION_JOIN attempts per account and per IP per hour (D-022: 20).
    pub code_redemptions_per_hour: u32,
    /// AUTH / ACCOUNT_REGISTER attempts per IP per minute.
    pub auth_attempts_per_ip_per_minute: u32,
    /// Password attempts per username per minute.
    pub auth_attempts_per_account_per_minute: u32,
    /// SESSION_RESUMEs of sessions not in relay memory (each one reads the store) per account
    /// per minute.
    pub resume_lookups_per_account_per_minute: u32,
    /// SESSION_RESUMEs of sessions not in relay memory per source IP per minute.
    pub resume_lookups_per_ip_per_minute: u32,
    /// SESSION_CREATEs per driver account per minute (each one writes the store).
    pub session_creates_per_account_per_minute: u32,
    /// AUTH attempts on one connection before it is closed.
    pub auth_attempts_per_connection: u32,
    /// Password hashes computed at the same time (Argon2id is memory-hard).
    pub max_concurrent_password_hashes: usize,
    /// Keys tracked by each keyed rate limiter (bounded map).
    pub rate_limiter_max_keys: usize,
}

/// Timeouts and lifetimes.
#[derive(Debug, Clone)]
pub struct Timeouts {
    /// Expected client PING period (announced).
    pub heartbeat_interval: Duration,
    /// An engineer whose last inbound bytes are older than this is shown `Degraded`.
    pub heartbeat_degraded: Duration,
    /// A connection without inbound bytes for this long is closed (announced).
    pub heartbeat_timeout: Duration,
    /// TLS handshake plus HELLO and AUTH must complete within this time.
    pub handshake: Duration,
    /// A receiver whose socket accepts no bytes for this long is disconnected.
    pub write_stall: Duration,
    /// The relay keeps a session this long after its driver disconnected (announced).
    pub driver_grace: Duration,
    /// An unused connection code expires this long after issue (D-022).
    pub connect_code_ttl: Duration,
    /// An engineer's resume ticket stays valid this long after its connection dropped (D-022).
    pub engineer_ticket_ttl: Duration,
    /// Period of the housekeeping pass (expiries, link health).
    pub housekeeping_interval: Duration,
    /// Period of `last_activity` updates of live sessions in the store.
    pub session_touch_interval: Duration,
    /// Live frames dropped for an engineer within this window mark its link `Degraded`.
    pub degraded_drop_window: Duration,
    /// Interval at which the TLS certificate files are checked for changes.
    pub tls_reload_interval: Duration,
    /// Cached TELEMETRY_SNAPSHOT / SCORING_UPDATE frames older than this are not replayed to a
    /// joining engineer (the cached SCHEMA_NEGOTIATE lives as long as its sequence space).
    pub join_cache_max_age: Duration,
    /// Period of the store cleanup (expired refresh tokens, sessions orphaned by a crash, closed
    /// sessions past their retention).
    pub store_cleanup_interval: Duration,
    /// Closed sessions are deleted from the store this long after they ended.
    pub closed_session_retention: Duration,
    /// Period of the check that closes the open connections of logins that may no longer be
    /// used (revoked, account deleted or disabled): the delay until a revocation takes effect.
    pub revocation_check_interval: Duration,
    /// Maximum time to flush and close connections on shutdown.
    pub shutdown_grace: Duration,
}

/// Token lifetimes and password hashing cost.
#[derive(Debug, Clone)]
pub struct AuthSettings {
    /// Access token (JWT) lifetime (D-008: 15 min).
    pub access_token_ttl: Duration,
    /// Refresh token lifetime since its last use (D-020: sliding 90 days).
    pub refresh_token_ttl: Duration,
    /// A rotated refresh token presented again within this time is still accepted (the client
    /// may never have received the AUTH_OK carrying its successor); later reuse revokes the family.
    pub refresh_reuse_grace: Duration,
    /// Active refresh tokens kept per login (family); older ones are revoked.
    pub max_active_refresh_per_family: usize,
    /// Argon2id parameters for new password hashes.
    pub password_hash: PasswordHashParams,
}

/// Argon2id cost parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasswordHashParams {
    /// Memory in KiB.
    pub m_cost_kib: u32,
    /// Iterations.
    pub t_cost: u32,
    /// Parallelism.
    pub p_cost: u32,
}

impl PasswordHashParams {
    /// OWASP recommendation for Argon2id (19 MiB, 2 iterations, 1 lane).
    pub const OWASP: PasswordHashParams = PasswordHashParams {
        m_cost_kib: 19_456,
        t_cost: 2,
        p_cost: 1,
    };
    /// Minimal cost for tests only (fast in debug builds). Never use in production.
    pub const TEST_ONLY_FAST: PasswordHashParams = PasswordHashParams {
        m_cost_kib: 64,
        t_cost: 1,
        p_cost: 1,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_connections: 2_048,
            max_connections_per_ip: 32,
            max_sessions: 512,
            max_engineers_per_session: limits::MAX_ENGINEERS_PER_SESSION,
            max_payload_bytes: MAX_PAYLOAD_BYTES,
            connection_rate: RateSpec {
                packets_per_sec: 2_000,
                bytes_per_sec: 8 * 1024 * 1024,
                burst_secs: 2,
            },
            session_rate: RateSpec {
                packets_per_sec: 3_000,
                bytes_per_sec: 12 * 1024 * 1024,
                burst_secs: 2,
            },
            malformed_budget: 32,
            reliable_queue_frames: 1_024,
            reliable_queue_bytes: 4 * 1024 * 1024,
            code_redemptions_per_minute: limits::CODE_REDEMPTIONS_PER_MINUTE,
            code_redemptions_per_hour: limits::CODE_REDEMPTIONS_PER_HOUR,
            auth_attempts_per_ip_per_minute: 30,
            auth_attempts_per_account_per_minute: 10,
            resume_lookups_per_account_per_minute: 20,
            resume_lookups_per_ip_per_minute: 60,
            session_creates_per_account_per_minute: 10,
            auth_attempts_per_connection: 3,
            max_concurrent_password_hashes: 4,
            rate_limiter_max_keys: 100_000,
        }
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_millis(timing::HEARTBEAT_INTERVAL_MS),
            heartbeat_degraded: Duration::from_millis(timing::HEARTBEAT_DEGRADED_MS),
            heartbeat_timeout: Duration::from_millis(timing::HEARTBEAT_TIMEOUT_MS),
            handshake: Duration::from_secs(10),
            write_stall: Duration::from_secs(15),
            driver_grace: Duration::from_secs(timing::SESSION_DRIVER_GRACE_S),
            connect_code_ttl: Duration::from_secs(timing::CONNECT_CODE_TTL_S),
            engineer_ticket_ttl: Duration::from_secs(timing::ENGINEER_TICKET_TTL_S),
            housekeeping_interval: Duration::from_millis(500),
            session_touch_interval: Duration::from_secs(30),
            degraded_drop_window: Duration::from_secs(5),
            tls_reload_interval: Duration::from_secs(30),
            join_cache_max_age: Duration::from_secs(30),
            store_cleanup_interval: Duration::from_secs(300),
            closed_session_retention: Duration::from_secs(CLOSED_SESSION_RETENTION_DAYS * DAY_S),
            revocation_check_interval: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

impl Default for AuthSettings {
    fn default() -> Self {
        Self {
            access_token_ttl: Duration::from_secs(15 * 60),
            refresh_token_ttl: Duration::from_secs(90 * 24 * 3_600),
            refresh_reuse_grace: Duration::from_secs(60),
            max_active_refresh_per_family: 4,
            password_hash: PasswordHashParams::OWASP,
        }
    }
}

/// Configuration rejected at startup.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error(
        "DATABASE_URL is required in {0:?}: the in-memory store is allowed only with RELAY_ENV=development"
    )]
    DatabaseRequired(Environment),
    #[error("JWT_KEY_FILE is required in {0:?} (generate one with `lmu-relay gen-jwt-key`)")]
    JwtKeyRequired(Environment),
    #[error("ADMIN_TOKEN is required in {0:?} when the admin listener is enabled")]
    AdminTokenRequired(Environment),
    #[error("ADMIN_TOKEN must have at least {MIN_ADMIN_TOKEN_CHARS} characters")]
    AdminTokenTooShort,
    #[error("max_engineers_per_session must be 1..={MAX_ENGINEERS_CAP}, got {0}")]
    EngineerSlots(u16),
    #[error("max_payload_bytes must be {MIN_PAYLOAD_LIMIT}..={MAX_PAYLOAD_BYTES}, got {0}")]
    PayloadLimit(usize),
    #[error("{0} must be greater than zero")]
    Zero(&'static str),
    #[error("heartbeat timeout must exceed the heartbeat interval")]
    Heartbeat,
    #[error("unknown environment {0:?} (expected development, staging or production)")]
    UnknownEnvironment(String),
}

/// Parses `development` / `staging` / `production` (case-insensitive).
pub fn parse_environment(text: &str) -> Result<Environment, ConfigError> {
    match text.trim().to_ascii_lowercase().as_str() {
        "development" | "dev" => Ok(Environment::Development),
        "staging" => Ok(Environment::Staging),
        "production" | "prod" => Ok(Environment::Production),
        _ => Err(ConfigError::UnknownEnvironment(text.to_owned())),
    }
}

/// Lower-case environment name (logs, health endpoint).
pub const fn environment_name(env: Environment) -> &'static str {
    match env {
        Environment::Development => "development",
        Environment::Staging => "staging",
        Environment::Production => "production",
    }
}

impl RelayConfig {
    /// Development defaults: loopback on an ephemeral port, in-memory store, ephemeral JWT key,
    /// admin listener disabled. Used by tests and `RELAY_ENV=development`.
    pub fn development() -> Self {
        Self {
            env: Environment::Development,
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            admin_bind: None,
            admin_token: None,
            database_url: None,
            database_max_connections: 10,
            jwt_key_file: None,
            open_registration: false,
            limits: Limits::default(),
            timeouts: Timeouts::default(),
            auth: AuthSettings::default(),
        }
    }

    /// Default listener address for production (`0.0.0.0:443`).
    pub fn default_bind() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_PORT)
    }

    /// Default admin address (`127.0.0.1:9090`).
    pub fn default_admin_bind() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_ADMIN_PORT)
    }

    /// Checks the configuration. Development may run without PostgreSQL, a JWT key file and an
    /// admin token; staging and production may not.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let strict = self.env != Environment::Development;
        if strict && self.database_url.is_none() {
            return Err(ConfigError::DatabaseRequired(self.env));
        }
        if strict && self.jwt_key_file.is_none() {
            return Err(ConfigError::JwtKeyRequired(self.env));
        }
        if strict && self.admin_bind.is_some() && self.admin_token.is_none() {
            return Err(ConfigError::AdminTokenRequired(self.env));
        }
        if let Some(token) = &self.admin_token
            && token.expose().chars().count() < MIN_ADMIN_TOKEN_CHARS
        {
            return Err(ConfigError::AdminTokenTooShort);
        }
        let l = &self.limits;
        if l.max_engineers_per_session == 0 || l.max_engineers_per_session > MAX_ENGINEERS_CAP {
            return Err(ConfigError::EngineerSlots(l.max_engineers_per_session));
        }
        if !(MIN_PAYLOAD_LIMIT..=MAX_PAYLOAD_BYTES).contains(&l.max_payload_bytes) {
            return Err(ConfigError::PayloadLimit(l.max_payload_bytes));
        }
        let positive = [
            ("max_connections", l.max_connections as u64),
            ("max_connections_per_ip", l.max_connections_per_ip as u64),
            ("max_sessions", l.max_sessions as u64),
            ("connection packet rate", l.connection_rate.packets_per_sec),
            ("connection byte rate", l.connection_rate.bytes_per_sec),
            ("session packet rate", l.session_rate.packets_per_sec),
            ("session byte rate", l.session_rate.bytes_per_sec),
            ("malformed_budget", u64::from(l.malformed_budget)),
            ("reliable_queue_frames", l.reliable_queue_frames as u64),
            ("reliable_queue_bytes", l.reliable_queue_bytes as u64),
            (
                "code_redemptions_per_minute",
                u64::from(l.code_redemptions_per_minute),
            ),
            (
                "code_redemptions_per_hour",
                u64::from(l.code_redemptions_per_hour),
            ),
            (
                "auth_attempts_per_ip_per_minute",
                u64::from(l.auth_attempts_per_ip_per_minute),
            ),
            (
                "auth_attempts_per_account_per_minute",
                u64::from(l.auth_attempts_per_account_per_minute),
            ),
            (
                "resume_lookups_per_account_per_minute",
                u64::from(l.resume_lookups_per_account_per_minute),
            ),
            (
                "resume_lookups_per_ip_per_minute",
                u64::from(l.resume_lookups_per_ip_per_minute),
            ),
            (
                "session_creates_per_account_per_minute",
                u64::from(l.session_creates_per_account_per_minute),
            ),
            (
                "auth_attempts_per_connection",
                u64::from(l.auth_attempts_per_connection),
            ),
            (
                "max_concurrent_password_hashes",
                l.max_concurrent_password_hashes as u64,
            ),
            (
                "database_max_connections",
                u64::from(self.database_max_connections),
            ),
        ];
        if let Some((name, _)) = positive.iter().find(|(_, v)| *v == 0) {
            return Err(ConfigError::Zero(name));
        }
        let t = &self.timeouts;
        let durations = [
            ("heartbeat_interval", t.heartbeat_interval),
            ("handshake timeout", t.handshake),
            ("write_stall", t.write_stall),
            ("driver_grace", t.driver_grace),
            ("connect_code_ttl", t.connect_code_ttl),
            ("engineer_ticket_ttl", t.engineer_ticket_ttl),
            ("housekeeping_interval", t.housekeeping_interval),
            ("session_touch_interval", t.session_touch_interval),
            ("tls_reload_interval", t.tls_reload_interval),
            ("join_cache_max_age", t.join_cache_max_age),
            ("store_cleanup_interval", t.store_cleanup_interval),
            ("closed_session_retention", t.closed_session_retention),
            ("revocation_check_interval", t.revocation_check_interval),
            ("access_token_ttl", self.auth.access_token_ttl),
            ("refresh_token_ttl", self.auth.refresh_token_ttl),
        ];
        if let Some((name, _)) = durations.iter().find(|(_, d)| d.is_zero()) {
            return Err(ConfigError::Zero(name));
        }
        if t.heartbeat_timeout <= t.heartbeat_interval {
            return Err(ConfigError::Heartbeat);
        }
        if self.auth.max_active_refresh_per_family == 0 {
            return Err(ConfigError::Zero("max_active_refresh_per_family"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_defaults_are_valid() {
        assert_eq!(RelayConfig::development().validate(), Ok(()));
    }

    #[test]
    fn production_requires_database_jwt_key_and_admin_token() {
        let mut c = RelayConfig::development();
        c.env = Environment::Production;
        assert_eq!(
            c.validate(),
            Err(ConfigError::DatabaseRequired(Environment::Production))
        );
        c.database_url = Some(Secret::new("postgres://relay@db/relay"));
        assert_eq!(
            c.validate(),
            Err(ConfigError::JwtKeyRequired(Environment::Production))
        );
        c.jwt_key_file = Some(PathBuf::from("/run/secrets/jwt.key"));
        c.admin_bind = Some(RelayConfig::default_admin_bind());
        assert_eq!(
            c.validate(),
            Err(ConfigError::AdminTokenRequired(Environment::Production))
        );
        c.admin_token = Some(Secret::new("short"));
        assert_eq!(c.validate(), Err(ConfigError::AdminTokenTooShort));
        c.admin_token = Some(Secret::new("x".repeat(MIN_ADMIN_TOKEN_CHARS)));
        assert_eq!(c.validate(), Ok(()));
    }

    #[test]
    fn limits_are_checked() {
        let mut c = RelayConfig::development();
        c.limits.max_engineers_per_session = 0;
        assert_eq!(c.validate(), Err(ConfigError::EngineerSlots(0)));
        c.limits.max_engineers_per_session = 2;
        c.limits.max_payload_bytes = MAX_PAYLOAD_BYTES + 1;
        assert!(matches!(c.validate(), Err(ConfigError::PayloadLimit(_))));
        c.limits.max_payload_bytes = MAX_PAYLOAD_BYTES;
        c.timeouts.heartbeat_timeout = c.timeouts.heartbeat_interval;
        assert_eq!(c.validate(), Err(ConfigError::Heartbeat));
    }

    #[test]
    fn environment_parsing() {
        assert_eq!(parse_environment("Production"), Ok(Environment::Production));
        assert_eq!(parse_environment("dev"), Ok(Environment::Development));
        assert!(parse_environment("live").is_err());
        assert_eq!(environment_name(Environment::Staging), "staging");
    }

    #[test]
    fn defaults_follow_the_protocol() {
        let c = RelayConfig::development();
        assert_eq!(c.limits.max_engineers_per_session, 2);
        assert_eq!(c.limits.code_redemptions_per_minute, 5);
        assert_eq!(c.limits.code_redemptions_per_hour, 20);
        assert_eq!(c.timeouts.connect_code_ttl, Duration::from_secs(600));
        assert_eq!(c.timeouts.engineer_ticket_ttl, Duration::from_secs(600));
        assert_eq!(c.timeouts.driver_grace, Duration::from_secs(600));
        assert_eq!(c.auth.access_token_ttl, Duration::from_secs(900));
        assert_eq!(c.auth.refresh_token_ttl, Duration::from_secs(90 * 86_400));
        assert_eq!(RelayConfig::default_bind().port(), 443);
    }
}

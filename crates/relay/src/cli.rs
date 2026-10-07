//! Command line of the `lmu-relay` binary. Every `serve` option can also be set through the
//! environment variable named in its help (the container image is configured that way, see
//! `deploy/relay.env.example`). Secrets (database URL, admin token, passwords) are never logged
//! and passwords are never accepted as command-line arguments.

use std::io::{self, BufRead as _};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::builder::BoolishValueParser;
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};
use protocol::messages::Secret;
use thiserror::Error;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tracing_subscriber::EnvFilter;

use crate::auth::{AuthError, AuthService};
use crate::config::{AuthSettings, ConfigError, RelayConfig, parse_environment};
use crate::jwt::{AccessTokenKeys, KeyFileError};
use crate::secrets::RandomError;
use crate::server::{RelayBuilder, StartError};
use crate::store::{AccountRole, MemoryStore, PgStore, Store, StoreError};
use crate::time::WallClock;
use crate::tls::{
    DEV_CA_FILE, DEV_CERT_FILE, DEV_DEFAULT_NAMES, DEV_KEY_FILE, ReloadingTls, TlsError,
};

/// Log component of this module.
const COMPONENT: &str = "relay.cli";
/// Log filter when `RUST_LOG` is not set.
const DEFAULT_LOG_FILTER: &str = "info";
/// Value of `ADMIN_BIND` that disables the admin listener.
const ADMIN_OFF: &str = "off";
/// Environment variable read by `create-user` for the password (else stdin).
const DEFAULT_PASSWORD_ENV: &str = "LMU_RELAY_PASSWORD";
/// Time limit of `healthcheck`.
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);
/// Largest `healthcheck` response read.
const MAX_HEALTH_RESPONSE: u64 = 64 * 1024;
/// PostgreSQL pool size of the one-shot commands.
const CLI_DB_CONNECTIONS: u32 = 2;
/// Seconds per day (refresh token lifetime option).
const SECS_PER_DAY: u64 = 86_400;

/// `lmu-relay` command line.
#[derive(Debug, Parser)]
#[command(
    name = "lmu-relay",
    version,
    about = "lmure relay server for LMU Race Engineering",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Options of `serve` (the default command).
    #[command(flatten)]
    pub serve: ServeArgs,
}

/// Subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the relay (default).
    Serve(Box<ServeArgs>),
    /// Generate a development CA and a server certificate signed by it.
    DevCert(DevCertArgs),
    /// Create a new JWT signing key file (Ed25519 seed).
    GenJwtKey(GenJwtKeyArgs),
    /// Apply the database migrations.
    Migrate(DatabaseArgs),
    /// Create an account. The password is read from $LMU_RELAY_PASSWORD or the first stdin line.
    CreateUser(CreateUserArgs),
    /// Revoke every refresh token of an account (log it out everywhere).
    RevokeTokens(RevokeTokensArgs),
    /// Query the admin `/health` endpoint; exit status 0 when healthy (container health checks).
    Healthcheck(HealthcheckArgs),
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum LogFormat {
    /// One JSON object per line (production).
    #[default]
    Json,
    /// Human-readable lines.
    Pretty,
}

/// Account role for `create-user`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RoleArg {
    Driver,
    Engineer,
    Admin,
}

impl From<RoleArg> for AccountRole {
    fn from(role: RoleArg) -> Self {
        match role {
            RoleArg::Driver => AccountRole::Driver,
            RoleArg::Engineer => AccountRole::Engineer,
            RoleArg::Admin => AccountRole::Admin,
        }
    }
}

/// Options of `serve`.
#[derive(Debug, Clone, Args)]
pub struct ServeArgs {
    /// Telemetry listener address.
    #[arg(long, env = "RELAY_BIND", default_value = "0.0.0.0")]
    pub bind: IpAddr,
    /// Telemetry listener port.
    #[arg(long, env = "RELAY_PORT", default_value_t = protocol::DEFAULT_PORT)]
    pub port: u16,
    /// PEM certificate chain (reloaded when it changes).
    #[arg(long, env = "TLS_CERT")]
    pub tls_cert: Option<PathBuf>,
    /// PEM private key (reloaded when it changes).
    #[arg(long, env = "TLS_KEY")]
    pub tls_key: Option<PathBuf>,
    /// PostgreSQL URL. Without it the in-memory store is used (RELAY_ENV=development only).
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: Option<String>,
    /// PostgreSQL pool size.
    #[arg(long, env = "DATABASE_MAX_CONNECTIONS")]
    pub database_max_connections: Option<u32>,
    /// development | staging | production (announced in HELLO).
    #[arg(long = "env", env = "RELAY_ENV", default_value = "production")]
    pub environment: String,
    /// Admin listener (`/health`, `/metrics`) as host:port, or "off".
    #[arg(long, env = "ADMIN_BIND", default_value = "127.0.0.1:9090")]
    pub admin_bind: String,
    /// Bearer token of the admin listener (at least 32 characters).
    #[arg(long, env = "ADMIN_TOKEN", hide_env_values = true)]
    pub admin_token: Option<String>,
    /// Ed25519 signing key file for access tokens (`lmu-relay gen-jwt-key`).
    #[arg(long, env = "JWT_KEY_FILE")]
    pub jwt_key_file: Option<PathBuf>,
    /// Allow ACCOUNT_REGISTER (off by default).
    #[arg(long, env = "RELAY_OPEN_REGISTRATION", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new())]
    pub open_registration: bool,
    /// Do not apply database migrations at startup.
    #[arg(long, env = "RELAY_NO_MIGRATE", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new())]
    pub no_migrate: bool,
    /// Log format.
    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = LogFormat::Json)]
    pub log_format: LogFormat,
    /// Maximum concurrent connections.
    #[arg(long, env = "RELAY_MAX_CONNECTIONS")]
    pub max_connections: Option<usize>,
    /// Maximum concurrent connections per client IP.
    #[arg(long, env = "RELAY_MAX_CONNECTIONS_PER_IP")]
    pub max_connections_per_ip: Option<usize>,
    /// Maximum sessions (live or in grace).
    #[arg(long, env = "RELAY_MAX_SESSIONS")]
    pub max_sessions: Option<usize>,
    /// Engineer slots per driver session (D-022 default 2).
    #[arg(long, env = "RELAY_MAX_ENGINEERS_PER_SESSION")]
    pub max_engineers_per_session: Option<u16>,
    /// Largest accepted frame payload in bytes.
    #[arg(long, env = "RELAY_MAX_PAYLOAD_BYTES")]
    pub max_payload_bytes: Option<usize>,
    /// Inbound frames per second per connection.
    #[arg(long, env = "RELAY_CONNECTION_PACKETS_PER_SEC")]
    pub connection_packets_per_sec: Option<u64>,
    /// Inbound bytes per second per connection.
    #[arg(long, env = "RELAY_CONNECTION_BYTES_PER_SEC")]
    pub connection_bytes_per_sec: Option<u64>,
    /// Inbound frames per second per session.
    #[arg(long, env = "RELAY_SESSION_PACKETS_PER_SEC")]
    pub session_packets_per_sec: Option<u64>,
    /// Inbound bytes per second per session.
    #[arg(long, env = "RELAY_SESSION_BYTES_PER_SEC")]
    pub session_bytes_per_sec: Option<u64>,
    /// Invalid frames tolerated per connection (refilled 1/s) before it is closed.
    #[arg(long, env = "RELAY_MALFORMED_BUDGET")]
    pub malformed_budget: Option<u32>,
    /// SESSION_JOIN attempts per account and per IP per minute.
    #[arg(long, env = "CODE_REDEMPTIONS_PER_MINUTE")]
    pub code_redemptions_per_minute: Option<u32>,
    /// SESSION_JOIN attempts per account and per IP per hour.
    #[arg(long, env = "CODE_REDEMPTIONS_PER_HOUR")]
    pub code_redemptions_per_hour: Option<u32>,
    /// AUTH attempts per IP per minute.
    #[arg(long, env = "AUTH_ATTEMPTS_PER_IP_PER_MINUTE")]
    pub auth_attempts_per_ip_per_minute: Option<u32>,
    /// Password attempts per account per minute.
    #[arg(long, env = "AUTH_ATTEMPTS_PER_ACCOUNT_PER_MINUTE")]
    pub auth_attempts_per_account_per_minute: Option<u32>,
    /// Expected client PING period (ms).
    #[arg(long, env = "HEARTBEAT_INTERVAL_MS")]
    pub heartbeat_interval_ms: Option<u64>,
    /// Connections silent for this long are closed (ms).
    #[arg(long, env = "HEARTBEAT_TIMEOUT_MS")]
    pub heartbeat_timeout_ms: Option<u64>,
    /// Session lifetime after a driver disconnect (s).
    #[arg(long, env = "SESSION_DRIVER_GRACE_S")]
    pub session_driver_grace_s: Option<u64>,
    /// Lifetime of an unused connection code (s).
    #[arg(long, env = "CONNECT_CODE_TTL_S")]
    pub connect_code_ttl_s: Option<u64>,
    /// Lifetime of an engineer resume ticket after its connection dropped (s).
    #[arg(long, env = "ENGINEER_TICKET_TTL_S")]
    pub engineer_ticket_ttl_s: Option<u64>,
    /// Access token lifetime (s).
    #[arg(long, env = "ACCESS_TOKEN_TTL_S")]
    pub access_token_ttl_s: Option<u64>,
    /// Refresh token lifetime since its last use (days, sliding).
    #[arg(long, env = "REFRESH_TOKEN_TTL_DAYS")]
    pub refresh_token_ttl_days: Option<u64>,
    /// Interval of the TLS certificate change check (s).
    #[arg(long, env = "TLS_RELOAD_INTERVAL_S")]
    pub tls_reload_interval_s: Option<u64>,
    /// Interval of the check that closes connections of revoked logins (s).
    #[arg(long, env = "REVOCATION_CHECK_INTERVAL_S")]
    pub revocation_check_interval_s: Option<u64>,
}

/// Options of `dev-cert`.
#[derive(Debug, Clone, Args)]
pub struct DevCertArgs {
    /// Output directory (ca.pem, server.pem, server.key.pem).
    #[arg(long)]
    pub out: PathBuf,
    /// DNS names / IP addresses of the server certificate (repeatable; default localhost,
    /// 127.0.0.1, ::1).
    #[arg(long = "name")]
    pub names: Vec<String>,
}

/// Options of `gen-jwt-key`.
#[derive(Debug, Clone, Args)]
pub struct GenJwtKeyArgs {
    /// Key file to create (never overwritten).
    #[arg(long)]
    pub out: PathBuf,
}

/// Database connection of the one-shot commands.
#[derive(Debug, Clone, Args)]
pub struct DatabaseArgs {
    /// PostgreSQL URL.
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,
}

/// Options of `create-user`.
#[derive(Debug, Clone, Args)]
pub struct CreateUserArgs {
    #[command(flatten)]
    pub db: DatabaseArgs,
    /// Login name (3-32 of a-z 0-9 . _ -; stored lower-case).
    #[arg(long)]
    pub username: String,
    /// Name shown to the other side (SESSION_INFO, engineer list).
    #[arg(long)]
    pub display_name: String,
    /// Account role.
    #[arg(long, value_enum)]
    pub role: RoleArg,
    /// Environment variable holding the password (else the first line of stdin).
    #[arg(long, default_value = DEFAULT_PASSWORD_ENV)]
    pub password_env: String,
}

/// Options of `revoke-tokens`.
#[derive(Debug, Clone, Args)]
pub struct RevokeTokensArgs {
    #[command(flatten)]
    pub db: DatabaseArgs,
    /// Account whose refresh tokens are revoked.
    #[arg(long)]
    pub username: String,
}

/// Options of `healthcheck`.
#[derive(Debug, Clone, Args)]
pub struct HealthcheckArgs {
    /// URL of the admin health endpoint (plain HTTP, loopback).
    #[arg(
        long,
        env = "RELAY_HEALTHCHECK_URL",
        default_value = "http://127.0.0.1:9090/health"
    )]
    pub url: String,
    /// Admin bearer token.
    #[arg(long, env = "ADMIN_TOKEN", hide_env_values = true)]
    pub admin_token: Option<String>,
}

/// Command failures.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("invalid configuration")]
    Config(#[from] ConfigError),
    #[error("invalid ADMIN_BIND {0:?} (expected host:port or \"off\")")]
    AdminBind(String),
    #[error("TLS_CERT and TLS_KEY are required (use `lmu-relay dev-cert` for development)")]
    TlsPathsMissing,
    #[error("TLS setup failed")]
    Tls(#[from] TlsError),
    #[error("database failure")]
    Store(#[from] StoreError),
    #[error("relay startup failed")]
    Start(#[from] StartError),
    #[error("account operation failed")]
    Auth(#[from] AuthError),
    #[error("JWT key file failure")]
    JwtKey(#[from] KeyFileError),
    #[error(transparent)]
    Random(#[from] RandomError),
    #[error("{what}")]
    Io {
        what: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("no password given (set ${0} or pipe it on stdin)")]
    NoPassword(String),
    #[error("unsupported health URL {0:?} (expected http://host:port/path)")]
    HealthUrl(String),
    #[error("relay unhealthy: {0}")]
    Unhealthy(String),
}

impl ServeArgs {
    /// The validated relay configuration.
    pub fn to_config(&self) -> Result<RelayConfig, CliError> {
        let mut c = RelayConfig::development();
        c.env = parse_environment(&self.environment)?;
        c.bind = SocketAddr::new(self.bind, self.port);
        c.admin_bind = parse_admin_bind(&self.admin_bind)?;
        c.admin_token = self
            .admin_token
            .clone()
            .filter(|t| !t.is_empty())
            .map(Secret::new);
        c.database_url = self
            .database_url
            .clone()
            .filter(|u| !u.is_empty())
            .map(Secret::new);
        c.jwt_key_file = self.jwt_key_file.clone();
        c.open_registration = self.open_registration;
        set(
            &mut c.database_max_connections,
            self.database_max_connections,
        );
        let l = &mut c.limits;
        set(&mut l.max_connections, self.max_connections);
        set(&mut l.max_connections_per_ip, self.max_connections_per_ip);
        set(&mut l.max_sessions, self.max_sessions);
        set(
            &mut l.max_engineers_per_session,
            self.max_engineers_per_session,
        );
        set(&mut l.max_payload_bytes, self.max_payload_bytes);
        set(
            &mut l.connection_rate.packets_per_sec,
            self.connection_packets_per_sec,
        );
        set(
            &mut l.connection_rate.bytes_per_sec,
            self.connection_bytes_per_sec,
        );
        set(
            &mut l.session_rate.packets_per_sec,
            self.session_packets_per_sec,
        );
        set(
            &mut l.session_rate.bytes_per_sec,
            self.session_bytes_per_sec,
        );
        set(&mut l.malformed_budget, self.malformed_budget);
        set(
            &mut l.code_redemptions_per_minute,
            self.code_redemptions_per_minute,
        );
        set(
            &mut l.code_redemptions_per_hour,
            self.code_redemptions_per_hour,
        );
        set(
            &mut l.auth_attempts_per_ip_per_minute,
            self.auth_attempts_per_ip_per_minute,
        );
        set(
            &mut l.auth_attempts_per_account_per_minute,
            self.auth_attempts_per_account_per_minute,
        );
        let t = &mut c.timeouts;
        set(
            &mut t.heartbeat_interval,
            self.heartbeat_interval_ms.map(Duration::from_millis),
        );
        set(
            &mut t.heartbeat_timeout,
            self.heartbeat_timeout_ms.map(Duration::from_millis),
        );
        set(
            &mut t.driver_grace,
            self.session_driver_grace_s.map(Duration::from_secs),
        );
        set(
            &mut t.connect_code_ttl,
            self.connect_code_ttl_s.map(Duration::from_secs),
        );
        set(
            &mut t.engineer_ticket_ttl,
            self.engineer_ticket_ttl_s.map(Duration::from_secs),
        );
        set(
            &mut t.tls_reload_interval,
            self.tls_reload_interval_s.map(Duration::from_secs),
        );
        set(
            &mut t.revocation_check_interval,
            self.revocation_check_interval_s.map(Duration::from_secs),
        );
        set(
            &mut c.auth.access_token_ttl,
            self.access_token_ttl_s.map(Duration::from_secs),
        );
        set(
            &mut c.auth.refresh_token_ttl,
            self.refresh_token_ttl_days
                .map(|d| Duration::from_secs(d.saturating_mul(SECS_PER_DAY))),
        );
        c.validate()?;
        Ok(c)
    }

    /// TLS certificate chain and key paths.
    pub fn tls_paths(&self) -> Result<(PathBuf, PathBuf), CliError> {
        match (&self.tls_cert, &self.tls_key) {
            (Some(cert), Some(key)) => Ok((cert.clone(), key.clone())),
            _ => Err(CliError::TlsPathsMissing),
        }
    }
}

fn set<T>(slot: &mut T, value: Option<T>) {
    if let Some(v) = value {
        *slot = v;
    }
}

/// `host:port` or "off".
fn parse_admin_bind(text: &str) -> Result<Option<SocketAddr>, CliError> {
    let text = text.trim();
    if text.eq_ignore_ascii_case(ADMIN_OFF) || text.is_empty() {
        return Ok(None);
    }
    text.parse::<SocketAddr>()
        .map(Some)
        .map_err(|_| CliError::AdminBind(text.to_owned()))
}

/// Installs the global tracing subscriber (JSON lines or human-readable) filtered by `RUST_LOG`.
pub fn init_logging(format: LogFormat) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    let installed = match format {
        LogFormat::Json => builder
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_span_list(false)
            .try_init(),
        LogFormat::Pretty => builder.try_init(),
    };
    // Fails only if a subscriber is already installed (tests); that one keeps working.
    drop(installed);
}

/// Log format a parsed command line asks for.
pub fn log_format(cli: &Cli) -> LogFormat {
    match &cli.command {
        None => cli.serve.log_format,
        Some(Command::Serve(args)) => args.log_format,
        Some(_) => LogFormat::Pretty,
    }
}

/// Runs the command.
pub async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        None => serve(cli.serve).await,
        Some(Command::Serve(args)) => serve(*args).await,
        Some(Command::DevCert(args)) => dev_cert(&args),
        Some(Command::GenJwtKey(args)) => {
            AccessTokenKeys::create_file(&args.out)?;
            tracing::info!(component = COMPONENT, path = %args.out.display(), "JWT signing key created");
            Ok(())
        }
        Some(Command::Migrate(args)) => {
            connect_and_migrate(&args).await?;
            tracing::info!(component = COMPONENT, "database migrations applied");
            Ok(())
        }
        Some(Command::CreateUser(args)) => create_user(args).await,
        Some(Command::RevokeTokens(args)) => {
            let auth = cli_auth(&args.db).await?;
            let revoked = auth.revoke_tokens(&args.username).await?;
            tracing::info!(component = COMPONENT, username = %args.username, revoked, "refresh tokens revoked");
            Ok(())
        }
        Some(Command::Healthcheck(args)) => healthcheck(&args).await,
    }
}

async fn serve(args: ServeArgs) -> Result<(), CliError> {
    let config = args.to_config()?;
    let (cert, key) = args.tls_paths()?;
    let tls = Arc::new(ReloadingTls::load(&cert, &key)?);
    let store = match &config.database_url {
        Some(url) => {
            let pg = PgStore::connect(url, config.database_max_connections).await?;
            if !args.no_migrate {
                pg.migrate().await?;
            }
            Store::Postgres(pg)
        }
        None => {
            tracing::warn!(
                component = COMPONENT,
                open_registration = config.open_registration,
                "no DATABASE_URL: in-memory store, accounts and sessions are lost on exit (development only); \
                 `create-user` needs PostgreSQL, so development clients register with RELAY_OPEN_REGISTRATION=true"
            );
            Store::Memory(MemoryStore::new())
        }
    };
    let handle = RelayBuilder::new(config, store, tls).start().await?;
    wait_for_signal().await;
    handle.shutdown().await;
    Ok(())
}

/// Completes on Ctrl-C, or SIGTERM on Unix (container stop).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(e) => {
                tracing::warn!(component = COMPONENT, error = %e, "SIGTERM handler not installed");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn dev_cert(args: &DevCertArgs) -> Result<(), CliError> {
    let names: Vec<String> = if args.names.is_empty() {
        DEV_DEFAULT_NAMES.iter().map(|n| (*n).to_owned()).collect()
    } else {
        args.names.clone()
    };
    crate::tls::write_dev_certificates(&args.out, &names)?;
    tracing::info!(
        component = COMPONENT,
        dir = %args.out.display(),
        ca = DEV_CA_FILE,
        cert = DEV_CERT_FILE,
        key = DEV_KEY_FILE,
        names = ?names,
        "development certificates written; trust ca.pem in development clients only"
    );
    Ok(())
}

async fn connect_and_migrate(args: &DatabaseArgs) -> Result<PgStore, CliError> {
    let pg = PgStore::connect(&Secret::new(args.database_url.clone()), CLI_DB_CONNECTIONS).await?;
    pg.migrate().await?;
    Ok(pg)
}

async fn cli_auth(args: &DatabaseArgs) -> Result<AuthService, CliError> {
    let pg = connect_and_migrate(args).await?;
    // Account commands never issue tokens: an ephemeral signing key suffices.
    Ok(AuthService::new(
        Store::Postgres(pg),
        AccessTokenKeys::generate()?,
        AuthSettings::default(),
        1,
        false,
        WallClock::system(),
    )
    .await?)
}

async fn create_user(args: CreateUserArgs) -> Result<(), CliError> {
    let password = read_password(&args.password_env)?;
    let auth = cli_auth(&args.db).await?;
    let account = auth
        .create_account(
            &args.username,
            &args.display_name,
            args.role.into(),
            &password,
        )
        .await?;
    tracing::info!(
        component = COMPONENT,
        account = %account.id,
        username = %account.username,
        role = account.role.as_str(),
        "account created"
    );
    Ok(())
}

/// The password from `$env_var`, else the first line of stdin.
fn read_password(env_var: &str) -> Result<Secret, CliError> {
    if let Ok(value) = std::env::var(env_var)
        && !value.is_empty()
    {
        return Ok(Secret::new(value));
    }
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|source| CliError::Io {
            what: "cannot read the password from stdin",
            source,
        })?;
    let password = line.trim_end_matches(['\r', '\n']).to_owned();
    if password.is_empty() {
        return Err(CliError::NoPassword(env_var.to_owned()));
    }
    Ok(Secret::new(password))
}

/// Splits `http://host:port/path` into (`host:port`, `/path`).
fn parse_http_url(url: &str) -> Result<(String, String), CliError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| CliError::HealthUrl(url.to_owned()))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || !authority.contains(':') {
        return Err(CliError::HealthUrl(url.to_owned()));
    }
    Ok((authority.to_owned(), path.to_owned()))
}

async fn healthcheck(args: &HealthcheckArgs) -> Result<(), CliError> {
    let (authority, path) = parse_http_url(&args.url)?;
    let token = args.admin_token.clone().unwrap_or_default();
    let exchange = async {
        let mut stream = TcpStream::connect(&authority).await?;
        let auth = if token.is_empty() {
            String::new()
        } else {
            format!("Authorization: Bearer {token}\r\n")
        };
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\n{auth}Connection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await?;
        let mut response = Vec::new();
        stream
            .take(MAX_HEALTH_RESPONSE)
            .read_to_end(&mut response)
            .await?;
        Ok::<Vec<u8>, io::Error>(response)
    };
    let response = tokio::time::timeout(HEALTHCHECK_TIMEOUT, exchange)
        .await
        .map_err(|_| CliError::Unhealthy("no answer in time".into()))?
        .map_err(|source| CliError::Io {
            what: "health endpoint unreachable",
            source,
        })?;
    let text = String::from_utf8_lossy(&response);
    let status_line = text.lines().next().unwrap_or_default();
    let ok = status_line
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code == "200");
    if ok {
        Ok(())
    } else {
        Err(CliError::Unhealthy(status_line.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use protocol::version::Environment;

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("valid command line")
    }

    #[test]
    fn serve_defaults_are_production_and_need_a_database() {
        let cli = parse(&["lmu-relay", "--tls-cert", "c.pem", "--tls-key", "k.pem"]);
        assert!(cli.command.is_none());
        assert!(matches!(
            cli.serve.to_config(),
            Err(CliError::Config(ConfigError::DatabaseRequired(
                Environment::Production
            )))
        ));
    }

    #[test]
    fn development_runs_with_the_memory_store_and_overrides_apply() {
        let cli = parse(&[
            "lmu-relay",
            "serve",
            "--env",
            "development",
            "--port",
            "4433",
            "--admin-bind",
            "off",
            "--max-engineers-per-session",
            "1",
            "--connect-code-ttl-s",
            "120",
            "--refresh-token-ttl-days",
            "30",
        ]);
        let Some(Command::Serve(args)) = cli.command else {
            panic!("serve expected");
        };
        let config = args.to_config().expect("valid");
        assert_eq!(config.env, Environment::Development);
        assert_eq!(config.bind.port(), 4433);
        assert_eq!(config.admin_bind, None);
        assert_eq!(config.limits.max_engineers_per_session, 1);
        assert_eq!(config.timeouts.connect_code_ttl, Duration::from_secs(120));
        assert_eq!(
            config.auth.refresh_token_ttl,
            Duration::from_secs(30 * SECS_PER_DAY)
        );
        assert!(matches!(args.tls_paths(), Err(CliError::TlsPathsMissing)));
    }

    #[test]
    fn subcommands_parse() {
        assert!(matches!(
            parse(&[
                "lmu-relay",
                "dev-cert",
                "--out",
                "certs",
                "--name",
                "relay.local"
            ])
            .command,
            Some(Command::DevCert(_))
        ));
        let cli = parse(&[
            "lmu-relay",
            "create-user",
            "--database-url",
            "postgres://x",
            "--username",
            "erin",
            "--display-name",
            "Erin",
            "--role",
            "engineer",
        ]);
        let Some(Command::CreateUser(args)) = cli.command else {
            panic!("create-user expected");
        };
        assert_eq!(args.role, RoleArg::Engineer);
        assert_eq!(args.password_env, DEFAULT_PASSWORD_ENV);
        assert!(Cli::try_parse_from(["lmu-relay", "create-user", "--password", "x"]).is_err());
    }

    #[test]
    fn admin_bind_and_health_urls() {
        assert_eq!(parse_admin_bind("off").expect("off"), None);
        assert!(parse_admin_bind("127.0.0.1:9090").expect("addr").is_some());
        assert!(parse_admin_bind("localhost").is_err());
        assert_eq!(
            parse_http_url("http://127.0.0.1:9090/health").expect("url"),
            ("127.0.0.1:9090".to_owned(), "/health".to_owned())
        );
        assert!(parse_http_url("https://x:1/health").is_err());
        assert!(parse_http_url("http://nohost/health").is_err());
    }
}
